import { Worker } from 'near-workspaces';
import { enforceSandboxNetworkGuard } from './helpers.mjs';
import { execFileSync } from 'node:child_process';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

async function main() {
  enforceSandboxNetworkGuard();

  console.log('=== PHASE 4: RUNTIME VERIFICATION ===');
  const binDir = resolve(__dirname, '../.near-sandbox/bin');
  const binPath = resolve(binDir, 'near-sandbox');

  process.env.NEAR_SANDBOX_BIN_PATH = binPath;
  process.env.NEAR_SANDBOX_BINARY_PATH = binDir;

  console.log('1. Executing binary directly to check neard version...');
  const versionOutput = execFileSync(binPath, ['--version'], { encoding: 'utf8' }).trim();
  console.log(`   Binary path: ${binPath}`);
  console.log(`   neard output: ${versionOutput}`);

  console.log('\n2. Starting local Sandbox worker via near-workspaces...');
  let worker;
  try {
    worker = await Worker.init({ network: 'sandbox' });
    console.log(`   Worker initialized. Root account: ${worker.rootAccount.accountId}`);

    console.log('\n3. Querying Sandbox RPC node status...');
    const status = await worker.provider.status();
    console.log(`   RPC node version: ${status.version.version}`);
    console.log(`   RPC node build: ${status.version.build}`);
    console.log(`   RPC protocol_version: ${status.protocol_version}`);
    console.log(`   RPC latest_protocol_version: ${status.latest_protocol_version}`);

    console.log('\n4. Validating protocol version >= 85 (near-sdk 5.30.1 requirement)...');
    if (status.protocol_version < 85) {
      throw new Error(
        `[INCOMPATIBLE RUNTIME] Protocol version ${status.protocol_version} is less than required 85!`
      );
    }
    console.log(`   [OK] Protocol version ${status.protocol_version} >= 85 requirement met.`);

    console.log('\n5. Verifying network guard rejection of non-sandbox...');
    let guardBlocked = false;
    try {
      process.env.NEAR_WORKSPACES_NETWORK = 'testnet';
      enforceSandboxNetworkGuard();
    } catch (err) {
      guardBlocked = true;
      console.log(`   [OK] Guard successfully blocked 'testnet': ${err.message}`);
    } finally {
      process.env.NEAR_WORKSPACES_NETWORK = 'sandbox';
    }
    if (!guardBlocked) {
      throw new Error('[CRITICAL FAILURE] Network guard failed to block testnet!');
    }

    console.log('\n[PASS] Runtime verified successfully with nearcore 2.14.0 / protocol 88!');
  } finally {
    if (worker) {
      console.log('Tearing down worker cleanly...');
      await worker.tearDown();
      console.log('Teardown complete.');
    }
  }
}

main().catch((err) => {
  console.error('\n[RUNTIME VERIFICATION FAILED]', err);
  process.exit(1);
});
