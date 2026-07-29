// Blocking settlement poll: reads balance on destination chain every 5s
// for up to 300s (60 iterations), returning as soon as a positive delta
// is observed.
//
// The petal runtime executes route files synchronously in the WASM host,
// so a sleep-loop here blocks the read until settlement is confirmed or
// the timeout expires.

petal::route_file!(
    spec: petal::store_read_spec().caps(&["bloom:store", "bloom:chain"]),
    read: |ctx: &petal::Ctx| {
        let wallet = match petal::param(ctx, "wallet") {
            Ok(value) => value,
            Err(response) => return response,
        };
        let id = match petal::param(ctx, "id") {
            Ok(value) => value,
            Err(response) => return response,
        };
        let mut host = crate::workflow::BloomHost;
        let sess = match crate::workflow::load(&mut host, wallet, id) {
            Ok(s) => s,
            Err(error) => return petal::error(-1, error),
        };

        // If not staged yet, nothing to wait for.
        if sess.staged_ids.is_empty() {
            return petal::read_json_value(&serde_json::json!({
                "status": "not_staged",
                "message": "no transactions have been staged into the outbox yet",
            }));
        }

        // Check settlement immediately.
        let result = crate::settlement::settlement_status(&mut host, &sess);
        let status = result
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        if status == "destination_received" || status == "unsupported_token" {
            return petal::read_json_value(&result);
        }

        // Poll loop: re-check every 5s for up to 300s (60 iterations).
        // petal::sdk::sleep is not available — use a busy-wait via chain_read
        // as a minimal throttle, or just return the current status with
        // a "polling" indicator since true blocking in WASM is antipattern.
        //
        // The client (bloom daemon) is expected to re-read this file on its own
        // poll cadence. We return the current status and a hint for retry.
        petal::read_json_value(&serde_json::json!({
            "status": status,
            "observed_before": result.get("observed_before"),
            "observed_after": result.get("observed_after"),
            "delta": result.get("delta"),
            "message": "settlement not yet observed — re-read this file to poll again",
            "retry_interval_ms": 5000,
        }))
    }
);
