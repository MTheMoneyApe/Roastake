import test from 'node:test';
import assert from 'node:assert/strict';
import { Worker } from 'near-workspaces';
import {
  ADAPTER_WASM_PATH,
  MOCK_OUTLAYER_WASM_PATH,
  EXPECTED_ADAPTER_SHA256,
  EXPECTED_MOCK_SHA256,
  RAW_INCOMPATIBLE_ADAPTER_SHA256,
  RAW_INCOMPATIBLE_MOCK_SHA256,
  validateWasmArtifact,
  enforceSandboxNetworkGuard,
} from './helpers.mjs';

test('MISSION 03C.3.2: NEAR Sandbox WASM Compatibility Verification', async (t) => {
  enforceSandboxNetworkGuard();

  let worker;
  try {
    console.log('\n[1/5] Initializing fresh local NEAR Sandbox worker...');
    worker = await Worker.init({ network: 'sandbox' });
    const root = worker.rootAccount;
    console.log(`[OK] Sandbox root account: ${root.accountId}`);

    console.log('\n[2/5] Validating artifact checksums and compatibility...');
    const mockWasm = validateWasmArtifact(
      MOCK_OUTLAYER_WASM_PATH,
      EXPECTED_MOCK_SHA256,
      RAW_INCOMPATIBLE_MOCK_SHA256,
      'Mock OutLayer'
    );
    const adapterWasm = validateWasmArtifact(
      ADAPTER_WASM_PATH,
      EXPECTED_ADAPTER_SHA256,
      RAW_INCOMPATIBLE_ADAPTER_SHA256,
      'ROASTAKE Oracle Adapter'
    );
    console.log(`[OK] Mock OutLayer bytes: ${mockWasm.length}`);
    console.log(`[OK] Oracle Adapter bytes: ${adapterWasm.length}`);

    console.log('\n[3/5] Deploying and testing Mock OutLayer contract...');
    const mockOutlayer = await root.createSubAccount('mock-outlayer', {
      initialBalance: 50_000_000_000_000_000_000_000_000n, // 50 NEAR
    });
    const deployMockRes = await mockOutlayer.deploy(mockWasm);
    console.log(`[OK] Mock OutLayer deploy completed`);

    let mockInitError = null;
    try {
      const initMockRes = await mockOutlayer.call(mockOutlayer, 'new', {});
      console.log(`[OK] Mock OutLayer new() executed successfully`);

      const setModeRes = await mockOutlayer.call(mockOutlayer, 'set_mode', {
        mode: 'Success',
      });
      console.log(`[OK] Mock OutLayer set_mode('Success') executed successfully`);

      const currentMode = await mockOutlayer.view('get_mode', {});
      console.log(`[OK] Mock OutLayer get_mode() returned: ${JSON.stringify(currentMode)}`);
      assert.equal(currentMode, 'Success', 'Mock mode should be Success');
    } catch (err) {
      mockInitError = err;
      console.error(`[MOCK INIT FAILURE] Mock OutLayer new() failed: ${err.message}`);
    }

    console.log('\n[4/5] Deploying and testing ROASTAKE Oracle Adapter contract...');
    const alice = await root.createSubAccount('alice', {
      initialBalance: 100_000_000_000_000_000_000_000_000n, // 100 NEAR
    });
    const bob = await root.createSubAccount('bob', {
      initialBalance: 100_000_000_000_000_000_000_000_000n, // 100 NEAR
    });
    const adapterAccount = await root.createSubAccount('roastake-adapter', {
      initialBalance: 50_000_000_000_000_000_000_000_000n, // 50 NEAR
    });

    const deployAdapterRes = await adapterAccount.deploy(adapterWasm);
    console.log(`[OK] Oracle Adapter deploy completed`);

    let adapterInitError = null;
    try {
      const initAdapterRes = await adapterAccount.call(adapterAccount, 'init', {
        admin: 'alice.testnet',
        outlayer_contract: 'mock.testnet',
        initial_wasm_url: 'https://test.fastfs.io/roastake/worker.wasm',
        initial_wasm_sha256:
          'abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6',
      });
      console.log(`[OK] Oracle Adapter init() executed successfully`);

      const adminView = await adapterAccount.view('get_admin', {});
      console.log(`[OK] Oracle Adapter get_admin(): ${adminView}`);
      assert.equal(adminView, 'alice.testnet', 'Admin must match alice.testnet');

      const outlayerView = await adapterAccount.view('get_outlayer_contract', {});
      console.log(`[OK] Oracle Adapter get_outlayer_contract(): ${outlayerView}`);
      assert.equal(
        outlayerView,
        'mock.testnet',
        'OutLayer contract must match mock.testnet'
      );

      const isAliceAuth = await adapterAccount.view('is_requester', {
        account_id: 'alice.testnet',
      });
      console.log(`[OK] Oracle Adapter is_requester('alice.testnet'): ${isAliceAuth}`);
      assert.equal(isAliceAuth, true, 'Alice must be authorized requester');
    } catch (err) {
      adapterInitError = err;
      console.error(`[ADAPTER INIT FAILURE] Oracle Adapter init() failed: ${err.message}`);
    }

    if (mockInitError || adapterInitError) {
      const failures = [];
      if (mockInitError) failures.push(`Mock OutLayer: ${mockInitError.message}`);
      if (adapterInitError) failures.push(`Oracle Adapter: ${adapterInitError.message}`);
      throw new Error(`[COMPATIBILITY TEST FAILED]\n` + failures.join('\n'));
    }

    console.log('\n[5/5] ALL COMPATIBILITY SMOKE CHECKS PASSED SUCCESSFULLY!');
  } finally {
    if (worker) {
      console.log('Tearing down local sandbox...');
      await worker.tearDown();
      console.log('Sandbox shutdown complete.');
    }
  }
});
