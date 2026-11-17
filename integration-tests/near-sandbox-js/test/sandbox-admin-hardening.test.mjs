import test from 'node:test';
import assert from 'node:assert/strict';
import {
  Worker,
} from 'near-workspaces';
import {
  ADAPTER_SECURITY_FIXED_WASM_PATH,
  ADAPTER_ADMIN_HARDENED_WASM_PATH,
  ADAPTER_RELEASE_CANDIDATE_WASM_PATH,
  MOCK_OUTLAYER_ABI_FIXED_WASM_PATH,
  EXPECTED_ADAPTER_SECURITY_FIXED_SHA256,
  EXPECTED_ADAPTER_ADMIN_HARDENED_SHA256,
  EXPECTED_ADAPTER_RELEASE_CANDIDATE_SHA256,
  EXPECTED_MOCK_ABI_FIXED_SHA256,
  RAW_INCOMPATIBLE_ADAPTER_SHA256,
  RAW_INCOMPATIBLE_MOCK_SHA256,
  MARKET_GESHA_DEMO,
  STANDARD_ATTACHED_DEPOSIT,
  DEFAULT_MIN_OPERATING_RESERVE,
  TEST_EVIDENCE_SHA256,
  STANDARD_CALL_GAS,
  STANDARD_CLAIM_GAS,
  EPHEMERAL_CLAIMANT_WASM,
  enforceSandboxNetworkGuard,
  validateWasmArtifact,
  createImplicitAccount,
  createSandboxNamedTestnetAccount,
  parseTransactionResult,
  setupHardenedSandboxFixture,
  injectStuckInFlightClaimInSandbox,
  injectStuckInFlightInsuranceInSandbox,
} from './helpers.mjs';

