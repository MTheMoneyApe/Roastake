import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Worker, KeyPair } from 'near-workspaces';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// Paths to compiled NEAR Sandbox-compatible WASM artifacts
export const ADAPTER_WASM_PATH = resolve(
  __dirname,
  '../artifacts/roastake_oracle_adapter.wasm'
);

export const ADAPTER_SECURITY_FIXED_WASM_PATH = resolve(
  __dirname,
  '../artifacts/roastake_oracle_adapter_security_fixed.wasm'
);

export const ADAPTER_ADMIN_HARDENED_WASM_PATH = resolve(
  __dirname,
  '../artifacts/roastake_oracle_adapter_admin_hardened.wasm'
);

export const ADAPTER_M03374_RELEASE_CANDIDATE_WASM_PATH = resolve(
  __dirname,
  '../artifacts/roastake_oracle_adapter_release_candidate.wasm'
);

export const ADAPTER_RELEASE_CANDIDATE_WASM_PATH = resolve(
  __dirname,
  '../artifacts/roastake_oracle_adapter_release_candidate_m03376.wasm'
);

export const MOCK_OUTLAYER_WASM_PATH = resolve(
  __dirname,
  '../artifacts/mock_outlayer.wasm'
);

export const MOCK_OUTLAYER_ABI_FIXED_WASM_PATH = resolve(
  __dirname,
  '../artifacts/mock_outlayer_abi_fixed.wasm'
);

// Constants - Expected SHA-256 of optimized compatible artifacts
export const EXPECTED_ADAPTER_SHA256 =
  'ebd76a9346ce5c978381b2d06689fdca67b37a4feed7d5b6a9154375b04dea2f';
export const EXPECTED_ADAPTER_SECURITY_FIXED_SHA256 =
  'f4ae912d18f8d219f2dd643a22a75ccb6a5681c3670c1bef1b4a3a87420b4186';
export const EXPECTED_ADAPTER_ADMIN_HARDENED_SHA256 =
  'bc5fe10d77fe8cca112a5174f52568fb8b9de662c30d16516cffc45813a843c7';
export const EXPECTED_ADAPTER_M03374_RELEASE_CANDIDATE_SHA256 =
  '8ef00c67059b888ddff5de60829b2ffa7c26e066c82befb208fff41d3cebd00f';
export const EXPECTED_ADAPTER_RELEASE_CANDIDATE_SHA256 =
  'a9a07710ed10133b36b503fe81185cd83a8bbc2179a3f17ce8f09cc94c3756b8';
export const EXPECTED_MOCK_SHA256 =
  'cf060259e3d2936e5ed4754e22ec8bb5e7d32ec2141d1ad9cfe9e44cf71b69cc';
export const EXPECTED_MOCK_ABI_FIXED_SHA256 =
  'a37bd40d21a7d85e0de6e6bfb369bebe41313222dd938b6d6a5361d1d99883e4';

// Known raw (unoptimized / incompatible) baselines
export const RAW_INCOMPATIBLE_ADAPTER_SHA256 =
  'ad83a42a634a33f4a1163744b09eec2b6f33118c925ef2aec294ac74421df93a';
export const RAW_INCOMPATIBLE_MOCK_SHA256 =
  '42ccd6cce7915d6da765302f5fd4b41f621103f133c9913170d1485bc593fc51';

export const STANDARD_ATTACHED_DEPOSIT = 50_000_000_000_000_000_000_000n; // 50 milliNEAR (0.05 NEAR)
export const DEFAULT_MIN_OPERATING_RESERVE = '10000000000000000000000'; // 10 milliNEAR (0.01 NEAR)
export const CALIBRATED_OPERATING_BUFFER = 15_000_000_000_000_000_000_000n; // 15 milliNEAR operating buffer
export const TEST_EVIDENCE_SHA256 =
  'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855';
export const STANDARD_CALL_GAS = 250_000_000_000_000n; // 250 TGas
export const STANDARD_CLAIM_GAS = 150_000_000_000_000n; // 150 TGas
export const MARKET_GESHA_DEMO = 'GESHA-DEMO/USD';

