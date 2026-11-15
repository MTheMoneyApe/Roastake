// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import { afterEach, beforeEach, describe, it } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createPerpsIndexerService } from "../src/indexer.mjs";
import { ARABICA_MARKET_ID, PERPS_CONTRACT_ID } from "../src/config.mjs";

function makeBlock(height) {
  return {
    block: {
      header: {
        height,
        hash: `BackfillResilience-${height}`,
        timestamp_nanosec: "1794400000000000000",
      },
    },
    shards: [
      {
        shard_id: 0,
        chunk: { transactions: [], receipts: [] },
        receipt_execution_outcomes: [],
      },
    ],
  };
}

describe("indexer seed-backfill resilience", () => {
  let tmpDir;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "roastake-backfill-resilience-"));
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it("keeps forward scanning active when the historical seed RPC times out", async () => {
    let headHeight = 277200000;
    let nowMs = 1_794_400_000_000;
    let backfillAttempts = 0;
    const fetchedBlocks = [];
    const source = {
      fetchLastFinalizedHeight: async () => headHeight,
      fetchFinalizedBlock: async (height) => {
        fetchedBlocks.push(height);
        return makeBlock(height);
      },
      fetchBackfillTxWithBlock: async () => {
        backfillAttempts += 1;
        const error = new Error("seed RPC timed out");
        error.code = "TIMEOUT";
        throw error;
      },
      verifyArchivalBlockAvailability: async () => ({
        available: false,
        reason: "not needed",
      }),
    };

    const service = createPerpsIndexerService(
      {
        dataDir: tmpDir,
        httpPort: 0,
        blockPacingMs: 0,
        backfillTransactions: [
          { txHash: "SeedTx1", senderAccountId: "roastake-dev-mta.testnet" },
        ],
      },
      {
        blockSource: source,
        nowFn: () => nowMs,
        randomFn: () => 0,
      },
    );

    await service.start({ startHttp: false, startPolling: false });
    assert.equal(service.store.getCursor().indexedThroughHeight, 277200000);
    assert.equal(service.store.getCursor().backfillCompleted, false);
    assert.equal(service.getState().lastErrorCode, "BACKFILL_TIMEOUT");

    headHeight += 1;
    nowMs += 2_000;
    const result = await service.pollOnce();

    assert.equal(result.processedBlocks, 1);
    assert.equal(service.store.getCursor().indexedThroughHeight, 277200001);
    assert.ok(fetchedBlocks.includes(277200001));
    assert.equal(service.store.getCursor().backfillCompleted, false);
    assert.equal(service.getState().providerStatus, "healthy");
    assert.equal(service.getState().lastErrorCode, "BACKFILL_TIMEOUT");
    assert.equal(backfillAttempts, 2);

    const health = service.store.getHealth();
    assert.equal(health.status, "degraded");
    assert.equal(health.coverageComplete, false);
    await service.stop();
  });
});
