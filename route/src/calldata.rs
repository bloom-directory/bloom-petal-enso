//! Router V2's signed source and destination constraints. Inner shortcuts stay
//! byte-for-byte intact; same-chain routes use the router's atomic balance checks.
//! Contract: EnsoBuild/shortcuts-client-contracts@483408a3219451858abcd92c2f7b19a9783c14cc.
//! The deployed routers 0xf755… (10 chains) and 0xa146… (Linea) were built from
//! 6f012e9/c2ba4c9 and require native input data to equal msg.value.

use alloy::primitives::{Address, Bytes, U256};
use alloy::sol_types::{SolCall, SolValue};

use crate::api_types::{IEnsoRouter, NATIVE_TOKEN, RouteRequest, RouteResponse};

pub(crate) struct Envelope {
    pub inputs: Vec<IEnsoRouter::Token>,
    outputs: Option<(Vec<IEnsoRouter::Token>, Address)>,
    shortcut: Bytes,
    single: bool,
}

/// Re-encoding excludes trailing bytes, aliased offsets and noncanonical words.
pub(crate) fn decode(data: &[u8]) -> Result<Envelope, String> {
    macro_rules! canonical {
        ($call:ty) => {{
            <$call>::abi_decode(data)
                .ok()
                .filter(|call| call.abi_encode() == data)
        }};
    }
    if let Some(call) = canonical!(IEnsoRouter::routeSingleCall) {
        return Ok(Envelope {
            inputs: vec![call.tokenIn],
            outputs: None,
            shortcut: call.data,
            single: true,
        });
    }
    if let Some(call) = canonical!(IEnsoRouter::routeMultiCall) {
        return Ok(Envelope {
            inputs: call.tokensIn,
            outputs: None,
            shortcut: call.data,
            single: false,
        });
    }
    if let Some(call) = canonical!(IEnsoRouter::safeRouteSingleCall) {
        return Ok(Envelope {
            inputs: vec![call.tokenIn],
            outputs: Some((vec![call.tokenOut], call.receiver)),
            shortcut: call.data,
            single: true,
        });
    }
    if let Some(call) = canonical!(IEnsoRouter::safeRouteMultiCall) {
        return Ok(Envelope {
            inputs: call.tokensIn,
            outputs: Some((call.tokensOut, call.receiver)),
            shortcut: call.data,
            single: false,
        });
    }
    Err("Enso route is not a canonical Router V2 call".into())
}

fn same_chain(req: &RouteRequest) -> bool {
    req.destination_chain_id.is_none_or(|id| id == req.chain_id)
}

/// Only a matching same-chain check is an output constraint for this request.
pub fn output_minimum(route: &RouteResponse, req: &RouteRequest) -> Option<U256> {
    if !same_chain(req) {
        return None;
    }
    let envelope = decode(&route.tx.data).ok()?;
    let (outputs, receiver) = envelope.outputs?;
    if receiver != req.receiver.unwrap_or(req.from_address) {
        return None;
    }
    let [token] = outputs.as_slice() else {
        return None;
    };
    if req.token_out == NATIVE_TOKEN.parse::<Address>().unwrap() {
        let (minimum,) = <(U256,)>::abi_decode_params(&token.data).ok()?;
        (token.tokenType == 0 && token.data.as_ref() == (minimum,).abi_encode_params())
            .then_some(minimum)
    } else {
        let (address, minimum) = <(Address, U256)>::abi_decode_params(&token.data).ok()?;
        (token.tokenType == 1
            && address == req.token_out
            && token.data.as_ref() == (address, minimum).abi_encode_params())
        .then_some(minimum)
    }
}

