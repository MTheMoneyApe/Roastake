// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import { afterEach, beforeEach, describe, it } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createHistoryStore, IndexerLockError } from "../src/history-store.mjs";
import { ARABICA_MARKET_ID, PERPS_CONTRACT_ID } from "../src/config.mjs";

function makeObsEvent(seq, ts, txHash) {
  return {
    eventId: `${txHash}:rcpt-${seq}:0`,
    network: "testnet",
    contractId: PERPS_CONTRACT_ID,
    marketId: ARABICA_MARKET_ID,
    category: "oracle_observation",
    eventType: "oracle_sandbox_tick",
    blockHeight: 277160000 + seq,
    blockHash: `Blk${seq}`,
    blockTimestamp: ts + 3,
    shardId: 4,
    receiptId: `rcpt-${seq}`,
    transactionHash: txHash,
    eventIndex: 0,
    data: {
      market_id: ARABICA_MARKET_ID,
      publisher: "roastake-dev-mta.testnet",
      sequence: seq,
      oracle_seq: seq,
      price_e8: "1000000000",
      priceUsd: 10,
      observed_at_sec: ts,
      data_class: "TESTNET_SANDBOX_FEED",
    },
    finality: "FINAL",
    source: "neardata_final_block",
    indexedAt: "2026-11-11T08:00:00.000Z",
  };
}

describe("perps-indexer durable JSONL store, deduplication, crash recovery & writer lock", () => {
  let tmpDir;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "roastake-store-test-"));
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it("persists JSONL, deduplicates eventId & oracle sequences, saves cursor, and prevents concurrent writers", () => {
    const store1 = createHistoryStore({ dataDir: tmpDir });
    store1.open();

    // Concurrent writer lock check while store1 is open
    const storeConcurrent = createHistoryStore({ dataDir: tmpDir });
    assert.throws(() => storeConcurrent.open(), IndexerLockError);

    const e1 = makeObsEvent(1, 1794368226, "Tx1");
    const e1DupHash = makeObsEvent(99, 1794368226, "Tx1"); // same tx & receipt -> duplicate eventId!
    e1DupHash.eventId = e1.eventId;
    const e1DupSeq = makeObsEvent(1, 1794368500, "Tx1Other"); // same oracle sequence 1!

    const res = store1.appendEvents([e1, e1DupHash, e1DupSeq]);
    assert.equal(res.appended.length, 1);
    assert.equal(res.duplicateRejections.length, 2);

    store1.saveCursor({
      coverageStartHeight: 277185000,
      indexedThroughHeight: 277185005,
      indexedThroughHash: "Blk277185005",
      lastFinalizedHeight: 277185005,
      blocksProcessed: 6,
    });
    store1.close();

    // Reopen cleanly after restart
    const store2 = createHistoryStore({ dataDir: tmpDir });
    store2.open();
    assert.equal(store2.getOracleObservations().length, 1);
    assert.equal(store2.getCursor().indexedThroughHeight, 277185005);
    store2.close();
  });

  it("truncates an incomplete corrupted trailing JSONL line during crash recovery without losing valid lines", () => {
    const store1 = createHistoryStore({ dataDir: tmpDir });
    store1.open();
    store1.appendEvents([makeObsEvent(1, 1794368226, "Tx1"), makeObsEvent(2, 1794369109, "Tx2")]);
    store1.close();

    // Simulate power failure mid-write on line 3
    const eventsPath = path.join(tmpDir, "events.jsonl");
    fs.appendFileSync(eventsPath, '{"eventId":"Tx3:rcpt:0","broken_partial_line":', "utf8");

    const recoveredStore = createHistoryStore({ dataDir: tmpDir });
    recoveredStore.open();
    assert.equal(recoveredStore.getOracleObservations().length, 2);
    assert.ok(recoveredStore.getStats().recoveredTruncatedBytes > 0);
    // Cleanly appends after recovery
    recoveredStore.appendEvents([makeObsEvent(3, 1794375377, "Tx3")]);
    assert.equal(recoveredStore.getOracleObservations().length, 3);
    recoveredStore.close();
  });
});
