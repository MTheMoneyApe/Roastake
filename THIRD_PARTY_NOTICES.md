# Third-Party Software & Dependency Notices (`Roastake`)

Prepared for [`MTheMoneyApe/Roastake`](https://github.com/MTheMoneyApe/Roastake). Attribution for original ROASTAKE materials identified in [`LICENSES.md`](./LICENSES.md) belongs solely to **@MTheMoneyApe** (Evaluation-Only Perpetuals scope) and the pre-existing Apache-2.0 OutLayer components. All third-party software, libraries, tools, and referenced external protocols remain the property of their respective authors and copyright holders under their own license terms.

---

## 1. Direct Rust Workspace Dependencies (`Cargo.toml` / `Cargo.lock`)

Inspectable via `cargo metadata --format-version 1`:

| Crate | Version | Declared Upstream License | Usage Scope |
| --- | --- | --- | --- |
| [`near-sdk`](https://crates.io/crates/near-sdk) | `5.30.1` | `MIT OR Apache-2.0` | NEAR smart contract framework (`oracle-adapter`, `perpetuals`, mocks) |
| [`serde`](https://crates.io/crates/serde) | `1.0.229` | `MIT OR Apache-2.0` | JSON / Borsh / config serialization |
| [`serde_json`](https://crates.io/crates/serde_json) | `1.0.154` | `MIT OR Apache-2.0` | JSON parsing & NEP-297 event serialization |
| [`url`](https://crates.io/crates/url) | `2.5.8` | `MIT OR Apache-2.0` | Fail-closed URL host/allowlist parsing (`coffee-price-index`) |
| [`wasi`](https://crates.io/crates/wasi) | `0.13.3+wasi-0.2.2` *(optional `wasi-http` feature in `workers/coffee-price-index`)* & `0.11.1+wasi-snapshot-preview1` *(transitive lock entry)* | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | WASI runtime / HTTP outbound handler bindings |

---

## 2. Transitive Rust Crate License Summary (`Cargo.lock`)

Across the resolved `Roastake` workspace dependency graph (`307` external crate entries returned by `cargo metadata --format-version 1`):

- `MIT OR Apache-2.0` (163 crates) & `Apache-2.0 OR MIT` (32 crates) & `MIT/Apache-2.0` (15 crates)
- `MIT` (40 crates), `Apache-2.0` (4 crates)
- `Unicode-3.0` (18 crates), `Unlicense OR MIT` (7 crates), `BSD-3-Clause` (6 crates)
- `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` (3 crates)
- `MPL-2.0` (2 crates: `memory_units@0.4.0`, `wee_alloc@0.4.5`), `CC0-1.0` (2 crates), `BSD-2-Clause OR Apache-2.0 OR MIT` (2 crates)
- `BSD-2-Clause` (1), `ISC` (1), `Zlib` (1), `Apache-2.0 OR BSL-1.0` (1), `(MIT OR Apache-2.0) AND Unicode-3.0` (1), plus combined multi-license expressions (`Zlib OR MIT OR Apache-2.0`, `Zlib OR Apache-2.0 OR MIT`, `CC0-1.0 OR MIT-0 OR Apache-2.0`, `MIT OR Apache-2.0 OR BSD-1-Clause`, `MIT OR Apache-2.0 OR LGPL-2.1-or-later` [`r-efi@6.0.0`, dual/tri-licensed under `MIT OR Apache-2.0`], `ISC AND ...` cryptography crates).

Full upstream license texts for resolved crates live in the respective upstream crate distributions / Cargo registry cache.

---

## 3. Sandbox Integration Test Dependencies (`integration-tests/near-sandbox-js/`)

Inspectable via `integration-tests/near-sandbox-js/package.json` and `pnpm-lock.yaml`:

- **Direct Package:** `near-workspaces@5.0.0` (NEAR local sandbox runner & JS workspace client; transitive pnpm store breakdown across resolved packages: `MIT` 76, `ISC` 25, `Apache-2.0` 3, `(MIT AND Apache-2.0)` 3, `BSD-2-Clause` 2).
- **Local Test Mock `mock-ft` (`integration-tests/near-sandbox-js/mock-ft/`):** `roastake-mock-tidepool-ft` implements standard NEAR NEP-141/145/148 fungible token test methods (`near-sdk`, `serde`, `serde_json`). Its `Cargo.toml` omits a `license` field (`license = null`), so it is classified in [`LICENSES.md`](./LICENSES.md) as an **unspecified test fixture (`UNRESOLVED`)**, not `@MTheMoneyApe` proprietary IP.

---

## 4. External Protocols, Runtime Services & Data Sources Referenced

- **Circle Native Testnet USDC (`3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`):** Deployed external NEAR Testnet token contract operated by Circle; governed solely by Circle's on-chain contract and Testnet faucet terms.
- **FastNear NEAR Data Testnet (`https://testnet.neardata.xyz`) & NEAR Testnet RPC (`https://rpc.testnet.fastnear.com`, `https://rpc.testnet.near.org`):** External finalized-block HTTP data providers queried exclusively server-side by `services/perps-indexer/` using Node.js built-in `fetch` (`0` external FastNear source files bundled). Unauthenticated public NEAR Data endpoints enforce an upstream rate limit (~180 req/min per IP); optional `FASTNEAR_API_KEY` authentication (`Authorization: Bearer`) stays strictly server-side if configured, never sent to browsers or logs. Subject to FastNear / NEAR Infrastructure Terms of Service & fair-use limits.
- **TradingView Lightweight Charts™ (`lightweight-charts@5.3.0`, Apache-2.0):** Used in the companion frontend repository (`MTheMoneyApe/pixel-perfect-replica`) for rendering verified indexer/RPC chart series with mandatory `attributionLogo: true` + `https://www.tradingview.com/` link preserved (documented in `Roastake-Frontend/THIRD_PARTY_NOTICES.md`).
- **NEAR OutLayer (`outlayer.testnet`) & IMF Primary Commodity Price System (`api.imf.org`):** External execution/data interfaces queried in tests or the separate `roastake-oracle-v1.testnet` track; subject to NEAR / OutLayer / IMF public data terms.