// 610-byte no_std WASM helper exporting `claim_and_delete` (calls adapter.claim_refund() and deletes self)
export const EPHEMERAL_CLAIMANT_WASM = Buffer.from(
  'AGFzbQEAAAABKAdgAX4AYAF+AX5gAn5+AGACfn4BfmAHfn5+fn5+fgBgA35+fgBgAAAC0wEIA2VudgVpbnB1dAAAA2VudgxyZWdpc3Rlcl9sZW4AAQNlbnYNcmVhZF9yZWdpc3RlcgACA2VudhRwcm9taXNlX2JhdGNoX2NyZWF0ZQADA2VudiJwcm9taXNlX2JhdGNoX2FjdGlvbl9mdW5jdGlvbl9jYWxsAAQDZW52EmN1cnJlbnRfYWNjb3VudF9pZAAAA2VudhFzaWduZXJfYWNjb3VudF9pZAAAA2VudiNwcm9taXNlX2JhdGNoX2FjdGlvbl9kZWxldGVfYWNjb3VudAAFAwIBBgUDAQARBgkBfwFBgIDAAAsHHQIGbWVtb3J5AgAQY2xhaW1fYW5kX2RlbGV0ZQAICosCAYgCAgF/BH4jgICAgABBkANrIgAkgICAgABCABCAgICAAEIAEIGAgIAAIQEgAEEAQYAB/AsAQgAgAK0iAhCCgICAACABIAIQg4CAgAAhASAAQgA3A4gBIABCADcDgAEgAUIMQYCAwIAArUICQYyAwIAArSAAQYABaq1CgIDpg7HeFhCEgICAAEIBEIWAgIAAQgEQgYCAgAAhASAAQZABakEAQYAB/AsAQgEgAEGQAWqtIgIQgoCAgABCAhCGgICAAEICEIGAgIAAIQMgAEGQAmpBAEGAAfwLAEICIABBkAJqrSIEEIKAgIAAIAEgAhCDgICAACADIAQQh4CAgAAgAEGQA2okgICAgAALCxcBAEGAgMAACw5jbGFpbV9yZWZ1bmR7fQ==',
  'base64'
);

/**
 * Validates artifact existence, integrity, and compatibility before deployment.
 */
export function validateWasmArtifact(path, expectedSha256, rawSha256, name) {
  if (!existsSync(path)) {
    throw new Error(
      `[PREDEPLOYMENT INTEGRITY ERROR] ${name} artifact missing at path: ${path}`
    );
  }
  const bytes = readFileSync(path);
  const actualSha256 = createHash('sha256').update(bytes).digest('hex').toLowerCase();

  if (actualSha256 === rawSha256.toLowerCase()) {
    throw new Error(
      `[PREDEPLOYMENT INTEGRITY ERROR] Raw unoptimized ${name} artifact detected (${actualSha256}). Only cargo-near optimized artifacts from artifacts/ are permitted.`
    );
  }

  if (actualSha256 !== expectedSha256.toLowerCase()) {
    throw new Error(
      `[PREDEPLOYMENT INTEGRITY ERROR] ${name} checksum mismatch. Expected ${expectedSha256}, got ${actualSha256}.`
    );
  }

  return bytes;
}

/**
 * Configures the verified local nearcore 2.14.0 binary path if available.
 */
export function configureSandboxBinary() {
  const binDir = resolve(__dirname, '../.near-sandbox/bin');
  const binPath = resolve(binDir, 'near-sandbox');
  if (existsSync(binPath)) {
    process.env.NEAR_SANDBOX_BIN_PATH = binPath;
    process.env.NEAR_SANDBOX_BINARY_PATH = binDir;
  }
}

/**
 * Enforces that tests run strictly against local Sandbox.
 */
export function enforceSandboxNetworkGuard() {
  configureSandboxBinary();
  const network = process.env.NEAR_WORKSPACES_NETWORK;
  if (network && network !== 'sandbox') {
    throw new Error(
      `[SECURITY GUARD] Rejected network '${network}'. Only local 'sandbox' is permitted.`
    );
  }
  process.env.NEAR_WORKSPACES_NETWORK = 'sandbox';
}

/**
 * Creates an implicit account by generating an ED25519 keypair, transferring funds to the 64-char hex account ID,
 * and registering the private key in the worker's keystore.
 * Implicit accounts bypass the .near mainnet rejection policy in validate_testnet_account_id.
 */
export async function createImplicitAccount(worker, root, initialBalance) {
  const keyPair = KeyPair.fromRandom('ed25519');
  const accountId = Buffer.from(keyPair.getPublicKey().data).toString('hex');
  await root.transfer(accountId, initialBalance);
  const account = root.getAccount(accountId);
  await worker.manager.setKey(accountId, keyPair);
  return account;
}

/**
 * Safely parses and validates a near-workspaces 5.0.0 TransactionResult object.
 * Extracts canonical transaction ID, receipt outcomes, and logs based on actual runtime properties.
 */
