# bloom-petal-enso

Bloom petal for [Enso Shortcuts](https://enso.finance) — DeFi route discovery,
simulation, and swap execution through the Bloom transaction pipeline.

## Quick Start for Agents

### 1. Create an intent

```
write: /intents/<wallet>/new
body:  {"intent":"swap 100 usdc to eth","chain":"ethereum"}
  or:  swap 100 usdc to eth
```

### 2. Inspect the plan

```
read: /intents/<wallet>/<session>/plan.md
read: /intents/<wallet>/<session>/route.json
read: /intents/<wallet>/<session>/tx.json
read: /intents/<wallet>/<session>/simulation.json
```

### 3. Confirm

```
write: /intents/<wallet>/<session>/confirm    # Stage into outbox
write: /wallets/<wallet>/chains/<chain>/outbox/pending/<id>/confirm  # Broadcast
```

## Configuration

Set the Enso API key:

```
write: /settings/api-key
body:  your-enso-api-key-here
```

Or configure via runtime setting `enso-api-key`.

## Safety Model

- Route discovery uses the Enso Shortcuts API (requires an API key)
- Route input is verified against the Enso Router V2 calldata envelope
- Broadcast requires the standard outbox confirm (owner gate)
- Settlement verification for cross-chain routes: read settlement.json

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
