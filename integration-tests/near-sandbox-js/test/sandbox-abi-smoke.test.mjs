import test from 'node:test';
import assert from 'node:assert/strict';
import { Worker } from 'near-workspaces';
import {
  ADAPTER_WASM_PATH,
  ADAPTER_SECURITY_FIXED_WASM_PATH,
  ADAPTER_ADMIN_HARDENED_WASM_PATH,
  ADAPTER_M03374_RELEASE_CANDIDATE_WASM_PATH,
  ADAPTER_RELEASE_CANDIDATE_WASM_PATH,
  MOCK_OUTLAYER_ABI_FIXED_WASM_PATH,
  EXPECTED_ADAPTER_SHA256,
  EXPECTED_ADAPTER_SECURITY_FIXED_SHA256,
  EXPECTED_ADAPTER_ADMIN_HARDENED_SHA256,
  EXPECTED_ADAPTER_M03374_RELEASE_CANDIDATE_SHA256,
  EXPECTED_ADAPTER_RELEASE_CANDIDATE_SHA256,
  EXPECTED_MOCK_ABI_FIXED_SHA256,
  RAW_INCOMPATIBLE_ADAPTER_SHA256,
  RAW_INCOMPATIBLE_MOCK_SHA256,
  STANDARD_ATTACHED_DEPOSIT,
  DEFAULT_MIN_OPERATING_RESERVE,
  STANDARD_CALL_GAS,
  validateWasmArtifact,
  enforceSandboxNetworkGuard,
  createImplicitAccount,
  parseTransactionResult,
} from './helpers.mjs';

