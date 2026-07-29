# Bloom Petal Enso — Release Plan

Updated: 2026-07-29

## Completed

- 27 route controllers expose create, inspection, ceremony, abandon, outbox,
  receipt, settlement, latest, metadata, and credential surfaces.
- Natural-language amounts, chain identity, token decimals, route input,
  signed wallet DeFi policy, slippage, router, receiver allowlist, protocols,
  and native value are checked fail-closed.
- Route simulation must pass before route staging.
- ERC-20 approval is exact-amount and uses a two-confirmation lifecycle:
  approval is staged alone, must have a successful receipt and sufficient live
  allowance, then the route is re-simulated and staged.
- Settlement requires a successful route receipt and a destination balance
  delta at least equal to Enso's quoted output.
- Host tests, strict Clippy, architecture checks, route component build,
  Petal validation, Bloom package validation, and Minnow read-only regression
  form the release gate.

## Release sequence

1. Run the complete validation gate and a non-broadcast Minnow regression.
2. Create `bloom-directory/bloom-petal-enso`, commit the current tree, and
   publish a pinned release.
3. Add the pinned Enso release to Bloom's preinstalled Petal catalog and
   default set.
4. Replace stale native `/defi` documentation/tests with
   `/petals/enso`, validate Bloom, commit, and push.
5. Install Enso in the server's persistent Bloom home, move the API key into
   Petal secret storage, remove the plaintext runtime fallback, and restart
   only if the running daemon requires it.

## Explicit production boundary

Enso Router V2 action bytes are still opaque to this Petal. A wallet with
`require_calldata_verification = true` is refused. Setting it to `false`
accepts explicit receiver/min-output warnings and requires human review. This
mode is not approved for unattended autonomous value movement.

Deferred feature parity: Enso bundles, Hyperliquid deposit compatibility,
address-book receiver classification, and a trusted USD oracle for
`max_input_usd`. Unsupported configured route-policy controls fail closed.
