import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Worker } from 'near-workspaces';
import {
  STANDARD_ATTACHED_DEPOSIT,
  DEFAULT_MIN_OPERATING_RESERVE,
  STANDARD_CALL_GAS,
  STANDARD_CLAIM_GAS,
  MARKET_GESHA_DEMO,
  enforceSandboxNetworkGuard,
  createImplicitAccount,
  parseTransactionResult,
} from './helpers.mjs';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const M042D2A_ADAPTER_WASM_PATH = resolve(
  __dirname,
  '../../../target/m042d2a-adapter-out/roastake_oracle_adapter_m042d2a.wasm'
);
const M042D2A_MOCK_OUTLAYER_WASM_PATH = resolve(
  __dirname,
  '../../../target/m042d2a-mock-outlayer-out/roastake_mock_outlayer.wasm'
);

const WORKER_FASTFS_URL =
  'https://test.fastfs.io/roastake/workers/roastake_coffee_price_index.wasm';
const M042D1_WORKER_SHA256 =
  '855bc554762aa9b367ac2b3c62a219c250410f3715256e822f8c24e23df100f1';

const MARKET_IMF = 'ARABICA-MONTHLY-IMF/USD';
const IMF_SOURCE = 'imf-pcps-pcoffotm';
const DATA_CLASS_IMF = 'PUBLIC_BENCHMARK_MONTHLY';
const DATA_CLASS_SYNTHETIC = 'SYNTHETIC_DEMO';
const HISTORICAL_MAX_AGE_SEC = 90 * 24 * 60 * 60; // 7,776,000 s (90 days)

function loadBuiltWasm(path, label) {
  assert.ok(
    existsSync(path),
    `[MISSING ARTIFACT] ${label} WASM not found at ${path}. Build isolated M04.2d.2a artifacts first.`
  );
  const bytes = readFileSync(path);
  const sha256 = createHash('sha256').update(bytes).digest('hex').toLowerCase();
  console.log(`[OK] Loaded ${label}: ${bytes.length} bytes | SHA-256: ${sha256}`);
  return { bytes, sha256 };
}

/**
 * Derive a deterministic monthly IMF observation timestamp (final UTC second of the
 * calendar month two months prior to the Sandbox chain block timestamp: YYYY-MM-lastT23:59:59Z).
 * Never reads local host `Date.now()` — strictly uses chain block time (~30–61 days old:
 * rejects 1-hour live markets while fitting inside IMF's 90-day historical window).
 */
async function deriveSandboxImfMonthEndUtc(worker) {
  const latestBlock = await worker.provider.block({ finality: 'final' });
  const chainSec = Number(BigInt(latestBlock.header.timestamp_nanosec) / 1_000_000_000n);
  const blockDate = new Date(chainSec * 1000);
  const monthEndMs = Date.UTC(
    blockDate.getUTCFullYear(),
    blockDate.getUTCMonth() - 1,
    0,
    23,
    59,
    59
  );
  const observedAt = Math.floor(monthEndMs / 1000);
  const ageSec = chainSec - observedAt;
  assert.ok(
    observedAt > 0 && ageSec > 3600 && ageSec <= HISTORICAL_MAX_AGE_SEC,
    `Derived deterministic IMF month-end ${observedAt} (age ${ageSec}s vs chain ${chainSec}) must sit between 1h and 90d`
  );
  return { chainSec, observedAt };
}