/// Add the Router V2 check locally rather than trusting a quote or bytes search.
/// An upstream safe-route check must already agree; never silently replace it.
pub fn bind_output_checks(route: &mut RouteResponse, req: &RouteRequest) -> Result<(), String> {
    if !route.input_matches_request(req) || !route.destination_matches_request(req) {
        return Err(
            "Enso route transaction input or destination does not match the request".into(),
        );
    }
    if !same_chain(req) {
        return Ok(());
    }
    let envelope = decode(&route.tx.data)?;
    if envelope.outputs.is_none() {
        let minimum = match req.min_amount_out.filter(|floor| !floor.is_zero()) {
            Some(floor) => floor,
            None => crate::quote::reviewed_minimum_output(route, req)?,
        };
        let native = req.token_out == NATIVE_TOKEN.parse::<Address>().unwrap();
        let output = IEnsoRouter::Token {
            tokenType: if native { 0 } else { 1 },
            data: if native {
                (minimum,).abi_encode_params()
            } else {
                (req.token_out, minimum).abi_encode_params()
            }
            .into(),
        };
        let receiver = req.receiver.unwrap_or(req.from_address);
        route.tx.data = if envelope.single {
            IEnsoRouter::safeRouteSingleCall {
                tokenIn: envelope.inputs.into_iter().next().unwrap(),
                tokenOut: output,
                receiver,
                data: envelope.shortcut,
            }
            .abi_encode()
        } else {
            IEnsoRouter::safeRouteMultiCall {
                tokensIn: envelope.inputs,
                tokensOut: vec![output],
                receiver,
                data: envelope.shortcut,
            }
            .abi_encode()
        }
        .into();
    }
    verify_output_checks(route, req).map(|_| ())
}

