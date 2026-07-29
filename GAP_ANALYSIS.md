# Comprehensive Gap Analysis: bloom-defi + bloom-vfs/defi.rs → bloom-petal-enso

**Generated:** 2026-07-29
**Sources compared:**
- Original: `bloom/crates/bloom-defi/src/lib.rs` (1351 lines)
- Original: `bloom/crates/bloom-vfs/src/handlers/defi.rs` (2433 lines)
- Petal: `bloom-petal-enso/route/src/*.rs` (9 modules)
- Petal: `bloom-petal-enso/route/files/**/*.rs` (18 route files)

---

## Executive Summary

The petal has **solid API-type parity** (all Enso route/quote/simulate/validate wire types are ported verbatim) and a **working create→confirm flow for simple single-token swaps**. However, it is missing **critical safety, verification, and multi-step features** from the original:

| Category | Ported | Missing | Partial |
|----------|--------|---------|---------|
| API types/structs | 15 | 3 | 2 |
| API endpoints | 4/5 | 1 (bundle) | 0 |
| VFS paths | 8/15 | 4 | 3 |
| Safety/verification features | 2 | 14 | 3 |
| Token resolution | partial | full registry | — |
| Session model fields | 12 | 8 | 0 |

**Critical gaps** (18 items) center on: ERC-20 approval staging, revert decoding, policy enforcement, receiver classification, settlement verification, simulation re-run, and route binding verification at confirm.

---

## Part A: bloom-defi/src/lib.rs — Domain Crate Gaps

### A1. Types / Structs / Enums

| # | Original (line) | Type | Petal Status | Details |
|---|-----------------|------|--------------|---------|
| A1.1 | lib.rs:45-61 | `EnsoError` enum (7 variants) | **NO** | Petal uses plain `String` for all errors. No structured error type with `Http`, `Json`, `Url`, `Api{status,body}`, `Disabled`, `MissingKey`, `InvalidIntent` variants. Error context (HTTP status codes, API key state) is lost. |
| A1.2 | lib.rs:64-84 | `RoutingStrategy` enum | **YES** | api_types.rs:24-41 — identical (Router/Delegate/EnsoWallet + `as_str()`). ✅ |
| A1.3 | lib.rs:87-150 | `RouteRequest` struct | **YES** | api_types.rs:44-113 — identical fields, `new()`, `to_query()`. Petal adds `to_url()`. ✅ |
| A1.4 | lib.rs:155-161 | `BundleStep` struct | **NO** | Multi-step bundle support entirely absent. |
| A1.5 | lib.rs:163-170 | `BundleRequest` struct | **NO** | Multi-step bundle support entirely absent. |
| A1.6 | lib.rs:173-181 | `RouteTx` struct | **YES** | api_types.rs:116-124 — identical. ✅ |
| A1.7 | lib.rs:186-314 | `RouteResponse` struct + 3 methods | **YES** | api_types.rs:127-214 — all three methods (`input_matches_request`, `protocols`, `calldata_contains_receiver`) ported verbatim. ✅ |
| A1.8 | lib.rs:316-324 | `BundleResponse` struct | **NO** | No bundle support. |
| A1.9 | lib.rs:326-335 | `QuoteResponse` struct | **YES** | api_types.rs:217-225 — identical. ✅ |
| A1.10 | lib.rs:337-531 | `EnsoClient` struct | **PARTIAL** | Replaced by `api.rs` functions. `route()` ✅ and `quote()` ✅ are wired. `bundle()` ❌ is missing. `from_env()` → replaced by `settings::resolve_api_key()`. `with_base_url()`/`base_url()`/`api_key()` accessors absent (base URLs are hardcoded constants). |
| A1.11 | lib.rs:543 | `DEFAULT_QUOTER_URL` const | **PARTIAL** | Hardcoded as `QUOTER_BASE` in api.rs:11. Not configurable. |
| A1.12 | lib.rs:551-573 | `QuoterTx` struct + `from_route_tx()` | **YES** | api_types.rs:233-254 — identical. ✅ |
| A1.13 | lib.rs:576-588 | `SimulateRequest` struct | **YES** | api_types.rs:256-268 — identical. ✅ |
| A1.14 | lib.rs:591-596 | `ValidateRequest` struct | **YES** | api_types.rs:270-275 — identical. ✅ |
| A1.15 | lib.rs:599-651 | `SimulateResponse` + 3 methods | **YES** | api_types.rs:277-312 — all methods ported (`status_success`, `amount_out_nonempty`, `produced_output`). ✅ |
| A1.16 | lib.rs:608-631 | `SimulateResult` struct | **YES** | api_types.rs:286-298 — identical. ✅ |
| A1.17 | lib.rs:654-671 | `ValidateResponse` + `ValidateChecks` | **YES** | api_types.rs:314-331 — identical. ✅ |
| A1.18 | lib.rs:675-748 | `QuoterClient` struct | **PARTIAL** | `simulate()` ✅ and `validate()` ✅ wired via api.rs. `from_env()` → shared key resolution. `with_base_url()`/`base_url()` absent. |
| A1.19 | lib.rs:801-809 | `NaturalIntent` struct | **YES** | input.rs:59-66 — identical. ✅ |
| A1.20 | lib.rs:32-43 | `IEnsoRouter` sol! interface | **YES** | api_types.rs:10-21 — identical. ✅ |
| A1.21 | lib.rs:24,27 | `DEFAULT_BASE_URL`, `NATIVE_TOKEN` consts | **YES** | api_types.rs:7 (`NATIVE_TOKEN`), api.rs:10 (`ROUTE_BASE`). ✅ |

