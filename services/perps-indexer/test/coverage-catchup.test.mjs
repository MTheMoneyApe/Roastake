// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import { afterEach, beforeEach, describe, it } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createHistoryStore } from "../src/history-store.mjs";
import { createPerpsIndexerService } from "../src/indexer.mjs";
import { buildCoverageSummary } from "../src/coverage.mjs";
import { computeBackoffDelayMs, createRateLimiter } from "../src/rate-limiter.mjs";
import { BlockSourceError } from "../src/block-source.mjs";
import { ARABICA_MARKET_ID, PERPS_CONTRACT_ID } from "../src/config.mjs";

function makeBlockWithEvents(height, tsSec, logsForPerps = [], txPrefix = "Tx") {
  return {
    block: {
      header: {
        height,
        hash: `Hash-${height}`,
        timestamp_nanosec: `${tsSec}000000000`,
      },
    },
    shards: [
      {
        shard_id: 2,
        chunk: { transactions: [], receipts: [] },
        receipt_execution_outcomes:
          logsForPerps.length === 0
            ? []
            : [
                {
                  tx_hash: `${txPrefix}-${height}`,
                  receipt: { receipt_id: `Rcpt-${height}`, receiver_id: PERPS_CONTRACT_ID },
                  execution_outcome: {
                    id: `Rcpt-${height}`,
                    outcome: {
                      executor_id: PERPS_CONTRACT_ID,
                      status: { SuccessValue: "" },
                      logs: logsForPerps,
                    },
                  },
                },
              ],
      },
    ],
  };
}

function makeOracleLog(seq, obsSec, priceE8 = "1015000000") {
  return `EVENT_JSON:${JSON.stringify({
    standard: "roastake-perps",
    version: "1.0.0",
    event: "oracle_sandbox_tick",
    data: [
      {
        market_id: ARABICA_MARKET_ID,
        publisher: "roastake-dev-mta.testnet",
        sequence: seq,
        price_e8: priceE8,
        observed_at_sec: obsSec,
        data_class: "TESTNET_SANDBOX_FEED",
      },
    ],
  })}`;
}

