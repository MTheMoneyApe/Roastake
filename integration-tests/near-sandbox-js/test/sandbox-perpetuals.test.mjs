// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Worker } from 'near-workspaces';
import {
  enforceSandboxNetworkGuard,
  createImplicitAccount,
  STANDARD_CALL_GAS,
  STANDARD_ATTACHED_DEPOSIT,
  DEFAULT_MIN_OPERATING_RESERVE,
  parseTransactionResult,
} from './helpers.mjs';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const PERPS_WASM_PATH = resolve(
  __dirname,
  '../../../target/m045b-perps-out/roastake_perpetuals.wasm'
);
const MOCK_FT_WASM_PATH = resolve(
  __dirname,
  '../../../target/m045b-mock-ft-out/roastake_mock_tidepool_ft.wasm'
);
const ADAPTER_WASM_PATH = resolve(
  __dirname,
  '../../../target/m042d2a-adapter-out/roastake_oracle_adapter_m042d2a.wasm'
);
const MOCK_OUTLAYER_WASM_PATH = resolve(
  __dirname,
  '../../../target/m042d2a-mock-outlayer-out/roastake_mock_outlayer.wasm'
);

const ONE_YOCTO = 1n;
const STORAGE_DEPOSIT_FT = 1_250_000_000_000_000_000_000n; // 0.00125 NEAR (exact Tidepool bound)
const STORAGE_DEPOSIT_PERPS = 50_000_000_000_000_000_000_000n; // 0.05 NEAR
const USD_SCALE = 100_000_000n; // 10^8
const USDC_SCALE = 1_000_000n; // 10^6
const SIZE_SCALE = 100_000_000n; // 10^8
const PRICE_SCALE = 100_000_000n; // 10^8

const CIRCLE_DISCLOSURE =
  'Circle Native Testnet USDC (no monetary value): official 6-decimal NEAR Testnet USDC; parity 1 USDC = 1 USD is accounting-only.';
const CIRCLE_MAINNET_USDC =
  '17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1';
// Inline SVG data URI matching live Circle USDC ft_metadata icon payload (>1.6 KB)
const CIRCLE_SVG_ICON = `data:image/svg+xml,${'C'.repeat(1661)}`;
const WORKER_FASTFS_URL =
  'https://test.fastfs.io/roastake/workers/roastake_coffee_price_index.wasm';
const VALID_WORKER_SHA256 =
  '855bc554762aa9b367ac2b3c62a219c250410f3715256e822f8c24e23df100f1';

function loadWasm(path, label) {
  assert.ok(
    existsSync(path),
    `[MISSING WASM] ${label} not found at ${path}. Run scripts/build-perpetuals.ps1 first.`
  );
  const bytes = readFileSync(path);
  const sha256 = createHash('sha256').update(bytes).digest('hex').toLowerCase();
  console.log(`[OK] Loaded ${label}: ${bytes.length} bytes | SHA-256: ${sha256}`);
  return bytes;
}

async function deployMockUsdcImplicit(worker, root, ftWasm, ownerAccount, metaOpts = {}) {
  const account = await createImplicitAccount(
    worker,
    root,
    10_000_000_000_000_000_000_000_000n
  );
  await account.deploy(ftWasm);
  await ownerAccount.call(account, 'new', {
    owner_id: ownerAccount.accountId,
    name: metaOpts.name ?? 'USDC',
    symbol: 'USDC',
    decimals: 6,
    icon: metaOpts.icon ?? CIRCLE_SVG_ICON,
  });
  return account;
}

async function deployMockUsdc(root, name, ftWasm, ownerAccount, metaOpts = {}) {
  const account = await root.createSubAccount(name, {
    initialBalance: 10_000_000_000_000_000_000_000_000n,
  });
  await account.deploy(ftWasm);
  await ownerAccount.call(account, 'new', {
    owner_id: ownerAccount.accountId,
    name: metaOpts.name ?? 'Tidepool Testnet USDC (no value)',
    symbol: 'USDC',
    decimals: 6,
    icon: metaOpts.icon ?? null,
  });
  return account;
}

async function setupFtAccountAndMint(owner, ftContract, userAccount, amountE6) {
  await owner.call(
    ftContract,
    'storage_deposit',
    { account_id: userAccount.accountId, registration_only: true },
    { attachedDeposit: STORAGE_DEPOSIT_FT }
  );
  if (amountE6 > 0n) {
    await owner.call(ftContract, 'mint', {
      account_id: userAccount.accountId,
      amount: amountE6.toString(),
    });
  }
}

