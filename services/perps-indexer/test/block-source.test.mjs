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
});
