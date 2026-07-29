petal::route_file!(spec: petal::static_spec(), read: |_ctx: &petal::Ctx| {
    petal::DispatchResponse::Read(br#"# DeFi Intents (Enso Shortcuts)

## Quick Start for Agents

### 1. Create an intent
```json
 write: /defi/intents/<wallet>/new
 example: {"intent":"swap 100 usdc to eth","chain":"ethereum"}
 or just NL text: swap 100 usdc to eth
```

### 2. Inspect the plan
```json
 read: /defi/intents/<wallet>/<session>/plan.md
 read: /defi/intents/<wallet>/<session>/route.json
 read: /defi/intents/<wallet>/<session>/tx.json
 read: /defi/intents/<wallet>/<session>/simulation.json
 read: /defi/intents/<wallet>/<session>/policy_check.json
```

### 3. Confirm
```json
 write: /defi/intents/<wallet>/<session>/confirm    # Stage into outbox
```

### 4. Verify settlement
```json
 read: /defi/intents/<wallet>/<session>/settlement.json
 read: /defi/intents/<wallet>/<session>/wait_settlement.json
```

## Safety Model

- Route discovery uses the Enso Shortcuts API (requires BLOOM_ENSO_KEY)
- Simulation runs during create and on each read; reverts are decoded
- Policy checks re-evaluate at confirm time; deny outcomes block staging
- Auto-approve handles ERC-20 allowances when needed
- Settlement verification for cross-chain routes
"#.to_vec())
});
