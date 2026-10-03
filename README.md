# bloom-petal-enso

Bloom petal for [Enso Shortcuts](https://enso.finance) — DeFi route discovery,
simulation, and swap execution through the Bloom transaction pipeline.

## Quick Start for Agents

### 1. Create an intent

```
write: /petals/enso/intents/<wallet>/<index>/new
body:  {"intent":"swap 100 usdc to eth","chain":"ethereum"}
  or:  swap 100 usdc to eth on ethereum
```

The source chain is required: set `"chain"` or end the text with `on <chain>`.

### 2. Inspect the plan

```
read: /petals/enso/intents/<wallet>/<index>/<session>/plan.md
read: /petals/enso/intents/<wallet>/<index>/<session>/route.json
read: /petals/enso/intents/<wallet>/<index>/<session>/tx.json
read: /petals/enso/intents/<wallet>/<index>/<session>/simulation.json
```

### 3. Confirm

```
write: /petals/enso/intents/<wallet>/<index>/<session>/confirm
body:  confirm
write: /wallets/<wallet>/<index>/chains/<chain>/outbox/pending/<id>/confirm  # Broadcast
```

For an ERC-20 route that needs approval, the first Petal confirmation stages
only an exact-amount approval. Broadcast it and wait for a successful receipt,
then write `confirm` to the Petal again. Only then is the swap simulated and
staged. The route transaction is never placed in the outbox alongside a
pending approval.

A refused confirmation reaches a mounted filesystem only as an I/O error
(`EIO`). Its reason is recorded in the session's `status.json` as
`last_error` (for example, an approval that has not yet been broadcast and
received); read it after any failed write to `confirm`. A successful
confirmation clears it.

Each quote's lifetime comes from Enso's route response: `validUntil` (Unix
seconds) when Enso sends it, otherwise five minutes after the Petal fetched the
route. The owner's approval can take minutes after staging, so staging
refreshes a quote older than 30 seconds, or one that has expired. The refresh asks Enso for the reviewed minimum output
(the reviewed quote less its slippage tolerance) as `minAmountOut`, so the
swap can never fill below what the owner reviewed. The fresh route must match
the stored request, keep the reviewed router and native value, report a
minimum output at least that floor (without a reported minimum, its quote
less the slippage must reach it), and pass the venue preferences again;
otherwise nothing is staged and `status.json` `last_error` explains the
refusal. A route outside those bounds means abandoning the intent; a
temporary Enso or network failure only needs another `confirm`. `status.json`
reports `quote.fetched_at_ms`, `quote.expires_at_ms`, `quote.expires_from`
(`enso_valid_until` or `fetch_time`) and `quote.expired`. When
a staged route's quote expires while its outbox entry is still pending,
`last_error` tells the agent to cancel that entry and create a new intent,
because confirming an expired route can revert or be refused.

## Configuration

Set the package-wide Enso API key once for all accounts:

```
write: /petals/enso/settings/api-key
body:  your-enso-api-key-here
```

Release builds can embed the repository secret `ENSO_API_KEY`. A key written to
`settings/api-key` takes precedence over that embedded release credential. The
runtime setting `enso-api-key` remains a compatibility fallback, and
`settings/status.json` reports the selected source without exposing the key.

Per-account Enso venue preferences live at `settings/<wallet>/<index>/venue.toml` in
the Petal's own state. No setup write is needed: an unconfigured wallet reads
and uses the bundled defaults. Swaps are enabled on all 13 chains supported by Bloom and Enso
(Ethereum, Base, Tempo, Robinhood Chain, Arbitrum, Optimism, Polygon, BNB Smart
Chain, Avalanche, Gnosis, Linea, HyperEVM, and Arc), with the
[canonical Enso Router V2](https://docs.enso.build/pages/build/reference/deployments.md)
allowlisted on each chain, the wallet itself as receiver, and a 100 bps (1%)
slippage ceiling. The default request slippage remains 50 bps (0.5%).
HyperEVM uses Bloom's chain key `hyperliquid`; Robinhood Chain uses `robinhood`.
Chain allowlisting is independent of the static token-symbol registry: use token
contract addresses when a symbol is not in the registry for that chain.

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
| `intents/` | dir | Lists authenticated wallets |
| `intents/<wallet>/` | dir | Lists canonical numbered accounts |
| `intents/<wallet>/<index>/` | dir | `new` + session ids |
| `intents/<wallet>/<index>/new` | writable | Create a new swap intent |
| `intents/<wallet>/<index>/<id>/intent.txt` | file | Original intent text |
| `intents/<wallet>/<index>/<id>/route.json` | file | Full Enso route response |
| `intents/<wallet>/<index>/<id>/plan.md` | file | Human-readable transaction plan |
| `intents/<wallet>/<index>/<id>/tx.json` | file | Prepared EVM transaction |
| `intents/<wallet>/<index>/<id>/simulation.json` | file | Simulation result |
| `intents/<wallet>/<index>/<id>/settlement.json` | file | Settlement status |
| `intents/<wallet>/<index>/<id>/status.json` | file | Session status |
| `intents/<wallet>/<index>/<id>/confirm` | writable | Stage into outbox |
| `settings/status.json` | file | API key credential status |
| `settings/api-key` | writable | Write Enso API key |
| `settings/<wallet>/<index>/venue.toml` | writable | Configure Enso-owned advisory venue preferences |

## Development

```sh
scripts/check-route-architecture.sh
cargo test --manifest-path route/Cargo.toml
scripts/build.sh
petal check --root .
```

## Account-scoped routes

Operations use `/petals/enso/intents/<wallet>/<index>/`; service credentials use the global `/petals/enso/settings/` routes. Bloom resolves the explicit adjacent wallet and canonical numbered index from its live authenticated account projection, then supplies trusted `bloom.wallet` and `bloom.account`. Every index, including 0, has a uniform private store for account state. The manifest shares only the exact service credential keys through the package-global store. Public metadata and documentation remain unscoped. Old packages and custom packages require a separate update; no legacy account-0 storage or old-host fallback is supported. The core wallet tree remains `/wallets/<wallet>/<index>/`.

Existing installed state must be retained through the storage cutover. Pending
sessions contain exact outbox and settlement correlation data needed for manual
reconciliation. This package does not migrate or remove that state. Inspect
pending sessions before replacing an installation; automatic recovery across
the old storage layout is unsupported.
