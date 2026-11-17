// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import { afterEach, beforeEach, describe, it } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createPerpsIndexerService } from "../src/indexer.mjs";
import {
  ARABICA_MARKET_ID,
  GESHA_MARKET_ID,
  PERPS_CONTRACT_ID,
  ROBUSTA_MARKET_ID,
  resolveConfiguredMarket,
} from "../src/config.mjs";

function makeMixedMultiMarketBlock(height, tsSec, logsForPerps = []) {
  return {
    block: {
      header: {
        height,
        hash: `BlkMulti${height}`,
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
                  tx_hash: `TxMixed${height}`,
                  receipt: { receipt_id: `RcptMixed${height}`, receiver_id: PERPS_CONTRACT_ID },
                  execution_outcome: {
                    id: `RcptMixed${height}`,
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

describe("M04.14–M04.15 Multi-Market Perps Indexer Isolation & Validated API", () => {
  let tmpDir;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "roastake-multi-mkt-"));
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it("validates market identifiers strictly and rejects unknown/prohibited/traversal inputs", () => {
    assert.equal(resolveConfiguredMarket("COFFEE-ROBUSTA")?.marketId, ROBUSTA_MARKET_ID);
    assert.equal(
      resolveConfiguredMarket("COFFEE-ROBUSTA-TESTNET%2FUSD")?.marketId,
      ROBUSTA_MARKET_ID,
    );
    assert.equal(resolveConfiguredMarket("robusta")?.slug, "robusta");
    assert.equal(
      resolveConfiguredMarket("COFFEE-ARABICA-TESTNET-USD")?.marketId,
      ARABICA_MARKET_ID,
    );
    assert.equal(resolveConfiguredMarket("COFFEE-GESHA")?.marketId, GESHA_MARKET_ID);
    assert.equal(
      resolveConfiguredMarket("COFFEE-GESHA-TESTNET%2FUSD")?.marketId,
      GESHA_MARKET_ID,
    );
    assert.equal(resolveConfiguredMarket("gesha")?.slug, "gesha");
    assert.equal(resolveConfiguredMarket("GESHA-DEMO/USD"), null);
    assert.equal(resolveConfiguredMarket("COFFEE-UNKNOWN"), null);
    assert.equal(resolveConfiguredMarket("../robusta"), null);
    assert.equal(resolveConfiguredMarket("COFFEE-ROBUSTA\0"), null);
  });

  it("isolates Arabica, Robusta & Gesha events, sequences, cursors, coverage, health, and HTTP endpoints with zero cross-contamination", async () => {
    let headHeight = 277191759;

    const mixedSource = {
      fetchLastFinalizedHeight: async () => headHeight,
      fetchFinalizedBlock: async (h) => {
        if (h === 277191760) {
          // Shared block contains:
          // - Robusta tick seq 1 ($5.00/kg), plus Robusta open #3
          // - Arabica tick seq 1 ($10.10/kg)
          // - Gesha tick seq 1 ($25.00/kg), plus Gesha open #4
          // - Protocol-wide lp_deposited without market_id (must NOT be misattributed!)
          return makeMixedMultiMarketBlock(h, 1794386427, [
            `EVENT_JSON:${JSON.stringify({
              standard: "roastake-perps",
              version: "1.0.0",
              event: "oracle_sandbox_tick",
              data: [
                {
                  market_id: ROBUSTA_MARKET_ID,
                  publisher: "roastake-dev-mta.testnet",
                  sequence: 1,
                  price_e8: "500000000",
                  observed_at_sec: 1794386406,
                  data_class: "TESTNET_SANDBOX_FEED",
                },
              ],
            })}`,
            `EVENT_JSON:${JSON.stringify({
              standard: "roastake-perps",
              version: "1.0.0",
              event: "oracle_sandbox_tick",
              data: [
                {
                  market_id: ARABICA_MARKET_ID,
                  publisher: "roastake-dev-mta.testnet",
                  sequence: 1,
                  price_e8: "1010000000",
                  observed_at_sec: 1794386410,
                  data_class: "TESTNET_SANDBOX_FEED",
                },
              ],
            })}`,
            `EVENT_JSON:${JSON.stringify({
              standard: "roastake-perps",
              version: "1.0.0",
              event: "oracle_sandbox_tick",
              data: [
                {
                  market_id: GESHA_MARKET_ID,
                  publisher: "roastake-dev-mta.testnet",
                  sequence: 1,
                  price_e8: "2500000000",
                  observed_at_sec: 1794386415,
                  data_class: "TESTNET_SANDBOX_FEED",
                },
              ],
            })}`,
            `EVENT_JSON:${JSON.stringify({
              standard: "roastake-perps",
              version: "1.0.0",
              event: "position_opened",
              data: [
                {
                  position_id: 3,
                  owner: "majorblog5570.testnet",
                  market_id: ROBUSTA_MARKET_ID,
                  side: "Long",
                  size_e8: "10000000",
                  notional_e8: "50000000",
                  exec_price_e8: "500000000",
                  margin_e8: "15000000",
                  fee_e8: "50000",
                  funding_index_e12: "0",
                },
              ],
            })}`,
            `EVENT_JSON:${JSON.stringify({
              standard: "roastake-perps",
              version: "1.0.0",
              event: "position_opened",
              data: [
                {
                  position_id: 4,
                  owner: "majorblog5570.testnet",
                  market_id: GESHA_MARKET_ID,
                  side: "Long",
                  size_e8: "4000000",
                  notional_e8: "100000000",
                  exec_price_e8: "2500000000",
                  margin_e8: "25000000",
                  fee_e8: "100000",
                  funding_index_e12: "0",
                },
              ],
            })}`,
            `EVENT_JSON:${JSON.stringify({
              standard: "roastake-perps",
              version: "1.0.0",
              event: "lp_deposited",
              data: [
                {
                  account_id: "roastake-dev-mta.testnet",
                  token_id: "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af",
                  amount_e8: "200000000",
                  shares_minted: "199521149",
                  seed_shares_locked: "0",
                  nav_e8: "10024",
                },
              ],
            })}`,
          ]);
        }
        return makeMixedMultiMarketBlock(h, 1794386400 + (h - 277191759));
      },
      fetchBackfillTxWithBlock: async () => {
        throw new Error("Should not fetch Arabica seed when backfillTransactions=[]");
      },
      verifyArchivalBlockAvailability: async () => ({ available: false, reason: "n/a" }),
    };

    const svc = createPerpsIndexerService(
      {
        dataDir: tmpDir,
        httpPort: 0,
        blockPacingMs: 0,
        backfillTransactions: [],
        robustaBackfillTransactions: [],
        geshaBackfillTransactions: [],
      },
      { blockSource: mixedSource },
    );

    const { address } = await svc.start({ startHttp: true, startPolling: false });
    const baseUrl = `http://127.0.0.1:${address.port}`;

    headHeight = 277191760;
    await svc.pollOnce();

    // 1. Storage isolation on disk
    assert.ok(fs.existsSync(path.join(tmpDir, "events.jsonl")));
    assert.ok(fs.existsSync(path.join(tmpDir, "markets", "robusta", "events.jsonl")));
    assert.ok(fs.existsSync(path.join(tmpDir, "markets", "gesha", "events.jsonl")));
    const readJsonl = (filePath) =>
      fs
        .readFileSync(filePath, "utf8")
        .trim()
        .split("\n")
        .filter(Boolean)
        .map((l) => JSON.parse(l));
    const arabicaLines = readJsonl(path.join(tmpDir, "events.jsonl"));
    const robustaLines = readJsonl(path.join(tmpDir, "markets", "robusta", "events.jsonl"));
    const geshaLines = readJsonl(path.join(tmpDir, "markets", "gesha", "events.jsonl"));

    assert.equal(arabicaLines.length, 1);
    assert.equal(arabicaLines[0].marketId, ARABICA_MARKET_ID);
    assert.equal(arabicaLines[0].data.price_e8, "1010000000");

    assert.equal(robustaLines.length, 2);
    assert.ok(robustaLines.every((r) => r.marketId === ROBUSTA_MARKET_ID));
    assert.equal(robustaLines[0].data.sequence, 1);
    assert.equal(robustaLines[0].data.price_e8, "500000000");
    assert.equal(robustaLines[1].eventType, "position_opened");

    assert.equal(geshaLines.length, 2);
    assert.ok(geshaLines.every((g) => g.marketId === GESHA_MARKET_ID));
    assert.equal(geshaLines[0].data.sequence, 1);
    assert.equal(geshaLines[0].data.price_e8, "2500000000");
    assert.equal(geshaLines[1].eventType, "position_opened");
    assert.equal(geshaLines[1].data.position_id, 4);

    // 2. Zero false rejections from seeing each other's market_id or shared lp_deposited
    assert.equal(svc.getMarketState("arabica").rejectedCount, 0);
    assert.equal(svc.getMarketState("robusta").rejectedCount, 0);
    assert.equal(svc.getMarketState("gesha").rejectedCount, 0);

    // 3. HTTP endpoints isolate Arabica, Robusta & Gesha and validate unknown/prohibited markets
    const marketsRes = await fetch(`${baseUrl}/v1/perps/markets`).then((r) => r.json());
    assert.equal(marketsRes.marketCount, 3);
    assert.deepEqual(
      marketsRes.markets.map((m) => m.marketId),
      [ARABICA_MARKET_ID, ROBUSTA_MARKET_ID, GESHA_MARKET_ID],
    );

    const robustaHist = await fetch(`${baseUrl}/v1/perps/robusta/history`).then((r) => r.json());
    assert.equal(robustaHist.marketId, ROBUSTA_MARKET_ID);
    assert.equal(robustaHist.observations.length, 1);
    assert.equal(robustaHist.observations[0].data.priceUsd, 5);
    assert.equal(robustaHist.executions.length, 1);
    assert.equal(robustaHist.executions[0].data.position_id, 3);
    assert.equal(robustaHist.coverage.coverageComplete, true);

    const geshaHist = await fetch(`${baseUrl}/v1/perps/gesha/history`).then((r) => r.json());
    assert.equal(geshaHist.marketId, GESHA_MARKET_ID);
    assert.equal(geshaHist.observations.length, 1);
    assert.equal(geshaHist.observations[0].data.priceUsd, 25);
    assert.equal(geshaHist.executions.length, 1);
    assert.equal(geshaHist.executions[0].data.position_id, 4);
    assert.equal(geshaHist.coverage.coverageComplete, true);

    const genericGesha = await fetch(
      `${baseUrl}/v1/perps/markets/COFFEE-GESHA/history`,
    ).then((r) => r.json());
    assert.equal(genericGesha.marketId, GESHA_MARKET_ID);
    assert.equal(genericGesha.observations.length, 1);

    const genericRobusta = await fetch(
      `${baseUrl}/v1/perps/markets/COFFEE-ROBUSTA/history`,
    ).then((r) => r.json());
    assert.equal(genericRobusta.marketId, ROBUSTA_MARKET_ID);
    assert.equal(genericRobusta.observations.length, 1);

    const arabicaHist = await fetch(`${baseUrl}/v1/perps/arabica/history`).then((r) => r.json());
    assert.equal(arabicaHist.marketId, ARABICA_MARKET_ID);
    assert.equal(arabicaHist.observations.length, 1);
    assert.equal(arabicaHist.observations[0].data.priceUsd, 10.1);
    assert.equal(arabicaHist.executions.length, 0);

    const unknownAttempt = await fetch(`${baseUrl}/v1/perps/markets/COFFEE-UNKNOWN/history`);
    assert.equal(unknownAttempt.status, 404);

    const prohibitedAttempt = await fetch(`${baseUrl}/v1/perps/history?market=GESHA-DEMO/USD`);
    assert.equal(prohibitedAttempt.status, 404);

    const traversalAttempt = await fetch(`${baseUrl}/v1/perps/history?market=../robusta`);
    assert.equal(traversalAttempt.status, 400);

    const missingQuery = await fetch(`${baseUrl}/v1/perps/history`);
    assert.equal(missingQuery.status, 400);

    await svc.stop();
  });
});
