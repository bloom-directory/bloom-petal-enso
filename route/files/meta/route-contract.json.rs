petal::route_file!(spec: petal::static_read_spec(), read: |_ctx: &petal::Ctx| {
    petal::DispatchResponse::Read(br#"{
  "name": "enso",
  "version": "0.1.0",
  "description": "Enso Shortcuts DeFi routing - swap intents via the Enso API.",
  "capabilities": ["bloom:http", "bloom:store", "bloom:tx.outbox", "bloom:chain", "bloom:vfs.read"],
  "network": {
    "api.enso.finance": ["GET /api/v1/shortcuts/route"]
  }
}"#.to_vec())
});
