# bloom-petal-enso

Bloom petal for [Enso Shortcuts](https://enso.finance) — DeFi route discovery,
simulation, and swap execution through the Bloom transaction pipeline.

## Quick Start for Agents

### 1. Create an intent

```
write: /petals/enso/intents/<wallet>/new
body:  {"intent":"swap 100 usdc to eth","chain":"ethereum"}
  or:  swap 100 usdc to eth
```

### 2. Inspect the plan

```
read: /petals/enso/intents/<wallet>/<session>/plan.md
read: /petals/enso/intents/<wallet>/<session>/route.json
read: /petals/enso/intents/<wallet>/<session>/tx.json
read: /petals/enso/intents/<wallet>/<session>/simulation.json
```

### 3. Confirm

```
write: /petals/enso/intents/<wallet>/<session>/confirm
body:  confirm
write: /wallets/<wallet>/chains/<chain>/outbox/pending/<id>/confirm  # Broadcast
```

For an ERC-20 route that needs approval, the first Petal confirmation stages
only an exact-amount approval. Broadcast it and wait for a successful receipt,
then write `confirm` to the Petal again. Only then is the swap simulated and
staged. The route transaction is never placed in the outbox alongside a
pending approval.

## Configuration

Set the Enso API key:

```
write: /petals/enso/settings/api-key
body:  your-enso-api-key-here
```

Release builds can embed the repository secret `ENSO_API_KEY`. A key written to
`settings/api-key` takes precedence over that embedded release credential. The
runtime setting `enso-api-key` remains a compatibility fallback, and
`settings/status.json` reports the selected source without exposing the key.

Per-wallet Enso venue preferences live at `settings/wallets/<wallet>/venue.toml` in
the Petal's own state. No setup write is needed: an unconfigured wallet reads
and uses the bundled defaults. Swaps are enabled on all seven supported chains
(Ethereum, Polygon, Base, Optimism, Arbitrum, BNB, and Avalanche), with the
[canonical Enso Router V2](https://docs.enso.build/pages/build/reference/deployments)
allowlisted on each chain, the wallet itself as receiver, and a 100 bps (1%)
slippage ceiling. The default request slippage remains 50 bps (0.5%).

Read this file to see the full effective TOML; edit it and write the full document
back to override it. To disable swaps, write `[defi]` followed by `enabled = false`.
Existing configuration at the former `settings/<wallet>/venue.toml` storage key
is retained as a fallback until settings are written at the new path. Explicit
configuration is never merged with permissive defaults: omitted fields retain
their conservative behavior, and malformed configuration fails closed.

Defaults permit missing protocol metadata and disable strict calldata
verification, with explicit review warnings. Receiver and minimum-output
parameters are not fully proven from Router V2 action bytes; inspect the plan
before approving a transaction. These are advisory application preferences only:
Bloom's Broker/Signer-authoritative wallet policy, approval budgets, and signing
limits remain host-enforced and
are never interpreted by this Petal.

## Safety Model

- Route discovery uses the Enso Shortcuts API (requires an API key)
- Whole-number natural-language amounts are token units (`100 USDC` means
  `100000000` base units)
- Route source asset, amount, sender, and native value are verified against
  the Enso Router V2 calldata envelope
- The Petal's `[defi]` venue preferences are evaluated at create and confirm;
  bundled defaults apply to unconfigured wallets, while authoritative wallet
  policy is independently enforced by Bloom when a transaction is staged
- Simulation must pass before the route transaction is staged
- ERC-20 approval is exact-amount and must have a successful receipt first
- Broadcast requires the standard outbox confirm (owner gate)
- Same-chain ERC-20 settlement requires a successful source receipt containing
  an attributable `Transfer` to the receiver for at least Enso's quoted output
- Cross-chain and native-output balance increases are reported as observed but
  unattributed; they are never presented as confirmed settlement

Enso's opaque Router V2 action bytes are not fully decoded by this version.
If `require_calldata_verification = false`, the plan reports explicit
receiver/min-output warnings. That mode is suitable only for operator-reviewed
transactions, not unattended autonomous value movement.

## Route Surface

| Route | Kind | Description |
| --- | --- | --- |
| `intents/` | dir | Lists wallets with sessions |
| `intents/<wallet>/` | dir | `new` + session ids |
| `intents/<wallet>/new` | writable | Create a new swap intent |
| `intents/<wallet>/<id>/intent.txt` | file | Original intent text |
| `intents/<wallet>/<id>/route.json` | file | Full Enso route response |
| `intents/<wallet>/<id>/plan.md` | file | Human-readable transaction plan |
| `intents/<wallet>/<id>/tx.json` | file | Prepared EVM transaction |
| `intents/<wallet>/<id>/simulation.json` | file | Simulation result |
| `intents/<wallet>/<id>/settlement.json` | file | Settlement status |
| `intents/<wallet>/<id>/status.json` | file | Session status |
| `intents/<wallet>/<id>/confirm` | writable | Stage into outbox |
| `settings/status.json` | file | API key credential status |
| `settings/api-key` | writable | Write Enso API key |
| `settings/wallets/<wallet>/venue.toml` | writable | Configure Enso-owned advisory venue preferences |

## Development

```sh
scripts/check-route-architecture.sh
cargo test --manifest-path route/Cargo.toml
scripts/build.sh
petal check --root .
```