test('MISSION 03C.3.7.6: Mock OutLayer ABI & Release-Candidate Adapter Validation', async (t) => {
  enforceSandboxNetworkGuard();

  let worker;
  try {
    console.log('\n[1/4] Starting local Sandbox worker...');
    worker = await Worker.init({ network: 'sandbox' });
    const root = worker.rootAccount;
    console.log(`[OK] Sandbox root: ${root.accountId}`);

    console.log('\n[2/4] Validating WASM artifacts...');
    const mockWasm = validateWasmArtifact(
      MOCK_OUTLAYER_ABI_FIXED_WASM_PATH,
      EXPECTED_MOCK_ABI_FIXED_SHA256,
      RAW_INCOMPATIBLE_MOCK_SHA256,
      'Mock OutLayer (ABI Fixed)'
    );
    const protectedAdapterWasm = validateWasmArtifact(
      ADAPTER_WASM_PATH,
      EXPECTED_ADAPTER_SHA256,
      RAW_INCOMPATIBLE_ADAPTER_SHA256,
      'ROASTAKE Oracle Adapter (Protected Baseline)'
    );
    const securityFixedAdapterWasm = validateWasmArtifact(
      ADAPTER_SECURITY_FIXED_WASM_PATH,
      EXPECTED_ADAPTER_SECURITY_FIXED_SHA256,
      RAW_INCOMPATIBLE_ADAPTER_SHA256,
      'ROASTAKE Oracle Adapter (Security Fixed M03C.3.6)'
    );
    const adminHardenedAdapterWasm = validateWasmArtifact(
      ADAPTER_ADMIN_HARDENED_WASM_PATH,
      EXPECTED_ADAPTER_ADMIN_HARDENED_SHA256,
      RAW_INCOMPATIBLE_ADAPTER_SHA256,
      'ROASTAKE Oracle Adapter (Admin Hardened M03C.3.7.2)'
    );
    const m03374CandidateWasm = validateWasmArtifact(
      ADAPTER_M03374_RELEASE_CANDIDATE_WASM_PATH,
      EXPECTED_ADAPTER_M03374_RELEASE_CANDIDATE_SHA256,
      RAW_INCOMPATIBLE_ADAPTER_SHA256,
      'ROASTAKE Oracle Adapter (Protected Baseline M03C.3.7.4)'
    );
    const adapterWasm = validateWasmArtifact(
      ADAPTER_RELEASE_CANDIDATE_WASM_PATH,
      EXPECTED_ADAPTER_RELEASE_CANDIDATE_SHA256,
      RAW_INCOMPATIBLE_ADAPTER_SHA256,
      'ROASTAKE Oracle Adapter (Release Candidate M03C.3.7.6)'
    );
    console.log(`[OK] Mock OutLayer bytes: ${mockWasm.length} (SHA-256: ${EXPECTED_MOCK_ABI_FIXED_SHA256})`);
    console.log(`[OK] Protected Baseline Adapter bytes: ${protectedAdapterWasm.length}`);
    console.log(`[OK] Security-Fixed Adapter (M03C.3.6) bytes: ${securityFixedAdapterWasm.length}`);
    console.log(`[OK] Admin-Hardened Adapter (M03C.3.7.2) bytes: ${adminHardenedAdapterWasm.length}`);
    console.log(`[OK] Protected RC Adapter (M03C.3.7.4) bytes: ${m03374CandidateWasm.length}`);
    console.log(`[OK] Release-Candidate Adapter (M03C.3.7.6) bytes: ${adapterWasm.length} (SHA-256: ${EXPECTED_ADAPTER_RELEASE_CANDIDATE_SHA256})`);

    console.log('\n[3/4] Deploying contracts to implicit accounts...');
    const alice = await createImplicitAccount(
      worker,
      root,
      100_000_000_000_000_000_000_000_000n
    );
    const mockOutlayer = await createImplicitAccount(
      worker,
      root,
      50_000_000_000_000_000_000_000_000n
    );
    const adapter = await createImplicitAccount(
      worker,
      root,
      50_000_000_000_000_000_000_000_000n
    );

    // Deploy and init Mock
    await mockOutlayer.deploy(mockWasm);
    const initMockRes = await mockOutlayer.callRaw(mockOutlayer, 'new', {});
    const parsedInitMock = parseTransactionResult(initMockRes);
    assert.ok(parsedInitMock.succeeded, 'Mock new() must succeed');
    console.log(`[OK] Mock OutLayer deployed and initialized (Tx: ${parsedInitMock.txId})`);

    // Deploy and init Release-Candidate Adapter (M03C.3.7.6)
    await adapter.deploy(adapterWasm);
    const initAdapterRes = await adapter.callRaw(adapter, 'init', {
      admin: alice.accountId,
      outlayer_contract: mockOutlayer.accountId,
      initial_wasm_url: 'https://test.fastfs.io/roastake/worker.wasm',
      initial_wasm_sha256:
        'abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6',
      minimum_operating_reserve: DEFAULT_MIN_OPERATING_RESERVE,
    });
    const parsedInitAdapter = parseTransactionResult(initAdapterRes);
    assert.ok(parsedInitAdapter.succeeded, 'Adapter init() must succeed');
    console.log(`[OK] Release-Candidate Oracle Adapter (M03C.3.7.6) deployed and initialized (Tx: ${parsedInitAdapter.txId})`);

    console.log('\n[4/4] Direct canonical 7-parameter ABI call to Mock OutLayer...');
    // Construct exact 7-parameter JSON ABI matching production OutLayer
    const directMockRes = await alice.callRaw(
      mockOutlayer,
      'request_execution',
      {
        source: {
          WasmUrl: {
            url: 'https://test.fastfs.io/roastake/worker.wasm',
            hash: 'abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6',
            build_target: 'wasm32-wasip2',
          },
        },
        resource_limits: {
          max_instructions: 1_000_000_000,
          max_memory_mb: 128,
          max_execution_seconds: 60,
        },
        input_data: JSON.stringify({
          schema_version: 1,
          market: 'GESHA-DEMO/USD',
          mode: 'fixture',
        }),
        secrets_ref: null,
        response_format: 'Json',
        payer_account_id: alice.accountId,
        params: null,
      },
      {
        attachedDeposit: STANDARD_ATTACHED_DEPOSIT,
        gas: STANDARD_CALL_GAS,
      }
    );

    const parsedDirectMock = parseTransactionResult(directMockRes);
    console.log(`[OK] Direct Mock request_execution Tx: ${parsedDirectMock.txId}`);
    console.log(`[OK] Receipts generated: ${parsedDirectMock.receiptIds.length}`);
    assert.ok(
      parsedDirectMock.succeeded,
      `Direct Mock call must succeed with canonical ABI! Result: ${JSON.stringify(parsedDirectMock.status)}`
    );
    assert.ok(
      parsedDirectMock.receipts.length > 0,
      'Must contain receipt outcomes'
    );
    assert.ok(
      parsedDirectMock.receipts.every((r) => r.isSuccess),
      'All receipt outcomes must be successful'
    );
    console.log('[OK] Canonical 7-parameter ABI accepted without deserialization errors!');

  } finally {
    if (worker) {
      await worker.tearDown();
      console.log('[OK] Sandbox teardown complete');
    }
  }
});
