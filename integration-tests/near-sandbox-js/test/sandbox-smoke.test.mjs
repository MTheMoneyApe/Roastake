import test from 'node:test';
import assert from 'node:assert/strict';
import { Worker } from 'near-workspaces';

/**
 * Absolute Local Network Protection Guard
 *
 * Verifies that the test harness runs exclusively against the local NEAR Sandbox.
 * Fails closed immediately if Testnet, Mainnet or any unknown network is configured.
 */
function enforceSandboxNetworkGuard() {
  const network = process.env.NEAR_WORKSPACES_NETWORK;
  if (network && network !== 'sandbox') {
    throw new Error(
      `[SECURITY ERROR] ABSOLUTE LOCAL NETWORK PROTECTION: Network '${network}' is strictly forbidden. Only local 'sandbox' is permitted.`
    );
  }
  // Enforce explicit sandbox network
  process.env.NEAR_WORKSPACES_NETWORK = 'sandbox';
}

// Enforce guard at module load time
enforceSandboxNetworkGuard();

test('NEAR Sandbox Integration Smoke Test', async (t) => {
  // Re-verify network guard inside test execution
  enforceSandboxNetworkGuard();

  let worker = null;

  try {
    // 1. Start actual local NEAR Sandbox using Worker.init
    console.log('[SMOKE TEST] Initializing local NEAR Sandbox worker...');
    worker = await Worker.init({
      network: 'sandbox',
    });
    assert.ok(worker, 'Worker instance must be successfully initialized');
    console.log('[SMOKE TEST] Local Sandbox worker initialized successfully.');

    // 2. Obtain the Sandbox root account
    const root = worker.rootAccount;
    assert.ok(root, 'Sandbox root account must exist');

    // 3. Verify that an account ID exists
    const rootAccountId = root.accountId;
    assert.ok(
      typeof rootAccountId === 'string' && rootAccountId.length > 0,
      'Root account ID must be a non-empty string'
    );
    assert.strictEqual(
      rootAccountId,
      'test.near',
      "Sandbox default root account ID is expected to be 'test.near'"
    );
    console.log(`[SMOKE TEST] Root account confirmed: ${rootAccountId}`);

    // Verify root account exists on the local Sandbox ledger
    const rootExists = await root.exists();
    assert.strictEqual(
      rootExists,
      true,
      'Root account must exist on the local Sandbox blockchain'
    );

    // 4. Create one child account using documented createSubAccount API
    const childName = 'smoke-child';
    const expectedChildId = `${childName}.${rootAccountId}`;
    console.log(`[SMOKE TEST] Creating child account '${expectedChildId}'...`);

    const childAccount = await root.createSubAccount(childName, {
      initialBalance: 10_000_000_000_000_000_000_000_000n, // 10 NEAR in yoctoNEAR
    });
    assert.ok(childAccount, 'Child account must be returned');
    assert.strictEqual(
      childAccount.accountId,
      expectedChildId,
      `Child account ID must equal '${expectedChildId}'`
    );
    console.log(`[SMOKE TEST] Child account created: ${childAccount.accountId}`);

    // 5. Confirm that the child account exists
    const childExists = await childAccount.exists();
    assert.strictEqual(
      childExists,
      true,
      'Child account must be confirmed as existing on chain'
    );

    // 6. Check that the child can be queried through the local Sandbox
    const accountView = await childAccount.accountView();
    assert.ok(accountView, 'Child accountView must return valid account state');
    assert.ok(
      BigInt(accountView.amount) > 0n,
      'Child account must have a positive balance'
    );

    const balance = await childAccount.balance();
    assert.ok(balance, 'Child balance query must succeed');
    assert.ok(
      balance.total > 0n,
      'Child balance total must be greater than zero'
    );
    console.log(
      `[SMOKE TEST] Child account verified on-chain. Balance: ${balance.total} yoctoNEAR`
    );

  } finally {
    // 7. Tear down the Sandbox cleanly even if an assertion fails
    if (worker) {
      console.log('[SMOKE TEST] Tearing down local Sandbox worker...');
      await worker.tearDown();
      console.log('[SMOKE TEST] Local Sandbox worker terminated cleanly.');
    }
  }
});