### A2. Public Functions

| # | Original (line) | Function | Petal Status | Priority |
|---|-----------------|----------|--------------|----------|
| A2.1 | lib.rs:815-847 | `parse_natural_intent()` | **YES** | input.rs:69-97 — identical logic. ✅ | — |
| A2.2 | lib.rs:854-876 | `resolve_token_symbol()` | **PARTIAL** | input.rs:101-123. Uses a tiny hardcoded `match` table (8 entries across 4 chains). Original delegates to `bloom_proto::tokens::resolve_symbol()` which has a full registry. Missing: most token symbols, most chains, WETH on non-mainnet chains. | **critical** |
| A2.3 | lib.rs:752-764 | `de_bytes_hex()` | **YES** | api_types.rs:335-347 — identical. ✅ | — |
| A2.4 | lib.rs:766-796 | `de_u256_dec_or_hex()` | **YES** | api_types.rs:349-377 — identical. ✅ | — |

### A3. API Endpoints

| # | Endpoint | Original | Petal Status | Priority |
|---|----------|----------|--------------|----------|
| A3.1 | `GET /api/v1/shortcuts/route` | EnsoClient::route() lib.rs:401 | **YES** — api.rs:55 `route()`. ✅ | — |
| A3.2 | `POST /api/v1/shortcuts/bundle` | EnsoClient::bundle() lib.rs:448 | **NO** — No bundle function, types, or wiring. | **important** |
| A3.3 | `GET /api/v1/shortcuts/quote` | EnsoClient::quote() lib.rs:493 | **YES** — api.rs:84 `quote()`. ✅ | — |
| A3.4 | `POST /api/v1/simulate` | QuoterClient::simulate() lib.rs:720 | **YES** — api.rs:104 `simulate()`. ✅ | — |
| A3.5 | `POST /api/v1/validate` | QuoterClient::validate() lib.rs:735 | **YES** — api.rs:122 `validate()`. ✅ | — |

> **Note:** route-contract.json (meta/route-contract.json.rs:8) advertises the bundle endpoint in its network contract, but no implementation exists.

### A4. Quoter Integration Into Workflow

| # | Feature | Original | Petal Status | Priority |
|---|---------|----------|--------------|----------|
| A4.1 | Simulate/validate wired into session lifecycle | Not directly in lib.rs (done in defi.rs) | **NO** — `simulate()` and `validate()` exist in api.rs but are **never called** from workflow.rs. The Session struct has a `simulation` field but it's always `None`. | **critical** |

---

## Part B: bloom-vfs/src/handlers/defi.rs — VFS Handler Gaps

### B1. VFS Paths Served

| # | Original Path (line) | What It Does | Petal Route File | Status | Priority |
|---|---------------------|--------------|------------------|--------|----------|
| B1.1 | `defi/` (list, line 1606) | Lists `[README.md, intents]` | `$index.rs` → `[intents, meta, settings]` | **PARTIAL** — No README.md; adds meta/settings dirs (fine). | nice-to-have |
| B1.2 | `defi/README.md` (read, line 1510) | Agent quick-start guide | — | **NO** — No README served. | nice-to-have |
| B1.3 | `defi/intents/` (list, line 1607) | Lists wallets with sessions | `intents/$index.rs` | **YES** ✅ | — |
| B1.4 | `defi/intents/<wallet>/` (list, line 1612) | Lists `new` + session dirs | `intents/[wallet]/$index.rs` | **YES** ✅ | — |
| B1.5 | `defi/intents/<wallet>/new` (write, line 1566) | Create session | `intents/[wallet]/new.rs` | **YES** ✅ | — |
| B1.6 | `.../<session>/intent.txt` (read, line 1520) | Original intent text | `intent.txt.rs` | **YES** ✅ | — |
| B1.7 | `.../<session>/route.json` (read, line 1521) | Full Enso route response | `route.json.rs` | **YES** ✅ | — |
| B1.8 | `.../<session>/plan.md` (read, line 1528) | Human-readable plan | `plan.md.rs` | **YES** ✅ | — |
| B1.9 | `.../<session>/policy_check.json` (read, line 1529) | Policy evaluation results | — | **NO** — No policy_check.json route file. | **critical** |
| B1.10 | `.../<session>/tx.json` (read, line 1530) | Prepared `Vec<RawIntent>` (approve+route) | `tx.json.rs` → returns `prepared_tx` (single tx) | **PARTIAL** — Returns single PreparedTx, not the full intent list with approve. Shape mismatch. | **critical** |
| B1.11 | `.../<session>/simulation.json` (read, line 1536) | **Re-runs eth_call on each read** | `simulation.json.rs` → reads stored `session.simulation` | **PARTIAL** — Returns stored value (always None), doesn't re-run. | **critical** |
| B1.12 | `.../<session>/settlement.json` (read, line 1540) | Balance delta settlement status | `settlement.json.rs` → inspects outbox only | **PARTIAL** — No destination chain balance delta. | **critical** |
| B1.13 | `.../<session>/wait_settlement` (read, line 1544) | Blocking settlement waiter (300s timeout, 5s poll) | — | **NO** | **critical** |
| B1.14 | `.../<session>/destination_chain.txt` (read, line 1550) | Destination chain name | — | **NO** | important |
| B1.15 | `.../<session>/confirm` (write, line 1583) | Stage into outbox | `confirm.rs` | **YES** ✅ (but confirm logic differs — see B5) | — |
| B1.16 | `.../<session>/status.json` (NOT in original) | — | `status.json.rs` | **ADDITION** — Petal adds this; not a gap. | — |