export function parseTransactionResult(txResult) {
  if (!txResult || !txResult.result) {
    throw new Error(`[API ERROR] Invalid TransactionResult: missing .result property`);
  }
  const { result } = txResult;
  if (!result.transaction_outcome || !result.transaction_outcome.id) {
    throw new Error(`[API ERROR] Invalid FinalExecutionOutcome: missing transaction_outcome.id`);
  }
  if (!Array.isArray(result.receipts_outcome)) {
    throw new Error(`[API ERROR] Invalid FinalExecutionOutcome: missing receipts_outcome array`);
  }

  const txId = result.transaction_outcome.id;
  const txHash = txResult.transactionReceipt ? txResult.transactionReceipt.hash : txId;
  const receipts = result.receipts_outcome.map((r) => ({
    id: r.id,
    executorId: r.outcome.executor_id,
    status: r.outcome.status,
    gasBurnt: r.outcome.gas_burnt,
    logs: r.outcome.logs,
    isSuccess:
      typeof r.outcome.status === 'object' && r.outcome.status.SuccessValue !== undefined,
    isFailure:
      typeof r.outcome.status === 'object' && r.outcome.status.Failure !== undefined,
    failureError:
      typeof r.outcome.status === 'object' && r.outcome.status.Failure !== undefined
        ? r.outcome.status.Failure
        : null,
  }));

  const receiptIds = receipts.map((r) => r.id);
  const logs = txResult.logs || [];
  const succeeded = txResult.succeeded;
  const failed = txResult.failed;
  const status = txResult.status;

  return {
    txId,
    txHash,
    txOutcome: result.transaction_outcome,
    receipts,
    receiptIds,
    logs,
    succeeded,
    failed,
    status,
    raw: txResult,
  };
}

/**
 * Creates a named `.testnet` account directly in local NEAR Sandbox state using `patchStateRecords`.
 * Useful for testing named accounts that are NOT auto-created upon receiving a Transfer if deleted.
 */
export async function createSandboxNamedTestnetAccount(worker, root, accountId, initialBalance) {
  const keyPair = KeyPair.fromRandom('ed25519');
  const account = root.getAccount(accountId);
  await account.updateAccount({
    amount: initialBalance.toString(),
    locked: '0',
    code_hash: '11111111111111111111111111111111',
    storage_usage: 182,
  });
  await account.updateAccessKey(keyPair, {
    nonce: 0,
    permission: 'FullAccess',
  });
  await worker.manager.setKey(accountId, keyPair);
  return { account, keyPair };
}

/**
 * Sets up a complete test fixture with:
 * - Local Sandbox Worker
 * - Root account (test.near)
 * - Alice (implicit account: authorized requester & admin)
 * - Bob (implicit account: authorized requester)
 * - Adapter account (implicit account) with deployed security-fixed WASM
 * - Mock OutLayer account (implicit account) with deployed mock WASM
 */
