import { Worker } from 'near-workspaces';
import { enforceSandboxNetworkGuard } from './helpers.mjs';

async function testTopLevel() {
  enforceSandboxNetworkGuard();
  const worker = await Worker.init({ network: 'sandbox' });
  const root = worker.rootAccount;
  console.log('Root:', root.accountId);
  try {
    const alice = await root.createAccount('alice', {
      initialBalance: 10_000_000_000_000_000_000_000_000n,
    });
    console.log('Created top-level account:', alice.accountId);
  } catch (e) {
    console.log('createAccount error:', e.message);
  }
  await worker.tearDown();
}

testTopLevel().catch(console.error);