/// Recheck stored/returned safe routes without rewriting bytes at confirmation.
pub fn verify_output_checks(route: &RouteResponse, req: &RouteRequest) -> Result<bool, String> {
    if !same_chain(req) {
        return Ok(false);
    }
    let envelope = decode(&route.tx.data)?;
    if envelope.outputs.is_none() {
        // Every same-chain route is bound to a safe-route check at review;
        // one without it predates that and needs a new review.
        return Err(
            "this same-chain Enso route has no on-chain output check; create a new intent".into(),
        );
    }
    let minimum = output_minimum(route, req)
        .ok_or("Enso safe route output token or receiver does not match the request")?;
    // After review the stored floor is the bar; before it is stored, the
    // quote less slippage is.
    let required = match req.min_amount_out.filter(|floor| !floor.is_zero()) {
        Some(floor) => floor,
        None => crate::quote::reviewed_minimum_output(route, req)?,
    };
    if minimum.is_zero() || minimum < required {
        return Err(format!(
            "Enso safe route's minimum output ({minimum}) is below the reviewed minimum output ({required})"
        ));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(native: bool, multi: bool) -> (RouteResponse, RouteRequest) {
        let sender = Address::repeat_byte(0x11);
        let token_in = if native {
            NATIVE_TOKEN.parse().unwrap()
        } else {
            Address::repeat_byte(0x22)
        };
        let req = RouteRequest::new(
            sender,
            1,
            token_in,
            Address::repeat_byte(0x33),
            U256::from(100),
        );
        let input = IEnsoRouter::Token {
            tokenType: if native { 0 } else { 1 },
            data: if native {
                (req.amount_in,).abi_encode_params()
            } else {
                (token_in, req.amount_in).abi_encode_params()
            }
            .into(),
        };
        let shortcut = Bytes::from_static(b"unchanged inner bytes");
        let data = if multi {
            IEnsoRouter::routeMultiCall {
                tokensIn: vec![input],
                data: shortcut,
            }
            .abi_encode()
        } else {
            IEnsoRouter::routeSingleCall {
                tokenIn: input,
                data: shortcut,
            }
            .abi_encode()
        };
        let route = serde_json::from_value(serde_json::json!({
            "tx": {"from": sender, "to": Address::repeat_byte(0x44), "value": if native {"100"} else {"0"}, "data": format!("0x{}", hex::encode(data))},
            "amountOut": "1000", "route": []
        })).unwrap();
        (route, req)
    }

    #[test]
    fn higher_unwrapped_refresh_preserves_reviewed_floor() {
        for native in [false, true] {
            for multi in [false, true] {
                let (mut route, mut req) = fixture(native, multi);
                req.min_amount_out = Some(U256::from(995));
                route.amount_out = "1100".into();
                bind_output_checks(&mut route, &req).unwrap();
                assert_eq!(output_minimum(&route, &req), Some(U256::from(995)));
            }
        }
    }

    #[test]
    fn native_input_data_must_equal_the_value_sent() {
        for (data, accepted) in [
            ((U256::from(100),).abi_encode_params(), true),
            (vec![], false),
            ((U256::from(1),).abi_encode_params(), false),
        ] {
            let (mut route, req) = fixture(true, false);
            route.tx.data = IEnsoRouter::routeSingleCall {
                tokenIn: IEnsoRouter::Token {
                    tokenType: 0,
                    data: data.into(),
                },
                data: Bytes::from_static(b"unchanged inner bytes"),
            }
            .abi_encode()
            .into();
            assert_eq!(route.input_matches_request(&req), accepted);
        }
    }

    #[test]
    fn guards_all_fungible_router_envelopes_without_changing_shortcuts() {
        for native_input in [false, true] {
            for native_output in [false, true] {
                for multi in [false, true] {
                    let (mut route, mut req) = fixture(native_input, multi);
                    req.receiver = Some(Address::repeat_byte(0x55));
                    if native_output {
                        req.token_out = NATIVE_TOKEN.parse().unwrap();
                    }
                    let original = decode(&route.tx.data).unwrap();
                    let tx = route.tx.clone();
                    bind_output_checks(&mut route, &req).unwrap();
                    assert!(route.input_matches_request(&req));
                    assert_eq!(output_minimum(&route, &req), Some(U256::from(995)));
                    assert!(verify_output_checks(&route, &req).unwrap());
                    let guarded = decode(&route.tx.data).unwrap();
                    assert_eq!(guarded.shortcut, original.shortcut);
                    assert_eq!(guarded.single, !multi);
                    assert_eq!(
                        (route.tx.to, route.tx.from, route.tx.value),
                        (tx.to, tx.from, tx.value)
                    );
                    let bytes = route.tx.data.clone();
                    bind_output_checks(&mut route, &req).unwrap();
                    assert_eq!(
                        route.tx.data, bytes,
                        "already safe calls must not be rewritten"
                    );
                }
            }
        }
    }

    #[test]
    fn stronger_explicit_and_calldata_floors_survive_quote_replacement() {
        let (mut route, mut req) = fixture(false, false);
        req.min_amount_out = Some(U256::from(998));
        bind_output_checks(&mut route, &req).unwrap();
        assert_eq!(output_minimum(&route, &req), Some(U256::from(998)));
        req.min_amount_out = Some(U256::from(996));
        assert_eq!(
            crate::quote::reviewed_minimum_output(&route, &req).unwrap(),
            U256::from(996)
        );
        let effective = crate::quote::reviewed_minimum_output(&route, &req)
            .unwrap()
            .max(output_minimum(&route, &req).unwrap());
        assert_eq!(effective, U256::from(998));
        req.min_amount_out = Some(effective);
        route.amount_out = "998".into();
        assert_eq!(
            crate::quote::reviewed_minimum_output(&route, &req).unwrap(),
            effective
        );
    }

    #[test]
    fn rejects_changed_receiver_token_floor_and_extra_outputs() {
        let (mut route, req) = fixture(false, false);
        bind_output_checks(&mut route, &req).unwrap();
        let original = IEnsoRouter::safeRouteSingleCall::abi_decode(&route.tx.data).unwrap();
        let mut changed = original.clone();
        changed.receiver = Address::repeat_byte(0x66);
        route.tx.data = changed.abi_encode().into();
        assert!(bind_output_checks(&mut route, &req).is_err());
        for (token_type, token, minimum) in [
            (1, Address::repeat_byte(0x77), 995),
            (1, req.token_out, 994),
            (1, req.token_out, 0),
            (2, req.token_out, 995),
        ] {
            let mut changed = original.clone();
            changed.tokenOut = IEnsoRouter::Token {
                tokenType: token_type,
                data: (token, U256::from(minimum)).abi_encode_params().into(),
            };
            route.tx.data = changed.abi_encode().into();
            assert!(bind_output_checks(&mut route, &req).is_err());
        }
        route.tx.data = IEnsoRouter::safeRouteMultiCall {
            tokensIn: vec![original.tokenIn],
            tokensOut: vec![original.tokenOut.clone(), original.tokenOut],
            receiver: original.receiver,
            data: original.data,
        }
        .abi_encode()
        .into();
        assert!(bind_output_checks(&mut route, &req).is_err());
    }

    #[test]
    fn rejects_noncanonical_outer_and_token_data() {
        let (mut route, req) = fixture(false, false);
        let mut bytes = route.tx.data.to_vec();
        bytes.extend([0u8; 32]);
        route.tx.data = bytes.into();
        assert!(!route.input_matches_request(&req));
        assert!(bind_output_checks(&mut route, &req).is_err());
        let (mut route, req) = fixture(false, false);
        let mut call = IEnsoRouter::routeSingleCall::abi_decode(&route.tx.data).unwrap();
        let mut token_data = call.tokenIn.data.to_vec();
        token_data.extend([0u8; 32]);
        call.tokenIn.data = token_data.into();
        route.tx.data = call.abi_encode().into();
        assert!(!route.input_matches_request(&req));
    }

    #[test]
    fn api_metadata_cannot_downgrade_a_refreshed_floor() {
        let (mut route, req) = fixture(true, false);
        bind_output_checks(&mut route, &req).unwrap();
        let (mut fresh, _) = fixture(true, false);
        fresh.amount_out = "997".into();
        fresh.min_amount_out = Some(serde_json::json!("1"));
        let mut refresh_request = req.clone();
        refresh_request.min_amount_out = Some(U256::from(995));
        bind_output_checks(&mut fresh, &refresh_request).unwrap();
        crate::quote::verify_refreshed(&route, &fresh, &refresh_request).unwrap();
        assert_eq!(output_minimum(&fresh, &req), Some(U256::from(995)));
        let (mut unguarded, _) = fixture(true, false);
        unguarded.min_amount_out = Some(serde_json::json!("1000"));
        assert!(crate::quote::verify_refreshed(&route, &unguarded, &refresh_request).is_err());
        // Without a stored floor there is nothing to refresh against.
        assert!(crate::quote::verify_refreshed(&route, &fresh, &req).is_err());
    }

    #[test]
    fn cross_chain_checks_are_never_claimed_as_destination_constraints() {
        let (mut route, mut req) = fixture(true, false);
        req.destination_chain_id = Some(8453);
        route.destination_chain_id = Some(8453);
        let original = route.tx.data.clone();
        bind_output_checks(&mut route, &req).unwrap();
        assert_eq!(route.tx.data, original);
        assert_eq!(output_minimum(&route, &req), None);
        assert!(!verify_output_checks(&route, &req).unwrap());
    }

    #[test]
    fn uint256_quote_math_cannot_wrap_or_silently_allow_zero() {
        let (mut route, mut req) = fixture(false, false);
        route.amount_out = U256::MAX.to_string();
        bind_output_checks(&mut route, &req).unwrap();
        assert!(output_minimum(&route, &req).unwrap() > U256::MAX / U256::from(2));
        req.slippage_bps = 10_000;
        assert!(crate::quote::reviewed_minimum_output(&route, &req).is_err());
        req.slippage_bps = 50;
        route.amount_out = "1".into();
        assert!(crate::quote::reviewed_minimum_output(&route, &req).is_err());
    }
}