export async function setupSandboxFixture(options = {}) {
  enforceSandboxNetworkGuard();

  const worker = await Worker.init({
    network: 'sandbox',
  });

  const root = worker.rootAccount;

  // Create implicit accounts (satisfies validate_testnet_account_id alphanumeric check)
  const alice = await createImplicitAccount(
    worker,
    root,
    100_000_000_000_000_000_000_000_000n // 100 NEAR
  );

  const bob = await createImplicitAccount(
    worker,
    root,
    100_000_000_000_000_000_000_000_000n // 100 NEAR
  );

  const mockOutlayer = await createImplicitAccount(
    worker,
    root,
    50_000_000_000_000_000_000_000_000n // 50 NEAR
  );

  const adapterAccount = await createImplicitAccount(
    worker,
    root,
    50_000_000_000_000_000_000_000_000n // 50 NEAR
  );

  // Validate and load WASM bytes with strict predeployment integrity checks
  const mockPath = options.mockWasmPath || MOCK_OUTLAYER_ABI_FIXED_WASM_PATH;
  const mockExpectedHash = options.mockExpectedHash || EXPECTED_MOCK_ABI_FIXED_SHA256;
  const adapterPath = options.adapterWasmPath || ADAPTER_RELEASE_CANDIDATE_WASM_PATH;
  const adapterExpectedHash =
    options.adapterExpectedHash || EXPECTED_ADAPTER_RELEASE_CANDIDATE_SHA256;

  const mockWasm = validateWasmArtifact(
    mockPath,
    mockExpectedHash,
    RAW_INCOMPATIBLE_MOCK_SHA256,
    'Mock OutLayer'
  );
  const adapterWasm = validateWasmArtifact(
    adapterPath,
    adapterExpectedHash,
    RAW_INCOMPATIBLE_ADAPTER_SHA256,
    'ROASTAKE Oracle Adapter (Release Candidate)'
  );

  let runtimeVersionStr = 'nearcore 2.14.0 / protocol 88';
  try {
    const status = await worker.provider.status();
    if (status && status.version && status.protocol_version) {
      runtimeVersionStr = `nearcore ${status.version.version} / protocol ${status.protocol_version}`;
    }
  } catch (_) {
    // Fallback to configured runtime string if status query is unavailable
  }

  console.log(
    `[SANDBOX FIXTURE] Runtime: ${runtimeVersionStr} | Adapter: ${adapterPath} (SHA-256: ${adapterExpectedHash}, ${adapterWasm.length} B) | Mock: ${mockExpectedHash} (${mockWasm.length} B)`
  );

  // Deploy Mock OutLayer
  await mockOutlayer.deploy(mockWasm);
  await mockOutlayer.call(mockOutlayer, 'new', {});

  // Deploy ROASTAKE Release-Candidate Adapter WASM
  await adapterAccount.deploy(adapterWasm);

  // Initialize ROASTAKE Adapter with explicit positive minimum_operating_reserve
  const initArgs = {
    admin: alice.accountId,
    outlayer_contract: mockOutlayer.accountId,
    initial_wasm_url: 'https://test.fastfs.io/roastake/worker.wasm',
    initial_wasm_sha256:
      'abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6',
    minimum_operating_reserve: options.minimumOperatingReserve || DEFAULT_MIN_OPERATING_RESERVE,
  };
  if (options.maxOpenRequests !== undefined) {
    initArgs.max_open_requests = options.maxOpenRequests;
  }
  await adapterAccount.call(adapterAccount, 'init', initArgs);

  // Admin Alice authorizes Bob as requester
  await alice.call(adapterAccount, 'add_requester', {
    account_id: bob.accountId,
  });

  // Calibrate minimum_operating_reserve so unallocated spendable headroom is ~15 mNEAR (< 50 mNEAR deposit).
  // This ensures `admin_designate_refund_reserve(50 mNEAR)` only succeeds if a 50 mNEAR deposit was
  // physically returned to the adapter (Test C) and fails closed if OutLayer retained the deposit (Test D).
  if (options.calibrateOperatingReserve !== false) {
    const spendableNow = BigInt(await adapterAccount.view('get_spendable_balance', {}));
    const calibratedMin = spendableNow - (options.operatingBuffer || CALIBRATED_OPERATING_BUFFER);
    await alice.call(adapterAccount, 'increase_minimum_operating_reserve', {
      new_minimum: calibratedMin.toString(),
    });
  }

  return {
    worker,
    root,
    alice,
    bob,
    mockOutlayer,
    adapterAccount,
  };
}

/**
 * Sets up a Sandbox fixture deploying the Mission 03C.3.7.4 release-candidate adapter WASM.
 */
export async function setupHardenedSandboxFixture(options = {}) {
  return setupSandboxFixture({
    ...options,
    adapterWasmPath: options.adapterWasmPath || ADAPTER_RELEASE_CANDIDATE_WASM_PATH,
    adapterExpectedHash:
      options.adapterExpectedHash || EXPECTED_ADAPTER_RELEASE_CANDIDATE_SHA256,
  });
}

export async function setupReleaseCandidateSandboxFixture(options = {}) {
  return setupHardenedSandboxFixture(options);
}

function encodeU128LE(val) {
  let v = BigInt(val);
  const buf = Buffer.alloc(16);
  for (let i = 0; i < 16; i++) {
    buf[i] = Number(v & 0xffn);
    v >>= 8n;
  }
  return buf;
}

function encodeU64LE(val) {
  const buf = Buffer.alloc(8);
  buf.writeBigUInt64LE(BigInt(val), 0);
  return buf;
}

function encodeU32LE(val) {
  const buf = Buffer.alloc(4);
  buf.writeUInt32LE(Number(val), 0);
  return buf;
}

function encodeBorshString(str) {
  const bytes = Buffer.from(str, 'utf8');
  const len = Buffer.alloc(4);
  len.writeUInt32LE(bytes.length, 0);
  return Buffer.concat([len, bytes]);
}

function findRawKeySize(rawItems, keyBuf) {
  const found = rawItems.find((item) => Buffer.from(item.key, 'base64').equals(keyBuf));
  if (!found) {
    return 0;
  }
  const valBuf = Buffer.from(found.value, 'base64');
  // NEAR trie per-record storage usage = 40 bytes overhead + key.length + value.length
  return 40 + keyBuf.length + valBuf.length;
}

/**
 * Deterministically reconstructs an InFlight stuck-claim state (simulating a missing callback
 * right after an outbound Transfer receipt has executed) via Sandbox `patchStateRecords`,
 * without double-counting liabilities (`F-06`), and verifies accounting identities post-injection.
 */
