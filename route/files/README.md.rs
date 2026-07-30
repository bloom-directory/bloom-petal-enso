petal::route_file!(spec: petal::static_read_spec(), read: |_ctx: &petal::Ctx| {
    petal::DispatchResponse::Read(br#"# DeFi Intents (Enso Shortcuts)

## Quick Start for Agents

### 1. Create an intent
```json
 write: /petals/enso/intents/<wallet>/new
 example: {"intent":"swap 100 usdc to eth","chain":"ethereum"}
 or just NL text: swap 100 usdc to eth
```

### 2. Inspect the plan
```json
 read: /petals/enso/intents/<wallet>/<session>/plan.md
 read: /petals/enso/intents/<wallet>/<session>/route.json
 read: /petals/enso/intents/<wallet>/<session>/tx.json
 read: /petals/enso/intents/<wallet>/<session>/simulation.json
 read: /petals/enso/intents/<wallet>/<session>/policy_check.json
```

### 3. Confirm
```json
 write: /petals/enso/intents/<wallet>/<session>/confirm
 body: confirm
```

### 4. Verify settlement
```json
 read: /petals/enso/intents/<wallet>/<session>/settlement.json
 read: /petals/enso/intents/<wallet>/<session>/wait_settlement.json
```

## Safety Model

- Route discovery uses the Enso Shortcuts API and Petal secret storage
- The signed wallet DeFi policy is enforced at create and confirm
- Simulation must pass before the route transaction is staged
- ERC-20 approval is exact-amount and must succeed before a second confirm
- Same-chain ERC-20 settlement requires an attributable receipt Transfer
- Native and cross-chain balance changes are reported as unattributed
"#.to_vec())
});