### B2. Session Model

| # | Original `DefiSession` field (line) | Purpose | Petal `Session` equivalent | Status |
|---|--------------------------------------|---------|---------------------------|--------|
| B2.1 | `id` (99) | Session ID | `id` | ✅ |
| B2.2 | `wallet` (100) | Wallet name | `wallet` | ✅ |
| B2.3 | `chain` (101) | Source chain | `chain` | ✅ |
| B2.4 | `destination_chain` (102) | Dest chain for x-chain | `destination_chain` | ✅ |
| B2.5 | `intent_text` (103) | Original NL intent | `intent_text` | ✅ |
| B2.6 | `route_request` (105) | **Persisted RouteRequest** | — (partially in `request_body`) | **PARTIAL** — `request_body` stores NewIntentBody, not the resolved RouteRequest. Needed for re-verification at confirm. |
| B2.7 | `route` (106) | Enso RouteResponse | `route` | ✅ |
| B2.8 | `plan_md` (107) | Plan markdown | `plan_md` | ✅ |
| B2.9 | `intents: Vec<RawIntent>` (108) | **Full intent list (approve + route)** | — (replaced by single `prepared_tx`) | **NO** — No multi-intent support. |
| B2.10 | `intent_states: Vec<DefiIntentState>` (110) | **Per-intent staging state** | — | **NO** — No per-intent tracking. |
| B2.11 | `staged_ids: Vec<String>` (111) | **Outbox IDs per intent** | `outbox_id: Option<String>` (single) | **PARTIAL** — Single outbox ID only. |
| B2.12 | `created_ms` (112) | Creation timestamp | `created_ms` | ✅ |
| B2.13 | `updated_ms` (114) | Last update | `updated_ms` | ✅ |
| B2.14 | `observed_before` (116) | **Pre-route destination balance** | — | **NO** — No settlement baseline. |
| B2.15 | `min_settlement_delta` (118) | **Minimum expected delta** | — | **NO** |
| B2.16 | `source_tx_hashes` (120) | **Source tx hashes for settlement** | — | **NO** |
| B2.17 | `policy_checks` (124) | **Policy evaluation JSON** | — | **NO** |
| B2.18 | `receiver_class` (128) | **Receiver classification** | — | **NO** |

Petal additions (not gaps): `schema_version`, `wallet_address`, `state`, `request_body`, `route_verified`, `simulation`, `prepared_tx`, `outbox_state`, `tx_hash`, `last_error`, `history`.

### B3. DefiHandler Configuration / Builder Methods

| # | Original (line) | Method | Petal Status | Priority |
|---|-----------------|--------|--------------|----------|
| B3.1 | defi.rs:216 | `new()` | N/A — petal uses Host trait | — |
| B3.2 | defi.rs:244 | `with_auth_services()` | **NO** — No auth services concept. | important |
| B3.3 | defi.rs:252 | `with_price_oracle()` | **NO** — No price oracle integration. | **critical** |
| B3.4 | defi.rs:259 | `with_hyperliquid()` | **NO** — No Hyperliquid config. | important |
| B3.5 | defi.rs:265-273 | `with_home_write_permit()` | N/A — petal uses tx_stage() | — |
| B3.6 | defi.rs:275 | `with_default_chain()` | **PARTIAL** — hardcoded `"ethereum"` in workflow.rs:102. | nice-to-have |
| B3.7 | defi.rs:280 | `with_store_root()` | N/A — petal uses KV store | — |
| B3.8 | defi.rs:296 | `with_revert_decoder()` | **NO** — No revert decoder. | **critical** |

### B4. Session Creation (`create_session`)