test('ROASTAKE M04.5B: Real Multi-Contract NEAR Sandbox Perpetuals & Collateral Integration Suite', async (t) => {
  enforceSandboxNetworkGuard();

  const perpsWasm = loadWasm(PERPS_WASM_PATH, 'ROASTAKE Perpetuals');
  const ftWasm = loadWasm(MOCK_FT_WASM_PATH, 'Sandbox Tidepool FT (NEP-141/145)');
  const adapterWasm = loadWasm(ADAPTER_WASM_PATH, 'Oracle Adapter Release Candidate');
  const mockOutlayerWasm = loadWasm(MOCK_OUTLAYER_WASM_PATH, 'Mock OutLayer');

  // =========================================================================
  // Suite 1: NEP-141/145 Collateral, 6<->8 Conversions, Residues & Callbacks
  // =========================================================================
  await t.test(
    'Part 1: NEP-141/145 custody, allowlist, 6<->8 residues, failed withdrawal restoration & private callback blocks',
    async () => {
      const worker = await Worker.init({ network: 'sandbox' });
      try {
        const root = worker.rootAccount;
        const admin = await root.createSubAccount('admin', {
          initialBalance: 20_000_000_000_000_000_000_000_000n,
        });
        const treasury = await root.createSubAccount('treasury', {
          initialBalance: 10_000_000_000_000_000_000_000_000n,
        });
        const lp = await root.createSubAccount('lp', {
          initialBalance: 10_000_000_000_000_000_000_000_000n,
        });
        const alice = await root.createSubAccount('alice', {
          initialBalance: 10_000_000_000_000_000_000_000_000n,
        });

        const ft = await deployMockUsdcImplicit(worker, root, ftWasm, admin, {
          name: 'USDC',
          icon: CIRCLE_SVG_ICON,
        });
        const rogueFt = await deployMockUsdc(root, 'rogueusdc', ftWasm, admin, {
          name: 'USDC',
        });
        const tidepoolAltFt = await deployMockUsdc(root, 'tidepool', ftWasm, admin, {
          name: 'Tidepool Testnet USDC (no value)',
        });
        const perps = await root.createSubAccount('perps', {
          initialBalance: 20_000_000_000_000_000_000_000_000n,
        });
        await perps.deploy(perpsWasm);
        await admin.call(perps, 'init', {
          admin: admin.accountId,
          treasury: treasury.accountId,
          primary_collateral_token: ft.accountId,
          collateral_symbol: 'USDC',
          collateral_decimals: 6,
          non_monetary_testnet_disclosure: CIRCLE_DISCLOSURE,
        });

        // Verify Circle-profile NEP-148 metadata (including >1.6 KB SVG icon) via live cross-contract call
        const metaVerified = await admin.call(
          perps,
          'verify_collateral_metadata',
          { token_id: ft.accountId },
          { gas: STANDARD_CALL_GAS }
        );
        assert.strictEqual(metaVerified, true);
        const tokenView = await perps.view('get_collateral_token', {
          token_id: ft.accountId,
        });
        assert.strictEqual(tokenView.decimals, 6);
        assert.strictEqual(tokenView.scale_to_usd_e8, '100');
        assert.strictEqual(tokenView.metadata_verified_on_chain, true);

        // Reject Circle Mainnet USDC ID fail-closed on add_collateral_token
        const mainnetCircleRes = await admin.callRaw(perps, 'add_collateral_token', {
          token_id: CIRCLE_MAINNET_USDC,
          symbol: 'USDC',
          decimals: 6,
          non_monetary_testnet_disclosure: CIRCLE_DISCLOSURE,
        });
        assert.strictEqual(
          parseTransactionResult(mainnetCircleRes).succeeded,
          false,
          'Mainnet Circle USDC ID must be rejected'
        );

        // Register perps + users on FT & register wallets on Perps
        await admin.call(
          ft,
          'storage_deposit',
          { account_id: perps.accountId, registration_only: true },
          { attachedDeposit: STORAGE_DEPOSIT_FT }
        );
        await admin.call(
          rogueFt,
          'storage_deposit',
          { account_id: perps.accountId, registration_only: true },
          { attachedDeposit: STORAGE_DEPOSIT_FT }
        );
        await admin.call(
          tidepoolAltFt,
          'storage_deposit',
          { account_id: perps.accountId, registration_only: true },
          { attachedDeposit: STORAGE_DEPOSIT_FT }
        );

        await setupFtAccountAndMint(admin, ft, lp, 10_000n * USDC_SCALE);
        await setupFtAccountAndMint(admin, ft, alice, 1_000n * USDC_SCALE);
        await setupFtAccountAndMint(admin, rogueFt, alice, 500n * USDC_SCALE);
        await setupFtAccountAndMint(admin, tidepoolAltFt, alice, 500n * USDC_SCALE);

        await lp.call(
          perps,
          'register_account',
          { account_id: lp.accountId },
          { attachedDeposit: STORAGE_DEPOSIT_PERPS }
        );
        await alice.call(
          perps,
          'register_account',
          { account_id: alice.accountId },
          { attachedDeposit: STORAGE_DEPOSIT_PERPS }
        );

        // 1. Rogue unapproved token AND default-unregistered Tidepool fixture must both refund cleanly via ft_resolve_transfer
        for (const unapproved of [rogueFt, tidepoolAltFt]) {
          await alice.call(
            unapproved,
            'ft_transfer_call',
            {
              receiver_id: perps.accountId,
              amount: (100n * USDC_SCALE).toString(),
              memo: null,
              msg: JSON.stringify({ action: 'trader_collateral' }),
            },
            { attachedDeposit: ONE_YOCTO, gas: STANDARD_CALL_GAS }
          );
          assert.strictEqual(
            await unapproved.view('ft_balance_of', { account_id: alice.accountId }),
            (500n * USDC_SCALE).toString(),
            'Unapproved USDC/Tidepool token must be refunded in full via ft_resolve_transfer'
          );
        }

        // 1b. Mainnet .near account proposal must be rejected fail-closed
        const badAdminProposal = await admin.callRaw(perps, 'propose_admin', {
          new_admin: 'attacker.near',
        });
        assert.strictEqual(
          parseTransactionResult(badAdminProposal).succeeded,
          false,
          'Mainnet .near account must be rejected'
        );

        // 2. Register Sandbox market & execute valid deposits (LP $10,000, Alice $1,000)
        await admin.call(perps, 'register_market', {
          market_id: 'COFFEE-SANDBOX/USD',
          oracle_source: {
            TestnetSandboxOperator: {
              oracle_operator: admin.accountId,
              min_price_e8: PRICE_SCALE.toString(),
              max_price_e8: (500n * PRICE_SCALE).toString(),
            },
          },
          risk: null,
          oracle_policy: null,
          exec_spread_bps: 0,
        });

        await lp.call(
          ft,
          'ft_transfer_call',
          {
            receiver_id: perps.accountId,
            amount: (10_000n * USDC_SCALE).toString(),
            memo: null,
            msg: JSON.stringify({ action: 'lp_deposit' }),
          },
          { attachedDeposit: ONE_YOCTO, gas: STANDARD_CALL_GAS }
        );
        await alice.call(
          ft,
          'ft_transfer_call',
          {
            receiver_id: perps.accountId,
            amount: (1_000n * USDC_SCALE).toString(),
            memo: null,
            msg: JSON.stringify({ action: 'trader_collateral' }),
          },
          { attachedDeposit: ONE_YOCTO, gas: STANDARD_CALL_GAS }
        );

        // Check solvency matches physical FT custody balance x 100
        const snap1 = await perps.view('get_accounting_snapshot', {});
        const ftBalPerps = BigInt(
          await ft.view('ft_balance_of', { account_id: perps.accountId })
        );
        assert.strictEqual(snap1.solvency_ok, true);
        assert.strictEqual(
          BigInt(snap1.tracked_collateral_e8),
          ftBalPerps * 100n
        );

        // 3. Odd-quantity trade creating `< 1 micro-USDC` (< 100 e8) residue & verify zero-loss max withdraw
        const block = await worker.provider.block({ finality: 'final' });
        const nowSec = Number(BigInt(block.header.timestamp_nanosec) / 1_000_000_000n);
        await admin.call(perps, 'push_sandbox_oracle_price', {
          market_id: 'COFFEE-SANDBOX/USD',
          price_e8: (8n * PRICE_SCALE).toString(),
          observed_at_sec: nowSec,
          sequence: 1,
        });
        const posId = await alice.call(
          perps,
          'open_position',
          {
            market_id: 'COFFEE-SANDBOX/USD',
            side: 'Long',
            size_e8: '3333333333',
            collateral_e8: (100n * USD_SCALE).toString(),
            price_limit_e8: null,
          },
          { gas: STANDARD_CALL_GAS }
        );
        await admin.call(perps, 'push_sandbox_oracle_price', {
          market_id: 'COFFEE-SANDBOX/USD',
          price_e8: '811111111',
          observed_at_sec: nowSec + 1,
          sequence: 2,
        });
        await alice.call(
          perps,
          'close_position',
          { position_id: posId, price_limit_e8: null },
          { gas: STANDARD_CALL_GAS }
        );

        const accBeforeWithdraw = await perps.view('get_account', {
          account_id: alice.accountId,
        });
        const residue = BigInt(accBeforeWithdraw.sub_micro_unit_residue_e8);
        assert.ok(residue > 0n && residue < 100n, 'Must create <100 e8 sub-micro-USDC residue');

        // Over-withdrawal by +1 base unit must fail closed without altering balance
        const maxWholeUnits = BigInt(accBeforeWithdraw.withdrawable_token_units_at_scale_100);
        const overRes = await alice.callRaw(
          perps,
          'withdraw_collateral',
          { token_id: ft.accountId, amount_token_units: (maxWholeUnits + 1n).toString() },
          { gas: STANDARD_CALL_GAS }
        );
        assert.strictEqual(parseTransactionResult(overRes).succeeded, false);

        // Withdraw failed test first via `set_fail_transfers(true)` -> verify balance restores 100%!
        await admin.call(ft, 'set_fail_transfers', { fail: true });
        await alice.call(
          perps,
          'withdraw_collateral',
          { token_id: ft.accountId, amount_token_units: null },
          { gas: STANDARD_CALL_GAS }
        );
        const accRestored = await perps.view('get_account', {
          account_id: alice.accountId,
        });
        assert.strictEqual(
          accRestored.free_collateral_e8,
          accBeforeWithdraw.free_collateral_e8,
          'Failed ft_transfer callback must restore 100% of free_collateral_e8'
        );
        assert.strictEqual(accRestored.has_pending_withdrawal, false);

        // Retry partial withdrawal (50.000000 USDC), then remaining whole units -> leaves exact residue!
        await admin.call(ft, 'set_fail_transfers', { fail: false });
        await alice.call(
          perps,
          'withdraw_collateral',
          { token_id: ft.accountId, amount_token_units: (50n * USDC_SCALE).toString() },
          { gas: STANDARD_CALL_GAS }
        );
        const accMid = await perps.view('get_account', { account_id: alice.accountId });
        assert.strictEqual(
          BigInt(accMid.free_collateral_e8),
          BigInt(accBeforeWithdraw.free_collateral_e8) - 50n * USD_SCALE
        );
        await alice.call(
          perps,
          'withdraw_collateral',
          { token_id: ft.accountId, amount_token_units: null },
          { gas: STANDARD_CALL_GAS }
        );
        const accAfter = await perps.view('get_account', {
          account_id: alice.accountId,
        });
        assert.strictEqual(BigInt(accAfter.free_collateral_e8), residue);
        assert.strictEqual(await perps.view('verify_solvency_invariant', {}), true);

        // Malicious private callback attempt from Alice must fail at runtime
        const privRes = await alice.callRaw(
          perps,
          'on_oracle_action',
          { action_id: 1 },
          { gas: STANDARD_CALL_GAS }
        );
        assert.strictEqual(parseTransactionResult(privRes).succeeded, false);
      } finally {
        await worker.tearDown();
      }
    }
  );

  // =========================================================================
  // Suite 2: Real Cross-Contract Async `roastake_oracle_adapter` Integration
  // =========================================================================
  await t.test(
    'Part 2: Real 2-receipt XCC Oracle integration with roastake_oracle_adapter & mock_outlayer',
    async () => {
      const worker = await Worker.init({ network: 'sandbox' });
      try {
        const root = worker.rootAccount;
        // oracle_adapter uses roastake_shared::validate_testnet_account_id (accepts *.testnet & 64-hex implicit accounts)
        const admin = await createImplicitAccount(
          worker,
          root,
          20_000_000_000_000_000_000_000_000n
        );
        const treasury = await createImplicitAccount(
          worker,
          root,
          10_000_000_000_000_000_000_000_000n
        );
        const lp = await createImplicitAccount(
          worker,
          root,
          10_000_000_000_000_000_000_000_000n
        );
        const alice = await createImplicitAccount(
          worker,
          root,
          10_000_000_000_000_000_000_000_000n
        );

        const mockOutlayer = await createImplicitAccount(
          worker,
          root,
          10_000_000_000_000_000_000_000_000n
        );
        await mockOutlayer.deploy(mockOutlayerWasm);
        await mockOutlayer.call(mockOutlayer, 'new', {});

        const adapter = await createImplicitAccount(
          worker,
          root,
          20_000_000_000_000_000_000_000_000n
        );
        await adapter.deploy(adapterWasm);
        await admin.call(adapter, 'init', {
          admin: admin.accountId,
          outlayer_contract: mockOutlayer.accountId,
          initial_wasm_url: WORKER_FASTFS_URL,
          initial_wasm_sha256: VALID_WORKER_SHA256,
          minimum_operating_reserve: DEFAULT_MIN_OPERATING_RESERVE,
        });
        await admin.call(adapter, 'add_requester', { account_id: alice.accountId });
        // Register live_index market on real oracle_adapter
        await admin.call(adapter, 'add_market', {
          config: {
            symbol: 'COFFEE-LIVE-XCC/USD',
            description: 'Live Aggregated XCC Test Market',
            decimals: 8,
            unit: 'USD/kg',
            min_price: (1n * PRICE_SCALE).toString(),
            max_price: (500n * PRICE_SCALE).toString(),
            max_price_age_sec: 300,
            max_divergence_bps: 2000,
            wasm_url: WORKER_FASTFS_URL,
            wasm_sha256: VALID_WORKER_SHA256,
            build_target: 'wasm32-wasip2',
            active: true,
            worker_mode: 'live_index',
            secrets_ref: null,
            min_source_count: 2,
            allow_synthetic: false,
            max_source_spread_bps: 500,
          },
        });

        const ft = await deployMockUsdcImplicit(worker, root, ftWasm, admin);
        const perps = await createImplicitAccount(
          worker,
          root,
          20_000_000_000_000_000_000_000_000n
        );
        await perps.deploy(perpsWasm);
        await admin.call(perps, 'init', {
          admin: admin.accountId,
          treasury: treasury.accountId,
          primary_collateral_token: ft.accountId,
          collateral_symbol: 'USDC',
          collateral_decimals: 6,
          non_monetary_testnet_disclosure: CIRCLE_DISCLOSURE,
        });
        await admin.call(
          ft,
          'storage_deposit',
          { account_id: perps.accountId, registration_only: true },
          { attachedDeposit: STORAGE_DEPOSIT_FT }
        );
        await setupFtAccountAndMint(admin, ft, lp, 10_000n * USDC_SCALE);
        await setupFtAccountAndMint(admin, ft, alice, 1_000n * USDC_SCALE);
        await lp.call(
          perps,
          'register_account',
          { account_id: lp.accountId },
          { attachedDeposit: STORAGE_DEPOSIT_PERPS }
        );
        await alice.call(
          perps,
          'register_account',
          { account_id: alice.accountId },
          { attachedDeposit: STORAGE_DEPOSIT_PERPS }
        );
        await lp.call(
          ft,
          'ft_transfer_call',
          {
            receiver_id: perps.accountId,
            amount: (10_000n * USDC_SCALE).toString(),
            memo: null,
            msg: JSON.stringify({ action: 'lp_deposit' }),
          },
          { attachedDeposit: ONE_YOCTO, gas: STANDARD_CALL_GAS }
        );
        await alice.call(
          ft,
          'ft_transfer_call',
          {
            receiver_id: perps.accountId,
            amount: (1_000n * USDC_SCALE).toString(),
            memo: null,
            msg: JSON.stringify({ action: 'trader_collateral' }),
          },
          { attachedDeposit: ONE_YOCTO, gas: STANDARD_CALL_GAS }
        );

        // 1. Verify IMF & GESHA are strictly blocked on registration
        const imfReg = await admin.callRaw(perps, 'register_market', {
          market_id: 'BAD-IMF/USD',
          oracle_source: {
            ExternalXcc: {
              oracle_contract: adapter.accountId,
              oracle_market: 'ARABICA-MONTHLY-IMF/USD',
            },
          },
          risk: null,
          oracle_policy: null,
          exec_spread_bps: 0,
        });
        assert.strictEqual(parseTransactionResult(imfReg).succeeded, false);

        await admin.call(perps, 'register_market', {
          market_id: 'PERPS-XCC/USD',
          oracle_source: {
            ExternalXcc: {
              oracle_contract: adapter.accountId,
              oracle_market: 'COFFEE-LIVE-XCC/USD',
            },
          },
          risk: null,
          oracle_policy: null,
          exec_spread_bps: 0,
        });

        // 2. Open when adapter has NO price yet -> XCC callback fails gracefully & releases Alice's lock!
        await alice.call(
          perps,
          'open_position',
          {
            market_id: 'PERPS-XCC/USD',
            side: 'Long',
            size_e8: (50n * SIZE_SCALE).toString(),
            collateral_e8: (100n * USD_SCALE).toString(),
            price_limit_e8: null,
          },
          { gas: STANDARD_CALL_GAS }
        );
        const accAfterMiss = await perps.view('get_account', { account_id: alice.accountId });
        assert.strictEqual(accAfterMiss.pending_action_lock, false);
        assert.strictEqual(
          (await perps.view('get_positions_by_owner', { owner: alice.accountId })).length,
          0
        );

        // 3. Drive real OutLayer -> OracleAdapter update to $10.00/kg (`LIVE_AGGREGATED`)
        const blk = await worker.provider.block({ finality: 'final' });
        const obsSec = Number(BigInt(blk.header.timestamp_nanosec) / 1_000_000_000n);
        await admin.call(mockOutlayer, 'set_custom_output', {
          output: {
            schema_version: 1,
            market: 'COFFEE-LIVE-XCC/USD',
            price: (10n * PRICE_SCALE).toString(),
            decimals: 8,
            unit: 'USD/kg',
            source: 'ice-spot+cepea',
            source_count: 2,
            observed_at: obsSec,
            data_class: 'LIVE_AGGREGATED',
            error: null,
          },
        });
        await alice.call(
          adapter,
          'request_price_update',
          { market: 'COFFEE-LIVE-XCC/USD' },
          { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
        );

        // Execute real 2-receipt XCC open_position (Perps -> Adapter.get_price -> Perps.on_oracle_action)!
        await alice.call(
          perps,
          'open_position',
          {
            market_id: 'PERPS-XCC/USD',
            side: 'Long',
            size_e8: (50n * SIZE_SCALE).toString(),
            collateral_e8: (100n * USD_SCALE).toString(),
            price_limit_e8: null,
          },
          { gas: STANDARD_CALL_GAS }
        );
        const positions = await perps.view('get_positions_by_owner', {
          owner: alice.accountId,
        });
        assert.strictEqual(positions.length, 1);
        assert.strictEqual(positions[0].entry_price_e8, (10n * PRICE_SCALE).toString());

        // Advance Oracle price +4% ($10.40/kg) and execute real async XCC close_position -> profit paid!
        const blk2 = await worker.provider.block({ finality: 'final' });
        const obsSec2 = Number(BigInt(blk2.header.timestamp_nanosec) / 1_000_000_000n);
        await admin.call(mockOutlayer, 'set_custom_output', {
          output: {
            schema_version: 1,
            market: 'COFFEE-LIVE-XCC/USD',
            price: '1040000000',
            decimals: 8,
            unit: 'USD/kg',
            source: 'ice-spot+cepea',
            source_count: 2,
            observed_at: obsSec2,
            data_class: 'LIVE_AGGREGATED',
            error: null,
          },
        });
        await alice.call(
          adapter,
          'request_price_update',
          { market: 'COFFEE-LIVE-XCC/USD' },
          { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
        );
        await alice.call(
          perps,
          'close_position',
          { position_id: positions[0].id, price_limit_e8: null },
          { gas: STANDARD_CALL_GAS }
        );
        const accClosed = await perps.view('get_account', { account_id: alice.accountId });
        assert.ok(
          BigInt(accClosed.free_collateral_e8) > 1_018n * USD_SCALE,
          'Real XCC trade profit (~+$19 net fees) must credit Alice free_collateral'
        );
        assert.strictEqual(await perps.view('verify_solvency_invariant', {}), true);
      } finally {
        await worker.tearDown();
      }
    }
  );

  // =========================================================================
  // Suite 3: Full E2E Liquidation + Uniform Global Emergency Settlement (Sandbox)
  // =========================================================================
  await t.test(
    'Part 3: E2E liquidation, uniform emergency settlement freeze & LP unlock when OI == 0 in Sandbox',
    async () => {
      const worker = await Worker.init({ network: 'sandbox' });
      try {
        const root = worker.rootAccount;
        const admin = await root.createSubAccount('admin3', {
          initialBalance: 20_000_000_000_000_000_000_000_000n,
        });
        const treasury = await root.createSubAccount('treas3', {
          initialBalance: 10_000_000_000_000_000_000_000_000n,
        });
        const lp = await root.createSubAccount('lp3', {
          initialBalance: 10_000_000_000_000_000_000_000_000n,
        });
        const alice = await root.createSubAccount('alice3', {
          initialBalance: 10_000_000_000_000_000_000_000_000n,
        });
        const bob = await root.createSubAccount('bob3', {
          initialBalance: 10_000_000_000_000_000_000_000_000n,
        });

        const ft = await deployMockUsdc(root, 'usdc3', ftWasm, admin);
        const perps = await root.createSubAccount('perps3', {
          initialBalance: 20_000_000_000_000_000_000_000_000n,
        });
        await perps.deploy(perpsWasm);
        await admin.call(perps, 'init', {
          admin: admin.accountId,
          treasury: treasury.accountId,
          primary_collateral_token: ft.accountId,
          collateral_symbol: 'USDC',
          collateral_decimals: 6,
          non_monetary_testnet_disclosure: CIRCLE_DISCLOSURE,
        });
        await admin.call(
          ft,
          'storage_deposit',
          { account_id: perps.accountId, registration_only: true },
          { attachedDeposit: STORAGE_DEPOSIT_FT }
        );
        const charlie = await root.createSubAccount('charlie3', {
          initialBalance: 10_000_000_000_000_000_000_000_000n,
        });
        for (const u of [lp, alice, bob, charlie]) {
          await setupFtAccountAndMint(admin, ft, u, 10_000n * USDC_SCALE);
          await u.call(
            perps,
            'register_account',
            { account_id: u.accountId },
            { attachedDeposit: STORAGE_DEPOSIT_PERPS }
          );
        }

        // Configure sandbox outage thresholds (max_exec = 15s, emergency = 30s) to run smoothly on live Sandbox blocks
        await admin.call(perps, 'register_market', {
          market_id: 'COFFEE-EMERG-TEST/USD',
          oracle_source: {
            TestnetSandboxOperator: {
              oracle_operator: admin.accountId,
              min_price_e8: PRICE_SCALE.toString(),
              max_price_e8: (500n * PRICE_SCALE).toString(),
            },
          },
          risk: null,
          oracle_policy: {
            allow_testnet_sandbox: true,
            max_execution_age_sec: 15,
            emergency_close_age_sec: 30,
            max_jump_bps: 500,
            max_clock_drift_sec: 60,
          },
          exec_spread_bps: 0,
        });

        await lp.call(
          ft,
          'ft_transfer_call',
          {
            receiver_id: perps.accountId,
            amount: (10_000n * USDC_SCALE).toString(),
            memo: null,
            msg: JSON.stringify({ action: 'lp_deposit' }),
          },
          { attachedDeposit: ONE_YOCTO, gas: STANDARD_CALL_GAS }
        );
        await alice.call(
          ft,
          'ft_transfer_call',
          {
            receiver_id: perps.accountId,
            amount: (1_000n * USDC_SCALE).toString(),
            memo: null,
            msg: JSON.stringify({ action: 'trader_collateral' }),
          },
          { attachedDeposit: ONE_YOCTO, gas: STANDARD_CALL_GAS }
        );
        await bob.call(
          ft,
          'ft_transfer_call',
          {
            receiver_id: perps.accountId,
            amount: (1_000n * USDC_SCALE).toString(),
            memo: null,
            msg: JSON.stringify({ action: 'trader_collateral' }),
          },
          { attachedDeposit: ONE_YOCTO, gas: STANDARD_CALL_GAS }
        );

        await charlie.call(
          ft,
          'ft_transfer_call',
          {
            receiver_id: perps.accountId,
            amount: (1_000n * USDC_SCALE).toString(),
            memo: null,
            msg: JSON.stringify({ action: 'trader_collateral' }),
          },
          { attachedDeposit: ONE_YOCTO, gas: STANDARD_CALL_GAS }
        );

        let curTs = 0;
        let seq = 1;
        const nextChainTs = async () => {
          const b = await worker.provider.block({ finality: 'final' });
          const chainSec = Number(BigInt(b.header.timestamp_nanosec) / 1_000_000_000n);
          curTs = Math.max(curTs + 1, chainSec);
          return curTs;
        };

        await admin.call(perps, 'push_sandbox_oracle_price', {
          market_id: 'COFFEE-EMERG-TEST/USD',
          price_e8: (10n * PRICE_SCALE).toString(),
          observed_at_sec: await nextChainTs(),
          sequence: seq++,
        });

        // 3a. Real on-chain E2E liquidation check (5x Long pushed smoothly within 5% jump steps into <8% MM)
        const charliePosId = await charlie.call(
          perps,
          'open_position',
          {
            market_id: 'COFFEE-EMERG-TEST/USD',
            side: 'Long',
            size_e8: (100n * SIZE_SCALE).toString(),
            collateral_e8: (200n * USD_SCALE).toString(),
            price_limit_e8: null,
          },
          { gas: STANDARD_CALL_GAS }
        );
        for (const stepPrice of ['955000000', '910000000', '865000000']) {
          await admin.call(perps, 'push_sandbox_oracle_price', {
            market_id: 'COFFEE-EMERG-TEST/USD',
            price_e8: stepPrice,
            observed_at_sec: await nextChainTs(),
            sequence: seq++,
          });
        }
        const bobBeforeLiq = await perps.view('get_account', { account_id: bob.accountId });
        const snapBeforeLiq = await perps.view('get_accounting_snapshot', {});
        await bob.call(
          perps,
          'liquidate_position',
          { position_id: charliePosId },
          { gas: STANDARD_CALL_GAS }
        );
        const bobAfterLiq = await perps.view('get_account', { account_id: bob.accountId });
        const snapAfterLiq = await perps.view('get_accounting_snapshot', {});
        assert.strictEqual(
          await perps.view('get_position', { position_id: charliePosId }),
          null,
          'Liquidated position must be removed'
        );
        assert.ok(
          BigInt(bobAfterLiq.free_collateral_e8) > BigInt(bobBeforeLiq.free_collateral_e8),
          'Keeper Bob must receive on-chain liquidation reward'
        );
        assert.ok(
          BigInt(snapAfterLiq.insurance_fund_e8) > BigInt(snapBeforeLiq.insurance_fund_e8),
          'Insurance fund must receive liquidation inflow'
        );

        // Step oracle back to $10.00/kg within <5% max jump per tick
        for (const stepPrice of ['908000000', '953000000', '1000000000']) {
          await admin.call(perps, 'push_sandbox_oracle_price', {
            market_id: 'COFFEE-EMERG-TEST/USD',
            price_e8: stepPrice,
            observed_at_sec: await nextChainTs(),
            sequence: seq++,
          });
        }

        const longId = await alice.call(
          perps,
          'open_position',
          {
            market_id: 'COFFEE-EMERG-TEST/USD',
            side: 'Long',
            size_e8: (50n * SIZE_SCALE).toString(),
            collateral_e8: (100n * USD_SCALE).toString(),
            price_limit_e8: null,
          },
          { gas: STANDARD_CALL_GAS }
        );
        const shortId = await bob.call(
          perps,
          'open_position',
          {
            market_id: 'COFFEE-EMERG-TEST/USD',
            side: 'Short',
            size_e8: (50n * SIZE_SCALE).toString(),
            collateral_e8: (100n * USD_SCALE).toString(),
            price_limit_e8: null,
          },
          { gas: STANDARD_CALL_GAS }
        );

        const tTick = await nextChainTs();
        await admin.call(perps, 'push_sandbox_oracle_price', {
          market_id: 'COFFEE-EMERG-TEST/USD',
          price_e8: '1040000000', // $10.40/kg (+$20 Long, -$20 Short)
          observed_at_sec: tTick,
          sequence: seq++,
        });

        // Fast-forward Sandbox blocks past `emergency_close_age_sec = 30s` cleanly!
        await worker.provider.fastForward(60);
        for (let i = 0; i < 15; i++) {
          const cur = await worker.provider.block({ finality: 'final' });
          const cSec = Number(BigInt(cur.header.timestamp_nanosec) / 1_000_000_000n);
          if (cSec >= tTick + 31) break;
          await worker.provider.fastForward(20);
        }

        // Trigger Uniform Global Emergency Settlement permissionlessly
        await bob.call(
          perps,
          'trigger_emergency_settlement',
          { market_id: 'COFFEE-EMERG-TEST/USD' },
          { gas: STANDARD_CALL_GAS }
        );
        const mView = await perps.view('get_market', {
          market_id: 'COFFEE-EMERG-TEST/USD',
        });
        assert.strictEqual(mView.mode, 'Emergency');
        assert.strictEqual(mView.feed_classification, 'TESTNET_SANDBOX_FEED');
        assert.strictEqual(mView.is_synthetic_testnet_sandbox, true);
        assert.strictEqual(mView.sandbox_operator, admin.accountId);
        assert.strictEqual(mView.sandbox_min_price_e8, PRICE_SCALE.toString());
        assert.strictEqual(mView.sandbox_max_price_e8, (500n * PRICE_SCALE).toString());
        assert.strictEqual(mView.sandbox_sequence, 8);
        assert.strictEqual(mView.frozen_settlement_price_e8, '1040000000');

        // LP withdrawal blocked while open positions exist
        const lpAcc = await perps.view('get_account', { account_id: lp.accountId });
        const lockedWithdraw = await lp.callRaw(
          perps,
          'withdraw_lp',
          {
            token_id: ft.accountId,
            shares_to_burn: lpAcc.lp_shares,
            min_assets_out_e8: null,
          },
          { gas: STANDARD_CALL_GAS }
        );
        assert.strictEqual(parseTransactionResult(lockedWithdraw).succeeded, false);

        // Claim both Emergency settlements (zero fee, symmetric frozen snapshot)
        await alice.call(
          perps,
          'claim_emergency_settlement',
          { position_id: longId },
          { gas: STANDARD_CALL_GAS }
        );
        await bob.call(
          perps,
          'claim_emergency_settlement',
          { position_id: shortId },
          { gas: STANDARD_CALL_GAS }
        );

        // Now OI == 0 -> LP withdrawal unlocks and receives real USDC tokens back!
        const lpBalBefore = BigInt(
          await ft.view('ft_balance_of', { account_id: lp.accountId })
        );
        await lp.call(
          perps,
          'withdraw_lp',
          {
            token_id: ft.accountId,
            shares_to_burn: lpAcc.lp_shares,
            min_assets_out_e8: null,
          },
          { gas: STANDARD_CALL_GAS }
        );
        const lpBalAfter = BigInt(
          await ft.view('ft_balance_of', { account_id: lp.accountId })
        );
        assert.ok(lpBalAfter > lpBalBefore, 'Unlocked LP must receive real NEP-141 USDC payout');
        assert.strictEqual(await perps.view('verify_solvency_invariant', {}), true);
      } finally {
        await worker.tearDown();
      }
    }
  );
});