test('ROASTAKE NEAR Sandbox Consolidated Hardening, Insurance Solvency & Release-Candidate Suite (Mission 03C.3.7.6)', async (t) => {

  // =========================================================================
  // Test A — Successful transfer + callback missing + no admin restoration
  // =========================================================================
  await t.test('Test A: Successful transfer with missing callback cannot be restored by admin ConfirmedFailure', async () => {
    console.log('\n===============================================================');
    console.log('--- [HARDENING TEST A: TRANSFER SUCCEEDED + CALLBACK MISSING] ---');
    console.log('===============================================================');
    const fixture = await setupHardenedSandboxFixture();
    const { worker, alice, mockOutlayer, adapterAccount } = fixture;

    try {
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'InitialPanic' });
      await alice.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      await alice.call(adapterAccount, 'admin_designate_refund_reserve', {
        amount: STANDARD_ATTACHED_DEPOSIT.toString(),
        evidence_sha256: TEST_EVIDENCE_SHA256,
      });
      await alice.call(adapterAccount, 'promote_unreconciled_refund', { nonce: 1 });

      // Execute genuine outbound Transfer payout to Alice (Alice receives 50 mNEAR, adapter loses 50 mNEAR)
      const aliceBefore = await alice.availableBalance();
      const claimTx = await alice.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedClaim = parseTransactionResult(claimTx);
      assert.ok(parsedClaim.succeeded, 'Original claim_refund transfer must execute and pay Alice');
      await new Promise((r) => setTimeout(r, 1000));
      const aliceAfter = await alice.availableBalance();
      assert.ok(aliceAfter > aliceBefore, 'Alice must have physically received the original Transfer');

      // Reconstruct the exact InFlight missing-callback contract state while real Transfer remains deducted
      await injectStuckInFlightClaimInSandbox(
        worker,
        adapterAccount,
        alice.accountId,
        STANDARD_ATTACHED_DEPOSIT,
        1,
        1
      );

      // Too early (< 100 blocks): ConfirmedFailure rejected
      const earlyFail = await alice.callRaw(
        adapterAccount,
        'admin_reconcile_stuck_claim',
        { claim_id: 1, outcome: 'ConfirmedFailure' },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(earlyFail).failed, 'ConfirmedFailure before 100 blocks must fail closed');

      // Fast-forward STUCK_CLAIM_MIN_AGE_BLOCKS (100 blocks)
      await worker.provider.fastForward(105);

      // Admin invokes ConfirmedFailure -> MUST transition to Disputed (NON-PAYABLE), NEVER Available!
      const disputeRes = await alice.callRaw(
        adapterAccount,
        'admin_reconcile_stuck_claim',
        { claim_id: 1, outcome: 'ConfirmedFailure' },
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedDispute = parseTransactionResult(disputeRes);
      assert.ok(parsedDispute.succeeded, 'admin_reconcile_stuck_claim(ConfirmedFailure) transitions InFlight -> Disputed');
      console.log(`[OK] Claim 1 moved to Disputed (Tx: ${parsedDispute.txId})`);

      const claimState = await adapterAccount.view('get_claim', { claim_id: 1 });
      assert.strictEqual(claimState.status, 'Disputed');

      const creditState = await adapterAccount.view('get_pending_refund', { account_id: alice.accountId });
      assert.strictEqual(creditState.status, 'Disputed');
      assert.strictEqual(creditState.available_amount, '0', 'Available amount must remain 0');
      assert.strictEqual(creditState.disputed_amount, STANDARD_ATTACHED_DEPOSIT.toString());

      // Attempting second payout MUST FAIL CLOSED
      const secondClaim = await alice.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(secondClaim).failed, 'Second claim_refund must fail closed while Disputed');

      // Solvency snapshot keeps disputed liability distinct from backed liability
      const snap = await adapterAccount.view('get_accounting_snapshot', {});
      assert.strictEqual(snap.disputed_liability, STANDARD_ATTACHED_DEPOSIT.toString());
      assert.strictEqual(snap.backed_refund_liability, '0');
      assert.strictEqual(snap.invariant_holds, true);
      console.log('[PASS] Test A: Transfer succeeded + missing callback stays non-payable.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test B — Failed transfer + callback missing + delayed system refund
  // =========================================================================
  await t.test('Test B: Failed transfer with missing callback and delayed system refund stays Disputed without early payout', async () => {
    console.log('\n===============================================================');
    console.log('--- [HARDENING TEST B: TRANSFER FAILED + DELAYED SYSTEM REFUND] ---');
    console.log('===============================================================');
    const fixture = await setupHardenedSandboxFixture();
    const { worker, root, alice, mockOutlayer, adapterAccount } = fixture;

    try {
      const charlieId = 'charlie-hardening.testnet';
      const { account: charlie } = await createSandboxNamedTestnetAccount(
        worker,
        root,
        charlieId,
        20_000_000_000_000_000_000_000_000n
      );
      await alice.call(adapterAccount, 'add_requester', { account_id: charlieId });

      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'InitialPanic' });
      await charlie.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      await alice.call(adapterAccount, 'admin_designate_refund_reserve', {
        amount: STANDARD_ATTACHED_DEPOSIT.toString(),
        evidence_sha256: TEST_EVIDENCE_SHA256,
      });
      await charlie.call(adapterAccount, 'promote_unreconciled_refund', { nonce: 1 });

      // Dispatch real Transfer to soon-deleted Charlie -> real on-chain AccountDoesNotExist Transfer receipt
      await charlie.deploy(EPHEMERAL_CLAIMANT_WASM);
      const failTx = await charlie.callRaw(
        charlie,
        'claim_and_delete',
        Buffer.from(adapterAccount.accountId, 'utf8'),
        { gas: 250_000_000_000_000n }
      );
      const parsedFail = parseTransactionResult(failTx);
      assert.ok(
        parsedFail.receipts.some((r) => r.isFailure),
        'Must observe genuine failed Transfer receipt on deleted charlie-hardening.testnet'
      );
      await new Promise((r) => setTimeout(r, 1000));

      // First reconcile normal callback ReturnPending cleanly so we record baseline returned balance,
      // then recreate Charlie and simulate callback-missing + delayed system refund!
      const { account: charlieRecreated } = await createSandboxNamedTestnetAccount(
        worker,
        root,
        charlieId,
        20_000_000_000_000_000_000_000_000n
      );
      await charlieRecreated.call(adapterAccount, 'reconcile_returned_refund', { claim_id: 1 });

      // Capture adapter balance right before placing Charlie's claim into InFlight.
      // `injectStuckInFlightClaimInSandbox` converts Available -> InFlight and deducts the 50 mNEAR
      // outbound transfer from `account.amount`, simulating the delayed/withheld system refund.
      const adapterViewBeforeDelay = await adapterAccount.accountView();
      await injectStuckInFlightClaimInSandbox(
        worker,
        adapterAccount,
        charlieId,
        STANDARD_ATTACHED_DEPOSIT,
        1,
        2
      );
      await worker.provider.fastForward(105);

      // Admin marks ConfirmedFailure while system refund is delayed -> Disputed (non-payable)
      const adminDispute = await alice.callRaw(
        adapterAccount,
        'admin_reconcile_stuck_claim',
        { claim_id: 2, outcome: 'ConfirmedFailure' },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(adminDispute).succeeded);
      assert.strictEqual((await adapterAccount.view('get_claim', { claim_id: 2 })).status, 'Disputed');

      // System refund finally arrives (+50 mNEAR restored to adapter account), yet without objective
      // on_refund_transfer proof or insurance enablement, the Disputed claim MUST STILL NOT become Available!
      const adapterViewDelayed = await adapterAccount.accountView();
      await adapterAccount.updateAccount({
        amount: adapterViewBeforeDelay.amount,
        locked: adapterViewDelayed.locked,
        code_hash: adapterViewDelayed.code_hash,
        storage_usage: adapterViewDelayed.storage_usage,
      });
      await worker.provider.fastForward(1);

      const retAttempt = await charlieRecreated.callRaw(
        adapterAccount,
        'reconcile_returned_refund',
        { claim_id: 2 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(retAttempt).failed, 'Disputed claim cannot enter ReturnPending restoration');

      const claimAttempt = await charlieRecreated.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(claimAttempt).failed, 'Disputed claim cannot be claimed without insurance/callback');
      console.log('[PASS] Test B: Delayed system refund and missing callback never silently auto-restore.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Tests C & D — Unverified ConfirmedFailure & Pooled Surplus Isolation
  // =========================================================================
  await t.test('Tests C & D: Unverified ConfirmedFailure and pooled surplus cannot turn ambiguous claim into Available refund', async () => {
    console.log('\n===============================================================');
    console.log('--- [HARDENING TESTS C & D: POOLED SURPLUS CANNOT RESTORE REFUND] ---');
    console.log('===============================================================');
    const fixture = await setupHardenedSandboxFixture();
    const { worker, root, alice, mockOutlayer, adapterAccount } = fixture;

    try {
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'InitialPanic' });
      await alice.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      await alice.call(adapterAccount, 'admin_designate_refund_reserve', {
        amount: STANDARD_ATTACHED_DEPOSIT.toString(),
        evidence_sha256: TEST_EVIDENCE_SHA256,
      });
      await alice.call(adapterAccount, 'promote_unreconciled_refund', { nonce: 1 });
      await alice.callRaw(adapterAccount, 'claim_refund', {}, { gas: STANDARD_CLAIM_GAS });

      await injectStuckInFlightClaimInSandbox(
        worker,
        adapterAccount,
        alice.accountId,
        STANDARD_ATTACHED_DEPOSIT,
        1,
        1
      );

      // Donate massive 5 NEAR pooled surplus directly to adapter
      await root.transfer(adapterAccount, 5_000_000_000_000_000_000_000_000n);
      await worker.provider.fastForward(110);

      // Test C: Admin ConfirmedFailure MUST move to Disputed, NOT Available despite 5 NEAR surplus!
      await alice.call(adapterAccount, 'admin_reconcile_stuck_claim', {
        claim_id: 1,
        outcome: 'ConfirmedFailure',
      });
      const snap = await adapterAccount.view('get_accounting_snapshot', {});
      assert.ok(BigInt(snap.unallocated_surplus) > 4_900_000_000_000_000_000_000_000n);
      assert.strictEqual(snap.disputed_liability, STANDARD_ATTACHED_DEPOSIT.toString());
      assert.strictEqual(snap.backed_refund_liability, '0');

      // Test D: Pooled surplus cannot pay claim_refund, reconcile_returned_refund, or compensate_disputed_claim
      for (const call of [
        { method: 'claim_refund', args: {} },
        { method: 'reconcile_returned_refund', args: { claim_id: 1 } },
        {
          method: 'compensate_disputed_claim',
          args: { claim_id: 1, evidence_sha256: TEST_EVIDENCE_SHA256 },
        },
      ]) {
        const res = await alice.callRaw(adapterAccount, call.method, call.args, {
          gas: STANDARD_CLAIM_GAS,
        });
        assert.ok(
          parseTransactionResult(res).failed,
          `${call.method} must fail closed despite pooled surplus`
        );
      }

      // Designating refund_reserve_balance cannot authorize insurance compensation either
      await alice.call(adapterAccount, 'admin_designate_refund_reserve', {
        amount: STANDARD_ATTACHED_DEPOSIT.toString(),
        evidence_sha256: TEST_EVIDENCE_SHA256,
      });
      await alice.call(adapterAccount, 'set_insurance_enabled', { enabled: true });
      await worker.provider.fastForward(1005);

      const insWithRefundReserveOnly = await alice.callRaw(
        adapterAccount,
        'compensate_disputed_claim',
        { claim_id: 1, evidence_sha256: TEST_EVIDENCE_SHA256 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(insWithRefundReserveOnly).failed,
        'Insurance compensation must fail when only refund_reserve_balance or pooled surplus exists'
      );
      console.log('[PASS] Tests C & D: Pooled surplus isolation verified.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Tests E & F — Explicit Insurance Reserve & Concurrent Claim Isolation
  // =========================================================================
  await t.test('Tests E & F: Explicit insurance compensation uses isolated insurance reserve and blocks concurrent reuse', async () => {
    console.log('\n===============================================================');
    console.log('--- [HARDENING TESTS E & F: EXPLICIT INSURANCE & CONCURRENCY] ---');
    console.log('===============================================================');
    const fixture = await setupHardenedSandboxFixture();
    const { worker, alice, bob, mockOutlayer, adapterAccount } = fixture;

    try {
      // Create & dispute claim 1 (Alice) and claim 2 (Bob)
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'InitialPanic' });
      for (const [idx, acct] of [
        [1, alice],
        [2, bob],
      ]) {
        await acct.callRaw(
          adapterAccount,
          'request_price_update',
          { market: MARKET_GESHA_DEMO },
          { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
        );
        await alice.call(adapterAccount, 'admin_designate_refund_reserve', {
          amount: STANDARD_ATTACHED_DEPOSIT.toString(),
          evidence_sha256: TEST_EVIDENCE_SHA256,
        });
        await acct.call(adapterAccount, 'promote_unreconciled_refund', { nonce: idx });
        await acct.callRaw(adapterAccount, 'claim_refund', {}, { gas: STANDARD_CLAIM_GAS });
      }

      await injectStuckInFlightClaimInSandbox(worker, adapterAccount, alice.accountId, STANDARD_ATTACHED_DEPOSIT, 1, 1);
      await injectStuckInFlightClaimInSandbox(worker, adapterAccount, bob.accountId, STANDARD_ATTACHED_DEPOSIT, 2, 2);

      // Disabled by default
      assert.strictEqual(await adapterAccount.view('is_insurance_enabled', {}), false);
      const disabledCall = await alice.callRaw(
        adapterAccount,
        'compensate_disputed_claim',
        { claim_id: 1, evidence_sha256: TEST_EVIDENCE_SHA256 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(disabledCall).failed, 'Disabled insurance must fail closed');

      await worker.provider.fastForward(105);
      await alice.call(adapterAccount, 'mark_claim_disputed', { claim_id: 1 });
      await alice.call(adapterAccount, 'mark_claim_disputed', { claim_id: 2 });
      await alice.call(adapterAccount, 'set_insurance_enabled', { enabled: true });

      // Fund insurance reserve for ONLY ONE claim (50 mNEAR)
      await alice.call(
        adapterAccount,
        'fund_insurance_reserve',
        {},
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CLAIM_GAS }
      );
      assert.strictEqual(
        await adapterAccount.view('get_insurance_reserve_balance', {}),
        STANDARD_ATTACHED_DEPOSIT.toString()
      );

      // Before 1000-block dispute delay, compensate_disputed_claim MUST reject
      const earlyComp = await alice.callRaw(
        adapterAccount,
        'compensate_disputed_claim',
        { claim_id: 1, evidence_sha256: TEST_EVIDENCE_SHA256 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(earlyComp).failed, 'Compensation before appeal window must fail');

      await worker.provider.fastForward(1005);

      // Test E: Compensate claim 1 -> succeeds, pays Alice 50 mNEAR, allocates compensation_id = 1, reserve -> 0
      const compRes = await alice.callRaw(
        adapterAccount,
        'compensate_disputed_claim',
        { claim_id: 1, evidence_sha256: TEST_EVIDENCE_SHA256 },
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedComp = parseTransactionResult(compRes);
      assert.ok(parsedComp.succeeded, 'First insurance compensation must succeed');
      assert.ok(
        parsedComp.logs.some((l) => l.includes('insurance_compensation_paid') && l.includes('"compensation_id":1')),
        'Must emit insurance_compensation_paid with compensation_id 1'
      );
      console.log(`[OK] Compensated Disputed claim 1 (Tx: ${parsedComp.txId})`);

      // Test F: Second concurrent stuck claim (Bob claim 2) CANNOT reuse the depleted 50 mNEAR reserve
      const comp2Res = await alice.callRaw(
        adapterAccount,
        'compensate_disputed_claim',
        { claim_id: 2, evidence_sha256: TEST_EVIDENCE_SHA256 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(comp2Res).failed,
        'Second concurrent stuck claim cannot reuse depleted insurance reserve'
      );

      const finalSnap = await adapterAccount.view('get_accounting_snapshot', {});
      assert.strictEqual(finalSnap.insurance_reserve_balance, '0');
      assert.strictEqual(finalSnap.disputed_liability, STANDARD_ATTACHED_DEPOSIT.toString());
      assert.strictEqual(finalSnap.next_compensation_id, 2);
      assert.strictEqual(finalSnap.invariant_holds, true);
      console.log('[PASS] Tests E & F: Separate insurance reserve and concurrent isolation verified.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test G — Open-Request Cap & Measured Storage Growth Budget
  // =========================================================================
  await t.test('Test G: Measured UnreconciledFailure storage growth and cap/headroom rejection', async () => {
    console.log('\n===============================================================');
    console.log('--- [HARDENING TEST G: STORAGE GROWTH & BUDGET ENFORCEMENT] ---');
    console.log('===============================================================');
    const fixture = await setupHardenedSandboxFixture({
      maxOpenRequests: 3,
      operatingBuffer: 25_000_000_000_000_000_000_000n, // 25 mNEAR operating buffer
    });
    const { worker, alice, mockOutlayer, adapterAccount } = fixture;

    try {
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'DownstreamPromiseFailure' });

      const storageBase = (await adapterAccount.accountView()).storage_usage;
      console.log(`Initial Hardened Adapter Storage Usage: ${storageBase} bytes`);

      for (let i = 1; i <= 3; i++) {
        const res = await alice.callRaw(
          adapterAccount,
          'request_price_update',
          { market: MARKET_GESHA_DEMO },
          { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
        );
        assert.ok(parseTransactionResult(res).receipts.length > 0);
        const curStorage = (await adapterAccount.accountView()).storage_usage;
        console.log(`After UnreconciledFailure #${i}: ${curStorage} bytes (cumulative delta: +${curStorage - storageBase} bytes)`);
      }

      const storageAfter3 = (await adapterAccount.accountView()).storage_usage;
      const totalDelta3 = storageAfter3 - storageBase;
      const avgPerEntry = totalDelta3 / 3;
      const avgStakeYocto = BigInt(Math.round(avgPerEntry)) * 10_000_000_000_000_000_000n;
      console.log(`Measured 3-entry delta: ${totalDelta3} bytes (~${avgPerEntry.toFixed(2)} bytes / entry, ~${avgStakeYocto} yocto / entry)`);

      const countsAtCap = await adapterAccount.view('get_open_request_counts', {});
      assert.deepStrictEqual(countsAtCap, [0, 3, 3]);

      // 4th request MUST be rejected due to max_open_requests = 3
      const res4 = await alice.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      assert.ok(
        parseTransactionResult(res4).failed,
        '4th open request must be rejected when max_open_requests = 3 is exhausted'
      );

      // Proposing external resolution preserves the record & slot until requester acknowledges (F-01)
      await alice.call(adapterAccount, 'admin_resolve_unreconciled_external', {
        nonce: 1,
        evidence_sha256: TEST_EVIDENCE_SHA256,
      });
      const proposedFailure = await adapterAccount.view('get_unreconciled_failure', { nonce: 1 });
      assert.ok(proposedFailure && proposedFailure.external_resolution_proposed, 'UnreconciledFailure preserved until requester acknowledges');
      assert.deepStrictEqual(await adapterAccount.view('get_open_request_counts', {}), [0, 3, 3]);

      await alice.call(adapterAccount, 'acknowledge_unreconciled_resolved_external', { nonce: 1 });
      const storageAfterResolve = (await adapterAccount.accountView()).storage_usage;
      console.log(`After requester acknowledging nonce 1: ${storageAfterResolve} bytes (reclaimed ${storageAfter3 - storageAfterResolve} bytes)`);
      assert.ok(storageAfterResolve < storageAfter3, 'Storage must shrink when UnreconciledFailure is acknowledged and removed');

      // Exhaust safe storage headroom by tightening minimum_operating_reserve (< 5.12 mNEAR headroom remaining)
      const snapNow = await adapterAccount.view('get_accounting_snapshot', {});
      const nearLimitMin =
        BigInt(snapNow.minimum_operating_reserve) + BigInt(snapNow.unallocated_surplus) - 2_000_000_000_000_000_000_000n; // leave 2 mNEAR < 5.12 mNEAR margin
      await alice.call(adapterAccount, 'increase_minimum_operating_reserve', {
        new_minimum: nearLimitMin.toString(),
      });

      const resLowStorage = await alice.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      assert.ok(
        parseTransactionResult(resLowStorage).failed,
        'New request must be rejected with ERR_INSUFFICIENT_STORAGE_RESERVE when unallocated headroom < 512 bytes'
      );
      console.log('[PASS] Test G: Storage delta measured and limits strictly enforced.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test H — Non-Destructive Admin Settlement Proposal & Late Call Rejection (F-01)
  // =========================================================================
  await t.test('Test H: Admin ConfirmedSuccess proposes settlement without deleting claim; claimant ack closes and late calls fail', async () => {
    console.log('\n===============================================================');
    console.log('--- [HARDENING TEST H: NON-DESTRUCTIVE ADMIN PROPOSAL & LATE CALLS] ---');
    console.log('===============================================================');
    const fixture = await setupHardenedSandboxFixture();
    const { worker, alice, bob, mockOutlayer, adapterAccount } = fixture;

    try {
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'InitialPanic' });
      await bob.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      await alice.call(adapterAccount, 'admin_designate_refund_reserve', {
        amount: STANDARD_ATTACHED_DEPOSIT.toString(),
        evidence_sha256: TEST_EVIDENCE_SHA256,
      });
      await bob.call(adapterAccount, 'promote_unreconciled_refund', { nonce: 1 });
      await bob.callRaw(adapterAccount, 'claim_refund', {}, { gas: STANDARD_CLAIM_GAS });

      await injectStuckInFlightClaimInSandbox(worker, adapterAccount, bob.accountId, STANDARD_ATTACHED_DEPOSIT, 1, 1);

      await worker.provider.fastForward(105);
      await alice.call(adapterAccount, 'admin_reconcile_stuck_claim', {
        claim_id: 1,
        outcome: 'ConfirmedFailure',
      });

      // Duplicate ConfirmedFailure on already Disputed claim -> rejected
      const dupDispute = await alice.callRaw(
        adapterAccount,
        'admin_reconcile_stuck_claim',
        { claim_id: 1, outcome: 'ConfirmedFailure' },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(dupDispute).failed, 'Duplicate ConfirmedFailure on Disputed claim must fail');

      // ConfirmedSuccess before 1000-block delay -> rejected
      const earlySuccess = await alice.callRaw(
        adapterAccount,
        'admin_reconcile_stuck_claim',
        { claim_id: 1, outcome: 'ConfirmedSuccess' },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(earlySuccess).failed, 'ConfirmedSuccess before appeal window must fail');

      await worker.provider.fastForward(1005);

      // Admin ConfirmedSuccess proposes settlement WITHOUT deleting Bob's claim or zeroing liability (F-01)
      await alice.call(adapterAccount, 'admin_reconcile_stuck_claim', {
        claim_id: 1,
        outcome: 'ConfirmedSuccess',
      });
      const claimAfterProposal = await adapterAccount.view('get_claim', { claim_id: 1 });
      assert.ok(claimAfterProposal, 'Admin ConfirmedSuccess must NOT unilaterally delete the Disputed claim');
      assert.strictEqual(claimAfterProposal.status, 'Disputed');
      assert.strictEqual(claimAfterProposal.admin_settlement_proposed, true);
      assert.strictEqual(
        await adapterAccount.view('get_total_refund_liability', {}),
        STANDARD_ATTACHED_DEPOSIT.toString(),
        'Liability must remain preserved until claimant acknowledges'
      );

      // Admin (Alice) cannot acknowledge on Bob's behalf
      const unauthAck = await alice.callRaw(
        adapterAccount,
        'acknowledge_disputed_transfer_received',
        { claim_id: 1 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(unauthAck).failed, 'Non-claimant cannot acknowledge disputed transfer receipt');

      // Claimant (Bob) acknowledges -> claim_id 1 deleted and liability zeroed
      await bob.call(adapterAccount, 'acknowledge_disputed_transfer_received', { claim_id: 1 });
      assert.strictEqual(await adapterAccount.view('get_claim', { claim_id: 1 }), null);
      assert.strictEqual(await adapterAccount.view('get_total_refund_liability', {}), '0');

      // Every subsequent reconciliation / compensation / acknowledgement on deleted claim 1 MUST fail closed
      for (const lateCall of [
        { method: 'admin_reconcile_stuck_claim', args: { claim_id: 1, outcome: 'ConfirmedSuccess' } },
        { method: 'admin_reconcile_stuck_claim', args: { claim_id: 1, outcome: 'ConfirmedFailure' } },
        { method: 'mark_claim_disputed', args: { claim_id: 1 } },
        { method: 'acknowledge_disputed_transfer_received', args: { claim_id: 1 } },
        { method: 'reconcile_returned_refund', args: { claim_id: 1 } },
      ]) {
        const res = await alice.callRaw(adapterAccount, lateCall.method, lateCall.args, {
          gas: STANDARD_CLAIM_GAS,
        });
        assert.ok(
          parseTransactionResult(res).failed,
          `Late ${lateCall.method} on removed claim must fail closed`
        );
      }
      console.log('[PASS] Test H: Non-destructive admin proposal and late reconciliation safety verified.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test I — Fresh Deployment Policy & Legacy State Deserialization Rejection
  // =========================================================================
  await t.test('Test I: Fresh deployment initializes cleanly while unsafe in-place upgrade over legacy state fails closed', async () => {
    console.log('\n===============================================================');
    console.log('--- [HARDENING TEST I: FRESH DEPLOYMENT & BORSH STATE POLICY] ---');
    console.log('===============================================================');
    enforceSandboxNetworkGuard();
    const worker = await Worker.init({ network: 'sandbox' });
    const root = worker.rootAccount;

    try {
      const alice = await createImplicitAccount(worker, root, 100_000_000_000_000_000_000_000_000n);
      const mockOutlayer = await createImplicitAccount(worker, root, 50_000_000_000_000_000_000_000_000n);
      const freshAccount = await createImplicitAccount(worker, root, 50_000_000_000_000_000_000_000_000n);
      const legacyM0336Account = await createImplicitAccount(worker, root, 50_000_000_000_000_000_000_000_000n);
      const legacyM03372Account = await createImplicitAccount(worker, root, 50_000_000_000_000_000_000_000_000n);

      const mockWasm = validateWasmArtifact(
        MOCK_OUTLAYER_ABI_FIXED_WASM_PATH,
        EXPECTED_MOCK_ABI_FIXED_SHA256,
        RAW_INCOMPATIBLE_MOCK_SHA256,
        'Mock OutLayer'
      );
      const legacyM0336Wasm = validateWasmArtifact(
        ADAPTER_SECURITY_FIXED_WASM_PATH,
        EXPECTED_ADAPTER_SECURITY_FIXED_SHA256,
        RAW_INCOMPATIBLE_ADAPTER_SHA256,
        'ROASTAKE Security-Fixed Baseline (Legacy M03C.3.6)'
      );
      const legacyM03372Wasm = validateWasmArtifact(
        ADAPTER_ADMIN_HARDENED_WASM_PATH,
        EXPECTED_ADAPTER_ADMIN_HARDENED_SHA256,
        RAW_INCOMPATIBLE_ADAPTER_SHA256,
        'ROASTAKE Admin-Hardened Baseline (Legacy M03C.3.7.2)'
      );
      const releaseCandidateWasm = validateWasmArtifact(
        ADAPTER_RELEASE_CANDIDATE_WASM_PATH,
        EXPECTED_ADAPTER_RELEASE_CANDIDATE_SHA256,
        RAW_INCOMPATIBLE_ADAPTER_SHA256,
        'ROASTAKE Release-Candidate Adapter (M03C.3.7.4)'
      );

      await mockOutlayer.deploy(mockWasm);
      await mockOutlayer.call(mockOutlayer, 'new', {});

      // 1. Fresh deployment of Release Candidate to a new account succeeds & re-init rejects
      await freshAccount.deploy(releaseCandidateWasm);
      await freshAccount.call(freshAccount, 'init', {
        admin: alice.accountId,
        outlayer_contract: mockOutlayer.accountId,
        minimum_operating_reserve: DEFAULT_MIN_OPERATING_RESERVE,
        max_open_requests: 50,
      });
      const freshSnap = await freshAccount.view('get_accounting_snapshot', {});
      assert.strictEqual(freshSnap.max_open_requests, 50);
      assert.strictEqual(freshSnap.insurance_enabled, false);
      assert.strictEqual(freshSnap.insurance_in_flight_liability, '0');
      assert.strictEqual(freshSnap.insurance_unresolved_liability, '0');
      assert.strictEqual(freshSnap.invariant_holds, true);

      const reinitRes = await alice.callRaw(
        freshAccount,
        'init',
        {
          admin: alice.accountId,
          outlayer_contract: mockOutlayer.accountId,
          minimum_operating_reserve: DEFAULT_MIN_OPERATING_RESERVE,
        },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(reinitRes).failed, 'Re-initializing deployed state must fail closed');

      // 2. Deploy M03C.3.6 and M03C.3.7.2 legacy contracts and verify unmigrated in-place overwrite fails closed
      for (const [label, acct, oldWasm] of [
        ['M03C.3.6', legacyM0336Account, legacyM0336Wasm],
        ['M03C.3.7.2', legacyM03372Account, legacyM03372Wasm],
      ]) {
        await acct.deploy(oldWasm);
        await acct.call(acct, 'init', {
          admin: alice.accountId,
          outlayer_contract: mockOutlayer.accountId,
          minimum_operating_reserve: DEFAULT_MIN_OPERATING_RESERVE,
        });
        assert.strictEqual(await acct.view('get_admin', {}), alice.accountId);

        await acct.deploy(releaseCandidateWasm);
        let upgradeThrew = false;
        try {
          await acct.view('get_admin', {});
        } catch (err) {
          upgradeThrew = true;
          console.log(`[OK] Unmigrated in-place upgrade over ${label} failed closed: ${err.message.slice(0, 120)}...`);
        }
        assert.ok(upgradeThrew, `Legacy ${label} state must reject unmigrated Borsh deserialization on M03C.3.7.4 WASM`);
      }
      console.log('[PASS] Test I: Fresh deployment vs legacy Borsh incompatibility verified.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test J — Disputed Claim Liveness (`F-03`) & Non-Duplicating State Injection (`F-06`)
  // =========================================================================
  await t.test('Test J: Disputed historical claim does not block claiming a new independent Available refund (F-03 & F-06)', async () => {
    console.log('\n===============================================================');
    console.log('--- [HARDENING TEST J: DISPUTED CLAIM LIVENESS & F-06 INJECTION] ---');
    console.log('===============================================================');
    const fixture = await setupHardenedSandboxFixture();
    const { worker, alice, mockOutlayer, adapterAccount } = fixture;

    try {
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'InitialPanic' });

      // 1. Alice gets a 50 mNEAR stuck InFlight claim_id 1 and marks it Disputed
      await alice.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      await alice.call(adapterAccount, 'admin_designate_refund_reserve', {
        amount: STANDARD_ATTACHED_DEPOSIT.toString(),
        evidence_sha256: TEST_EVIDENCE_SHA256,
      });
      await alice.call(adapterAccount, 'promote_unreconciled_refund', { nonce: 1 });
      await alice.callRaw(adapterAccount, 'claim_refund', {}, { gas: STANDARD_CLAIM_GAS });

      await injectStuckInFlightClaimInSandbox(
        worker,
        adapterAccount,
        alice.accountId,
        STANDARD_ATTACHED_DEPOSIT,
        1,
        1
      );
      await worker.provider.fastForward(105);
      await alice.call(adapterAccount, 'mark_claim_disputed', { claim_id: 1 });

      const creditDisputed = await adapterAccount.view('get_pending_refund', { account_id: alice.accountId });
      assert.strictEqual(creditDisputed.disputed_amount, STANDARD_ATTACHED_DEPOSIT.toString());
      assert.strictEqual(creditDisputed.available_amount, '0');
      assert.strictEqual(creditDisputed.active_claim_id, null, 'Disputed claim must clear active_claim_id');

      // 2. Alice incurs a second failed request (nonce 2), funds refund reserve, and promotes 50 mNEAR into Available
      await alice.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      await alice.call(
        adapterAccount,
        'fund_refund_reserve',
        {},
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      await alice.call(adapterAccount, 'promote_unreconciled_refund', { nonce: 2 });

      const creditBoth = await adapterAccount.view('get_pending_refund', { account_id: alice.accountId });
      assert.strictEqual(creditBoth.available_amount, STANDARD_ATTACHED_DEPOSIT.toString());
      assert.strictEqual(creditBoth.disputed_amount, STANDARD_ATTACHED_DEPOSIT.toString());

      // 3. Alice calls claim_refund() -> MUST succeed and pay ONLY the new 50 mNEAR available_amount!
      const aliceBalBefore = await alice.availableBalance();
      const newClaimRes = await alice.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedNewClaim = parseTransactionResult(newClaimRes);
      assert.ok(parsedNewClaim.succeeded, 'New Available refund must be claimable while historical claim is Disputed');
      await new Promise((r) => setTimeout(r, 1000));
      const aliceBalAfter = await alice.availableBalance();
      assert.ok(aliceBalAfter > aliceBalBefore, 'Alice must receive the new 50 mNEAR refund payout');

      // Verify claim_id 2 settled and was deleted, while claim_id 1 remains Disputed and non-payable!
      assert.strictEqual(await adapterAccount.view('get_claim', { claim_id: 2 }), null);
      const claim1StillDisputed = await adapterAccount.view('get_claim', { claim_id: 1 });
      assert.ok(claim1StillDisputed && claim1StillDisputed.status === 'Disputed');

      const creditAfterSecondClaim = await adapterAccount.view('get_pending_refund', {
        account_id: alice.accountId,
      });
      assert.strictEqual(creditAfterSecondClaim.available_amount, '0');
      assert.strictEqual(creditAfterSecondClaim.in_flight_amount, '0');
      assert.strictEqual(creditAfterSecondClaim.disputed_amount, STANDARD_ATTACHED_DEPOSIT.toString());
      assert.strictEqual(creditAfterSecondClaim.status, 'Disputed');

      const snapAfter = await adapterAccount.view('get_accounting_snapshot', {});
      assert.strictEqual(snapAfter.total_refund_liability, STANDARD_ATTACHED_DEPOSIT.toString());
      assert.strictEqual(snapAfter.disputed_liability, STANDARD_ATTACHED_DEPOSIT.toString());
      assert.strictEqual(snapAfter.backed_refund_liability, '0');
      assert.strictEqual(snapAfter.invariant_holds, true);
      console.log('[PASS] Test J: Disputed claim liveness (F-03) and non-duplicating injection (F-06) verified.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test K — Insurance In-Flight Solvency, Failed Return & Missing Callback Recovery (`F-02` & `L-01`)
  // =========================================================================
  await t.test('Test K: Insurance in-flight liability, failed transfer return, and missing-callback Disputed recovery (F-02 & L-01)', async () => {
    console.log('\n===============================================================');
    console.log('--- [HARDENING TEST K: INSURANCE SOLVENCY & CALLBACK RECOVERY] ---');
    console.log('===============================================================');
    const fixture = await setupHardenedSandboxFixture();
    const { worker, root, alice, mockOutlayer, adapterAccount } = fixture;

    try {
      const charlieId = 'charlie-insurance.testnet';
      const { account: charlie } = await createSandboxNamedTestnetAccount(
        worker,
        root,
        charlieId,
        20_000_000_000_000_000_000_000_000n
      );
      await alice.call(adapterAccount, 'add_requester', { account_id: charlieId });

      // Part 1: Create Disputed claim #1 for Charlie via real claim_refund(), then test failed insurance payout -> ReturnPending -> reconcile
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'InitialPanic' });
      await charlie.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      await alice.call(adapterAccount, 'admin_designate_refund_reserve', {
        amount: STANDARD_ATTACHED_DEPOSIT.toString(),
        evidence_sha256: TEST_EVIDENCE_SHA256,
      });
      await charlie.call(adapterAccount, 'promote_unreconciled_refund', { nonce: 1 });

      // Execute genuine outbound claim_refund Transfer to Charlie so adapter loses 50 mNEAR (zero phantom surplus)
      const realCharlieClaim = await charlie.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(realCharlieClaim).succeeded, 'Charlie real claim_refund must execute');

      await injectStuckInFlightClaimInSandbox(
        worker,
        adapterAccount,
        charlieId,
        STANDARD_ATTACHED_DEPOSIT,
        1,
        1
      );

      const snapClaim1InFlight = await adapterAccount.view('get_accounting_snapshot', {});
      assert.ok(
        BigInt(snapClaim1InFlight.unallocated_surplus) < 25_000_000_000_000_000_000_000n,
        `Must have zero phantom 50 mNEAR surplus after claim 1 dispatch (surplus: ${snapClaim1InFlight.unallocated_surplus})`
      );

      await worker.provider.fastForward(105);
      await alice.call(adapterAccount, 'mark_claim_disputed', { claim_id: 1 });
      await alice.call(adapterAccount, 'set_insurance_enabled', { enabled: true });
      await alice.call(
        adapterAccount,
        'fund_insurance_reserve',
        {},
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CLAIM_GAS }
      );
      await worker.provider.fastForward(1005);

      // Delete charlie-insurance.testnet right before admin calls compensate_disputed_claim so the
      // outbound insurance Transfer fails on-chain with AccountDoesNotExist -> ReturnPending!
      await charlie.delete(root.accountId);
      const compFailRes = await alice.callRaw(
        adapterAccount,
        'compensate_disputed_claim',
        { claim_id: 1, evidence_sha256: TEST_EVIDENCE_SHA256 },
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedCompFail = parseTransactionResult(compFailRes);
      assert.ok(
        parsedCompFail.logs.some((l) => l.includes('insurance_compensation_return_pending')),
        'Failed insurance transfer must transition to ReturnPending via on_insurance_transfer'
      );

      const compPending = await adapterAccount.view('get_insurance_compensation', {
        compensation_id: 1,
      });
      assert.strictEqual(compPending.status, 'ReturnPending');
      const snapReturnPending = await adapterAccount.view('get_accounting_snapshot', {});
      const acctReturnPending = await adapterAccount.accountView();
      assert.strictEqual(snapReturnPending.insurance_reserve_balance, '0');
      assert.strictEqual(snapReturnPending.insurance_in_flight_liability, '0');
      assert.strictEqual(snapReturnPending.insurance_unresolved_liability, '0');
      assert.strictEqual(
        snapReturnPending.insurance_return_pending_liability,
        STANDARD_ATTACHED_DEPOSIT.toString()
      );
      assert.strictEqual(snapReturnPending.account_balance, acctReturnPending.amount);
      assert.strictEqual(snapReturnPending.storage_usage_bytes, acctReturnPending.storage_usage);
      assert.strictEqual(snapReturnPending.invariant_holds, true);

      // Reconcile returned insurance -> restores insurance_reserve_balance and removes terminal record #1
      await alice.call(adapterAccount, 'reconcile_returned_insurance', { compensation_id: 1 });
      assert.strictEqual(await adapterAccount.view('get_insurance_compensation', { compensation_id: 1 }), null);
      const snapReconciled = await adapterAccount.view('get_accounting_snapshot', {});
      const acctReconciled = await adapterAccount.accountView();
      assert.strictEqual(snapReconciled.insurance_reserve_balance, STANDARD_ATTACHED_DEPOSIT.toString());
      assert.strictEqual(snapReconciled.insurance_in_flight_liability, '0');
      assert.strictEqual(snapReconciled.insurance_unresolved_liability, '0');
      assert.strictEqual(snapReconciled.insurance_return_pending_liability, '0');
      assert.strictEqual(snapReconciled.account_balance, acctReconciled.amount);
      assert.strictEqual(snapReconciled.storage_usage_bytes, acctReconciled.storage_usage);
      assert.strictEqual(snapReconciled.invariant_holds, true);

      // Part 2: Create genuine Disputed claim #2 for Alice, then execute real compensate_disputed_claim(2)
      // so insurance_reserve_balance drops to 0, claim #2 is removed, and Alice receives 50 mNEAR.
      await alice.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      await alice.call(
        adapterAccount,
        'fund_refund_reserve',
        {},
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CLAIM_GAS }
      );
      await alice.call(adapterAccount, 'promote_unreconciled_refund', { nonce: 2 });
      const realAliceClaim2 = await alice.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(realAliceClaim2).succeeded, 'Alice real claim_refund #2 must execute');

      await injectStuckInFlightClaimInSandbox(
        worker,
        adapterAccount,
        alice.accountId,
        STANDARD_ATTACHED_DEPOSIT,
        2,
        2
      );
      await worker.provider.fastForward(105);
      await alice.call(adapterAccount, 'mark_claim_disputed', { claim_id: 2 });
      await worker.provider.fastForward(1005);

      const acctBeforeComp2 = await adapterAccount.accountView();
      const comp2Res = await alice.callRaw(
        adapterAccount,
        'compensate_disputed_claim',
        { claim_id: 2, evidence_sha256: TEST_EVIDENCE_SHA256 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(comp2Res).succeeded, 'Real compensate_disputed_claim #2 must succeed');
      const snapAfterComp2 = await adapterAccount.view('get_accounting_snapshot', {});
      const acctAfterComp2 = await adapterAccount.accountView();
      assert.strictEqual(snapAfterComp2.insurance_reserve_balance, '0');
      assert.strictEqual(snapAfterComp2.insurance_in_flight_liability, '0');
      assert.strictEqual(await adapterAccount.view('get_claim', { claim_id: 2 }), null);
      assert.ok(
        BigInt(acctAfterComp2.amount) <= BigInt(acctBeforeComp2.amount) - STANDARD_ATTACHED_DEPOSIT,
        'Adapter balance must reflect outbound 50 mNEAR insurance payout'
      );

      // Fail-closed helper verification (L-01): impossible injection against zero reserve MUST throw!
      await assert.rejects(
        () =>
          injectStuckInFlightInsuranceInSandbox(
            worker,
            adapterAccount,
            2,
            2,
            alice.accountId,
            STANDARD_ATTACHED_DEPOSIT,
            TEST_EVIDENCE_SHA256,
            { originatingNonce: 2, allowRemovedClaimReference: true }
          ),
        /ERR_IMPOSSIBLE_INSURANCE_ACCOUNTING/,
        'Injecting InFlight insurance when insurance_reserve_balance == 0 must fail closed'
      );

      // Reconstruct ONLY the missing on_insurance_transfer callback for comp #2 without double-deducting reserve/balance
      await injectStuckInFlightInsuranceInSandbox(
        worker,
        adapterAccount,
        2,
        2,
        alice.accountId,
        STANDARD_ATTACHED_DEPOSIT,
        TEST_EVIDENCE_SHA256,
        { originatingNonce: 2, outboundTransferAlreadyExecuted: true }
      );

      const snapComp2InFlight = await adapterAccount.view('get_accounting_snapshot', {});
      const acctComp2InFlight = await adapterAccount.accountView();
      assert.strictEqual(snapComp2InFlight.insurance_reserve_balance, '0');
      assert.strictEqual(
        snapComp2InFlight.insurance_in_flight_liability,
        STANDARD_ATTACHED_DEPOSIT.toString()
      );
      assert.strictEqual(snapComp2InFlight.insurance_unresolved_liability, '0');
      assert.strictEqual(snapComp2InFlight.insurance_return_pending_liability, '0');
      assert.strictEqual(snapComp2InFlight.account_balance, acctComp2InFlight.amount);
      assert.strictEqual(snapComp2InFlight.storage_usage_bytes, acctComp2InFlight.storage_usage);
      assert.strictEqual(snapComp2InFlight.invariant_holds, true);

      // Attempting admin_designate_refund_reserve against encumbered in-flight insurance MUST fail closed
      const usurpRes = await alice.callRaw(
        adapterAccount,
        'admin_designate_refund_reserve',
        { amount: STANDARD_ATTACHED_DEPOSIT.toString(), evidence_sha256: TEST_EVIDENCE_SHA256 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(usurpRes).failed,
        'admin_designate_refund_reserve must fail while insurance compensation is InFlight'
      );

      // After STUCK_CLAIM_MIN_AGE_BLOCKS, mark insurance compensation #2 Disputed, then recipient acknowledges
      await worker.provider.fastForward(105);
      await alice.call(adapterAccount, 'mark_insurance_compensation_disputed', {
        compensation_id: 2,
      });
      const snapInsDisputed = await adapterAccount.view('get_accounting_snapshot', {});
      const acctInsDisputed = await adapterAccount.accountView();
      assert.strictEqual(snapInsDisputed.insurance_reserve_balance, '0');
      assert.strictEqual(snapInsDisputed.insurance_in_flight_liability, '0');
      assert.strictEqual(
        snapInsDisputed.insurance_unresolved_liability,
        STANDARD_ATTACHED_DEPOSIT.toString()
      );
      assert.strictEqual(snapInsDisputed.insurance_return_pending_liability, '0');
      assert.strictEqual(snapInsDisputed.account_balance, acctInsDisputed.amount);
      assert.strictEqual(snapInsDisputed.storage_usage_bytes, acctInsDisputed.storage_usage);
      assert.strictEqual(snapInsDisputed.invariant_holds, true);

      await alice.call(adapterAccount, 'acknowledge_insurance_compensation_received', {
        compensation_id: 2,
      });
      assert.strictEqual(await adapterAccount.view('get_insurance_compensation', { compensation_id: 2 }), null);

      // Part 3: Verify default-mode insurance injection (reserve deduction + outbound account.amount deduction)
      // explicitly against funded reserve, followed by clean recipient acknowledgement.
      await alice.call(
        adapterAccount,
        'fund_insurance_reserve',
        {},
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CLAIM_GAS }
      );
      const acctBeforeComp3 = await adapterAccount.accountView();
      await injectStuckInFlightInsuranceInSandbox(
        worker,
        adapterAccount,
        3,
        2,
        alice.accountId,
        STANDARD_ATTACHED_DEPOSIT,
        TEST_EVIDENCE_SHA256,
        { originatingNonce: 2, allowRemovedClaimReference: true }
      );
      const snapComp3 = await adapterAccount.view('get_accounting_snapshot', {});
      const acctAfterComp3 = await adapterAccount.accountView();
      assert.strictEqual(snapComp3.insurance_reserve_balance, '0');
      assert.strictEqual(
        snapComp3.insurance_in_flight_liability,
        STANDARD_ATTACHED_DEPOSIT.toString()
      );
      assert.strictEqual(snapComp3.insurance_unresolved_liability, '0');
      assert.strictEqual(snapComp3.insurance_return_pending_liability, '0');
      assert.strictEqual(
        BigInt(acctAfterComp3.amount),
        BigInt(acctBeforeComp3.amount) - STANDARD_ATTACHED_DEPOSIT,
        'Default-mode injection must deduct exact outbound transfer amount from account.amount'
      );
      assert.strictEqual(snapComp3.account_balance, acctAfterComp3.amount);
      assert.strictEqual(snapComp3.storage_usage_bytes, acctAfterComp3.storage_usage);
      assert.strictEqual(snapComp3.invariant_holds, true);

      await alice.call(adapterAccount, 'acknowledge_insurance_compensation_received', {
        compensation_id: 3,
      });

      const snapInsClosed = await adapterAccount.view('get_accounting_snapshot', {});
      const acctInsClosed = await adapterAccount.accountView();
      assert.strictEqual(snapInsClosed.insurance_reserve_balance, '0');
      assert.strictEqual(snapInsClosed.insurance_in_flight_liability, '0');
      assert.strictEqual(snapInsClosed.insurance_unresolved_liability, '0');
      assert.strictEqual(snapInsClosed.insurance_return_pending_liability, '0');
      assert.strictEqual(snapInsClosed.active_insurance_compensation_count, 0);
      assert.strictEqual(snapInsClosed.account_balance, acctInsClosed.amount);
      assert.strictEqual(snapInsClosed.storage_usage_bytes, acctInsClosed.storage_usage);
      assert.strictEqual(snapInsClosed.invariant_holds, true);
      console.log('[PASS] Test K: Insurance in-flight liability, failed return, zero-phantom-surplus accounting, and missing-callback recovery verified.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test L — Operating Reserve Protection (`F-07`) & Two-Step Admin Transfer (`F-01`)
  // =========================================================================
  await t.test('Test L: fund_operating_reserve increases protected floor (F-07) and two-step admin transfer works safely (F-01)', async () => {
    console.log('\n===============================================================');
    console.log('--- [HARDENING TEST L: OPERATING RESERVE & TWO-STEP ADMIN] ---');
    console.log('===============================================================');
    const fixture = await setupHardenedSandboxFixture();
    const { worker, alice, bob, adapterAccount } = fixture;

    try {
      const minBefore = BigInt(await adapterAccount.view('get_minimum_operating_reserve', {}));
      await alice.call(
        adapterAccount,
        'fund_operating_reserve',
        {},
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CLAIM_GAS }
      );
      const minAfter = BigInt(await adapterAccount.view('get_minimum_operating_reserve', {}));
      assert.strictEqual(
        minAfter,
        minBefore + STANDARD_ATTACHED_DEPOSIT,
        'fund_operating_reserve must permanently increase minimum_operating_reserve'
      );

      // Admin cannot reclassify operating reserve contributions into refund_reserve_balance
      const reclassifyAttempt = await alice.callRaw(
        adapterAccount,
        'admin_designate_refund_reserve',
        { amount: STANDARD_ATTACHED_DEPOSIT.toString(), evidence_sha256: TEST_EVIDENCE_SHA256 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(reclassifyAttempt).failed,
        'Operating reserve contribution must not be reclassifiable into refund_reserve_balance'
      );

      // Single-step set_admin is disabled
      const singleStepAdmin = await alice.callRaw(
        adapterAccount,
        'set_admin',
        { new_admin: bob.accountId },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(singleStepAdmin).failed, 'Single-step set_admin must fail closed');

      // Two-step propose_admin + accept_admin
      await alice.call(adapterAccount, 'propose_admin', { new_admin: bob.accountId });
      assert.strictEqual(await adapterAccount.view('get_pending_admin', {}), bob.accountId);
      await bob.call(adapterAccount, 'accept_admin', {});
      assert.strictEqual(await adapterAccount.view('get_admin', {}), bob.accountId);
      assert.strictEqual(await adapterAccount.view('get_pending_admin', {}), null);
      console.log('[PASS] Test L: Protected operating reserve and two-step admin transfer verified.');
    } finally {
      await worker.tearDown();
    }
  });

});