export async function injectStuckInFlightClaimInSandbox(
  worker,
  adapterAccount,
  claimantAccountId,
  claimAmount,
  originatingNonce,
  claimId,
  options = {}
) {
  const amountBig = BigInt(claimAmount);
  const snapBefore = await adapterAccount.view('get_accounting_snapshot', {});
  const existingCredit = await adapterAccount.view('get_pending_refund', {
    account_id: claimantAccountId,
  });
  const existingClaim = await adapterAccount.view('get_claim', { claim_id: claimId });

  const minReserveBuf = encodeU128LE(snapBefore.minimum_operating_reserve);
  const claimsAnchor = Buffer.concat([
    minReserveBuf,
    encodeU64LE(snapBefore.next_claim_id),
    Buffer.from([1, 0, 0, 0, 0x63]),
  ]);

  const rawItems = await adapterAccount.viewStateRaw();
  const stateItem = rawItems.find((item) =>
    Buffer.from(item.key, 'base64').equals(Buffer.from('STATE'))
  );
  assert.ok(stateItem, 'Contract STATE key must exist');
  const stateBuf = Buffer.from(stateItem.value, 'base64');

  const anchorOffset = stateBuf.indexOf(claimsAnchor);
  assert.ok(anchorOffset >= 73, 'Must locate minimum_operating_reserve anchor inside STATE');
  assert.ok(
    stateBuf.subarray(anchorOffset - 73, anchorOffset - 68).equals(Buffer.from([1, 0, 0, 0, 0x66])),
    'Must verify pending_refunds prefix b"f" at anchorOffset - 73'
  );

  // Compute non-duplicating net liability deltas (Finding F-06 & L-01)
  const prevAvailable = existingCredit ? BigInt(existingCredit.available_amount) : 0n;
  const prevInFlight = existingCredit ? BigInt(existingCredit.in_flight_amount) : 0n;
  const prevReturnPending = existingCredit ? BigInt(existingCredit.return_pending_amount) : 0n;
  const prevDisputed = existingCredit ? BigInt(existingCredit.disputed_amount) : 0n;

  let nextAvailable = prevAvailable;
  let nextInFlight = amountBig;
  let totalRefundDelta = 0n;
  let inFlightDelta = 0n;
  let outboundTransferDelta = 0n;

  if (prevInFlight >= amountBig) {
    // Already counted in both total_refund_liability and total_in_flight_liability
    nextInFlight = prevInFlight;
    totalRefundDelta = 0n;
    inFlightDelta = 0n;
  } else if (prevAvailable >= amountBig) {
    // Converting an existing Available entitlement into InFlight: deduct outbound Transfer from account.amount
    nextAvailable = prevAvailable - amountBig;
    nextInFlight = prevInFlight + amountBig;
    totalRefundDelta = 0n;
    inFlightDelta = amountBig;
    outboundTransferDelta = options.deductAccountBalance === false ? 0n : amountBig;
  } else {
    // Reconstructing an InFlight state after a real claim_refund() outbound transfer callback already settled
    nextInFlight = prevInFlight + amountBig;
    totalRefundDelta = amountBig;
    inFlightDelta = amountBig;
  }

  const newTotalRefund = BigInt(snapBefore.total_refund_liability) + totalRefundDelta;
  const newInFlight = BigInt(snapBefore.in_flight_liability) + inFlightDelta;
  const newNextClaimId = BigInt(Math.max(Number(snapBefore.next_claim_id), claimId + 1));
  const newActiveCreditCount =
    Number(snapBefore.active_refund_credit_count) + (existingCredit ? 0 : 1);
  const newActiveClaimCount =
    Number(snapBefore.active_claim_count) + (existingClaim ? 0 : 1);

  // Exact Borsh layout relative to anchorOffset (minimum_operating_reserve):
  //   anchorOffset - 73: pending_refunds prefix [1, 0, 0, 0, 0x66] (5 bytes)
  //   anchorOffset - 68: active_refund_credit_count (u32, 4 bytes)
  //   anchorOffset - 64: total_refund_liability (u128, 16 bytes)
  //   anchorOffset - 48: total_in_flight_liability (u128, 16 bytes)
  //   anchorOffset - 32: total_disputed_liability (u128, 16 bytes)
  //   anchorOffset - 16: refund_reserve_balance (u128, 16 bytes)
  //   anchorOffset + 0:  minimum_operating_reserve (u128, 16 bytes)
  //   anchorOffset + 16: next_claim_id (u64, 8 bytes)
  //   anchorOffset + 24: claims prefix [1, 0, 0, 0, 0x63] (5 bytes)
  //   anchorOffset + 29: active_claim_count (u32, 4 bytes)
  const patchedState = Buffer.from(stateBuf);
  encodeU32LE(newActiveCreditCount).copy(patchedState, anchorOffset - 68);
  encodeU128LE(newTotalRefund).copy(patchedState, anchorOffset - 64);
  encodeU128LE(newInFlight).copy(patchedState, anchorOffset - 48);
  encodeU64LE(newNextClaimId).copy(patchedState, anchorOffset + 16);
  encodeU32LE(newActiveClaimCount).copy(patchedState, anchorOffset + 29);

  // Construct pending_refunds entry key `b"f" || borsh_string(claimant)`
  const creditKey = Buffer.concat([Buffer.from('f'), encodeBorshString(claimantAccountId)]);
  const creditValue = Buffer.concat([
    encodeBorshString(claimantAccountId),
    encodeU128LE(nextAvailable), // available_amount preserved
    encodeU128LE(nextInFlight), // in_flight_amount
    encodeU128LE(prevReturnPending), // return_pending_amount preserved
    encodeU128LE(prevDisputed), // disputed_amount preserved
    Buffer.from([1]), // RefundStatus::InFlight
    encodeU64LE(originatingNonce),
    Buffer.from([1]), // Option::Some
    encodeU64LE(claimId),
  ]);

  const blockHeight = (await worker.provider.block({ finality: 'final' })).header.height;
  const nowSec = BigInt(Math.floor(Date.now() / 1000));
  const claimKey = Buffer.concat([Buffer.from('c'), encodeU64LE(claimId)]);
  const claimValue = Buffer.concat([
    encodeU64LE(claimId),
    encodeBorshString(claimantAccountId),
    encodeU128LE(amountBig),
    encodeU64LE(originatingNonce),
    encodeU64LE(nowSec),
    encodeU64LE(BigInt(blockHeight)),
    Buffer.from([0]), // disputed_at_block = None
    Buffer.from([0]), // ClaimStatus::InFlight
    Buffer.from([0]), // admin_settlement_proposed = false
    Buffer.from([0]), // settlement_proposed_at_block = None
    Buffer.from([0]), // settlement_evidence_sha256 = None
  ]);

  const oldCreditBytes = findRawKeySize(rawItems, creditKey);
  const newCreditBytes = 40 + creditKey.length + creditValue.length;
  const oldClaimBytes = findRawKeySize(rawItems, claimKey);
  const newClaimBytes = 40 + claimKey.length + claimValue.length;
  const storageDeltaBytes = newCreditBytes - oldCreditBytes + (newClaimBytes - oldClaimBytes);

  const acctBefore = await adapterAccount.accountView();
  const expectedAmount = BigInt(acctBefore.amount) - outboundTransferDelta;
  assert.ok(expectedAmount >= 0n, 'Insufficient account balance for simulated outbound claim transfer');
  const expectedStorageUsage = Math.max(0, Number(acctBefore.storage_usage) + storageDeltaBytes);

  await adapterAccount.updateData(Buffer.from('STATE'), patchedState);
  await adapterAccount.updateData(creditKey, creditValue);
  await adapterAccount.updateData(claimKey, claimValue);
  if (storageDeltaBytes !== 0 || outboundTransferDelta !== 0n) {
    await adapterAccount.updateAccount({
      amount: expectedAmount.toString(),
      locked: acctBefore.locked,
      code_hash: acctBefore.code_hash,
      storage_usage: expectedStorageUsage,
    });
  }

  // Sandbox fast-forward 1 block so view queries at optimistic/final height observe patched state cleanly
  await worker.provider.fastForward(1);

  const verifiedClaim = await adapterAccount.view('get_claim', { claim_id: claimId });
  const verifiedCredit = await adapterAccount.view('get_pending_refund', {
    account_id: claimantAccountId,
  });
  const snapAfter = await adapterAccount.view('get_accounting_snapshot', {});
  const acctAfter = await adapterAccount.accountView();

  assert.ok(
    verifiedClaim && verifiedClaim.status === 'InFlight',
    'Injected InFlight claim must deserialize cleanly'
  );
  assert.strictEqual(verifiedClaim.admin_settlement_proposed, false);
  assert.strictEqual(BigInt(verifiedCredit.in_flight_amount), nextInFlight);
  assert.strictEqual(BigInt(snapAfter.total_refund_liability), newTotalRefund);
  assert.strictEqual(BigInt(snapAfter.in_flight_liability), newInFlight);
  assert.strictEqual(BigInt(acctAfter.amount), expectedAmount);
  assert.strictEqual(Number(acctAfter.storage_usage), expectedStorageUsage);
  assert.strictEqual(
    BigInt(snapAfter.total_refund_liability),
    BigInt(snapAfter.backed_refund_liability) +
      BigInt(snapAfter.in_flight_liability) +
      BigInt(snapAfter.disputed_liability),
    'Post-injection total_refund_liability must equal backed + in_flight + disputed liability without duplication'
  );
  assert.strictEqual(snapAfter.invariant_holds, true, 'Post-injection solvency invariant must hold');
  return verifiedClaim;
}

