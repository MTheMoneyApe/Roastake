// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { loadIndexerConfig, PolicyError } from "../src/config.mjs";
import {
  createBlockSource,
  BlockSourceError,
  extractHeightFromRedirectLocation,
  parseRetryAfterMs,
} from "../src/block-source.mjs";

describe("perps-indexer block-source & policy guards", () => {
  it("blocks Mainnet NEAR Data/RPC endpoints and CORS wildcards", () => {
    assert.throws(
      () => loadIndexerConfig({ nearDataBaseUrl: "https://mainnet.neardata.xyz" }),
      PolicyError,
    );
    assert.throws(
      () => loadIndexerConfig({ rpcEndpoints: ["https://rpc.mainnet.near.org"] }),
      PolicyError,
    );
    assert.throws(
      () => loadIndexerConfig({ corsOrigins: ["*"] }),
      PolicyError,
    );
  });

  it("reads finalized height from /v0/last_block/final redirect using Bearer auth without URL key leaks", async () => {
    const requested = [];
    const mockFetch = async (url, init) => {
      requested.push({ url, headers: init?.headers, redirect: init?.redirect });
      if (url.endsWith("/v0/last_block/final")) {
        return new Response(null, {
          status: 302,
          headers: { location: "/v0/block/277185500" },
        });
      }
      return new Response(
        JSON.stringify({
          block: { header: { height: 277185500, hash: "Blk277185500", timestamp_nanosec: "1794383000000000000" } },
          shards: [],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      );
    };

    const config = loadIndexerConfig({ fastnearApiKey: "secret-test-key" });
    const src = createBlockSource(config, { fetchFn: mockFetch });
    const h = await src.fetchLastFinalizedHeight();
    assert.equal(h, 277185500);

    const blk = await src.fetchFinalizedBlock(h);
    assert.equal(blk.block.header.hash, "Blk277185500");

    for (const r of requested) {
      assert.equal(r.url.includes("secret-test-key"), false);
      assert.equal(r.headers?.authorization, "Bearer secret-test-key");
    }
    assert.equal(extractHeightFromRedirectLocation("/v0/block/277185500"), 277185500);
  });


  it("falls back to an approved archival RPC for historical seed transactions", async () => {
    const requested = [];
    const config = loadIndexerConfig({
      rpcEndpoints: ["https://rpc.testnet.fastnear.com"],
      archivalRpcEndpoints: ["https://archival-rpc.testnet.near.org"],
      fastnearApiKey: "secret-test-key",
    });
    const source = createBlockSource(config, {
      fetchFn: async (url, init) => {
        const body = JSON.parse(init.body);
        requested.push({ url, method: body.method, headers: init.headers });
        if (url.includes("rpc.testnet.fastnear.com")) {
          return new Response("temporarily unavailable", { status: 503 });
        }
        if (body.method === "EXPERIMENTAL_tx_status") {
          return new Response(JSON.stringify({
            result: { transaction_outcome: { block_hash: "SeedBlockHash" }, receipts_outcome: [] },
          }), { status: 200, headers: { "content-type": "application/json" } });
        }
        return new Response(JSON.stringify({
          result: {
            header: {
              height: 277185500,
              hash: "SeedBlockHash",
              timestamp_nanosec: "1794383000000000000",
            },
          },
        }), { status: 200, headers: { "content-type": "application/json" } });
      },
    });

    const result = await source.fetchBackfillTxWithBlock(
      "SeedTransactionHash",
      "roastake-dev-mta.testnet",
    );
    assert.equal(result.blockHeader.height, 277185500);
    assert.deepEqual(requested.map((x) => x.url), [
      "https://rpc.testnet.fastnear.com",
      "https://archival-rpc.testnet.near.org",
      "https://rpc.testnet.fastnear.com",
      "https://archival-rpc.testnet.near.org",
    ]);
    assert.deepEqual(requested.map((x) => x.method), [
      "EXPERIMENTAL_tx_status",
      "EXPERIMENTAL_tx_status",
      "block",
      "block",
    ]);
    for (const request of requested) {
      assert.equal(request.url.includes("secret-test-key"), false);
      assert.equal(request.headers.authorization, "Bearer secret-test-key");
    }
  });

  it("marks 429 rate limits & null blocks as retryable BlockSourceErrors, parses Retry-After, and rejects 404 gaps explicitly", async () => {
    const config = loadIndexerConfig();
    const rateLimited = createBlockSource(config, {
      fetchFn: async () =>
        new Response("rate limited", { status: 429, headers: { "retry-after": "7" } }),
    });
    await assert.rejects(
      () => rateLimited.fetchFinalizedBlock(277185501),
      (err) =>
        err instanceof BlockSourceError &&
        err.retryable === true &&
        err.code === "RATE_LIMIT" &&
        err.retryAfterMs === 7_000,
    );

    assert.equal(parseRetryAfterMs("12"), 12_000);
    assert.equal(
      parseRetryAfterMs("Wed, 11 Nov 2026 08:00:15 GMT", Date.parse("2026-11-11T08:00:05Z")),
      10_000,
    );
    assert.equal(parseRetryAfterMs(""), null);

    const nullBlock = createBlockSource(config, {
      fetchFn: async () => new Response("null", { status: 200 }),
    });
    await assert.rejects(
      () => nullBlock.fetchFinalizedBlock(277185501),
      (err) => err instanceof BlockSourceError && err.retryable === true && err.code === "BLOCK_NULL",
    );

    const missingBlock = createBlockSource(config, {
      fetchFn: async () => new Response("not found", { status: 404 }),
    });
    await assert.rejects(
      () => missingBlock.fetchFinalizedBlock(277100000),
      (err) => err instanceof BlockSourceError && err.retryable === false && err.code === "BLOCK_NOT_FOUND",
    );
  });
  it("verifies transaction and each contract receipt against its own finalized archival block", async () => {
    const requested = [];
    const config = loadIndexerConfig({
      rpcEndpoints: ["https://rpc.testnet.fastnear.com"],
      archivalRpcEndpoints: ["https://archival-rpc.testnet.near.org"],
      fastnearApiKey: "secret-test-key",
    });
    const source = createBlockSource(config, {
      fetchFn: async (url, init) => {
        const body = JSON.parse(init.body);
        requested.push({ url, method: body.method, params: body.params });
        if (body.method === "EXPERIMENTAL_tx_status") {
          return new Response(JSON.stringify({
            result: {
              final_execution_status: "FINAL",
              transaction: {
                hash: "HistoricalTx",
                signer_id: "roastake-dev-mta.testnet",
              },
              transaction_outcome: { block_hash: "TransactionBlockHash" },
              status: { SuccessReceiptId: "RootReceipt" },
              receipts_outcome: [{
                id: "PerpsReceipt",
                block_hash: "ReceiptBlockHash",
                outcome: {
                  executor_id: "perps-v1.roastake-dev-mta.testnet",
                  status: { SuccessValue: "" },
                  logs: [],
                },
              }],
            },
          }), { status: 200, headers: { "content-type": "application/json" } });
        }
        const hash = body.params.block_id;
        const headers = {
          TransactionBlockHash: {
            height: 277180001,
            hash: "TransactionBlockHash",
            timestamp_nanosec: "1794383000000000000",
          },
          ReceiptBlockHash: {
            height: 277180004,
            hash: "ReceiptBlockHash",
            timestamp_nanosec: "1794383004000000000",
          },
        };
        return new Response(JSON.stringify({ result: { header: headers[hash] } }), {
          status: 200,
          headers: { "content-type": "application/json" },
        });
      },
    });

    const result = await source.fetchAccountHistoryTxWithBlocks(
      "HistoricalTx",
      "roastake-dev-mta.testnet",
    );
    assert.equal(result.txBlockHeader.height, 277180001);
    assert.equal(result.blockHeader.height, 277180004);
    assert.equal(result.blockHeadersByHash.ReceiptBlockHash.height, 277180004);
    assert.deepEqual(requested.map((item) => item.method), [
      "EXPERIMENTAL_tx_status",
      "block",
      "block",
    ]);
    assert.deepEqual(requested.slice(1).map((item) => item.params.block_id), [
      "TransactionBlockHash",
      "ReceiptBlockHash",
    ]);
    assert.ok(requested.every((item) => !item.url.includes("secret-test-key")));
  });


});
