// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  ARABICA_MARKET_ID,
  GESHA_MARKET_ID,
  PERPS_CONTRACT_ID,
  ROBUSTA_MARKET_ID,
  SANDBOX_PRICE_MIN_E8,
  SANDBOX_PRICE_MAX_E8,
} from "../src/config.mjs";
import { importAccountHistoryBackfill } from "../src/account-history-backfill.mjs";

function makeStore(initialCursor) {
  let cursor = structuredClone(initialCursor);
  const events = [];
  const ids = new Set();
  return {
    events,
    getCursor: () => structuredClone(cursor),
    saveCursor(next) {
      cursor = structuredClone(next);
      return structuredClone(cursor);
    },
    appendEvents(records) {
      const appended = [];
      const duplicateRejections = [];
      for (const record of records) {
        if (ids.has(record.eventId)) {
          duplicateRejections.push({ reason: "duplicate_event_id", eventId: record.eventId });
        } else {
          ids.add(record.eventId);
          events.push(record);
          appended.push(record);
        }
      }
      return { appended, duplicateRejections };
    },
  };
}

function tickLog(marketId, sequence, priceE8, observedAtSec) {
  return `EVENT_JSON:${JSON.stringify({
    standard: "roastake-perps",
    version: "1.0.0",
    event: "oracle_sandbox_tick",
    data: [{
      market_id: marketId,
      publisher: "roastake-dev-mta.testnet",
      sequence,
      price_e8: String(priceE8),
      observed_at_sec: observedAtSec,
      data_class: "TESTNET_SANDBOX_FEED",
    }],
  })}`;
}