/**
 * Deterministically reconstructs an InFlight insurance compensation record (simulating a missing
 * `on_insurance_transfer` callback) without double-counting insurance reserve or creating phantom surplus (`L-01`).
 *
 * Modes:
 * - Default (`outboundTransferAlreadyExecuted: false`): models transitioning an existing funded
 *   `insurance_reserve_balance` into `InFlight` by deducting `amountBig` from both
 *   `insurance_reserve_balance` and `account.amount` while adding `amountBig` to
 *   `total_in_flight_insurance_liability`. Fails if `insurance_reserve_balance < amountBig`.
 * - Callback-missing post-settlement (`outboundTransferAlreadyExecuted: true`): used when
 *   `compensate_disputed_claim` already ran on-chain (so `insurance_reserve_balance` and `account.amount`
 *   were genuinely deducted and the originating `Disputed` claim was closed) and `on_insurance_transfer`
 *   already deleted the compensation record; reconstructs only the missing-callback `InFlight` record.
 */
export async function injectStuckInFlightInsuranceInSandbox(
  worker,
  adapterAccount,
  compensationId,
  originatingClaimId,
  recipientAccountId,
  compensationAmount,
  evidenceSha256,
  options = {}
) {
  const amountBig = BigInt(compensationAmount);
  const originatingNonce = BigInt(options.originatingNonce ?? originatingClaimId);
  const outboundAlreadyExecuted = options.outboundTransferAlreadyExecuted === true;

  const snapBefore = await adapterAccount.view('get_accounting_snapshot', {});
  const existingComp = await adapterAccount.view('get_insurance_compensation', {
    compensation_id: compensationId,
  });
  assert.strictEqual(
    existingComp,
    null,
    `Insurance compensation #${compensationId} already exists; refusing to duplicate or overwrite`
  );
  const existingClaim = await adapterAccount.view('get_claim', {
    claim_id: originatingClaimId,
  });
  assert.strictEqual(
    existingClaim,
    null,
    `Originating claim #${originatingClaimId} must already be closed before an InFlight insurance compensation exists`
  );

  if (!outboundAlreadyExecuted && options.allowRemovedClaimReference !== true) {
    throw new Error(
      `ERR_UNJUSTIFIED_REMOVED_CLAIM_REF: call real compensate_disputed_claim(${originatingClaimId}) with { outboundTransferAlreadyExecuted: true } or set { allowRemovedClaimReference: true } when explicitly modeling reserve+transfer initiation`
    );
  }
  const reserveBefore = BigInt(snapBefore.insurance_reserve_balance);
  if (!outboundAlreadyExecuted && reserveBefore < amountBig) {
    throw new Error(
      `ERR_IMPOSSIBLE_INSURANCE_ACCOUNTING: insurance_reserve_balance (${reserveBefore}) < compensation amount (${amountBig})`
    );
  }

  const minReserveBuf = encodeU128LE(snapBefore.minimum_operating_reserve);
  const claimsAnchor = Buffer.concat([
    minReserveBuf,
    encodeU64LE(snapBefore.next_claim_id),
    Buffer.from([1, 0, 0, 0, 0x63]),
  ]);

  const rawItems = await adapterAccount.viewStateRaw();
  const stateItem = rawItems.find((item) =>
    Buffer.from(item.key, 'base64').equals(Buffer.from('STATE'))
  );
  assert.ok(stateItem, 'Contract STATE key must exist');
  const stateBuf = Buffer.from(stateItem.value, 'base64');

  const anchorOffset = stateBuf.indexOf(claimsAnchor);
  assert.ok(anchorOffset >= 73, 'Must locate minimum_operating_reserve anchor inside STATE');
  assert.ok(
    stateBuf.subarray(anchorOffset + 106, anchorOffset + 111).equals(Buffer.from([1, 0, 0, 0, 0x69])),
    'Must verify insurance_compensations prefix b"i" at anchorOffset + 106'
  );

  const newInsuranceReserve = outboundAlreadyExecuted ? reserveBefore : reserveBefore - amountBig;
  const newInsuranceInFlight = BigInt(snapBefore.insurance_in_flight_liability) + amountBig;
  const newNextCompId = BigInt(
    Math.max(Number(snapBefore.next_compensation_id), compensationId + 1)
  );
  const newActiveCompCount = Number(snapBefore.active_insurance_compensation_count) + 1;

  // Exact Borsh layout relative to anchorOffset (minimum_operating_reserve):
  //   anchorOffset + 33:  insurance_enabled (bool, 1 byte)
  //   anchorOffset + 34:  insurance_reserve_balance (u128, 16 bytes)
  //   anchorOffset + 50:  total_in_flight_insurance_liability (u128, 16 bytes)
  //   anchorOffset + 66:  insurance_return_pending_balance (u128, 16 bytes)
  //   anchorOffset + 82:  total_unresolved_insurance_liability (u128, 16 bytes)
  //   anchorOffset + 98:  next_compensation_id (u64, 8 bytes)
  //   anchorOffset + 106: insurance_compensations prefix [1, 0, 0, 0, 0x69] (5 bytes)
  //   anchorOffset + 111: active_insurance_compensation_count (u32, 4 bytes)
  const patchedState = Buffer.from(stateBuf);
  encodeU128LE(newInsuranceReserve).copy(patchedState, anchorOffset + 34);
  encodeU128LE(newInsuranceInFlight).copy(patchedState, anchorOffset + 50);
  encodeU64LE(newNextCompId).copy(patchedState, anchorOffset + 98);
  encodeU32LE(newActiveCompCount).copy(patchedState, anchorOffset + 111);

  const blockHeight = (await worker.provider.block({ finality: 'final' })).header.height;
  const nowSec = BigInt(Math.floor(Date.now() / 1000));
  const compKey = Buffer.concat([Buffer.from('i'), encodeU64LE(compensationId)]);
  const compValue = Buffer.concat([
    encodeU64LE(compensationId),
    encodeU64LE(originatingClaimId),
    encodeU64LE(originatingNonce),
    encodeBorshString(recipientAccountId),
    encodeU128LE(amountBig),
    encodeU64LE(nowSec),
    encodeU64LE(BigInt(blockHeight)),
    Buffer.from([0]), // disputed_at_block = None
    encodeBorshString(evidenceSha256),
    Buffer.from([0]), // InsuranceCompensationStatus::InFlight
  ]);

  const oldCompBytes = findRawKeySize(rawItems, compKey);
  const newCompBytes = 40 + compKey.length + compValue.length;
  const storageDeltaBytes = newCompBytes - oldCompBytes;

  const acctBefore = await adapterAccount.accountView();
  const outboundAccountDelta =
    outboundAlreadyExecuted || options.deductAccountBalance === false ? 0n : amountBig;
  const expectedAmount = BigInt(acctBefore.amount) - outboundAccountDelta;
  assert.ok(expectedAmount >= 0n, 'Insufficient account balance for simulated outbound insurance transfer');
  const expectedStorageUsage = Math.max(0, Number(acctBefore.storage_usage) + storageDeltaBytes);

  await adapterAccount.updateData(Buffer.from('STATE'), patchedState);
  await adapterAccount.updateData(compKey, compValue);
  await adapterAccount.updateAccount({
    amount: expectedAmount.toString(),
    locked: acctBefore.locked,
    code_hash: acctBefore.code_hash,
    storage_usage: expectedStorageUsage,
  });

  await worker.provider.fastForward(1);

  const verifiedComp = await adapterAccount.view('get_insurance_compensation', {
    compensation_id: compensationId,
  });
  const snapAfter = await adapterAccount.view('get_accounting_snapshot', {});
  const acctAfter = await adapterAccount.accountView();

  assert.ok(
    verifiedComp && verifiedComp.status === 'InFlight',
    'Injected InFlight insurance compensation must deserialize cleanly'
  );
  assert.strictEqual(BigInt(snapAfter.insurance_reserve_balance), newInsuranceReserve);
  assert.strictEqual(BigInt(snapAfter.insurance_in_flight_liability), newInsuranceInFlight);
  assert.strictEqual(
    BigInt(snapAfter.insurance_unresolved_liability),
    BigInt(snapBefore.insurance_unresolved_liability)
  );
  assert.strictEqual(
    BigInt(snapAfter.insurance_return_pending_liability),
    BigInt(snapBefore.insurance_return_pending_liability)
  );
  assert.strictEqual(BigInt(acctAfter.amount), expectedAmount);
  assert.strictEqual(BigInt(snapAfter.account_balance), expectedAmount);
  assert.strictEqual(Number(acctAfter.storage_usage), expectedStorageUsage);
  assert.strictEqual(snapAfter.storage_usage_bytes, expectedStorageUsage);
  assert.strictEqual(snapAfter.invariant_holds, true);
  return verifiedComp;
}
