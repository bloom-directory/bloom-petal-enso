# Bloom Petal Enso — Completion Plan

**Created:** 2026-07-29
**Goal:** Replace `bloom-defi` + `bloom-vfs/handlers/defi.rs` with `bloom-petal-enso`, remove old code from bloom, set enso as default preinstalled petal.

---

## Current State

**Commit:** `de2aa21` on `main` (uncommitted: clean)
**Tests:** 39 passing, 0 failures, 0 warnings
**LOC:** ~3,127 across 12 source modules + 18 route files

### What the rewrite already addressed (from the original 38 critical gaps)

- ✅ ERC-20 allowance check + approve staging (`workflow.rs::create`)
- ✅ Route input mismatch rejection at create time
- ✅ Wallet ownership check at confirm
- ✅ Route binding re-verification at confirm (from, tx.from, input_matches_request, chain_id)
- ✅ Multi-intent sequential staging (approve → route)
- ✅ Cross-chain token_out resolution on destination chain
- ✅ Cross-chain receiver default (sender EOA)
- ✅ Per-intent state tracking (`IntentState` with outbox_id, tx_hash)
- ✅ Settlement baseline observation + balance delta verification (`settlement.rs`)
- ✅ Simulation via eth_call with basic revert decoding (`simulation.rs`)
- ✅ Policy evaluation (route_verified, erc20_approval warn, cross_chain warn, receiver class)
- ✅ Receiver classification
- ✅ Native token skip for approve
- ✅ On-chain decimal resolution for hex tokens
- ✅ Session route_request persistence for re-verification
- ✅ Plan markdown with receiver, router, protocols, policy checks
- ✅ tx.json returns full intent list

---

## Remaining Work

### Phase 1: Missing Route Files (small, high-impact)

| Task | Priority | Effort |
|------|----------|--------|
| `policy_check.json.rs` — return `session.policy_checks` | critical | 15 min |
| `wait_settlement.rs` — blocking poll loop (read balance every 5s up to 300s) | critical | 45 min |
| `destination_chain.txt.rs` — return `session.destination_chain` | medium | 5 min |
| Add all three to `$index.rs` child list | — | 5 min |

### Phase 2: Sequential Dependency (critical for ERC-20 swaps)

The approve tx must mine before the route tx. Currently both are staged independently.

| Task | Priority | Effort |
|------|----------|--------|
| Add `depends_on: Option<String>` to `IntentState` | critical | 10 min |
| After staging approve, set route intent's `depends_on = approve.outbox_id` | critical | 20 min |
| Wire through to `tx_stage` if SDK supports dependency ordering | — | investigate |

### Phase 3: Simulation Wiring into Create (important for safety)

The `simulate_route()` function exists but isn't called during `create`.

| Task | Priority | Effort |
|------|----------|--------|
| Call `simulate_route()` in `create()` after route verification | important | 20 min |
| Store result in `session.simulation` | — | included |
| Reject route if simulation reverts with actionable error | important | 15 min |
| Call Enso Quoter `validate()` endpoint for additional checks | important | 30 min |

### Phase 4: Token Registry Expansion (important for coverage)

Current table has ~20 entries. Original uses `bloom_proto::tokens` with full registry.

| Task | Priority | Effort |
|------|----------|--------|
| Add all major tokens across Arbitrum, BNB, Avalanche | important | 1 hr |
| Add WBTC, DAI, LINK, UNI, AAVE, PEPE on Ethereum | important | 30 min |
| Add WETH variants on L2s | important | 30 min |
| Document approach for arbitrary hex-token fallback | medium | 15 min |

### Phase 5: Plan Markdown Polish (medium)

| Task | Priority | Effort |
|------|----------|--------|
| Add policy section (PASS/WARN/DENY per check) | important | 20 min |
| Add slippage bps display | medium | 5 min |
| Add tx value (wei) and data length | medium | 10 min |
| Add receiver checksum address | medium | 10 min |
| Add confirm instructions with tx count | nice-to-have | 10 min |