describe("FastNear account-history backfill", () => {
  it("paginates, verifies every candidate through RPC, imports events into the matching market, and does not claim block coverage", async () => {
    const primaryStore = makeStore({
      version: 2,
      coverageStartHeight: 277802000,
      indexedThroughHeight: 277802000,
      indexedThroughHash: "ForwardHead",
      scannedSegments: [{ startHeight: 277802000, endHeight: 277802000, source: "forward_scan" }],
      blocksProcessed: 1,
    });
    const arabicaStore = primaryStore;
    const robustaStore = makeStore({ version: 2, coverageStartHeight: 277802000, indexedThroughHeight: 277802000, scannedSegments: [{ startHeight: 277802000, endHeight: 277802000, source: "forward_scan" }] });
    const geshaStore = makeStore({ version: 2, coverageStartHeight: 277802000, indexedThroughHeight: 277802000, scannedSegments: [{ startHeight: 277802000, endHeight: 277802000, source: "forward_scan" }] });

    const markets = [
      { spec: { slug: "arabica", marketId: ARABICA_MARKET_ID, minPriceE8: 100000000n, maxPriceE8: SANDBOX_PRICE_MAX_E8 }, store: arabicaStore },
      { spec: { slug: "robusta", marketId: ROBUSTA_MARKET_ID, minPriceE8: SANDBOX_PRICE_MIN_E8, maxPriceE8: SANDBOX_PRICE_MAX_E8 }, store: robustaStore },
      { spec: { slug: "gesha", marketId: GESHA_MARKET_ID, minPriceE8: SANDBOX_PRICE_MIN_E8, maxPriceE8: SANDBOX_PRICE_MAX_E8 }, store: geshaStore },
    ];

    const txRows = [
      { transactionHash: "TxArabica", signer: "roastake-dev-mta.testnet", height: 277180001, marketId: ARABICA_MARKET_ID, seq: 100, price: 1000000000 },
      { transactionHash: "TxRobusta", signer: "roastake-dev-mta.testnet", height: 277180002, marketId: ROBUSTA_MARKET_ID, seq: 101, price: 500000000 },
      { transactionHash: "TxGesha", signer: "majorblog5570.testnet", height: 277181003, marketId: GESHA_MARKET_ID, seq: 102, price: 2500000000 },
    ];
    let pageCalls = 0;
    const pageRequests = [];
    const txBatches = [];
    const rpcVerified = [];
    const blockSource = {
      async fetchAccountTransactionPage(args) {
        pageRequests.push(args);
        pageCalls += 1;
        if (pageCalls === 1) {
          return {
            accountTransactions: txRows.slice(0, 2).map((row) => ({
              transaction_hash: row.transactionHash,
              tx_block_height: row.height,
            })),
            resumeToken: "opaque-page-2",
            totalTransactions: 3,
          };
        }
        return {
          accountTransactions: txRows.slice(2).map((row) => ({
            transaction_hash: row.transactionHash,
            tx_block_height: row.height,
          })),
          resumeToken: null,
          totalTransactions: 3,
        };
      },
      async fetchTransactionsByHash(hashes) {
        txBatches.push(hashes);
        return hashes.map((hash) => {
          const row = txRows.find((candidate) => candidate.transactionHash === hash);
          return { transaction: { hash, signer_id: row.signer } };
        });
      },
      async fetchBackfillTxWithBlock(hash, signer) {
        rpcVerified.push({ hash, signer });
        const row = txRows.find((candidate) => candidate.transactionHash === hash);
        return {
          txStatus: {
            final_execution_status: "FINAL",
            transaction: { hash, signer_id: signer },
            status: { SuccessReceiptId: `receipt-${hash}` },
            receipts_outcome: [{
              id: `receipt-${hash}`,
              block_hash: `Block-${row.height}`,
              outcome: {
                executor_id: PERPS_CONTRACT_ID,
                status: { SuccessValue: "" },
                logs: [tickLog(row.marketId, row.seq, row.price, 1794382000 + row.seq)],
              },
            }],
          },
          blockHeader: {
            height: row.height,
            hash: `Block-${row.height}`,
            timestamp_nanosec: "1794383000000000000",
          },
        };
      },
    };

    const result = await importAccountHistoryBackfill({
      config: {
        accountHistoryBackfillEnabled: true,
        accountHistoryBackfillFromHeight: 277174473,
        accountHistoryBackfillTransactionsPerCycle: 3,
        fastnearApiKey: "test-only-key",
        contractId: PERPS_CONTRACT_ID,
      },
      blockSource,
      primaryStore,
      marketPipelines: markets,
      finalHeight: 277802000,
      maxTransactions: 3,
    });

    assert.equal(result.completed, true);
    assert.equal(result.discoveredTransactions, 3);
    assert.equal(result.processedTransactions, 3);
    assert.equal(result.verifiedTransactions, 3);
    assert.equal(result.importedEvents, 3);
    assert.deepEqual(pageRequests.map((request) => request.resumeToken), [null, "opaque-page-2"]);
    assert.deepEqual(txBatches, [["TxArabica", "TxRobusta"], ["TxGesha"]]);
    assert.equal(rpcVerified.length, 3);

    assert.equal(arabicaStore.events.length, 1);
    assert.equal(robustaStore.events.length, 1);
    assert.equal(geshaStore.events.length, 1);
    assert.equal(arabicaStore.events[0].data.sequence, 100);
    assert.equal(robustaStore.events[0].data.sequence, 101);
    assert.equal(geshaStore.events[0].data.sequence, 102);

    const persistedCursor = primaryStore.getCursor();
    assert.equal(persistedCursor.accountHistoryBackfill.completed, true);
    assert.deepEqual(persistedCursor.scannedSegments, [
      { startHeight: 277802000, endHeight: 277802000, source: "forward_scan", blockCount: 1 },
    ]);
    assert.equal(persistedCursor.indexedThroughHeight, 277802000);
  });

  it("resumes within a page after a bounded run and retries the same pending row without advancing on RPC failure", async () => {
    const primaryStore = makeStore({
      version: 2,
      coverageStartHeight: 277802000,
      indexedThroughHeight: 277802000,
      scannedSegments: [{ startHeight: 277802000, endHeight: 277802000, source: "forward_scan" }],
    });
    const row = { transaction_hash: "TxRetry", tx_block_height: 277180010 };
    let fail = true;
    let detailCalls = 0;
    let rpcCalls = 0;
    const blockSource = {
      async fetchAccountTransactionPage() {
        return { accountTransactions: [row], resumeToken: null, totalTransactions: 1 };
      },
      async fetchTransactionsByHash() {
        detailCalls += 1;
        return [{ transaction: { hash: row.transaction_hash, signer_id: "roastake-dev-mta.testnet" } }];
      },
      async fetchBackfillTxWithBlock() {
        rpcCalls += 1;
        if (fail) throw new Error("archival RPC temporarily unavailable");
        return {
          txStatus: {
            final_execution_status: "FINAL",
            transaction: { hash: row.transaction_hash, signer_id: "roastake-dev-mta.testnet" },
            status: { SuccessReceiptId: "receipt" },
            receipts_outcome: [],
          },
          blockHeader: { height: 277180011, hash: "BlockRetry", timestamp_nanosec: "1794383000000000000" },
        };
      },
    };
    const config = {
      accountHistoryBackfillEnabled: true,
      accountHistoryBackfillFromHeight: 277174473,
      fastnearApiKey: "test-only-key",
      contractId: PERPS_CONTRACT_ID,
    };
    const marketPipelines = [{
      spec: { slug: "arabica", marketId: ARABICA_MARKET_ID, minPriceE8: 100000000n, maxPriceE8: SANDBOX_PRICE_MAX_E8 },
      store: primaryStore,
    }];

    await assert.rejects(() => importAccountHistoryBackfill({
      config, blockSource, primaryStore, marketPipelines, finalHeight: 277802000, maxTransactions: 1,
    }), /archival RPC temporarily unavailable/);
    assert.equal(primaryStore.getCursor().accountHistoryBackfill.pendingIndex, 0);

    fail = false;
    const result = await importAccountHistoryBackfill({
      config, blockSource, primaryStore, marketPipelines, finalHeight: 277802001, maxTransactions: 1,
    });
    assert.equal(result.processedTransactions, 1);
    assert.equal(result.completed, false);
    assert.equal(detailCalls, 2);
    assert.equal(rpcCalls, 2);
  });
});
