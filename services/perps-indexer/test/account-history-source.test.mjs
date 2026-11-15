// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { loadIndexerConfig } from "../src/config.mjs";
import { createBlockSource, BlockSourceError } from "../src/block-source.mjs";

describe("FastNear Testnet transaction-history API client", () => {
  it("uses the Testnet endpoints, sends Bearer auth, and preserves opaque account pagination tokens", async () => {
    const requests = [];
    const source = createBlockSource(loadIndexerConfig({ fastnearApiKey: "secret-test-key" }), {
      fetchFn: async (url, init) => {
        requests.push({ url, init, body: JSON.parse(init.body) });
        if (url.endsWith("/v0/account")) {
          return new Response(JSON.stringify({
            account_txs: [{
              transaction_hash: "AccountHistoryHash",
              tx_block_height: 277180001,
            }],
            resume_token: "opaque-token-123",
            txs_count: 12,
          }), { status: 200, headers: { "content-type": "application/json" } });
        }
        return new Response(JSON.stringify({
          transactions: [{
            transaction: {
              hash: "AccountHistoryHash",
              signer_id: "roastake-dev-mta.testnet",
            },
            receipts: [],
          }],
        }), { status: 200, headers: { "content-type": "application/json" } });
      },
    });

    const page = await source.fetchAccountTransactionPage({
      accountId: "perps-v1.roastake-dev-mta.testnet",
      fromHeight: 277174473,
      toHeight: 277802105,
      limit: 200,
      filters: { is_receiver: true, is_success: true },
    });
    assert.equal(page.accountTransactions[0].transaction_hash, "AccountHistoryHash");
    assert.equal(page.resumeToken, "opaque-token-123");
    assert.equal(page.totalTransactions, 12);

    const txs = await source.fetchTransactionsByHash(["AccountHistoryHash"]);
    assert.equal(txs[0].transaction.signer_id, "roastake-dev-mta.testnet");

    assert.deepEqual(requests.map((request) => request.url), [
      "https://tx.test.fastnear.com/v0/account",
      "https://tx.test.fastnear.com/v0/transactions",
    ]);
    assert.equal(requests[0].body.from_tx_block_height, 277174473);
    assert.equal(requests[0].body.to_tx_block_height, 277802105);
    assert.equal(requests[0].body.is_receiver, true);
    assert.equal(requests[0].body.is_function_call, true);
    assert.equal(requests[0].body.is_success, true);
    for (const request of requests) {
      assert.equal(request.init.headers.authorization, "Bearer secret-test-key");
      assert.equal(request.url.includes("secret-test-key"), false);
    }

    const nextPage = await source.fetchAccountTransactionPage({
      accountId: "perps-v1.roastake-dev-mta.testnet",
      fromHeight: 277174473,
      toHeight: 277802105,
      resumeToken: page.resumeToken,
      limit: 200,
      filters: { is_receiver: true, is_success: true },
    }).catch(() => null);
    // The mock endpoint intentionally returns a transaction page shape on every request;
    // the token itself is preserved exactly in the posted request.
    assert.equal(nextPage.accountTransactions[0].transaction_hash, "AccountHistoryHash");
    assert.equal(requests[2].body.resume_token, "opaque-token-123");
  });

  it("fails closed when the server-only FastNear key is absent", async () => {
    const source = createBlockSource(loadIndexerConfig(), {
      fetchFn: async () => {
        throw new Error("fetch must not be called without credentials");
      },
    });
    await assert.rejects(
      () => source.fetchAccountTransactionPage({
        accountId: "perps-v1.roastake-dev-mta.testnet",
        fromHeight: 277174473,
        toHeight: 277802105,
      }),
      (error) => error instanceof BlockSourceError && error.code === "AUTH_ERROR",
    );
  });
});