test('ROASTAKE M04.2d.2a: Init Hardening, Configurable Mock ABI & End-to-End IMF Sandbox Callbacks', async (t) => {
  enforceSandboxNetworkGuard();

  const { bytes: adapterWasm } = loadBuiltWasm(
    M042D2A_ADAPTER_WASM_PATH,
    'Oracle Adapter (M04.2d.2a)'
  );
  const { bytes: mockWasm } = loadBuiltWasm(
    M042D2A_MOCK_OUTLAYER_WASM_PATH,
    'Configurable Mock OutLayer (M04.2d.2a)'
  );

  // =========================================================================
  // 1. Initialization SHA-256 Hardening (reject missing/malformed/all-zero)
  // =========================================================================
  await t.test('Part 1: init() rejects missing, malformed, and all-zero initial_wasm_sha256', async () => {
    const worker = await Worker.init({ network: 'sandbox' });
    try {
      const root = worker.rootAccount;
      const alice = await createImplicitAccount(worker, root, 100_000_000_000_000_000_000_000_000n);
      const mockOutlayer = await createImplicitAccount(worker, root, 50_000_000_000_000_000_000_000_000n);

      const badCases = [
        ['missing (omitted)', undefined],
        ['empty string', ''],
        ['wrong length (63)', 'a'.repeat(63)],
        ['wrong length (65)', 'a'.repeat(65)],
        ['non-hex char', `${'a'.repeat(63)}g`],
        ['all-zero placeholder', '0'.repeat(64)],
      ];

      for (const [label, badHash] of badCases) {
        const acct = await createImplicitAccount(worker, root, 20_000_000_000_000_000_000_000_000n);
        await acct.deploy(adapterWasm);
        const args = {
          admin: alice.accountId,
          outlayer_contract: mockOutlayer.accountId,
          initial_wasm_url: WORKER_FASTFS_URL,
          minimum_operating_reserve: DEFAULT_MIN_OPERATING_RESERVE,
        };
        if (badHash !== undefined) {
          args.initial_wasm_sha256 = badHash;
        }
        const res = parseTransactionResult(
          await acct.callRaw(acct, 'init', args, { gas: STANDARD_CLAIM_GAS })
        );
        assert.ok(res.failed, `init() must reject ${label} initial_wasm_sha256`);
      }
      console.log('[PASS] Part 1: All invalid/placeholder init() SHA-256 cases rejected fail-closed.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // 2. Dual Market Initialization, Full IMF Callback, Negative Cases & Gesha Isolation
  // =========================================================================
  await t.test('Part 2: Dual market init, 7-arg ABI inspection, IMF success callback, negative cases & Gesha isolation', async () => {
    const worker = await Worker.init({ network: 'sandbox' });
    try {
      const root = worker.rootAccount;
      const alice = await createImplicitAccount(worker, root, 100_000_000_000_000_000_000_000_000n);
      const bob = await createImplicitAccount(worker, root, 100_000_000_000_000_000_000_000_000n);
      const mockOutlayer = await createImplicitAccount(worker, root, 50_000_000_000_000_000_000_000_000n);
      const adapter = await createImplicitAccount(worker, root, 50_000_000_000_000_000_000_000_000n);

      await mockOutlayer.deploy(mockWasm);
      await mockOutlayer.call(mockOutlayer, 'new', {});
      await adapter.deploy(adapterWasm);

      // 1-2. Single init with explicit valid SHA-256
      const initTx = parseTransactionResult(
        await adapter.callRaw(adapter, 'init', {
          admin: alice.accountId,
          outlayer_contract: mockOutlayer.accountId,
          initial_wasm_url: WORKER_FASTFS_URL,
          initial_wasm_sha256: M042D1_WORKER_SHA256,
          minimum_operating_reserve: DEFAULT_MIN_OPERATING_RESERVE,
        })
      );
      assert.ok(initTx.succeeded, 'Valid init() must succeed');

      const reinitTx = parseTransactionResult(
        await alice.callRaw(
          adapter,
          'init',
          {
            admin: alice.accountId,
            outlayer_contract: mockOutlayer.accountId,
            initial_wasm_url: WORKER_FASTFS_URL,
            initial_wasm_sha256: M042D1_WORKER_SHA256,
            minimum_operating_reserve: DEFAULT_MIN_OPERATING_RESERVE,
          },
          { gas: STANDARD_CLAIM_GAS }
        )
      );
      assert.ok(reinitTx.failed, 'Second init() must fail closed');

      // 3-5. Confirm BOTH default markets exist immediately without calling add_market()
      const markets = await adapter.view('list_markets', {});
      assert.deepStrictEqual(markets, [MARKET_GESHA_DEMO, MARKET_IMF]);
      const snapInit = await adapter.view('get_accounting_snapshot', {});
      assert.strictEqual(snapInit.market_count, 2);

      const imfCfg = await adapter.view('get_market_config', { market: MARKET_IMF });
      assert.ok(imfCfg, 'IMF market must be immediately accessible after init()');
      assert.strictEqual(imfCfg.symbol, MARKET_IMF);
      assert.strictEqual(imfCfg.worker_mode, 'public_benchmark');
      assert.strictEqual(imfCfg.allow_synthetic, false);
      assert.strictEqual(imfCfg.min_source_count, 1);
      assert.strictEqual(imfCfg.max_source_spread_bps, 0);
      assert.strictEqual(imfCfg.max_price_age_sec, HISTORICAL_MAX_AGE_SEC);
      assert.strictEqual(imfCfg.secrets_ref, null);
      assert.strictEqual(imfCfg.wasm_url, WORKER_FASTFS_URL);
      assert.strictEqual(imfCfg.wasm_sha256, M042D1_WORKER_SHA256);
      assert.ok(imfCfg.description.includes('International Monetary Fund (IMF)'));
      assert.ok(imfCfg.description.includes('not live ICE Coffee C futures'));
      assert.ok(imfCfg.description.includes('does not imply IMF endorsement'));

      const geshaCfg = await adapter.view('get_market_config', { market: MARKET_GESHA_DEMO });
      assert.ok(geshaCfg, 'Gesha market must be immediately accessible after init()');
      assert.strictEqual(geshaCfg.worker_mode, 'fixture');
      assert.strictEqual(geshaCfg.allow_synthetic, true);
      assert.strictEqual(geshaCfg.min_source_count, 1);
      assert.strictEqual(geshaCfg.max_source_spread_bps, 0);
      assert.strictEqual(geshaCfg.max_price_age_sec, 3600);
      assert.strictEqual(geshaCfg.secrets_ref, null);
      assert.strictEqual(geshaCfg.wasm_url, WORKER_FASTFS_URL);
      assert.strictEqual(geshaCfg.wasm_sha256, M042D1_WORKER_SHA256);

      // Verify admin-only protection still enforces authorization
      const nonAdminAddRes = parseTransactionResult(
        await bob.callRaw(
          adapter,
          'add_market',
          {
            config: {
              ...imfCfg,
              symbol: 'ARABICA-CUSTOM/USD',
              worker_mode: 'live_index',
              max_price_age_sec: 3600,
              max_source_spread_bps: 300,
            },
          },
          { gas: STANDARD_CLAIM_GAS }
        )
      );
      assert.ok(nonAdminAddRes.failed, 'Non-admin add_market must fail closed');
      await alice.call(adapter, 'add_requester', { account_id: bob.accountId });

      // Ensure an initial NullResponse failure on IMF NEVER creates a synthetic price
      let nextNonce = 1;
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'NullResponse' });
      const preNullTx = parseTransactionResult(
        await bob.callRaw(
          adapter,
          'request_price_update',
          { market: MARKET_IMF },
          { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
        )
      );
      assert.ok(preNullTx.succeeded);
      assert.strictEqual(await adapter.view('get_price', { market: MARKET_IMF }), null);
      assert.strictEqual(await adapter.view('get_pending_request', { nonce: nextNonce }), null);
      nextNonce += 1;

      // 6-13. Configure valid historical IMF WorkerOutput & deliver full end-to-end callback
      const { chainSec, observedAt: validImfObservedAt } = await deriveSandboxImfMonthEndUtc(worker);
      const validImfPayload = {
        schema_version: 1,
        market: MARKET_IMF,
        price: '727794219',
        decimals: 8,
        unit: 'USD/kg',
        source: IMF_SOURCE,
        source_count: 1,
        observed_at: validImfObservedAt,
        data_class: DATA_CLASS_IMF,
      };
      await mockOutlayer.call(mockOutlayer, 'set_custom_output', { output: validImfPayload });

      const okImfTx = parseTransactionResult(
        await bob.callRaw(
          adapter,
          'request_price_update',
          { market: MARKET_IMF },
          { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
        )
      );
      assert.ok(okImfTx.succeeded, 'Valid IMF request/callback transaction must succeed');

      // 8. Verify exact 7-argument OutLayer ABI metadata captured on Mock OutLayer
      const meta = await mockOutlayer.view('get_last_execution', {});
      assert.ok(meta, 'Mock OutLayer must expose captured execution metadata');
      assert.strictEqual(meta.source_kind, 'WasmUrl');
      assert.strictEqual(meta.wasm_url, WORKER_FASTFS_URL);
      assert.strictEqual(meta.wasm_hash, M042D1_WORKER_SHA256);
      assert.strictEqual(meta.build_target, 'wasm32-wasip2');
      assert.strictEqual(meta.max_instructions, 1_000_000_000);
      assert.strictEqual(meta.max_memory_mb, 128);
      assert.strictEqual(meta.max_execution_seconds, 60);
      assert.strictEqual(meta.secrets_ref_present, false, 'IMF secrets_ref must be null');
      assert.strictEqual(meta.response_format, 'Json');
      assert.strictEqual(meta.payer_account_id, bob.accountId);
      assert.strictEqual(meta.params_present, false);
      const capturedInput = JSON.parse(meta.input_data);
      assert.deepStrictEqual(capturedInput, {
        schema_version: 1,
        market: MARKET_IMF,
        mode: 'public_benchmark',
        min_source_count: 1,
        max_upstream_age_sec: HISTORICAL_MAX_AGE_SEC,
        max_source_spread_bps: 0,
      });

      // 10-13. Verify persisted PriceObservation, cleared PendingRequest, and solvency invariant
      const persistedImf = await adapter.view('get_price', { market: MARKET_IMF });
      assert.ok(persistedImf, 'Valid IMF observation must persist');
      assert.strictEqual(persistedImf.market, MARKET_IMF);
      assert.strictEqual(persistedImf.price, '727794219');
      assert.strictEqual(persistedImf.decimals, 8);
      assert.strictEqual(persistedImf.unit, 'USD/kg');
      assert.strictEqual(persistedImf.source, IMF_SOURCE);
      assert.strictEqual(persistedImf.source_count, 1);
      assert.strictEqual(persistedImf.observed_at_sec, validImfObservedAt);
      assert.strictEqual(persistedImf.data_class, DATA_CLASS_IMF);
      assert.strictEqual(persistedImf.nonce, nextNonce);
      assert.strictEqual(await adapter.view('get_pending_request', { nonce: nextNonce }), null);
      assert.strictEqual(await adapter.view('verify_solvency_invariant', {}), true);
      nextNonce += 1;

      console.log(`[PASS] Full IMF Sandbox E2E succeeded (price=$7.27794219/kg, observed_at=${validImfObservedAt}, nonce=${persistedImf.nonce})`);

      // =========================================================================
      // Part 3: Comprehensive Negative IMF Callback Tests (persisted price never overwritten)
      // =========================================================================
      const staleImfObservedAt = chainSec - (HISTORICAL_MAX_AGE_SEC + 120);
      const negativeCases = [
        ['wrong source', { ...validImfPayload, source: 'fred-pcoffotmusdm' }, 'ERR_INVALID_PUBLIC_BENCHMARK_SOURCE'],
        ['LIVE_VERIFIED on IMF', { ...validImfPayload, data_class: 'LIVE_VERIFIED' }, 'ERR_LIVE_OUTPUT_IN_FIXTURE_MODE'],
        ['LIVE_AGGREGATED on IMF', { ...validImfPayload, data_class: 'LIVE_AGGREGATED', source_count: 2 }, 'ERR_LIVE_OUTPUT_IN_FIXTURE_MODE'],
        ['SYNTHETIC_DEMO on IMF', { ...validImfPayload, source: 'roastake-fixture', observed_at: 0, data_class: DATA_CLASS_SYNTHETIC }, 'ERR_SYNTHETIC_NOT_PERMITTED_FOR_MARKET'],
        ['wrong market (ARABICA-REF/USD)', { ...validImfPayload, market: 'ARABICA-REF/USD' }, 'ERR_WRONG_MARKET'],
        ['wrong market (GESHA-DEMO/USD spoof)', { ...validImfPayload, market: MARKET_GESHA_DEMO, price: '1850000000', source: 'roastake-fixture', observed_at: 0, data_class: DATA_CLASS_SYNTHETIC }, 'ERR_WRONG_MARKET'],
        ['observed_at = 0', { ...validImfPayload, observed_at: 0 }, 'ERR_ZERO_TIMESTAMP_NOT_PERMITTED'],
        ['stale > 90 days', { ...validImfPayload, observed_at: staleImfObservedAt }, 'ERR_STALE_TIMESTAMP'],
        ['unknown data_class', { ...validImfPayload, data_class: 'OFFICIAL_BENCHMARK' }, 'ERR_UNSUPPORTED_DATA_CLASS'],
        ['invalid source_count = 0', { ...validImfPayload, source_count: 0 }, 'ERR_ZERO_SOURCE_COUNT'],
        ['invalid source_count = 2', { ...validImfPayload, source_count: 2 }, 'ERR_INVALID_PUBLIC_BENCHMARK_SOURCE_COUNT'],
      ];

      for (const [label, badOutput, expectedErrSubstr] of negativeCases) {
        await mockOutlayer.call(mockOutlayer, 'set_custom_output', { output: badOutput });
        const negTx = parseTransactionResult(
          await bob.callRaw(
            adapter,
            'request_price_update',
            { market: MARKET_IMF },
            { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
          )
        );
        assert.ok(negTx.succeeded, `${label}: callback must handle error safely without panicking`);
        assert.ok(
          negTx.logs.some((l) => l.includes('execution_failed') && l.includes(expectedErrSubstr)),
          `${label}: expected '${expectedErrSubstr}' in logs, got ${JSON.stringify(negTx.logs)}`
        );
        assert.deepStrictEqual(
          await adapter.view('get_price', { market: MARKET_IMF }),
          persistedImf,
          `${label}: must preserve persisted IMF observation`
        );
        assert.strictEqual(await adapter.view('get_pending_request', { nonce: nextNonce }), null);
        assert.strictEqual(await adapter.view('verify_solvency_invariant', {}), true);
        nextNonce += 1;
      }

      // NullResponse after persisted price preserves IMF observation too
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'NullResponse' });
      const postNullTx = parseTransactionResult(
        await bob.callRaw(
          adapter,
          'request_price_update',
          { market: MARKET_IMF },
          { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
        )
      );
      assert.ok(postNullTx.succeeded);
      assert.deepStrictEqual(await adapter.view('get_price', { market: MARKET_IMF }), persistedImf);
      assert.strictEqual(await adapter.view('get_pending_request', { nonce: nextNonce }), null);
      nextNonce += 1;

      // =========================================================================
      // Part 4: Gesha Fixture Compatibility & Cross-Spoofing Protection
      // =========================================================================
      // IMF payload cannot spoof GESHA-DEMO/USD
      await mockOutlayer.call(mockOutlayer, 'set_custom_output', {
        output: { ...validImfPayload, market: MARKET_GESHA_DEMO },
      });
      const imfSpoofGeshaTx = parseTransactionResult(
        await bob.callRaw(
          adapter,
          'request_price_update',
          { market: MARKET_GESHA_DEMO },
          { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
        )
      );
      assert.ok(imfSpoofGeshaTx.succeeded);
      assert.ok(
        imfSpoofGeshaTx.logs.some((l) => l.includes('ERR_PUBLIC_BENCHMARK_NOT_PERMITTED_FOR_MARKET'))
      );
      assert.strictEqual(await adapter.view('get_price', { market: MARKET_GESHA_DEMO }), null);
      nextNonce += 1;

      // Valid Gesha synthetic fixture still stores $18.50/kg cleanly
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'Success' });
      const okGeshaTx = parseTransactionResult(
        await bob.callRaw(
          adapter,
          'request_price_update',
          { market: MARKET_GESHA_DEMO },
          { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
        )
      );
      assert.ok(okGeshaTx.succeeded);
      const persistedGesha = await adapter.view('get_price', { market: MARKET_GESHA_DEMO });
      assert.ok(persistedGesha);
      assert.strictEqual(persistedGesha.market, MARKET_GESHA_DEMO);
      assert.strictEqual(persistedGesha.price, '1850000000');
      assert.strictEqual(persistedGesha.unit, 'USD/kg');
      assert.strictEqual(persistedGesha.source, 'roastake-fixture');
      assert.strictEqual(persistedGesha.data_class, DATA_CLASS_SYNTHETIC);
      assert.deepStrictEqual(await adapter.view('get_price', { market: MARKET_IMF }), persistedImf);
      assert.strictEqual(await adapter.view('verify_solvency_invariant', {}), true);
      console.log('[PASS] Parts 3-4: All 12 negative IMF cases, NullResponse, cross-spoofing guards & Gesha fixture verified.');
    } finally {
      await worker.tearDown();
    }
  });
});
