// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import { afterEach, beforeEach, describe, it } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createHistoryStore } from "../src/history-store.mjs";
import { createPerpsIndexerService } from "../src/indexer.mjs";
import { ARABICA_MARKET_ID, PERPS_CONTRACT_ID } from "../src/config.mjs";

function makeMockBlock(height, tsSec, logsForPerps = []) {
  return {
    block: {
      header: {
        height,
        hash: `BlkHash${height}`,
        timestamp_nanosec: `${tsSec}000000000`,
      },
    },
    shards: [
      {
        shard_id: 4,
        chunk: { transactions: [], receipts: [] },
        receipt_execution_outcomes:
          logsForPerps.length === 0
            ? []
            : [
                {
                  tx_hash: `TxBlock${height}`,
                  receipt: { receipt_id: `Rcpt${height}`, receiver_id: PERPS_CONTRACT_ID },
                  execution_outcome: {
                    id: `Rcpt${height}`,
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

describe("perps-indexer API, backfill & restart continuity integration", () => {
  let tmpDir;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "roastake-indexer-int-"));
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it("initializes M04.6-G backfill, scans new finalized blocks, restarts cleanly without duplicates, and enforces strict CORS/HTTP API", async () => {
    let headHeight = 277190000;
    const backfillMap = new Map([
      [
        "TxSeedTick1",
        {
          blockHeight: 277160696,
          tsSec: 1794368249,
          logs: [
            `EVENT_JSON:${JSON.stringify({
              standard: "roastake-perps",
              version: "1.0.0",
              event: "oracle_sandbox_tick",
              data: [
                {
                  market_id: ARABICA_MARKET_ID,
                  publisher: "roastake-dev-mta.testnet",
                  sequence: 1,
                  price_e8: "1000000000",
                  observed_at_sec: 1794368226,
                  data_class: "TESTNET_SANDBOX_FEED",
                },
              ],
            })}`,
          ],
        },
      ],
      [
        "TxSeedOpen2",
        {
          blockHeight: 277174020,
          tsSec: 1794376008,
          logs: [
            `EVENT_JSON:${JSON.stringify({
              standard: "roastake-perps",
              version: "1.0.0",
              event: "position_opened",
              data: [
                {
                  position_id: 2,
                  owner: "majorblog5570.testnet",
                  market_id: ARABICA_MARKET_ID,
                  side: "Long",
                  size_e8: "10000000",
                  notional_e8: "100000000",
                  exec_price_e8: "1000000000",
                  margin_e8: "25000000",
                  fee_e8: "100000",
                  funding_index_e12: "0",
                },
              ],
            })}`,
          ],
        },
      ],
    ]);

    const mockBlockSource = {
      fetchLastFinalizedHeight: async () => headHeight,
      fetchFinalizedBlock: async (h) => {
        if (h === 277190001) {
          return makeMockBlock(h, 1794383100, [
            `EVENT_JSON:${JSON.stringify({
              standard: "roastake-perps",
              version: "1.0.0",
              event: "oracle_sandbox_tick",
              data: [
                {
                  market_id: ARABICA_MARKET_ID,
                  publisher: "roastake-dev-mta.testnet",
                  sequence: 6,
                  price_e8: "1025000000",
                  observed_at_sec: 1794383095,
                  data_class: "TESTNET_SANDBOX_FEED",
                },
              ],
            })}`,
          ]);
        }
        return makeMockBlock(h, 1794383000 + (h - 277190000));
      },
      fetchBackfillTxWithBlock: async (txHash) => {
        const b = backfillMap.get(txHash);
        return {
          txStatus: {
            final_execution_status: "FINAL",
            status: { SuccessValue: "" },
            transaction: { hash: txHash },
            receipts_outcome: [
              {
                id: `RcptBackfill-${txHash}`,
                block_hash: `BlkBackfill-${b.blockHeight}`,
                outcome: { executor_id: PERPS_CONTRACT_ID, status: { SuccessValue: "" }, logs: b.logs },
              },
            ],
          },
          blockHeader: {
            height: b.blockHeight,
            hash: `BlkBackfill-${b.blockHeight}`,
            timestamp_nanosec: `${b.tsSec}000000000`,
          },
        };
      },
      verifyArchivalBlockAvailability: async () => ({ available: false, reason: "n/a" }),
    };

    const backfillPtrs = [
      { txHash: "TxSeedTick1", senderAccountId: "roastake-dev-mta.testnet", role: "oracle_tick" },
      { txHash: "TxSeedOpen2", senderAccountId: "majorblog5570.testnet", role: "position_open" },
    ];

    const svc1 = createPerpsIndexerService(
      { dataDir: tmpDir, httpPort: 0, blockPacingMs: 0, backfillTransactions: backfillPtrs },
      { blockSource: mockBlockSource },
    );
    const { address } = await svc1.start({ startHttp: true, startPolling: false });
    const baseUrl = `http://127.0.0.1:${address.port}`;

    // Advance chain head & poll finalized block 277190001
    headHeight = 277190001;
    await svc1.pollOnce();

    // Verify /health & strict CORS
    const healthRes = await fetch(`${baseUrl}/health`, {
      headers: { origin: "http://127.0.0.1:3000" },
    });
    assert.equal(healthRes.status, 200);
    assert.equal(healthRes.headers.get("access-control-allow-origin"), "http://127.0.0.1:3000");
    const healthJson = await healthRes.json();
    assert.equal(healthJson.coverageStartHeight, 277190000);
    assert.equal(healthJson.indexedThroughHeight, 277190001);
    assert.equal(healthJson.observationCount, 2);
    assert.equal(healthJson.executionCount, 1);

    // Reject disallowed origin cleanly
    const badOrigin = await fetch(`${baseUrl}/health`, {
      headers: { origin: "https://evil.example" },
    });
    assert.equal(badOrigin.status, 403);

    // Reject mutating methods cleanly
    const postAttempt = await fetch(`${baseUrl}/health`, { method: "POST" });
    assert.equal(postAttempt.status, 405);

    // Inspect /history & /candles validation
    const histRes = await fetch(`${baseUrl}/v1/perps/arabica/history?timeframe=5m`);
    assert.equal(histRes.status, 200);
    const histJson = await histRes.json();
    assert.equal(histJson.sourceMode, "persistent_indexer");
    assert.equal(histJson.observations.length, 2);
    assert.equal(histJson.observations[1].data.priceUsd, 10.25);
    assert.equal(histJson.executions.length, 1);
    assert.equal(histJson.volume.available, false);

    const badTf = await fetch(`${baseUrl}/v1/perps/arabica/candles?timeframe=99m`);
    assert.equal(badTf.status, 400);

    await svc1.stop();

    // Restart cleanly against the same dataDir — verify zero lost events and zero duplicate backfill
    headHeight = 277190002;
    const svc2 = createPerpsIndexerService(
      { dataDir: tmpDir, httpPort: 0, backfillTransactions: backfillPtrs },
      { blockSource: mockBlockSource },
    );
    const { address: addr2 } = await svc2.start({ startHttp: true, startPolling: false });
    const h2 = await fetch(`http://127.0.0.1:${addr2.port}/health`).then((r) => r.json());
    assert.equal(h2.coverageStartHeight, 277190000);
    assert.equal(h2.indexedThroughHeight, 277190002);
    assert.equal(h2.observationCount, 2);
    assert.equal(h2.executionCount, 1);
    await svc2.stop();
  });

  it("halts sequentially without skipping blocks when NEAR Data reports 404 pruned gap and archival RPC is unavailable", async () => {
    const gapSource = {
      fetchLastFinalizedHeight: async () => 277195000,
      fetchFinalizedBlock: async (h) => {
        if (h === 277190001) {
          const err = new Error("missing");
          err.code = "BLOCK_NOT_FOUND";
          throw err;
        }
        return makeMockBlock(h, 1794383000);
      },
      fetchBackfillTxWithBlock: async () => {
        throw new Error("should not be called");
      },
      verifyArchivalBlockAvailability: async () => ({
        available: false,
        reason: "Archival RPC unavailable in test.",
      }),
    };

    const seedStore = createHistoryStore({ dataDir: tmpDir });
    seedStore.open();
    seedStore.saveCursor({
      version: 1,
      network: "testnet",
      contractId: PERPS_CONTRACT_ID,
      marketId: ARABICA_MARKET_ID,
      coverageStartHeight: 277190000,
      indexedThroughHeight: 277190000,
      indexedThroughHash: "Blk277190000",
      lastFinalizedHeight: 277190000,
      backfillCompleted: true,
      blocksProcessed: 1,
      rejectedCount: 0,
      rejectionsByReason: {},
    });
    seedStore.close();

    const svc = createPerpsIndexerService(
      { dataDir: tmpDir, httpPort: 0, backfillTransactions: [] },
      { blockSource: gapSource },
    );
    svc.store.open();
    const res = await svc.pollOnce();
    assert.equal(res.gapDetected, true);
    assert.equal(res.haltedAtHeight, 277190001);
    assert.equal(svc.store.getCursor().indexedThroughHeight, 277190000); // Cursor did NOT jump!
    assert.equal(svc.getState().providerStatus, "halted_historical_gap");
    await svc.stop();
  });

  it("applies exponential backoff on transient upstream errors and skips polls while backoff or concurrent poll is active", async () => {
    let failMode = true;
    const flakySource = {
      fetchLastFinalizedHeight: async () => {
        if (failMode) throw new Error("Upstream temporary 503");
        return 277190005;
      },
      fetchFinalizedBlock: async (h) => makeMockBlock(h, 1794383000),
      fetchBackfillTxWithBlock: async () => {
        throw new Error("no backfill needed");
      },
      verifyArchivalBlockAvailability: async () => ({ available: false, reason: "n/a" }),
    };

    const seedStore = createHistoryStore({ dataDir: tmpDir });
    seedStore.open();
    seedStore.saveCursor({
      version: 1,
      network: "testnet",
      contractId: PERPS_CONTRACT_ID,
      marketId: ARABICA_MARKET_ID,
      coverageStartHeight: 277190004,
      indexedThroughHeight: 277190004,
      indexedThroughHash: "Blk277190004",
      lastFinalizedHeight: 277190004,
      backfillCompleted: true,
      blocksProcessed: 1,
      rejectedCount: 0,
      rejectionsByReason: {},
    });
    seedStore.close();

    const svc = createPerpsIndexerService(
      { dataDir: tmpDir, httpPort: 0, backfillTransactions: [] },
      { blockSource: flakySource },
    );
    svc.store.open();
    const first = await svc.pollOnce();
    assert.match(first.error, /503/);
    assert.ok(first.backoffMs >= 1500);
    assert.equal(svc.getState().providerStatus, "degraded");
    assert.equal(svc.store.getCursor().indexedThroughHeight, 277190004);

    // Immediate next poll should skip cleanly because backoff window is active
    const second = await svc.pollOnce();
    assert.equal(second.skipped, true);
    assert.equal(second.reason, "backoff_active");
    await svc.stop();
  });
});
