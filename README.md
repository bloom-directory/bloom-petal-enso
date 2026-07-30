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

## Safety Model

- Route discovery uses the Enso Shortcuts API (requires an API key)
- Whole-number natural-language amounts are token units (`100 USDC` means
  `100000000` base units)
- Route source asset, amount, sender, and native value are verified against
  the Enso Router V2 calldata envelope
- The wallet's current signed `[defi]` policy is evaluated at create and
  confirm; a stale or unsigned passkey policy fails closed
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

## Development

```sh
scripts/check-route-architecture.sh
cargo test --manifest-path route/Cargo.toml
scripts/build.sh
petal check --root .
```
