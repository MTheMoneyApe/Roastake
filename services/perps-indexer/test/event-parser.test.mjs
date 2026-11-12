// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { ARABICA_MARKET_ID, PERPS_CONTRACT_ID } from "../src/config.mjs";
import { parseNearDataFinalBlock, parseRpcTxStatusBackfillOutcome } from "../src/event-parser.mjs";

function makeTickEnvelope(overrides = {}) {
  return `EVENT_JSON:${JSON.stringify({
    standard: "roastake-perps",
    version: "1.0.0",
    event: "oracle_sandbox_tick",
    data: [
      {
        market_id: ARABICA_MARKET_ID,
        publisher: "roastake-dev-mta.testnet",
        sequence: 6,
        price_e8: "1000000000",
        observed_at_sec: 1794382000,
        data_class: "TESTNET_SANDBOX_FEED",
        ...overrides,
      },
    ],
  })}`;
}

describe("perps-indexer unified NEP-297 event parser", () => {
  it("parses finalized NEAR Data blocks across both chunk.transactions (local) and receipt_execution_outcomes (cross-shard)", () => {
    const block = {
      block: {
        header: {
          height: 277185999,
          hash: "Blk277185999",
          timestamp_nanosec: "1794382005000000000",
        },
      },
      shards: [
        {
          shard_id: 2,
          chunk: {
            transactions: [
              {
                transaction: {
                  hash: "TxLocalTick6",
                  receiver_id: PERPS_CONTRACT_ID,
                },
                outcome: {
                  execution_outcome: {
                    id: "RcptLocal6",
                    outcome: {
                      executor_id: PERPS_CONTRACT_ID,
                      status: { SuccessValue: "" },
                      logs: [makeTickEnvelope({ sequence: 6 })],
                    },
                  },
                },
              },
            ],
            receipts: [],
          },
          receipt_execution_outcomes: [],
        },
        {
          shard_id: 4,
          chunk: { transactions: [], receipts: [{ receiver_id: PERPS_CONTRACT_ID }] },
          receipt_execution_outcomes: [
            {
              tx_hash: "TxCrossShardMulti",
              receipt: { receipt_id: "RcptCross4", receiver_id: PERPS_CONTRACT_ID },
              execution_outcome: {
                id: "RcptCross4",
                outcome: {
                  executor_id: PERPS_CONTRACT_ID,
                  status: { SuccessValue: "Mg==" },
                  logs: [
                    `EVENT_JSON:${JSON.stringify({
                      standard: "roastake-perps",
                      version: "1.0.0",
                      event: "position_opened",
                      data: [
                        {
                          position_id: 3,
                          owner: "majorblog5570.testnet",
                          market_id: ARABICA_MARKET_ID,
                          side: "Long",
                          size_e8: "10000000",
                          notional_e8: "100000000",
                          exec_price_e8: "1000000000",
                          margin_e8: "25000000",
                          fee_e8: "100000",
                          funding_index_e12: "20826044",
                        },
                        {
                          position_id: 4,
                          owner: "majorblog5570.testnet",
                          market_id: ARABICA_MARKET_ID,
                          side: "Short",
                          size_e8: "20000000",
                          notional_e8: "200000000",
                          exec_price_e8: "1000000000",
                          margin_e8: "50000000",
                          fee_e8: "200000",
                          funding_index_e12: "20826044",
                        },
                      ],
                    })}`,
                  ],
                },
              },
            },
          ],
        },
      ],
    };

    const res = parseNearDataFinalBlock(block);
    assert.equal(res.rejected.length, 0);
    assert.equal(res.events.length, 3);
    assert.equal(res.events[0].shardId, 2);
    assert.equal(res.events[0].data.sequence, 6);
    assert.equal(res.events[1].shardId, 4);
    assert.equal(res.events[1].data.position_id, 3);
    assert.equal(res.events[2].data.position_id, 4);
    assert.equal(res.events[2].data.side, "Short");
  });

  it("rejects failed receipts, optimistic blocks, wrong contracts, wrong markets, Mainnet accounts, and out-of-bounds prices", () => {
    assert.throws(
      () =>
        parseNearDataFinalBlock({
          optimistic: true,
          block: { header: { height: 100, hash: "Opt", timestamp_nanosec: "1794382005000000000" } },
          shards: [],
        }),
      /optimistic/i,
    );

    const nonFinalRpc = parseRpcTxStatusBackfillOutcome({
      txHash: "TxNonFinal",
      txStatus: { final_execution_status: "EXECUTED_OPTIMISTIC", status: { SuccessValue: "" }, receipts_outcome: [] },
      blockHeader: { height: 100, hash: "Blk100", timestamp_nanosec: "1794382005000000000" },
    });
    assert.equal(nonFinalRpc.events.length, 0);
    assert.equal(nonFinalRpc.rejected[0].reason, "non_final_transaction");

    const mixedBadBlock = {
      block: { header: { height: 277186000, hash: "BlkBad", timestamp_nanosec: "1794382005000000000" } },
      shards: [
        {
          shard_id: 1,
          chunk: { transactions: [], receipts: [] },
          receipt_execution_outcomes: [
            {
              tx_hash: "TxFail",
              execution_outcome: {
                id: "RcptFail",
                outcome: {
                  executor_id: PERPS_CONTRACT_ID,
                  status: { Failure: { ActionError: {} } },
                  logs: [makeTickEnvelope({ sequence: 7 })],
                },
              },
            },
            {
              tx_hash: "TxSpoofContract",
              execution_outcome: {
                id: "RcptSpoof",
                outcome: {
                  executor_id: "fake-perps.testnet",
                  status: { SuccessValue: "" },
                  logs: [makeTickEnvelope({ sequence: 8 })],
                },
              },
            },
            {
              tx_hash: "TxOtherMarket",
              execution_outcome: {
                id: "RcptOtherMkt",
                outcome: {
                  executor_id: PERPS_CONTRACT_ID,
                  status: { SuccessValue: "" },
                  logs: [makeTickEnvelope({ market_id: "COFFEE-ROBUSTA", sequence: 9 })],
                },
              },
            },
            {
              tx_hash: "TxMainnetAcc",
              execution_outcome: {
                id: "RcptMainnet",
                outcome: {
                  executor_id: PERPS_CONTRACT_ID,
                  status: { SuccessValue: "" },
                  logs: [makeTickEnvelope({ publisher: "attacker.near", sequence: 10 })],
                },
              },
            },
            {
              tx_hash: "TxBadPrice",
              execution_outcome: {
                id: "RcptBadPrice",
                outcome: {
                  executor_id: PERPS_CONTRACT_ID,
                  status: { SuccessValue: "" },
                  logs: [makeTickEnvelope({ price_e8: "9999999999999", sequence: 11 })],
                },
              },
            },
          ],
        },
      ],
    };

    const parsed = parseNearDataFinalBlock(mixedBadBlock);
    assert.equal(parsed.events.length, 0);
    assert.deepEqual(
      parsed.rejected.map((r) => r.reason),
      ["failed_receipt", "wrong_market", "mainnet_or_invalid_account", "price_out_of_bounds"],
    );
  });
});
