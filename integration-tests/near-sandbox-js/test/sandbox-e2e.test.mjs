import test from 'node:test';
import assert from 'node:assert/strict';
import {
  setupSandboxFixture,
  createSandboxNamedTestnetAccount,
  EPHEMERAL_CLAIMANT_WASM,
  MARKET_GESHA_DEMO,
  STANDARD_ATTACHED_DEPOSIT,
  TEST_EVIDENCE_SHA256,
  STANDARD_CALL_GAS,
  STANDARD_CLAIM_GAS,
  parseTransactionResult,
} from './helpers.mjs';

test('ROASTAKE NEAR Sandbox E2E Integration Suite (Mission 03C.3.6 Security Verification)', async (t) => {

  // =========================================================================
  // Test A — Successful worker execution
  // =========================================================================
  await t.test('Test A: Successful worker execution stores price, clears pending, and preserves solvency', async () => {
    console.log('\n===============================================================');
    console.log('--- [TEST A: SUCCESSFUL WORKER EXECUTION] ---');
    console.log('===============================================================');
    const fixture = await setupSandboxFixture();
    const { worker, alice, mockOutlayer, adapterAccount } = fixture;

    try {
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'Success' });

      const txResult = await alice.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );

      const parsed = parseTransactionResult(txResult);
      console.log(`Test A Transaction ID: ${parsed.txId}`);
      console.log(`Receipt outcomes count: ${parsed.receipts.length}`);
      assert.ok(parsed.succeeded, 'Transaction must succeed');

      // 1. Verify latest price observation
      const priceObs = await adapterAccount.view('get_price', {
        market: MARKET_GESHA_DEMO,
      });
      assert.ok(priceObs, 'Price observation must exist in contract storage');
      assert.strictEqual(priceObs.market, MARKET_GESHA_DEMO);
      assert.strictEqual(priceObs.price, '1850000000', 'Price must equal fixture price ($18.50/kg)');
      assert.strictEqual(priceObs.decimals, 8);
      assert.strictEqual(priceObs.unit, 'USD/kg');
      assert.strictEqual(priceObs.nonce, 1);
      console.log(`[OK] Stored Price: ${priceObs.price} (decimals: ${priceObs.decimals}, nonce: ${priceObs.nonce})`);

      // 2. Verify pending request and unreconciled failure are absent
      const pendingReq = await adapterAccount.view('get_pending_request', { nonce: 1 });
      assert.strictEqual(pendingReq, null, 'Pending request must be removed after successful execution');

      const unreconciled = await adapterAccount.view('get_unreconciled_failure', { nonce: 1 });
      assert.strictEqual(unreconciled, null, 'No unreconciled failure on success');

      // 3. Verify no refund liability or credit is created and solvency holds
      const totalLiability = await adapterAccount.view('get_total_refund_liability', {});
      assert.strictEqual(totalLiability, '0', 'Total refund liability must remain 0 on success');

      const aliceRefund = await adapterAccount.view('get_pending_refund', {
        account_id: alice.accountId,
      });
      assert.strictEqual(aliceRefund, null, 'Alice must have no pending refund credit on success');

      const isSolvent = await adapterAccount.view('verify_solvency_invariant', {});
      assert.strictEqual(isSolvent, true, 'Formal solvency invariant must hold');
      console.log('[PASS] Test A: Successful worker response verified.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test B — Normal worker failure (null)
  // =========================================================================
  await t.test('Test B: Normal worker failure returning null terminates without price update or local liability', async () => {
    console.log('\n===============================================================');
    console.log('--- [TEST B: NORMAL WORKER FAILURE RETURNING NULL] ---');
    console.log('===============================================================');
    const fixture = await setupSandboxFixture();
    const { worker, alice, mockOutlayer, adapterAccount } = fixture;

    try {
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'NullResponse' });

      const txResult = await alice.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );

      const parsed = parseTransactionResult(txResult);
      console.log(`Test B Transaction ID: ${parsed.txId}`);
      assert.ok(parsed.succeeded, 'Transaction execution must succeed');

      // 1. Verify price observation was NOT created
      const priceObs = await adapterAccount.view('get_price', {
        market: MARKET_GESHA_DEMO,
      });
      assert.strictEqual(priceObs, null, 'No price observation must be created for failed worker');

      // 2. Verify pending request for nonce 1 is cleaned up and no unreconciled failure exists
      const pendingReq = await adapterAccount.view('get_pending_request', { nonce: 1 });
      assert.strictEqual(pendingReq, null, 'Pending request 1 must be cleaned up');

      const unreconciled = await adapterAccount.view('get_unreconciled_failure', { nonce: 1 });
      assert.strictEqual(unreconciled, null, 'Normal worker null response must not create UnreconciledFailure');

      // 3. Verify execution_failed event was emitted
      const hasFailedEvent = parsed.logs.some(
        (l) => l.includes('execution_failed') && l.includes('null response')
      );
      assert.ok(hasFailedEvent, 'execution_failed log with null response reason must be emitted');

      // 4. Verify payer_account_id = Some(alice) was passed to OutLayer and no local liability created
      const mockStats = await mockOutlayer.view('get_stats', {});
      assert.strictEqual(mockStats[3], alice.accountId, 'payer_account_id must be set to caller');

      const aliceRefund = await adapterAccount.view('get_pending_refund', {
        account_id: alice.accountId,
      });
      assert.strictEqual(aliceRefund, null, 'No local refund credit recorded for normal worker failure');

      const totalLiability = await adapterAccount.view('get_total_refund_liability', {});
      assert.strictEqual(totalLiability, '0', 'Total liability must remain 0');

      const isSolvent = await adapterAccount.view('verify_solvency_invariant', {});
      assert.strictEqual(isSolvent, true, 'Formal solvency invariant must hold');
      console.log('[PASS] Test B: Worker null response handling verified.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test C — Initial OutLayer receipt failure (Unreconciled -> Designated -> Promoted -> Claimed)
  // =========================================================================
  await t.test('Test C: Initial OutLayer receipt failure returns deposit to adapter, designates reserve, promotes, and claims', async () => {
    console.log('\n===============================================================');
    console.log('--- [TEST C: INITIAL OUTLAYER RECEIPT FAILURE] ---');
    console.log('===============================================================');
    const fixture = await setupSandboxFixture();
    const { worker, alice, mockOutlayer, adapterAccount } = fixture;

    try {
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'InitialPanic' });

      const spendableBefore = BigInt(await adapterAccount.view('get_spendable_balance', {}));

      const txResult = await alice.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );

      const parsed = parseTransactionResult(txResult);
      console.log(`Test C Request Transaction ID: ${parsed.txId}`);

      const spendableAfter = BigInt(await adapterAccount.view('get_spendable_balance', {}));
      console.log(`Adapter Spendable Before: ${spendableBefore} yoctoNEAR`);
      console.log(`Adapter Spendable After:  ${spendableAfter} yoctoNEAR`);
      assert.ok(
        spendableAfter > spendableBefore + 45_000_000_000_000_000_000_000n,
        'NEAR protocol returned the 50 mNEAR deposit to the adapter on InitialPanic'
      );

      // 1. Verify pending request 1 cleaned up and UnreconciledFailure recorded (0 liability)
      const pendingReq = await adapterAccount.view('get_pending_request', { nonce: 1 });
      assert.strictEqual(pendingReq, null, 'Pending request 1 must be cleaned up');

      const unreconciled = await adapterAccount.view('get_unreconciled_failure', { nonce: 1 });
      assert.ok(unreconciled, 'UnreconciledFailure must be recorded for nonce 1');
      assert.strictEqual(unreconciled.requester, alice.accountId);
      assert.strictEqual(unreconciled.deposit_at_risk, STANDARD_ATTACHED_DEPOSIT.toString());

      assert.strictEqual(
        await adapterAccount.view('get_total_refund_liability', {}),
        '0',
        'Total refund liability must remain 0 before promotion'
      );
      assert.strictEqual(
        await adapterAccount.view('get_refund_reserve_balance', {}),
        '0',
        'Refund reserve balance must remain 0 before designation'
      );

      // 2. Premature promotion BEFORE reserve designation MUST fail closed
      const prematurePromote = await alice.callRaw(
        adapterAccount,
        'promote_unreconciled_refund',
        { nonce: 1 },
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedPremature = parseTransactionResult(prematurePromote);
      assert.ok(parsedPremature.failed, 'Promotion before reserve designation must fail closed');
      console.log(`[OK] Premature promotion rejected as expected (Tx: ${parsedPremature.txId})`);

      // 3. Admin designates the returned 50 mNEAR into refund_reserve_balance
      const designateRes = await alice.callRaw(
        adapterAccount,
        'admin_designate_refund_reserve',
        {
          amount: STANDARD_ATTACHED_DEPOSIT.toString(),
          evidence_sha256: TEST_EVIDENCE_SHA256,
        },
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedDesignate = parseTransactionResult(designateRes);
      assert.ok(parsedDesignate.succeeded, 'admin_designate_refund_reserve must succeed when funds were returned');
      console.log(`[OK] Admin designated 50 mNEAR reserve (Tx: ${parsedDesignate.txId})`);

      assert.strictEqual(
        await adapterAccount.view('get_refund_reserve_balance', {}),
        STANDARD_ATTACHED_DEPOSIT.toString()
      );

      // 4. Promote UnreconciledFailure into backed RefundCredit
      const promoteRes = await alice.callRaw(
        adapterAccount,
        'promote_unreconciled_refund',
        { nonce: 1 },
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedPromote = parseTransactionResult(promoteRes);
      assert.ok(parsedPromote.succeeded, 'promote_unreconciled_refund must succeed after reserve designation');
      console.log(`[OK] Promoted nonce 1 to backed RefundCredit (Tx: ${parsedPromote.txId})`);

      assert.strictEqual(await adapterAccount.view('get_unreconciled_failure', { nonce: 1 }), null);
      assert.strictEqual(await adapterAccount.view('get_refund_reserve_balance', {}), '0');
      assert.strictEqual(
        await adapterAccount.view('get_total_refund_liability', {}),
        STANDARD_ATTACHED_DEPOSIT.toString()
      );
      assert.strictEqual(await adapterAccount.view('verify_solvency_invariant', {}), true);

      // 5. Alice claims her refund and balances reconcile
      const aliceBalBeforeClaim = await alice.availableBalance();
      const claimRes = await alice.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedClaim = parseTransactionResult(claimRes);
      assert.ok(parsedClaim.succeeded, 'Alice claim_refund must succeed');
      console.log(`[OK] Alice claimed 50 mNEAR refund (Tx: ${parsedClaim.txId})`);

      // Wait 1 block for the NEAR system gas-refund receipt to settle on Alice's account
      await new Promise((resolve) => setTimeout(resolve, 1000));

      const aliceBalAfterClaim = await alice.availableBalance();
      assert.ok(aliceBalAfterClaim > aliceBalBeforeClaim, 'Alice balance must increase after refund claim');
      assert.strictEqual(await adapterAccount.view('get_total_refund_liability', {}), '0');
      assert.strictEqual(await adapterAccount.view('get_pending_refund', { account_id: alice.accountId }), null);
      assert.strictEqual(await adapterAccount.view('verify_solvency_invariant', {}), true);
      console.log('[PASS] Test C: Initial receipt failure reconciliation and claim verified.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test D — Downstream failure after OutLayer accepted deposit (Verified Security Control)
  // =========================================================================
  await t.test('Test D: Downstream failure after deposit acceptance records UnreconciledFailure and prevents unbacked liability', async () => {
    console.log('\n===============================================================');
    console.log('--- [TEST D: DOWNSTREAM FAILURE SECURITY CONTROL] ---');
    console.log('===============================================================');
    const fixture = await setupSandboxFixture();
    const { worker, alice, bob, mockOutlayer, adapterAccount } = fixture;

    try {
      // First give Alice a legitimate 50 mNEAR backed refund credit (via funded reserve + InitialPanic)
      // so we also prove Bob's downstream failure cannot touch Alice's backing!
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
      assert.strictEqual(
        await adapterAccount.view('get_total_refund_liability', {}),
        STANDARD_ATTACHED_DEPOSIT.toString(),
        'Alice has a legitimate 50 mNEAR backed credit'
      );

      // Now switch Mock OutLayer to DownstreamPromiseFailure and have Bob request price update (nonce = 2)
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'DownstreamPromiseFailure' });

      const txResult = await bob.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );

      const parsed = parseTransactionResult(txResult);
      console.log(`Test D Bob Request Transaction ID: ${parsed.txId}`);

      // 1. Verify Mock OutLayer physically accepted and retained Bob's 50 mNEAR deposit
      const mockStats = await mockOutlayer.view('get_stats', {});
      assert.strictEqual(
        mockStats[2],
        STANDARD_ATTACHED_DEPOSIT.toString(),
        'Mock OutLayer physically accepted and retained the 50 mNEAR deposit'
      );

      // 2. Verify Adapter recorded UnreconciledFailure for nonce 2, NOT a payable RefundCredit for Bob
      const unreconciledBob = await adapterAccount.view('get_unreconciled_failure', { nonce: 2 });
      assert.ok(unreconciledBob, 'Adapter must record UnreconciledFailure for Bob (nonce 2)');
      assert.strictEqual(unreconciledBob.requester, bob.accountId);
      assert.strictEqual(unreconciledBob.deposit_at_risk, STANDARD_ATTACHED_DEPOSIT.toString());

      const bobRefund = await adapterAccount.view('get_pending_refund', {
        account_id: bob.accountId,
      });
      assert.strictEqual(bobRefund, null, 'Bob must NOT receive an unbacked RefundCredit');

      // 3. Verify total_refund_liability did NOT increase (remains 50 mNEAR for Alice only) and reserve is 0
      assert.strictEqual(
        await adapterAccount.view('get_total_refund_liability', {}),
        STANDARD_ATTACHED_DEPOSIT.toString(),
        'Total liability must remain 50 mNEAR (Alice only); 0 added for Bob'
      );
      assert.strictEqual(
        await adapterAccount.view('get_refund_reserve_balance', {}),
        '0',
        'Refund reserve balance remains 0'
      );

      // 4. Attempt admin_designate_refund_reserve without returned/funded reserve -> MUST FAIL CLOSED
      const designateAttempt = await alice.callRaw(
        adapterAccount,
        'admin_designate_refund_reserve',
        {
          amount: STANDARD_ATTACHED_DEPOSIT.toString(),
          evidence_sha256: TEST_EVIDENCE_SHA256,
        },
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedDesignateAttempt = parseTransactionResult(designateAttempt);
      assert.ok(
        parsedDesignateAttempt.failed,
        'admin_designate_refund_reserve must fail closed with SOLVENCY_INVARIANT_VIOLATION when deposit was retained by OutLayer'
      );
      console.log(`[OK] Unbacked admin_designate_refund_reserve rejected (Tx: ${parsedDesignateAttempt.txId})`);

      // 5. Attempt promote_unreconciled_refund without reserve -> MUST FAIL CLOSED
      const promoteAttempt = await bob.callRaw(
        adapterAccount,
        'promote_unreconciled_refund',
        { nonce: 2 },
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedPromoteAttempt = parseTransactionResult(promoteAttempt);
      assert.ok(
        parsedPromoteAttempt.failed,
        'promote_unreconciled_refund must fail closed with ERR_INSUFFICIENT_REFUND_RESERVE'
      );
      console.log(`[OK] Unbacked promote_unreconciled_refund rejected (Tx: ${parsedPromoteAttempt.txId})`);

      // 6. Verify Alice's backed credit is completely untouched and Alice can still claim her 50 mNEAR
      const aliceClaim = await alice.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(aliceClaim).succeeded, 'Alice can still claim her backed 50 mNEAR credit');
      assert.strictEqual(await adapterAccount.view('get_total_refund_liability', {}), '0');
      assert.strictEqual(await adapterAccount.view('verify_solvency_invariant', {}), true);
      console.log('[PASS] Test D: Downstream failure security control verified (no unbacked liability created).');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test D2 — Explicit insurance reserve (`fund_refund_reserve`)
  // =========================================================================
  await t.test('Test D2: Explicit insurance reserve funding enables solvent promotion of downstream failure', async () => {
    console.log('\n===============================================================');
    console.log('--- [TEST D2: EXPLICIT INSURANCE RESERVE PROMOTION] ---');
    console.log('===============================================================');
    const fixture = await setupSandboxFixture();
    const { worker, alice, bob, mockOutlayer, adapterAccount } = fixture;

    try {
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'DownstreamPromiseFailure' });

      const reqRes = await bob.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      console.log(`Test D2 Request Tx: ${parseTransactionResult(reqRes).txId}`);

      assert.ok(await adapterAccount.view('get_unreconciled_failure', { nonce: 1 }));
      assert.strictEqual(await adapterAccount.view('get_total_refund_liability', {}), '0');
      assert.strictEqual(await adapterAccount.view('get_refund_reserve_balance', {}), '0');

      // 1. Explicitly fund refund reserve with 50 mNEAR
      const fundRes = await alice.callRaw(
        adapterAccount,
        'fund_refund_reserve',
        {},
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CLAIM_GAS }
      );
      const parsedFund = parseTransactionResult(fundRes);
      assert.ok(parsedFund.succeeded, 'fund_refund_reserve must succeed');
      console.log(`[OK] Funded 50 mNEAR insurance reserve (Tx: ${parsedFund.txId})`);

      assert.strictEqual(
        await adapterAccount.view('get_refund_reserve_balance', {}),
        STANDARD_ATTACHED_DEPOSIT.toString()
      );
      assert.strictEqual(await adapterAccount.view('verify_solvency_invariant', {}), true);

      // 2. Promote Bob's UnreconciledFailure (nonce 1)
      const promoteRes = await bob.callRaw(
        adapterAccount,
        'promote_unreconciled_refund',
        { nonce: 1 },
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedPromote = parseTransactionResult(promoteRes);
      assert.ok(parsedPromote.succeeded, 'promote_unreconciled_refund must succeed after insurance funding');
      console.log(`[OK] Promoted nonce 1 using insurance reserve (Tx: ${parsedPromote.txId})`);

      // 3. Verify reserve decreased by 50 mNEAR, liability increased by 50 mNEAR, and solvency invariant holds
      assert.strictEqual(
        await adapterAccount.view('get_refund_reserve_balance', {}),
        '0',
        'Refund reserve balance must decrease by 50 mNEAR to 0'
      );
      assert.strictEqual(
        await adapterAccount.view('get_total_refund_liability', {}),
        STANDARD_ATTACHED_DEPOSIT.toString(),
        'Total refund liability must increase by 50 mNEAR'
      );
      assert.strictEqual(await adapterAccount.view('get_unreconciled_failure', { nonce: 1 }), null);
      assert.strictEqual(await adapterAccount.view('verify_solvency_invariant', {}), true);

      // 4. Bob claims his 50 mNEAR refund
      const claimRes = await bob.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(claimRes).succeeded, 'Bob claim_refund must succeed');
      assert.strictEqual(await adapterAccount.view('get_total_refund_liability', {}), '0');
      assert.strictEqual(await adapterAccount.view('verify_solvency_invariant', {}), true);
      console.log('[PASS] Test D2: Explicit insurance reserve promotion and payout verified.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test E — Successful user refund claim & terminal claim cleanup
  // =========================================================================
  await t.test('Test E: Local claim payout transfers NEAR, deletes terminal RefundClaim, and rejects repeat claims', async () => {
    console.log('\n===============================================================');
    console.log('--- [TEST E: CLAIM REFUND PAYOUT & TERMINAL CLEANUP] ---');
    console.log('===============================================================');
    const fixture = await setupSandboxFixture();
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

      const aliceBalBefore = await alice.availableBalance();
      const adapterBalBefore = await adapterAccount.availableBalance();

      const claimResult = await alice.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );

      const parsedClaim = parseTransactionResult(claimResult);
      console.log(`Test E Claim Transaction ID: ${parsedClaim.txId}`);
      assert.ok(parsedClaim.succeeded, 'Claim transaction must succeed');

      // Wait 1 block for the NEAR system gas-refund receipt to settle on Alice's account
      await new Promise((resolve) => setTimeout(resolve, 1000));

      const aliceBalAfter = await alice.availableBalance();
      const adapterBalAfter = await adapterAccount.availableBalance();

      const aliceGain = aliceBalAfter - aliceBalBefore;
      const adapterLoss = adapterBalBefore - adapterBalAfter;
      console.log(`Alice Net Gain: ${aliceGain} yoctoNEAR`);
      console.log(`Adapter Loss:   ${adapterLoss} yoctoNEAR`);

      assert.ok(aliceGain > 0n, 'Alice net balance must increase after receiving refund transfer');
      assert.ok(
        adapterLoss >= 45_000_000_000_000_000_000_000n && adapterLoss <= STANDARD_ATTACHED_DEPOSIT,
        'Adapter balance must decrease by ~50 mNEAR (net of contract gas reward)'
      );

      // 1. Verify liability decreased once to 0
      assert.strictEqual(await adapterAccount.view('get_total_refund_liability', {}), '0');
      assert.strictEqual(await adapterAccount.view('get_pending_refund', { account_id: alice.accountId }), null);

      // 2. Verify terminal RefundClaim (claim_id = 1) was deleted from claims LookupMap
      const claim1 = await adapterAccount.view('get_claim', { claim_id: 1 });
      assert.strictEqual(claim1, null, 'Terminal RefundClaim must be deleted from claims storage after settlement');

      // 3. Verify repeated claim is impossible
      const repeatClaim = await alice.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(repeatClaim).failed, 'Repeated claim_refund must fail closed');
      console.log('[PASS] Test E: Refund claim payout and terminal claim deletion verified.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test F — Refund-claim storage lifecycle across multiple claims
  // =========================================================================
  await t.test('Test F: Refund-claim storage lifecycle removes per-claim records with zero cumulative storage leak', async () => {
    console.log('\n===============================================================');
    console.log('--- [TEST F: REFUND-CLAIM STORAGE LIFECYCLE] ---');
    console.log('===============================================================');
    const fixture = await setupSandboxFixture();
    const { worker, alice, mockOutlayer, adapterAccount } = fixture;

    try {
      const storageInitial = (await adapterAccount.accountView()).storage_usage;
      console.log(`Initial Adapter Storage Usage: ${storageInitial} bytes`);

      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'InitialPanic' });

      const storageAfterEachClaim = [];
      for (let i = 1; i <= 3; i++) {
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
        await alice.call(adapterAccount, 'promote_unreconciled_refund', { nonce: i });

        const storageWithCredit = (await adapterAccount.accountView()).storage_usage;
        console.log(`Cycle ${i} Storage with Active Credit: ${storageWithCredit} bytes (+${storageWithCredit - storageInitial} bytes)`);

        const claimRes = await alice.callRaw(
          adapterAccount,
          'claim_refund',
          {},
          { gas: STANDARD_CLAIM_GAS }
        );
        assert.ok(parseTransactionResult(claimRes).succeeded, `Claim ${i} must succeed`);

        // Confirm terminal claim record i is removed
        const claimRecord = await adapterAccount.view('get_claim', { claim_id: i });
        assert.strictEqual(claimRecord, null, `Terminal claim ${i} must be removed from claims storage`);

        const storageSettled = (await adapterAccount.accountView()).storage_usage;
        storageAfterEachClaim.push(storageSettled);
        console.log(`Cycle ${i} Storage after Settlement:   ${storageSettled} bytes (delta vs initial: ${storageSettled - storageInitial} bytes)`);
      }

      // Verify zero per-claim cumulative storage growth across cycles 1, 2, and 3
      assert.strictEqual(
        storageAfterEachClaim[0],
        storageInitial,
        'Storage after 1st settled claim must return to exact initial baseline (0 residual bytes)'
      );
      assert.strictEqual(
        storageAfterEachClaim[1],
        storageInitial,
        'Storage after 2nd settled claim must return to exact initial baseline (0 residual bytes)'
      );
      assert.strictEqual(
        storageAfterEachClaim[2],
        storageInitial,
        'Storage after 3rd settled claim must return to exact initial baseline (0 residual bytes)'
      );
      console.log('[PASS] Test F: Zero per-claim terminal storage leak verified across 3 full cycles.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test G — Concurrent unresolved failures
  // =========================================================================
  await t.test('Test G: Concurrent unresolved failures cannot both be promoted from a single reserve funding', async () => {
    console.log('\n===============================================================');
    console.log('--- [TEST G: CONCURRENT UNRESOLVED FAILURES] ---');
    console.log('===============================================================');
    const fixture = await setupSandboxFixture();
    const { worker, alice, bob, mockOutlayer, adapterAccount } = fixture;

    try {
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'DownstreamPromiseFailure' });

      // 1. Create two concurrent UnreconciledFailure records (nonce 1 for Alice, nonce 2 for Bob)
      await alice.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      await bob.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );

      assert.ok(await adapterAccount.view('get_unreconciled_failure', { nonce: 1 }));
      assert.ok(await adapterAccount.view('get_unreconciled_failure', { nonce: 2 }));

      // 2. Fund reserve for ONLY ONE failure (50 mNEAR)
      await alice.call(
        adapterAccount,
        'fund_refund_reserve',
        {},
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CLAIM_GAS }
      );
      assert.strictEqual(
        await adapterAccount.view('get_refund_reserve_balance', {}),
        STANDARD_ATTACHED_DEPOSIT.toString()
      );

      // 3. Promote first failure (nonce 1) -> succeeds and consumes the 50 mNEAR reserve
      const promote1 = await alice.callRaw(
        adapterAccount,
        'promote_unreconciled_refund',
        { nonce: 1 },
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedPromote1 = parseTransactionResult(promote1);
      assert.ok(parsedPromote1.succeeded, 'First promotion (nonce 1) must succeed');
      console.log(`[OK] First promotion (nonce 1) succeeded (Tx: ${parsedPromote1.txId})`);

      assert.strictEqual(await adapterAccount.view('get_refund_reserve_balance', {}), '0');
      assert.strictEqual(
        await adapterAccount.view('get_total_refund_liability', {}),
        STANDARD_ATTACHED_DEPOSIT.toString()
      );

      // 4. Attempt second promotion (nonce 2) -> MUST FAIL CLOSED because reserve is now 0
      const promote2 = await bob.callRaw(
        adapterAccount,
        'promote_unreconciled_refund',
        { nonce: 2 },
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedPromote2 = parseTransactionResult(promote2);
      assert.ok(parsedPromote2.failed, 'Second promotion (nonce 2) must fail closed when reserve is exhausted');
      console.log(`[OK] Second promotion (nonce 2) rejected as expected (Tx: ${parsedPromote2.txId})`);

      // Verify nonce 2 remains in unreconciled_failures and solvency invariant holds
      assert.ok(await adapterAccount.view('get_unreconciled_failure', { nonce: 2 }));
      assert.strictEqual(await adapterAccount.view('verify_solvency_invariant', {}), true);
      console.log('[PASS] Test G: Concurrent unresolved failures reserve isolation verified.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test H — Failed payout transfer and delayed return (`ReturnPending` -> `reconcile_returned_refund`)
  // =========================================================================
  await t.test('Test H: Failed payout transfer transitions to ReturnPending, reconciles safely, and pays once', async () => {
    console.log('\n===============================================================');
    console.log('--- [TEST H: FAILED PAYOUT TRANSFER & RETURN RECONCILIATION] ---');
    console.log('===============================================================');
    const fixture = await setupSandboxFixture();
    const { worker, root, alice, mockOutlayer, adapterAccount } = fixture;

    try {
      // 1. Create a named `.testnet` account `charlie-claimant.testnet` in Sandbox
      // Named accounts are NOT auto-created when receiving a Transfer if they have been deleted!
      const charlieId = 'charlie-claimant.testnet';
      const { account: charlie } = await createSandboxNamedTestnetAccount(
        worker,
        root,
        charlieId,
        20_000_000_000_000_000_000_000_000n // 20 NEAR
      );

      await alice.call(adapterAccount, 'add_requester', { account_id: charlieId });

      // 2. Give both Charlie and Alice a 50 mNEAR backed refund credit (100 mNEAR total liability)
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
      await alice.call(adapterAccount, 'promote_unreconciled_refund', { nonce: 2 });

      assert.strictEqual(
        await adapterAccount.view('get_total_refund_liability', {}),
        (STANDARD_ATTACHED_DEPOSIT * 2n).toString(),
        'Total liability is 100 mNEAR (50 mNEAR Charlie + 50 mNEAR Alice)'
      );

      // 3. Deploy the 610-byte EPHEMERAL_CLAIMANT_WASM on Charlie and invoke `claim_and_delete`:
      // Charlie schedules `adapterAccount.claim_refund()` and deletes `charlie-claimant.testnet` in the same block,
      // causing the adapter's outgoing 50 mNEAR Transfer receipt to fail on-chain with AccountDoesNotExist!
      await charlie.deploy(EPHEMERAL_CLAIMANT_WASM);
      const failedTransferTx = await charlie.callRaw(
        charlie,
        'claim_and_delete',
        Buffer.from(adapterAccount.accountId, 'utf8'),
        { gas: 250_000_000_000_000n }
      );
      const parsedFailedTransfer = parseTransactionResult(failedTransferTx);
      console.log(`Test H Ephemeral Claim+Delete Tx: ${parsedFailedTransfer.txId}`);

      // Verify that a receipt failed with AccountDoesNotExist and `refund_transfer_return_pending` was emitted
      const hasReturnPendingLog = parsedFailedTransfer.logs.some((l) =>
        l.includes('refund_transfer_return_pending')
      );
      assert.ok(
        hasReturnPendingLog,
        'on_refund_transfer must emit refund_transfer_return_pending when Transfer receipt fails'
      );

      // 4. Verify Charlie's credit and claim_id 1 are in ReturnPending state and entitlement is NOT lost
      const charlieCreditPending = await adapterAccount.view('get_pending_refund', {
        account_id: charlieId,
      });
      assert.ok(charlieCreditPending, 'Charlie credit must still exist');
      assert.strictEqual(charlieCreditPending.status, 'ReturnPending');
      assert.strictEqual(charlieCreditPending.available_amount, '0');
      assert.strictEqual(charlieCreditPending.in_flight_amount, '0');
      assert.strictEqual(
        charlieCreditPending.return_pending_amount,
        STANDARD_ATTACHED_DEPOSIT.toString()
      );
      assert.strictEqual(charlieCreditPending.active_claim_id, 1);

      const claim1Pending = await adapterAccount.view('get_claim', { claim_id: 1 });
      assert.ok(claim1Pending, 'Active ReturnPending claim record must remain in claims map until reconciled');
      assert.strictEqual(claim1Pending.status, 'ReturnPending');

      // Total liability is still 100 mNEAR (Charlie's 50 mNEAR ReturnPending + Alice's 50 mNEAR Available)
      assert.strictEqual(
        await adapterAccount.view('get_total_refund_liability', {}),
        (STANDARD_ATTACHED_DEPOSIT * 2n).toString()
      );

      // 5. Recreate `charlie-claimant.testnet` on Sandbox
      const { account: charlieRecreated } = await createSandboxNamedTestnetAccount(
        worker,
        root,
        charlieId,
        20_000_000_000_000_000_000_000_000n
      );

      // Attempting claim_refund while ReturnPending (before reconcile_returned_refund) MUST fail closed
      const prematureClaim = await charlieRecreated.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(prematureClaim).failed,
        'claim_refund must fail closed while status is ReturnPending'
      );

      // 6. Reconcile the returned refund via `reconcile_returned_refund({ claim_id: 1 })`
      const reconcileRes = await charlieRecreated.callRaw(
        adapterAccount,
        'reconcile_returned_refund',
        { claim_id: 1 },
        { gas: STANDARD_CLAIM_GAS }
      );
      const parsedReconcile = parseTransactionResult(reconcileRes);
      assert.ok(parsedReconcile.succeeded, 'reconcile_returned_refund must succeed once system refund has settled');
      console.log(`[OK] Reconciled returned refund for claim_id 1 (Tx: ${parsedReconcile.txId})`);

      // Verify terminal claim_id 1 was deleted and Charlie's credit is restored to Available
      assert.strictEqual(await adapterAccount.view('get_claim', { claim_id: 1 }), null);
      const charlieRestored = await adapterAccount.view('get_pending_refund', {
        account_id: charlieId,
      });
      assert.strictEqual(charlieRestored.status, 'Available');
      assert.strictEqual(charlieRestored.available_amount, STANDARD_ATTACHED_DEPOSIT.toString());
      assert.strictEqual(charlieRestored.return_pending_amount, '0');
      assert.strictEqual(charlieRestored.active_claim_id, null);

      // 7. Charlie retries `claim_refund()` (now succeeds!) and Alice also claims her 50 mNEAR
      const charlieRetry = await charlieRecreated.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(charlieRetry).succeeded, 'Charlie retry claim_refund must succeed');
      assert.strictEqual(await adapterAccount.view('get_claim', { claim_id: 2 }), null);

      const aliceClaim = await alice.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(aliceClaim).succeeded, 'Alice claim_refund must succeed');
      assert.strictEqual(await adapterAccount.view('get_claim', { claim_id: 3 }), null);

      assert.strictEqual(await adapterAccount.view('get_total_refund_liability', {}), '0');
      assert.strictEqual(await adapterAccount.view('verify_solvency_invariant', {}), true);
      console.log('[PASS] Test H: Failed payout transfer ReturnPending lifecycle and single payout verified.');
    } finally {
      await worker.tearDown();
    }
  });

  // =========================================================================
  // Test I — Duplicate and late callback safety
  // =========================================================================
  await t.test('Test I: Direct/replayed callbacks, duplicate promotions, and post-settlement reconciliations fail closed', async () => {
    console.log('\n===============================================================');
    console.log('--- [TEST I: DUPLICATE & LATE CALLBACK SAFETY] ---');
    console.log('===============================================================');
    const fixture = await setupSandboxFixture();
    const { worker, alice, bob, mockOutlayer, adapterAccount } = fixture;

    try {
      // 1. Direct external call to private callback `on_outlayer_execution` MUST fail closed
      const directOutlayerCb = await alice.callRaw(
        adapterAccount,
        'on_outlayer_execution',
        { nonce: 1, market: MARKET_GESHA_DEMO },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(directOutlayerCb).failed,
        'Direct external call to on_outlayer_execution must fail closed'
      );

      // 2. Direct external call to private callback `on_refund_transfer` MUST fail closed
      const directRefundCb = await alice.callRaw(
        adapterAccount,
        'on_refund_transfer',
        {
          account_id: alice.accountId,
          amount: STANDARD_ATTACHED_DEPOSIT.toString(),
          claim_id: 1,
        },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(directRefundCb).failed,
        'Direct external call to on_refund_transfer must fail closed'
      );

      // 3. Create two unresolved failures (nonce 1 and nonce 2)
      await mockOutlayer.call(mockOutlayer, 'set_mode', { mode: 'DownstreamPromiseFailure' });
      await alice.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );
      await bob.callRaw(
        adapterAccount,
        'request_price_update',
        { market: MARKET_GESHA_DEMO },
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT, gas: STANDARD_CALL_GAS }
      );

      // Fund 100 mNEAR reserve and promote nonce 1
      await alice.call(
        adapterAccount,
        'fund_refund_reserve',
        {},
        { attachedDeposit: STANDARD_ATTACHED_DEPOSIT * 2n, gas: STANDARD_CLAIM_GAS }
      );
      await alice.call(adapterAccount, 'promote_unreconciled_refund', { nonce: 1 });

      // Duplicate promotion of nonce 1 MUST fail closed
      const dupPromote = await alice.callRaw(
        adapterAccount,
        'promote_unreconciled_refund',
        { nonce: 1 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(dupPromote).failed, 'Duplicate promotion of nonce 1 must fail closed');

      // External resolution of already-promoted nonce 1 MUST fail closed
      const resolveAfterPromote = await alice.callRaw(
        adapterAccount,
        'admin_resolve_unreconciled_external',
        { nonce: 1, evidence_sha256: TEST_EVIDENCE_SHA256 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(resolveAfterPromote).failed,
        'admin_resolve_unreconciled_external after promotion must fail closed'
      );

      // 4. External resolution state machine (L-02): propose blocks promotion -> requester reject clears proposal -> re-propose & ack -> duplicate/late calls fail closed
      const resolve2 = await alice.callRaw(
        adapterAccount,
        'admin_resolve_unreconciled_external',
        { nonce: 2, evidence_sha256: TEST_EVIDENCE_SHA256 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(resolve2).succeeded, 'External resolution proposal of nonce 2 must succeed');

      const proposedUnreconciled = await adapterAccount.view('get_unreconciled_failure', { nonce: 2 });
      assert.ok(proposedUnreconciled && proposedUnreconciled.external_resolution_proposed, 'Record preserved with external_resolution_proposed = true');

      const promoteWhileProposed = await bob.callRaw(
        adapterAccount,
        'promote_unreconciled_refund',
        { nonce: 2 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(promoteWhileProposed).failed,
        'promote_unreconciled_refund must fail closed while external_resolution_proposed == true (L-02)'
      );

      const nonRequesterReject = await alice.callRaw(
        adapterAccount,
        'reject_unreconciled_external_resolution',
        { nonce: 2 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(nonRequesterReject).failed,
        'Non-requester reject_unreconciled_external_resolution must fail closed'
      );

      const bobReject = await bob.callRaw(
        adapterAccount,
        'reject_unreconciled_external_resolution',
        { nonce: 2 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(bobReject).succeeded, 'Requester rejection of external resolution proposal must succeed');
      const clearedUnreconciled = await adapterAccount.view('get_unreconciled_failure', { nonce: 2 });
      assert.ok(
        clearedUnreconciled && clearedUnreconciled.external_resolution_proposed === false,
        'Requester rejection clears proposal while preserving UnreconciledFailure entitlement'
      );

      // Admin re-proposes external resolution after off-chain confirmation; duplicate proposal fails closed; requester acknowledges
      const reResolve2 = await alice.callRaw(
        adapterAccount,
        'admin_resolve_unreconciled_external',
        { nonce: 2, evidence_sha256: TEST_EVIDENCE_SHA256 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(reResolve2).succeeded, 'Re-proposing external resolution after rejection must succeed');

      const dupResolve2 = await alice.callRaw(
        adapterAccount,
        'admin_resolve_unreconciled_external',
        { nonce: 2, evidence_sha256: TEST_EVIDENCE_SHA256 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(dupResolve2).failed, 'Duplicate external resolution proposal must fail closed');

      const ackResolve2 = await bob.callRaw(
        adapterAccount,
        'acknowledge_unreconciled_resolved_external',
        { nonce: 2 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(ackResolve2).succeeded, 'Requester acknowledgement of external resolution must succeed');
      assert.strictEqual(await adapterAccount.view('get_unreconciled_failure', { nonce: 2 }), null);

      const promoteAfterResolve = await bob.callRaw(
        adapterAccount,
        'promote_unreconciled_refund',
        { nonce: 2 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(promoteAfterResolve).failed,
        'Promotion of externally resolved failure must fail closed'
      );
      const lateRejectAfterAck = await bob.callRaw(
        adapterAccount,
        'reject_unreconciled_external_resolution',
        { nonce: 2 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(lateRejectAfterAck).failed,
        'Late rejection after terminal resolution must fail closed'
      );

      // 5. Alice claims her refund (claim_id = 1 settles and is removed), then admin attempts reconciliation on settled claim
      const claim1Res = await alice.callRaw(
        adapterAccount,
        'claim_refund',
        {},
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(parseTransactionResult(claim1Res).succeeded, 'Alice claim_refund must succeed');

      const lateAdminReconcile = await alice.callRaw(
        adapterAccount,
        'admin_reconcile_stuck_claim',
        { claim_id: 1, outcome: 'ConfirmedFailure' },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(lateAdminReconcile).failed,
        'admin_reconcile_stuck_claim after terminal claim removal must fail closed'
      );

      const lateReturnReconcile = await alice.callRaw(
        adapterAccount,
        'reconcile_returned_refund',
        { claim_id: 1 },
        { gas: STANDARD_CLAIM_GAS }
      );
      assert.ok(
        parseTransactionResult(lateReturnReconcile).failed,
        'reconcile_returned_refund after terminal claim removal must fail closed'
      );

      assert.strictEqual(await adapterAccount.view('verify_solvency_invariant', {}), true);
      console.log('[PASS] Test I: Duplicate and late callback safety verified.');
    } finally {
      await worker.tearDown();
    }
  });

});