describe("M04.13.1 coverage reconciliation, lag catch-up & rate-limit resilience", () => {
  let tmpDir;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "roastake-m04131-"));
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it("keeps seedBackfill separate from forwardScan, detects gaps between segments, and marks coverageComplete correctly", () => {
    const partialSummary = buildCoverageSummary({
      cursor: {
        coverageStartHeight: 277186961,
        indexedThroughHeight: 277187022,
        indexedThroughHash: "Hash22",
        backfillCompleted: true,
        backfillTxHashes: ["Tx1", "Tx2"],
        backfillBlockHeights: [277160696, 277174472],
        backfillBlockRange: { minHeight: 277160696, maxHeight: 277174472 },
        scannedSegments: [
          { startHeight: 277174473, endHeight: 277174572, source: "range_backfill" },
          { startHeight: 277186961, endHeight: 277187022, source: "forward_scan" },
        ],
        blocksProcessed: 162,
      },
      runtime: { gapDetected: false },
      backfillTxCount: 2,
    });

    assert.equal(partialSummary.coverageComplete, false);
    assert.equal(partialSummary.seedBackfillRange.maxHeight, 277174472);
    assert.equal(partialSummary.seedBackfillRange.contiguousBlockScan, false);
    assert.equal(partialSummary.forwardScanRange.startHeight, 277186961);
    assert.equal(partialSummary.forwardScanRange.endHeight, 277187022);
    assert.equal(partialSummary.coverageSegments.length, 3);
    assert.equal(partialSummary.coverageGaps.length, 1);
    assert.deepEqual(
      {
        fromHeight: partialSummary.coverageGaps[0].fromHeight,
        toHeight: partialSummary.coverageGaps[0].toHeight,
        missingBlocks: partialSummary.coverageGaps[0].missingBlocks,
      },
      {
        fromHeight: 277174573,
        toHeight: 277186960,
        missingBlocks: 12388,
      },
    );

    // Once the entire interval 277174473..277186960 is scanned, coverageComplete flips true.
    const completeSummary = buildCoverageSummary({
      cursor: {
        coverageStartHeight: 277186961,
        indexedThroughHeight: 277187022,
        indexedThroughHash: "Hash22",
        backfillCompleted: true,
        backfillTxHashes: ["Tx1", "Tx2"],
        backfillBlockHeights: [277160696, 277174472],
        backfillBlockRange: { minHeight: 277160696, maxHeight: 277174472 },
        scannedSegments: [
          { startHeight: 277174473, endHeight: 277186960, source: "range_backfill" },
          { startHeight: 277186961, endHeight: 277187022, source: "forward_scan" },
        ],
        blocksProcessed: 12550,
      },
      runtime: { gapDetected: false },
      backfillTxCount: 2,
    });

    assert.equal(completeSummary.coverageComplete, true);
    assert.equal(completeSummary.coverageGaps.length, 0);
    assert.equal(completeSummary.totalGapBlocks, 0);
  });

  it("resumes bounded historical range backfill after interruption, migrates legacy v1 cursor cleanly, and deduplicates overlapping events", async () => {
    // Seed a legacy v1 cursor (like M04.13 on-disk cursor) plus an existing oracle tick #1 at tx SharedTx
    const seedStore = createHistoryStore({ dataDir: tmpDir });
    seedStore.open();
    seedStore.appendEvents([
      {
        eventId: "SharedTx:Rcpt-102:0",
        network: "testnet",
        contractId: PERPS_CONTRACT_ID,
        marketId: ARABICA_MARKET_ID,
        category: "oracle_observation",
        eventType: "oracle_sandbox_tick",
        blockHeight: 102,
        blockHash: "Hash-102",
        blockTimestamp: 1794380102,
        shardId: 2,
        receiptId: "Rcpt-102",
        transactionHash: "SharedTx",
        eventIndex: 0,
        data: {
          market_id: ARABICA_MARKET_ID,
          publisher: "roastake-dev-mta.testnet",
          sequence: 1,
          oracle_seq: 1,
          price_e8: "1000000000",
          priceUsd: 10,
          observed_at_sec: 1794380100,
          data_class: "TESTNET_SANDBOX_FEED",
        },
        finality: "FINAL",
        source: "rpc_experimental_tx_status_backfill",
        indexedAt: "2026-11-11T08:00:00.000Z",
      },
    ]);
    seedStore.saveCursor({
      version: 1,
      network: "testnet",
      contractId: PERPS_CONTRACT_ID,
      marketId: ARABICA_MARKET_ID,
      coverageStartHeight: 110,
      indexedThroughHeight: 112,
      indexedThroughHash: "Hash-112",
      lastFinalizedHeight: 112,
      backfillCompleted: true,
      backfillTxHashes: ["SharedTx"],
      backfillBlockRange: { minHeight: 102, maxHeight: 102 },
      blocksProcessed: 3,
      rejectedCount: 0,
      rejectionsByReason: {},
    });
    seedStore.close();

    const fetchedHeights = [];
    const mockBlockSource = {
      fetchLastFinalizedHeight: async () => 112,
      fetchFinalizedBlock: async (h) => {
        fetchedHeights.push(h);
        if (h === 104) {
          // Same oracle sequence 1 on a different block + new seq 2 on height 106
          return makeBlockWithEvents(h, 1794380104, [makeOracleLog(1, 1794380100)], "DupTx");
        }
        if (h === 106) {
          return makeBlockWithEvents(h, 1794380106, [makeOracleLog(2, 1794380105)], "NewTx");
        }
        return makeBlockWithEvents(h, 1794380000 + h);
      },
      fetchBackfillTxWithBlock: async () => {
        throw new Error("backfill already complete");
      },
      verifyArchivalBlockAvailability: async () => ({ available: false, reason: "n/a" }),
    };

    // Run first chunk (3 blocks: 103..105)
    const svc1 = createPerpsIndexerService(
      { dataDir: tmpDir, httpPort: 0, blockPacingMs: 0, backfillTransactions: [] },
      { blockSource: mockBlockSource },
    );
    await svc1.start({ startHttp: false, startPolling: false });
    const run1 = await svc1.runHistoricalRangeBackfill({
      fromHeight: 103,
      toHeight: 109,
      maxBlocks: 3,
    });
    assert.equal(run1.scannedThisRun, 3);
    assert.equal(run1.lastVerifiedHeight, 105);
    assert.equal(run1.completedRange, false);
    assert.equal(svc1.store.getOracleObservations().length, 1); // seq 1 dup was deduplicated!
    await svc1.stop();

    // Restart and resume same range — 103..105 MUST be skipped (not re-fetched), 106..109 finishes range cleanly
    fetchedHeights.length = 0;
    const svc2 = createPerpsIndexerService(
      { dataDir: tmpDir, httpPort: 0, blockPacingMs: 0, backfillTransactions: [] },
      { blockSource: mockBlockSource },
    );
    const { address } = await svc2.start({ startHttp: true, startPolling: false });
    const run2 = await svc2.runHistoricalRangeBackfill({
      fromHeight: 103,
      toHeight: 109,
      maxBlocks: 10,
    });

    assert.deepEqual(fetchedHeights, [106, 107, 108, 109]);
    assert.equal(run2.skippedAlreadyCovered, 3);
    assert.equal(run2.scannedThisRun, 4);
    assert.equal(run2.completedRange, true);
    assert.equal(svc2.store.getOracleObservations().length, 2);

    const health = await fetch(`http://127.0.0.1:${address.port}/health`).then((r) => r.json());
    assert.equal(health.coverageComplete, true);
    assert.equal(health.coverageGaps.length, 0);
    assert.equal(health.status, "healthy");
    assert.equal(health.forwardScanRange.startHeight, 110);
    assert.equal(health.forwardScanRange.endHeight, 112);

    await svc2.stop();
  });

  it("handles HTTP 429 with Retry-After + deterministic jitter backoff, never advances cursor on error, and surfaces degraded /health", async () => {
    const seedStore = createHistoryStore({ dataDir: tmpDir });
    seedStore.open();
    seedStore.saveCursor({
      version: 2,
      network: "testnet",
      contractId: PERPS_CONTRACT_ID,
      marketId: ARABICA_MARKET_ID,
      coverageStartHeight: 500,
      indexedThroughHeight: 500,
      indexedThroughHash: "Hash-500",
      lastFinalizedHeight: 500,
      backfillCompleted: true,
      scannedSegments: [{ startHeight: 500, endHeight: 500, source: "forward_scan" }],
      blocksProcessed: 1,
    });
    seedStore.close();

    // Unit check for computeBackoffDelayMs: Retry-After floor + jitter
    const dWithRetryAfter = computeBackoffDelayMs({
      consecutiveErrors: 1,
      initialBackoffMs: 1_500,
      retryAfterMs: 4_000,
      jitterRatio: 0.2,
      randomFn: () => 0.5,
    });
    assert.ok(dWithRetryAfter >= 4_000 && dWithRetryAfter <= 4_500);

    const dExpJitter = computeBackoffDelayMs({
      consecutiveErrors: 3,
      initialBackoffMs: 1_000,
      jitterRatio: 0.2,
      randomFn: () => 0.5,
    });
    assert.equal(dExpJitter, 4_400); // 4000 * (1 + 0.5 * 0.2)

    let nowClockMs = 1_794_390_000_000;
    let should429 = true;

    const rateLimitSource = {
      fetchLastFinalizedHeight: async () => 505,
      fetchFinalizedBlock: async (h) => {
        if (should429) {
          throw new BlockSourceError("Rate limited at block #501", "RATE_LIMIT", {
            status: 429,
            retryable: true,
            retryAfterMs: 6_000,
          });
        }
        return makeBlockWithEvents(h, 1794390000 + h);
      },
      verifyArchivalBlockAvailability: async () => ({ available: false, reason: "n/a" }),
    };

    const svc = createPerpsIndexerService(
      { dataDir: tmpDir, httpPort: 0, blockPacingMs: 0, backfillTransactions: [] },
      {
        blockSource: rateLimitSource,
        nowFn: () => nowClockMs,
        randomFn: () => 0.5,
      },
    );
    const { address } = await svc.start({ startHttp: true, startPolling: false });

    // Initial poll inside start() hit 429 at block 501
    assert.equal(svc.store.getCursor().indexedThroughHeight, 500); // Cursor did NOT advance!
    assert.equal(svc.getState().providerStatus, "degraded");
    assert.equal(svc.getState().lastErrorCode, "RATE_LIMIT");
    assert.ok(svc.getState().backoffUntilMs >= nowClockMs + 6_000);

    const hDegraded = await fetch(`http://127.0.0.1:${address.port}/health`).then((r) => r.json());
    assert.equal(hDegraded.status, "degraded");
    assert.equal(hDegraded.providerStatus, "degraded");
    assert.equal(hDegraded.lastErrorCode, "RATE_LIMIT");
    assert.equal(hDegraded.lagBlocks, 5);
    assert.ok(hDegraded.lastErrorAt);

    // Next immediate poll before Retry-After elapsed stays skipped
    const skipped = await svc.pollOnce();
    assert.equal(skipped.skipped, true);
    assert.equal(skipped.reason, "backoff_active");

    // Advance virtual clock past Retry-After + recover provider cleanly
    nowClockMs += 10_000;
    should429 = false;
    const recovered = await svc.pollOnce();
    assert.equal(recovered.processedBlocks, 5);
    assert.equal(svc.store.getCursor().indexedThroughHeight, 505);
    assert.equal(svc.getState().providerStatus, "healthy");
    assert.equal(svc.getState().lastErrorCode, null);

    const hHealthy = await fetch(`http://127.0.0.1:${address.port}/health`).then((r) => r.json());
    assert.equal(hHealthy.status, "healthy");
    assert.equal(hHealthy.providerStatus, "healthy");
    assert.equal(hHealthy.lagBlocks, 0);
    assert.ok(hHealthy.lastSuccessfulBlockAt);

    await svc.stop();
  });

  it("distinguishes temporary tip/archival-verified 404 propagation lag from permanent irreversible 404 gaps", async () => {
    const seedStore = createHistoryStore({ dataDir: tmpDir });
    seedStore.open();
    seedStore.saveCursor({
      version: 2,
      network: "testnet",
      contractId: PERPS_CONTRACT_ID,
      marketId: ARABICA_MARKET_ID,
      coverageStartHeight: 900,
      indexedThroughHeight: 900,
      indexedThroughHash: "Hash-900",
      lastFinalizedHeight: 900,
      backfillCompleted: true,
      scannedSegments: [{ startHeight: 900, endHeight: 900, source: "forward_scan" }],
      blocksProcessed: 1,
    });
    seedStore.close();

    let nowClockMs = 1_794_391_000_000;
    let tipMissingTemporarily = true;

    const svc = createPerpsIndexerService(
      {
        dataDir: tmpDir,
        httpPort: 0,
        blockPacingMs: 0,
        nearTip404ToleranceBlocks: 5,
        maxTransient404Retries: 2,
        backfillTransactions: [],
      },
      {
        nowFn: () => nowClockMs,
        randomFn: () => 0,
        blockSource: {
          fetchLastFinalizedHeight: async () => 902,
          fetchFinalizedBlock: async (h) => {
            if (h === 901 && tipMissingTemporarily) {
              throw new BlockSourceError(`Temp missing #${h}`, "BLOCK_NOT_FOUND", {
                status: 404,
                retryable: false,
                gapAtHeight: h,
              });
            }
            return makeBlockWithEvents(h, 1794391000 + h);
          },
          verifyArchivalBlockAvailability: async () => ({ available: false, reason: "n/a" }),
        },
      },
    );
    svc.store.open();

    // First 404 within 2 blocks of tip (902) must NOT mark halted_historical_gap; marks degraded retry!
    const r1 = await svc.pollOnce();
    assert.equal(r1.gapDetected, undefined);
    assert.equal(r1.errorCode, "TEMP_TIP_BLOCK_404");
    assert.equal(svc.getState().providerStatus, "degraded");
    assert.equal(svc.store.getCursor().indexedThroughHeight, 900);

    // Block propagates cleanly on retry after backoff
    nowClockMs += 10_000;
    tipMissingTemporarily = false;
    const r2 = await svc.pollOnce();
    assert.equal(r2.processedBlocks, 2);
    assert.equal(svc.store.getCursor().indexedThroughHeight, 902);
    assert.equal(svc.getState().providerStatus, "healthy");
    assert.equal(svc.getState().gapDetected, false);
    await svc.stop();
  });

  it("executes bounded consecutive catch-up exceeding 4 blocks (25 queued blocks) cleanly without hitting artificial cap", async () => {
    const seedStore = createHistoryStore({ dataDir: tmpDir });
    seedStore.open();
    seedStore.saveCursor({
      version: 2,
      network: "testnet",
      contractId: PERPS_CONTRACT_ID,
      marketId: ARABICA_MARKET_ID,
      coverageStartHeight: 1000,
      indexedThroughHeight: 1000,
      indexedThroughHash: "Hash-1000",
      lastFinalizedHeight: 1025,
      backfillCompleted: true,
      scannedSegments: [{ startHeight: 1000, endHeight: 1000, source: "forward_scan" }],
      blocksProcessed: 1,
    });
    seedStore.close();

    const rateLimiter = createRateLimiter({ maxRequestsPerMinute: 6000, minIntervalMs: 0 });
    const svc = createPerpsIndexerService(
      {
        dataDir: tmpDir,
        httpPort: 0,
        blockPacingMs: 0,
        maxBlocksPerCycle: 10,
        maxCatchupBlocksPerRun: 50,
        backfillTransactions: [],
      },
      {
        rateLimiter,
        blockSource: {
          fetchLastFinalizedHeight: async () => 1025,
          fetchFinalizedBlock: async (h) => makeBlockWithEvents(h, 1794392000 + h),
          verifyArchivalBlockAvailability: async () => ({ available: false, reason: "n/a" }),
        },
      },
    );

    const { address } = await svc.start({ startHttp: true, startPolling: false });
    // Note start() ran 1 pollOnce processing blocks 1001..1010; catchUp drains remaining 1011..1025
    const cu = await svc.catchUp({ maxBlocks: 50 });
    assert.equal(cu.totalProcessed, 15);
    assert.equal(cu.indexedThroughHeight, 1025);
    assert.equal(cu.lagBlocks, 0);

    const health = await fetch(`http://127.0.0.1:${address.port}/health`).then((r) => r.json());
    assert.equal(health.indexedThroughHeight, 1025);
    assert.equal(health.lagBlocks, 0);
    assert.equal(health.blocksProcessed, 26);
    assert.equal(health.coverageComplete, true);
    assert.equal(health.status, "healthy");
    await svc.stop();
  });

  it("reports API health status degraded + coverageComplete false on historical gap while keeping providerStatus healthy, and treats archival-verified 404 as temporary", async () => {
    const seedStore = createHistoryStore({ dataDir: tmpDir });
    seedStore.open();
    seedStore.saveCursor({
      version: 2,
      network: "testnet",
      contractId: PERPS_CONTRACT_ID,
      marketId: ARABICA_MARKET_ID,
      coverageStartHeight: 277186961,
      indexedThroughHeight: 277186961,
      indexedThroughHash: "Hash-61",
      lastFinalizedHeight: 277186961,
      backfillCompleted: true,
      backfillTxHashes: ["Seed1"],
      backfillBlockHeights: [277174472],
      backfillBlockRange: { minHeight: 277174472, maxHeight: 277174472 },
      scannedSegments: [{ startHeight: 277186961, endHeight: 277186961, source: "forward_scan" }],
      blocksProcessed: 1,
    });
    seedStore.close();

    let archivalTemp404 = true;
    let clockMs = 1_794_393_000_000;
    const svc = createPerpsIndexerService(
      {
        dataDir: tmpDir,
        httpPort: 0,
        blockPacingMs: 0,
        nearTip404ToleranceBlocks: 0,
        maxTransient404Retries: 2,
        backfillTransactions: [],
      },
      {
        nowFn: () => clockMs,
        randomFn: () => 0,
        blockSource: {
          fetchLastFinalizedHeight: async () => 277186975,
          fetchFinalizedBlock: async (h) => {
            if (h === 277186962 && archivalTemp404) {
              throw new BlockSourceError(`Transient cache 404 #${h}`, "BLOCK_NOT_FOUND", {
                status: 404,
                retryable: false,
                gapAtHeight: h,
              });
            }
            return makeBlockWithEvents(h, 1794393000 + (h - 277186900));
          },
          verifyArchivalBlockAvailability: async (h) => ({
            available: true,
            endpoint: "https://archival-rpc.testnet.near.org",
            blockHeader: { height: h, hash: `ArchHash-${h}` },
          }),
        },
      },
    );

    const { address } = await svc.start({ startHttp: true, startPolling: false });
    // Even far from tip (975 - 962 = 13 > 0), archival.available=true classifies 404 as temporary UPSTREAM_TEMP_BLOCK_404
    const hTemp = await fetch(`http://127.0.0.1:${address.port}/health`).then((r) => r.json());
    assert.equal(hTemp.status, "degraded");
    assert.equal(hTemp.providerStatus, "degraded");
    assert.equal(hTemp.lastErrorCode, "UPSTREAM_TEMP_BLOCK_404");
    assert.equal(hTemp.coverageComplete, false);
    assert.equal(hTemp.coverageGaps.length, 1);

    clockMs += 10_000;
    archivalTemp404 = false;
    await svc.pollOnce();

    // Now provider itself recovered (providerStatus === "healthy"), yet health.status stays "degraded" because historical gap 277174473..277186960 remains open!
    const hGapOnly = await fetch(`http://127.0.0.1:${address.port}/health`).then((r) => r.json());
    assert.equal(hGapOnly.providerStatus, "healthy");
    assert.equal(hGapOnly.lastErrorCode, null);
    assert.equal(hGapOnly.coverageComplete, false);
    assert.equal(hGapOnly.status, "degraded");
    assert.equal(hGapOnly.coverageGaps[0].fromHeight, 277174473);
    assert.equal(hGapOnly.coverageGaps[0].toHeight, 277186960);

    await svc.stop();
  });
});
