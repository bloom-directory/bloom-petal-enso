pub mod api;
pub mod api_types;
pub mod input;
pub mod policy;
pub mod quote;
pub mod redaction;
pub mod runtime;
pub mod session;
pub mod settings;
pub mod settlement;
pub mod simulation;
pub mod wallet;
pub mod workflow;

#[cfg(test)]
mod integration_tests;

pub mod prelude {
    pub use crate::workflow::*;
    pub use petal::*;
}

/// Require the trusted numbered account supplied for a matched wallet/index route.
pub fn account_number(ctx: &petal::Ctx) -> Result<u32, petal::DispatchResponse> {
    petal::route_param(ctx, "bloom.account")
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| petal::error(-2, "trusted account context is required"))
}

#[cfg(test)]
mod account_context_tests {
    struct Scoped;
    impl petal::RouteIdentity for Scoped {
        const PATH: &'static str = "operations/[wallet]/[index]/new";
        const CANONICAL_PATH: &'static str = Self::PATH;
        const PARAMS: &'static [(&'static str, usize)] = &[("wallet", 1), ("index", 2)];
    }

    #[test]
    fn account_selection_requires_trusted_context() {
        for trusted in [None, Some("invalid"), Some("0"), Some("1")] {
            let mut params = vec![("index".into(), "1".into())];
            if let Some(value) = trusted {
                params.push(("bloom.account".into(), value.into()));
            }
            let ctx = petal::Ctx::bind::<Scoped>(petal::RawCtx {
                petal_root: String::new(),
                package_hash: String::new(),
                path: "operations/alice/1/new".into(),
                params,
                actor: None,
            });
            match trusted {
                Some("0") => assert_eq!(super::account_number(&ctx).unwrap(), 0),
                Some("1") => assert_eq!(super::account_number(&ctx).unwrap(), 1),
                _ => assert!(super::account_number(&ctx).is_err()),
            }
        }
    }
}

#[cfg(test)]
mod global_settings_tests;