### Phase 6: Deferred / Out of Scope for Initial Cutover

These features exist in `bloom-defi` but can be added after the petal is live:

| Feature | Why Deferable |
|---------|--------------|
| Price oracle / USD valuation | Used for policy thresholds; petal has simpler policy checks. Can add when oracle is available as a petal cap. |
| Hyperliquid deposit flow | Separate intent type; can add as a second phase after swap flow is proven. |
| Bundle endpoint | Multi-step bundles; not needed for standard swaps. |
| Address book integration | Receiver alias resolution; nice-to-have but not blocking. |
| Full revert decoder (DecoderChain) | We have basic `Error(string)` decode. Can extend later. |
| Auth services | Not applicable in petal context (caps replace auth). |
| Legacy session migration | New petal starts fresh. |

---

## Validation Plan

### Step 1: Compile check
- `cargo test --manifest-path route/Cargo.toml` — all tests pass
- `scripts/build.sh` — petal builds to WASM successfully

### Step 2: Feature parity audit
- Line-by-line comparison of `workflow.rs::create` vs `defi.rs::create_session` (lines 507-1000)
- Line-by-line comparison of `workflow.rs::confirm` vs `defi.rs::confirm_session` (lines 1095-1260)
- Verify every VFS path from `defi.rs` has a corresponding route file

### Step 3: Integration testing (requires bloom runtime)
- Create a swap intent on testnet
- Read plan.md, tx.json, simulation.json, status.json
- Confirm → verify intents staged in outbox
- Read settlement.json → verify balance delta
- Test ERC-20 swap (triggers approve flow)
- Test native ETH swap (no approve)
- Test cross-chain swap

### Step 4: Cutover
- Push `bloom-petal-enso` to GitHub
- Add to bloom's preinstalled petal list
- Remove `bloom-defi` crate from workspace
- Remove `bloom-vfs/handlers/defi.rs`
- Update bloom daemon to route `defi/` VFS to the enso petal

---

## Execution Approach

**Recommended:** Use subagents for Phases 1-5 (independent, well-scoped tasks). Main session handles Phase 6 decisions and validation.

**Risk:** zai connection errors may interrupt subagents. Mitigation: `api_max_retries` now 8, and each subagent task is small enough to retry.

**Timeline estimate:** 4-6 hours of focused work to complete Phases 1-5. Phase 6 (validation + cutover) depends on having bloom runtime access.

---

## File Inventory

```
route/src/
  lib.rs          — module exports
  api.rs          — Enso API client (route, quote, simulate, validate)
  api_types.rs    — RouteRequest, RouteResponse, ABI types, SimulateResponse
  input.rs        — NL intent parser, token symbol table
  runtime.rs      — Host trait, BloomHost, ERC-20 chain helpers
  session.rs      — Session, PreparedIntent, IntentState
  settings.rs     — API key resolution
  workflow.rs     — create(), confirm(), load(), helpers
  simulation.rs   — eth_call with revert decoding
  settlement.rs   — balance delta verification
  redaction.rs    — error message sanitization

route/files/
  $index.rs                           — root dir listing
  intents/$index.rs                   — wallet listing
  intents/[wallet]/$index.rs          — session listing
  intents/[wallet]/new.rs             — create session (write)
  intents/[wallet]/[id]/$index.rs     — session file listing
  intents/[wallet]/[id]/intent.txt.rs
  intents/[wallet]/[id]/route.json.rs
  intents/[wallet]/[id]/plan.md.rs
  intents/[wallet]/[id]/tx.json.rs
  intents/[wallet]/[id]/simulation.json.rs
  intents/[wallet]/[id]/settlement.json.rs
  intents/[wallet]/[id]/status.json.rs
  intents/[wallet]/[id]/confirm.rs    — stage to outbox (write)
  meta/route-contract.json.rs
  settings/api-key.rs
  settings/status.json.rs
```
