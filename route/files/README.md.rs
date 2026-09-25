petal::route_file!(spec: petal::static_read_spec(), read: |_ctx: &petal::Ctx| {
    petal::DispatchResponse::Read(br#"# DeFi Intents (Enso Shortcuts)

## Quick Start for Agents

### Default venue settings

No configuration write is needed. Read and override preferences at:
`/petals/enso/settings/wallets/<wallet>/venue.toml`.
Defaults enable swaps on all 13 Bloom/Enso chains: Ethereum, Base, Tempo,
Robinhood Chain, Arbitrum, Optimism, Polygon, BNB Smart Chain, Avalanche,
Gnosis, Linea, HyperEVM, and Arc. Use `hyperliquid` for HyperEVM and
`robinhood` for Robinhood Chain. Use token contract addresses for symbols
not in the static registry. Each chain uses its Enso Router V2, to the wallet
itself, with at most
100 bps slippage (requests default to 50 bps).
Missing protocol metadata and unverified receiver/minimum-output calldata
produce warnings: review the plan before owner approval and broadcast.
Write the full TOML document to customize; `[defi]` with `enabled = false`
disables swaps. Existing saved restrictions take precedence over defaults.

### 1. Create an intent
```json
 write: /petals/enso/intents/<wallet>/new
 example: {"intent":"swap 100 usdc to eth","chain":"ethereum"}
 or just NL text: swap 100 usdc to eth on ethereum
 The source chain is required: set "chain" or end the text with `on <chain>`.
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

Bloom refuses an Enso quote older than five minutes on every outbox
confirm, including the one after the owner's approval. When the stored
quote is older than 30 seconds, this confirm fetches a fresh one and stages
it only if it keeps the reviewed router and native value and quotes at least
the reviewed output less its slippage. Otherwise nothing is staged and
`status.json` `last_error` says why: abandon the intent and create a new one.
Confirm the outbox entry promptly after staging. `status.json` `quote` gives
`expires_at_ms`; once a staged, unsent quote expires, `last_error` says to
cancel that outbox entry and create a new intent. Retrying cannot help.

### 4. Verify settlement
```json
 read: /petals/enso/intents/<wallet>/<session>/settlement.json
 read: /petals/enso/intents/<wallet>/<session>/wait_settlement.json
```

## Safety Model

- Route discovery uses the Enso Shortcuts API and Petal secret storage
- Enso-owned venue preferences are enforced at create and confirm
- Bloom wallet policy and approval limits remain host-enforced
- Simulation must pass before the route transaction is staged
- An aged quote is refreshed at staging only within the reviewed bounds
- ERC-20 approval is exact-amount and must succeed before a second confirm
- Same-chain ERC-20 settlement requires an attributable receipt Transfer
- Native and cross-chain balance changes are reported as unattributed
"#.to_vec())
});
