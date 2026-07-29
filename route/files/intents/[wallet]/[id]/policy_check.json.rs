petal::route_file!(spec: petal::store_read_spec().caps(&["bloom:store"]), read: |ctx: &petal::Ctx| {
    let wallet = match petal::param(ctx, "wallet") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let id = match petal::param(ctx, "id") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut host = crate::workflow::BloomHost;
    match crate::workflow::load(&mut host, wallet, id) {
        Ok(session) => {
            let mut overall = "pass";
            let checks = session.policy_checks.as_array().cloned().unwrap_or_default();
            for check in &checks {
                if let Some(outcome) = check.get("outcome").and_then(|v| v.as_str()) {
                    if outcome == "deny" {
                        overall = "deny";
                        break;
                    }
                    if outcome == "warn" && overall != "deny" {
                        overall = "warn";
                    }
                }
            }
            petal::read_json_value(&serde_json::json!({
                "overall": overall,
                "checks": checks,
            }))
        }
        Err(error) => petal::error(-1, error),
    }
});