| # | Feature | Original (line) | Petal workflow::create | Status | Priority |
|---|---------|-----------------|----------------------|--------|----------|
| B4.1 | Parse NL intent | defi.rs:507 | workflow.rs:108 | ✅ | — |
| B4.2 | Resolve chain from body or NL | defi.rs:864-867 | workflow.rs:92-102 | ✅ | — |
| B4.3 | Resolve chain_id via chain client | defi.rs:869-872 (on-chain `chain_id()`) | workflow.rs:104 (`chain_to_id()` static map) | **PARTIAL** — Static map vs. live RPC. If chain name isn't in the map, fails. | important |
| B4.4 | Resolve destination chain_id | defi.rs:874-884 (on-chain RPC) | workflow.rs:134-138 (`chain_to_id()` static map) | **PARTIAL** — Same static map limitation. | important |
| B4.5 | Parse receiver address | defi.rs:886-892 | workflow.rs:139-143 | ✅ | — |
| B4.6 | On-chain decimals for hex tokens | defi.rs:517-538 (`erc20_decimals()` call) | **NO** — `parse_amount()` uses hardcoded symbol→decimals map. | **critical** |
| B4.7 | Decimal lookup via `decimals_for_symbol()` | defi.rs:1708-1722 (uses `bloom_proto::tokens`) | **NO** — Hardcoded USDC=6, USDT=6, WBTC=8, default=18. | **critical** |
| B4.8 | Raw integer amounts for hex tokens (decimals=0) | defi.rs:518-520 | **NO** — `parse_amount()` tries decimal scaling on all inputs. | important |
| B4.9 | `compose_route_request` — pure builder | defi.rs:551-577 | workflow.rs:132-143 (inline) | **PARTIAL** — Not split out; no receiver default for x-chain. | important |
| B4.10 | Cross-chain receiver default | defi.rs:575 (`receiver.or(destination_chain_id.map(\|_| from))`) | **NO** — workflow.rs:139-143 only sets receiver if explicitly provided. Cross-chain routes without explicit receiver won't default to sender. | **critical** |
| B4.11 | Destination chain token_out resolution | defi.rs:561-562 (`resolve_token_symbol(token_out_chain, ...)`) | **NO** — workflow.rs:119 always uses `chain_id` (source), not destination chain for token_out. | **critical** |
| B4.12 | Enso route call | defi.rs:907 | workflow.rs:146 | ✅ | — |
| B4.13 | `route.input_matches_request()` verification | defi.rs:908-912 (fails session creation) | workflow.rs:149 (stores `route_verified` but doesn't fail) | **PARTIAL** — Stores bool but doesn't reject mismatched routes. | **critical** |
| B4.14 | ERC-20 allowance check | defi.rs:920-941 | **NO** — No allowance check. Never stages approve. | **critical** |
| B4.15 | Approve intent staging | defi.rs:928-940 | **NO** | **critical** |
| B4.16 | Route intent construction (`route_raw_intent`) | defi.rs:658-671 | workflow.rs:178-183 (PreparedTx) | **PARTIAL** — Different shape; uses petal's EvmTransaction, not RawIntent. | — |
| B4.17 | Policy evaluation (`evaluate_defi_route`) | defi.rs:960 | **NO** | **critical** |
| B4.18 | Receiver classification (`classify_receiver`) | defi.rs:581-592 | **NO** | **critical** |
| B4.19 | Input valuation via price oracle | defi.rs:950 (`route_input_valuation()`) | **NO** | **critical** |
| B4.20 | Build `DefiRouteCtx` for policy | defi.rs:951-958 | **NO** | **critical** |
| B4.21 | Token display (symbol/decimals via on-chain calls) | defi.rs:1265-1319 | **NO** — Plan uses NL token names only. | important |
| B4.22 | Plan markdown with policy + auto-approve + token amounts | defi.rs:1770-1867 (`render_plan_md`) | workflow.rs:163-176 (simplified format) | **PARTIAL** — Missing: receiver, receiver class, router, policy section, auto-approve section, token amounts with decimals, slippage, tx value/data. | **critical** |
| B4.23 | Observed settlement baseline | defi.rs:989-994 | **NO** | **critical** |
| B4.24 | Per-intent state initialization | defi.rs:980-988 | **NO** | important |
| B4.25 | `slippage_bps` from body | defi.rs:903-905 | workflow.rs:133 | ✅ | — |

### B5. Confirm / Staging (`confirm_session`)

| # | Feature | Original (line) | Petal workflow::confirm | Status | Priority |
|---|---------|-----------------|------------------------|--------|----------|
| B5.1 | Wallet ownership check | defi.rs:1095-1097 | **NO** — No `sess.wallet != wallet` check. | **critical** |
| B5.2 | Intent emptiness check | defi.rs:1099-1101 | workflow.rs:228-231 (checks prepared_tx) | ✅ (different shape) | — |
| B5.3 | **Policy re-evaluation at confirm** | defi.rs:1107-1136 (re-evaluates from CURRENT policy, denies on any Deny) | **NO** — No policy check at confirm. | **critical** |
| B5.4 | `route.input_matches_request()` re-check | defi.rs:1137-1141 | **NO** | **critical** |
| B5.5 | Route-from-wallet binding check | defi.rs:1142-1149 (`from_address == info.address`, `tx.from == info.address`, `session_intents_match_route`) | **NO** | **critical** |
| B5.6 | Source chain ID verification | defi.rs:1150-1159 (`req.chain_id == source_chain_id`) | **NO** | **critical** |
| B5.7 | Multi-intent sequential staging | defi.rs:1168-1258 | **NO** — Stages single tx only. | **critical** |
| B5.8 | Outbox re-staging detection | defi.rs:1169-1191 (checks existing outbox entry state) | **PARTIAL** — workflow.rs:224-226 checks session state only. | important |
| B5.9 | `stage_with_oracle_valuation_target` for route intent | defi.rs:1202-1219 (binds expected tx fields) | **NO** — No valuation target binding. | **critical** |
| B5.10 | Sequential dependency (`set_pending_depends_on`) | defi.rs:1239-1244 (approve must mine before route) | **NO** | **critical** |
| B5.11 | Per-intent state updates | defi.rs:1245-1257 | **NO** | important |
| B5.12 | Idempotent confirm (already staged) | defi.rs:1169-1178 | workflow.rs:224-226 | ✅ (simpler) | — |

### B6. Simulation (`simulate_session`)

| # | Feature | Original (line) | Petal | Status | Priority |
|---|---------|-----------------|-------|--------|----------|
| B6.1 | eth_call simulation via `eth_call_capture_revert` | defi.rs:1031-1090 | **NO** — Petal has api.rs `simulate()` (Enso Quoter) but it's never called. The original uses direct chain `eth_call`. | **critical** |
| B6.2 | Re-run on each read of `simulation.json` | defi.rs:1536-1538 | **NO** — simulation.json.rs reads stored `session.simulation` (always None). | **critical** |
| B6.3 | Revert decoding via `DecoderChain` | defi.rs:1056-1075 | **NO** — No revert decoder at all. | **critical** |
| B6.4 | Return data hex encoding | defi.rs:1047 | **NO** | important |
| B6.5 | Gas estimate passthrough | defi.rs:1048 | **NO** | nice-to-have |

### B7. Settlement Verification

| # | Feature | Original (line) | Petal | Status | Priority |
|---|---------|-----------------|-------|--------|----------|
| B7.1 | `settlement_status()` — full balance delta | defi.rs:1321-1384 | settlement.json.rs (outbox inspect only) | **PARTIAL** | **critical** |
| B7.2 | Destination chain balance read (`erc20_balance`) | defi.rs:1935-1938 | **NO** | **critical** |
| B7.3 | Before/after delta computation | defi.rs:1341-1344 | **NO** | **critical** |
| B7.4 | Minimum delta / expected output floor | defi.rs:1345-1350 | **NO** | **critical** |
| B7.5 | Status classification (`destination_received`, `destination_pending`, `not_broadcast`, etc.) | defi.rs:1351-1361 | **NO** — petal settlement.json just echoes outbox state. | **critical** |
| B7.6 | Source tx hash collection | defi.rs:1415-1427 (`source_hashes_for_session`) | **NO** | important |
| B7.7 | `wait_settlement()` — blocking poll loop | defi.rs:1386-1413 (300s timeout, 5s interval) | **NO** — No wait_settlement route file. | **critical** |
| B7.8 | Settlement note (same-chain vs cross-chain guidance) | defi.rs:1378-1383 | **NO** | nice-to-have |

### B8. Hyperliquid Deposit

| # | Feature | Original (line) | Petal | Status | Priority |
|---|---------|-----------------|-------|--------|----------|
| B8.1 | `parse_hyperliquid_deposit()` | defi.rs:1680-1706 | **NO** | important |
| B8.2 | `stage_hyperliquid_deposit()` | defi.rs:763-853 | **NO** | important |
| B8.3 | USDC-only validation | defi.rs:790-794 | **NO** | important |
| B8.4 | Arbitrum-only chain validation | defi.rs:795-801 | **NO** | important |
| B8.5 | `check_deposit()` guardrails (min 5 USDC, dust warning) | defi.rs:806-813 | **NO** | important |
| B8.6 | Bridge address config (`hl_bridge`, `hl_deposit_chain_id`) | defi.rs:236-239, 259-262 | **NO** | important |

### B9. ERC-20 Approval / Allowance

| # | Feature | Original (line) | Petal | Status | Priority |
|---|---------|-----------------|-------|--------|----------|
| B9.1 | `erc20_allowance()` check | defi.rs:922-926 | **NO** — Host trait has no allowance method. | **critical** |
| B9.2 | Approve intent construction (`RawIntentBody::Approve`) | defi.rs:928-940 | **NO** | **critical** |
| B9.3 | Native token skip (no approve for ETH) | defi.rs:920 | **NO** (no approve logic at all) | **critical** |
| B9.4 | Allowance-read Host capability | — | **NO** — `Host::chain_read` exists but no typed `erc20_allowance` wrapper. | **critical** |

### B10. Receiver Classification & Policy

| # | Feature | Original (line) | Petal | Status | Priority |
|---|---------|-----------------|-------|--------|----------|
| B10.1 | `classify_receiver()` — WalletEoa / AddressbookAlias / Unknown | defi.rs:581-592 | **NO** | **critical** |
| B10.2 | Keystore info lookup for EOA match | defi.rs:583-587 | **NO** — No keystore access (wallet address read from VFS only). | **critical** |
| B10.3 | Address book alias lookup | defi.rs:588-590 | **NO** | **critical** |
| B10.4 | `build_route_ctx()` — full DefiRouteCtx | defi.rs:598-626 | **NO** | **critical** |
| B10.5 | `evaluate_defi_route()` policy evaluation | defi.rs:960, 1124 | **NO** | **critical** |
| B10.6 | Deny-level policy enforcement at confirm | defi.rs:1127-1136 | **NO** | **critical** |
| B10.7 | Protocol extraction for policy | defi.rs:609 (`route.protocols()`) | **PARTIAL** — Called in workflow.rs:154 but only for plan display, not policy. | important |

### B11. Price Oracle / Valuation

| # | Feature | Original (line) | Petal | Status | Priority |
|---|---------|-----------------|-------|--------|----------|
| B11.1 | `DynPriceOracle` integration | defi.rs:195, 252-255 | **NO** | **critical** |
| B11.2 | `route_input_valuation()` | defi.rs:705-757 | **NO** | **critical** |
| B11.3 | `route_input_valuation_target()` | defi.rs:628-635 | **NO** | **critical** |
| B11.4 | `route_input_decimals()` | defi.rs:637-656 | **NO** | **critical** |
| B11.5 | Valuation quote validation | defi.rs:727-756 | **NO** | **critical** |
| B11.6 | `stage_with_oracle_valuation_target` | defi.rs:1202-1219 | **NO** | **critical** |

### B12. Address Book

| # | Feature | Original (line) | Petal | Status | Priority |
|---|---------|-----------------|-------|--------|----------|
| B12.1 | `AddressBook` integration | defi.rs:196, 221 | **NO** | important |
| B12.2 | `alias_for()` lookup | defi.rs:588 | **NO** | important |
| B12.3 | Address book passed to `TxEngine::stage` | defi.rs:842 | **NO** — Petal stages via `tx_stage()` SDK call. | important |

### B13. Cross-Chain Route Handling

| # | Feature | Original (line) | Petal | Status | Priority |
|---|---------|-----------------|-------|--------|----------|
| B13.1 | `destination_chain_id` in RouteRequest | defi.rs:95 | ✅ RouteRequest has it. | — |
| B13.2 | Destination chain_id resolution via RPC | defi.rs:874-884 | **PARTIAL** — Uses static `chain_to_id()` map. | important |
| B13.3 | Token_out resolution on **destination** chain | defi.rs:561-562 | **NO** — Always uses source chain_id for token_out. | **critical** |
| B13.4 | Cross-chain receiver default (sender EOA) | defi.rs:575 | **NO** | **critical** |
| B13.5 | `destination_chain_id` extraction from route hops | defi.rs:430-434, api.rs:74-78 | ✅ Ported in api.rs. | — |
| B13.6 | Settlement balance on destination chain | defi.rs:1922-1939 | **NO** | **critical** |

### B14. Session Persistence & Management

| # | Feature | Original (line) | Petal | Status | Priority |
|---|---------|-----------------|-------|--------|----------|
| B14.1 | Session disk persistence (JSON) | defi.rs:480-490 | ✅ Via KV store (`host.put`). | — |
| B14.2 | Session validation on load (wallet/id match) | defi.rs:429-434 | **NO** — No cross-check of loaded session against requested wallet/id. | important |
| B14.3 | `normalize_session_state()` | defi.rs:1886-1907 | **NO** — No legacy session migration. | nice-to-have |
| B14.4 | Atomic write (tmp + rename) | defi.rs:486-488 | **NO** — Direct KV put (host-managed atomicity). | nice-to-have |
| B14.5 | Session listing sorted by created_ms desc | defi.rs:384-401 | **PARTIAL** — Petal uses BTreeSet (lexical sort). | nice-to-have |
| B14.6 | Path segment validation (`validate_segment`) | defi.rs:329-336 | **PARTIAL** — `validate_wallet_name()` in workflow.rs:49-59 checks wallet only, not session ID. `petal::is_safe_segment()` used in listing. | important |
| B14.7 | `allocate_id()` — collision-free ID | defi.rs:301-315 (checks file existence) | **PARTIAL** — `generate_id()` uses 8 random bytes; collision probability negligible but no existence check. | nice-to-have |

### B15. Plan Markdown Rendering

| # | Feature | Original `render_plan_md` (line) | Petal plan_md | Status | Priority |
|---|---------|----------------------------------|---------------|--------|----------|
| B15.1 | Intent text | defi.rs:1783 | workflow.rs:165 | ✅ | — |
| B15.2 | Chain + chain ID | defi.rs:1784 | workflow.rs:165 | ✅ | — |
| B15.3 | Destination chain | defi.rs:1785-1791 | **NO** | important |
| B15.4 | From address (owner EOA) | defi.rs:1792-1795 | workflow.rs:164 (wallet name only) | **PARTIAL** | nice-to-have |
| B15.5 | Receiver + checksum | defi.rs:1797-1798 | **NO** | **critical** |
| B15.6 | Receiver classification | defi.rs:1799 | **NO** | **critical** |
| B15.7 | Token in with decimals + human amount | defi.rs:1800-1803 (`render_token_amount`) | **PARTIAL** — Shows NL amount/symbol only. | important |
| B15.8 | Token out with quote + human amount | defi.rs:1804-1807 (`render_token_quote`) | **PARTIAL** — Shows raw amount_out + NL symbol. | important |
| B15.9 | Slippage in bps | defi.rs:1808 | **NO** | important |
| B15.10 | Router address | defi.rs:1809 | **NO** | **critical** |
| B15.11 | Protocols list | defi.rs:1810-1814 | workflow.rs:155-161 | ✅ (simplified) | — |
| B15.12 | Gas estimate | defi.rs:1815-1817 | workflow.rs:171 | ✅ | — |
| B15.13 | Price impact (display-only label) | defi.rs:1818-1822 | workflow.rs:172-175 | ✅ | — |
| B15.14 | Tx value (wei) | defi.rs:1823 | **NO** | important |
| B15.15 | Tx data length | defi.rs:1824 | **NO** | nice-to-have |
| B15.16 | Policy section (PASS/WARN/DENY per check) | defi.rs:1827-1841 | **NO** | **critical** |
| B15.17 | Auto-approve section | defi.rs:1843-1857 | **NO** | **critical** |
| B15.18 | Confirm instructions (tx count) | defi.rs:1858-1865 | workflow.rs (simple) | **PARTIAL** | nice-to-have |

### B16. Token Symbol Resolution Completeness

| # | Token | Original (via `bloom_proto::tokens`) | Petal (hardcoded match) | Status |
|---|-------|--------------------------------------|------------------------|--------|
| B16.1 | ETH/ETHER/MATIC/BNB/AVAX → native | lib.rs:866 | input.rs:107 | ✅ |
| B16.2 | USDC on Ethereum (1) | ✅ | input.rs:113 | ✅ |
| B16.3 | USDC on Polygon (137) | ✅ | input.rs:116 | ✅ |
| B16.4 | USDC on Base (8453) | ✅ | input.rs:119 | ✅ |
| B16.5 | USDC on Optimism (10) | ✅ | input.rs:120 | ✅ |
| B16.6 | USDC on Arbitrum (42161) | ✅ | **NO** | important |
| B16.7 | USDC on BNB (56) | ✅ | **NO** | important |
| B16.8 | USDC on Avalanche (43114) | ✅ | **NO** | important |
| B16.9 | USDT on Ethereum (1) | ✅ | input.rs:114 | ✅ |
| B16.10 | USDT on Polygon (137) | ✅ | input.rs:117 | ✅ |
| B16.11 | USDT on other chains | ✅ | **NO** | important |
| B16.12 | WETH on Ethereum (1) | ✅ | input.rs:115 | ✅ |
| B16.13 | WETH on other chains | ✅ | **NO** | important |
| B16.14 | WMATIC on Polygon (137) | ✅ | input.rs:118 | ✅ |
| B16.15 | WBTC, DAI, LINK, UNI, etc. | ✅ | **NO** | important |
| B16.16 | Any arbitrary token | ✅ (via bloom_proto) | **NO** | **critical** |

> The original has a full token registry via `bloom_proto::tokens::resolve_symbol()` covering all major tokens across all supported chains. The petal has **8 hardcoded entries**. This is a significant coverage gap.

### B17. Natural Intent Parsing Completeness

| # | Feature | Original (lib.rs:815) | Petal (input.rs:69) | Status |
|---|---------|----------------------|---------------------|--------|
| B17.1 | Basic `swap <amt> <in> to <out>` | ✅ | ✅ | ✅ |
| B17.2 | `on <chain>` qualifier | ✅ | ✅ | ✅ |
| B17.3 | Tracing/debug logging on parse failure | lib.rs:819,823,832 | **NO** — Silent `None` return. | nice-to-have |
| B17.4 | `deposit <amt> [token] to hyperliquid` | defi.rs:1680-1706 | **NO** | important |

### B18. Decimal Scaling

| # | Feature | Original | Petal | Status | Priority |
|---|---------|----------|-------|--------|----------|
| B18.1 | `parse_units()` from bloom_proto | defi.rs:564 | **NO** — Custom `parse_amount()` in workflow.rs:260 | — | — |
| B18.2 | Symbol→decimals via token table | defi.rs:1708-1722 | **NO** — Hardcoded USDC/USDT=6, WBTC=8, default=18 | **critical** |
| B18.3 | On-chain `erc20_decimals()` for hex tokens | defi.rs:525-535 | **NO** | **critical** |
| B18.4 | Raw integer amounts (decimals=0 for hex) | defi.rs:518-520 | **NO** | important |
| B18.5 | WETH=18 special case | defi.rs:1711-1715 | **NO** — Falls through to default 18 (accidentally correct) | nice-to-have |

### B19. Error Handling & Edge Cases

| # | Feature | Original (line) | Petal | Status | Priority |
|---|---------|-----------------|-------|--------|----------|
| B19.1 | `map_enso_err()` — structured EnsoError→HandlerError | defi.rs:1667-1675 | **NO** — All errors are String. `Disabled`/`MissingKey` not distinguished from `InvalidIntent`. | important |
| B19.2 | Empty body rejection | defi.rs:340-342 | input.rs:32-34 | ✅ | — |
| B19.3 | Non-UTF8 body rejection | defi.rs:1567-1568 | input.rs:28-29 | ✅ | — |
| B19.4 | Empty confirm rejection | defi.rs:1585-1587 | **NO** — confirm.rs accepts any body, workflow::confirm ignores body. | important |
| B19.5 | API error status/body propagation | lib.rs:415-427 | api.rs:45-52 (truncates body to 512 chars) | **PARTIAL** | nice-to-have |
| B19.6 | `parse_decimal_u256()` — reject decimal strings | defi.rs:1915-1920 | **NO** | nice-to-have |

### B20. Host Trait Capabilities

The petal `Host` trait (runtime.rs) is missing methods that the original handler relies on:

| # | Capability | Original equivalent | Petal Host method | Status | Priority |
|---|-----------|---------------------|-------------------|--------|----------|
| B20.1 | ERC-20 allowance read | `chain.erc20_allowance()` | **NO** — Only `chain_read(method, params)` exists; no typed wrapper. | **critical** |
| B20.2 | ERC-20 balance read | `chain.erc20_balance()` | **NO** — Only generic `chain_read`. | **critical** |
| B20.3 | ERC-20 decimals read | `chain.erc20_decimals()` | **NO** | **critical** |
| B20.4 | ERC-20 symbol read | `chain.erc20_symbol()` | **NO** | important |
| B20.5 | `eth_call` with revert capture | `chain.eth_call_capture_revert()` | **NO** — Only generic `chain_read`. | **critical** |
| B20.6 | Chain spec (native symbol/decimals) | `chain.spec()` | **NO** | important |
| B20.7 | Keystore info (wallet address, policy) | `self.keystore.info(wallet)` | **PARTIAL** — `vfs_read("wallets/{wallet}/address")` only; no policy access. | **critical** |
| B20.8 | Outbox read/inspect by ID | `tx_engine.outbox.read()` | `tx_inspect()` | ✅ | — |
| B20.9 | Outbox dependency setting | `outbox.set_pending_depends_on()` | **NO** | **critical** |
| B20.10 | Wallet policy access | `info.policy.defi` | **NO** | **critical** |

---

## Part C: Summary of Missing Route Files

| # | Route File Needed | Original Path | Priority |
|---|-------------------|---------------|----------|
| C1 | `policy_check.json.rs` | `.../<session>/policy_check.json` | **critical** |
| C2 | `wait_settlement.rs` | `.../<session>/wait_settlement` | **critical** |
| C3 | `destination_chain.txt.rs` | `.../<session>/destination_chain.txt` | important |
| C4 | `README.md.rs` (or static) | `README.md` | nice-to-have |

---

## Part D: Priority-Ranked Action Items

### CRITICAL (blocks safe money movement — 38 items)

1. **ERC-20 allowance check + approve staging** (B9.1-B9.4, B4.14-B4.15) — Without this, ERC-20 swaps will fail on-chain.
2. **Policy evaluation at confirm** (B5.3, B10.4-B10.6) — No deny-level gate exists.
3. **Receiver classification** (B10.1-B10.3) — Cannot distinguish owner EOA from unknown receiver.
4. **Route binding verification at confirm** (B5.4-B5.6) — Staged tx not re-verified against session.
5. **Wallet ownership check at confirm** (B5.1) — Cross-wallet session access possible.
6. **Simulation re-run with revert decoding** (B6.1-B6.3) — No pre-flight validation.
7. **Settlement verification with balance delta** (B7.1-B7.7) — No cross-chain completion proof.
8. **Cross-chain token_out resolution** (B13.3) — Output token resolved on wrong chain.
9. **Cross-chain receiver default** (B13.4) — X-chain routes without explicit receiver won't set one.
10. **Route input mismatch rejection at create** (B4.13) — Stores but doesn't reject bad routes.
11. **Price oracle integration** (B11.1-B11.6) — No USD valuation for policy thresholds.
12. **Full token symbol registry** (B16) — Only 8 tokens supported.
13. **On-chain decimal resolution** (B18.2-B18.3) — Wrong decimals = wrong amounts.
14. **Policy check route file** (C1) — No visibility into policy evaluation.
15. **Wait settlement route file** (C2) — No blocking settlement waiter.
16. **Wallet policy access** (B20.10) — No `[defi]` policy to evaluate against.
17. **tx.json shape** (B1.10) — Must return full intent list, not single tx.
18. **Plan markdown completeness** (B15.5-B15.6, B15.10, B15.16-B15.17) — Missing receiver, router, policy, auto-approve sections.
19. **Sequential dependency tracking** (B5.10) — Approve must mine before route.
20. **Multi-intent staging** (B5.7) — Only single tx staged.
21. **Session route_request persistence** (B2.6) — Needed for re-verification.

### IMPORTANT (functional completeness — 22 items)

22. Bundle endpoint support (A3.2, A1.4-A1.5, A1.8)
23. Hyperliquid deposit flow (B8.1-B8.6)
24. Address book integration (B12.1-B12.3)
25. Auth services (B3.2)
26. Destination chain route file (C3)
27. Token display via on-chain calls (B4.21)
28. Chain ID resolution via RPC (B4.3-B4.4)
29. Session ID validation (B14.6)
30. Empty confirm rejection (B19.4)
31. Structured error types (A1.1, B19.1)
32. Destination chain in plan (B15.3)
33. Slippage in plan (B15.9)
34. Tx value in plan (B15.14)
35. Source tx hash collection (B7.6)
36. Per-intent state tracking (B4.24)
37. API error body truncation (B19.5)
38. Additional token symbols (B16.6-B16.16)
39. `deposit` NL intent parsing (B17.4)
40. Raw integer amount handling (B18.4, B4.8)
41. Chain spec access (B20.6)
42. ERC-20 symbol read (B20.4)
43. Session load validation (B14.2)

### NICE-TO-HAVE (polish — 10 items)

44. README.md route file (C4, B1.2)
45. Debug tracing in intent parsing (B17.3)
46. Session listing chronological sort (B14.5)
47. Atomic write pattern (B14.4)
48. Legacy session migration (B14.3)
49. ID collision check (B14.7)
50. Gas estimate in simulation (B6.5)
51. Settlement note text (B7.8)
52. Default chain configurability (B3.6)
53. WETH decimals special case (B18.5)
54. `parse_decimal_u256` guard (B19.6)
