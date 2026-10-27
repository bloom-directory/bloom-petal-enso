petal::route_file!(spec: petal::static_read_spec(), read: |_ctx: &petal::Ctx| {
    petal::DispatchResponse::Read(br#"# DeFi Intents (Enso Shortcuts)

## Quick Start for Agents

### Default venue settings

No configuration write is needed. Read and override preferences at:
`/petals/enso/settings/<wallet>/<index>/venue.toml`.
Defaults enable swaps on 12 chains: Ethereum, Base, Tempo, Robinhood Chain,
Arbitrum, Optimism, Polygon, BNB Smart Chain, Avalanche, Gnosis, Linea and
HyperEVM. Use `hyperliquid` for HyperEVM and
`robinhood` for Robinhood Chain. Use token contract addresses for symbols
not in the static registry. Each chain uses its Enso Router V2, to the wallet
itself, with at most
100 bps slippage (requests default to 50 bps).
Same-chain swaps carry an on-chain receiver and minimum-output check; the
plan states it. Cross-chain swaps and missing protocol metadata produce
warnings: review the plan before owner approval and broadcast.
Write the full TOML document to customize; `[defi]` with `enabled = false`
disables swaps. Existing saved restrictions take precedence over defaults.

### 1. Create an intent
```json
 write: /petals/enso/intents/<wallet>/<index>/new
 example: {"intent":"swap 100 usdc to eth","chain":"ethereum"}
 or just NL text: swap 100 usdc to eth on ethereum
 The source chain is required: set "chain" or end the text with `on <chain>`.
```

### 2. Inspect the plan
```json
 read: /petals/enso/intents/<wallet>/<index>/<session>/plan.md
 read: /petals/enso/intents/<wallet>/<index>/<session>/route.json
 read: /petals/enso/intents/<wallet>/<index>/<session>/tx.json
 read: /petals/enso/intents/<wallet>/<index>/<session>/simulation.json
 read: /petals/enso/intents/<wallet>/<index>/<session>/policy_check.json
```

### 3. Confirm
```json
 write: /petals/enso/intents/<wallet>/<index>/<session>/confirm
 body: confirm
```

A quote expires at Enso's `validUntil`, or five minutes after the Petal
fetched it when Enso sends none. When the stored quote is older than 30
seconds or has expired, this confirm fetches a fresh one, asking
Enso for the reviewed minimum output (reviewed quote less slippage), and
stages it only if it keeps the reviewed router and native value and its
minimum output reaches that floor. Otherwise nothing is staged and
`status.json` `last_error` says why: for a route outside the reviewed bounds,
abandon the intent and create a new one; for a temporary Enso or network
failure, write `confirm` again.
Confirm the outbox entry promptly after staging, and ask the owner to approve
within about two minutes: Enso quotes expire quickly, and an approval binds
the exact transaction, so a late approval reverts in simulation rather than
refreshing. `status.json` `quote` gives `expires_at_ms`. Once a staged, unsent
quote expires, discard the entry by writing `cancel` to its outbox
`pending/<id>/confirm` file, write `abandon` here, and create a new intent.

### 4. Verify settlement
```json
 read: /petals/enso/intents/<wallet>/<index>/<session>/settlement.json
 read: /petals/enso/intents/<wallet>/<index>/<session>/wait_settlement.json
```

## Safety Model

- Route discovery uses the Enso Shortcuts API and Petal secret storage
- Enso-owned venue preferences are enforced at create and confirm
- Bloom wallet policy and approval limits remain host-enforced
- Simulation must pass before the route transaction is staged
- An aged quote is refreshed at staging only within the reviewed bounds
- ERC-20 approval is exact-amount and must succeed before a second confirm
- Successful protected same-chain swaps report destination_minimum_enforced;
  the router's atomic output check passed. Later balance changes include fees
  and are informational, not a reason to repeat a successful swap
- Cross-chain source success reports destination_unverified; this package
  does not track destination settlement
- Only source_pending carries a retry hint; an outbox inspection that Machine
  refuses or cannot find returns error
"#.to_vec())
});
