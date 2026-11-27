// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import {
  assertTestnetNearDataUrl,
  assertTestnetRpcUrl,
  assertTestnetAccountId,
} from "./config.mjs";

export class BlockSourceError extends Error {
  constructor(message, code = "UPSTREAM_ERROR", opts = {}) {
    super(message);
    this.name = "BlockSourceError";
    this.code = code;
    this.status = opts.status ?? null;
    this.retryable = Boolean(opts.retryable);
    this.gapAtHeight = opts.gapAtHeight ?? null;
    this.retryAfterMs = opts.retryAfterMs ?? null;
  }
}

/**
 * Parse RFC 9110 `Retry-After` header (integer/decimal seconds or HTTP date) into bounded ms.
 */
export function parseRetryAfterMs(headerValue, nowMs = Date.now()) {
  if (typeof headerValue !== "string") return null;
  const trimmed = headerValue.trim();
  if (!trimmed) return null;
  if (/^\d+(\.\d+)?$/.test(trimmed)) {
    const sec = Number(trimmed);
    if (!Number.isFinite(sec) || sec <= 0) return null;
    return Math.min(300_000, Math.max(250, Math.round(sec * 1_000)));
  }
  const targetMs = Date.parse(trimmed);
  if (!Number.isFinite(targetMs)) return null;
  const delta = targetMs - nowMs;
  if (delta <= 0) return 500;
  return Math.min(300_000, Math.max(250, Math.round(delta)));
}

export function redactUrlForLogs(rawUrl) {
  try {
    const u = new URL(rawUrl);
    return `${u.origin}${u.pathname}`;
  } catch {
    return "[invalid-url]";
  }
}

export function extractHeightFromRedirectLocation(locationHeader) {
  if (typeof locationHeader !== "string" || !locationHeader.trim()) return null;
  const match = locationHeader.trim().match(/\/v0\/block\/(\d+)(?:$|[/?#])/);
  if (!match) return null;
  const parsed = Number.parseInt(match[1], 10);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : null;
}

export function parseNanosecTimestampSec(header) {
  if (!header || typeof header !== "object") {
    throw new BlockSourceError("Missing block header.", "INVALID_BLOCK", { retryable: false });
  }
  if (typeof header.timestamp_nanosec === "string" && /^\d+$/.test(header.timestamp_nanosec)) {
    const sec = Number(BigInt(header.timestamp_nanosec) / 1_000_000_000n);
    if (Number.isSafeInteger(sec) && sec > 0) return sec;
  }
  if (typeof header.timestamp === "number" && Number.isFinite(header.timestamp) && header.timestamp > 0) {
    return Math.floor(header.timestamp / 1_000_000_000);
  }
  throw new BlockSourceError(
    `Block ${header.hash ?? "?"} lacks valid timestamp_nanosec.`,
    "INVALID_BLOCK",
    { retryable: false },
  );
}

/**
 * Validated NEAR Data (`https://testnet.neardata.xyz`) + NEAR Testnet RPC client.
 * - Never sends `FASTNEAR_API_KEY` in URLs or logs (uses `Authorization: Bearer` header only).
 * - Resolves `/v0/last_block/final` redirect manually so `Authorization` is never leaked across redirects.
 * - Uses strict finalized block checks; never falls back to optimistic `/v0/block_opt/` routes.
 */
export function createBlockSource(config, deps = {}) {
  const nearDataBase = assertTestnetNearDataUrl(config.nearDataBaseUrl);
  const rpcEndpoints = config.rpcEndpoints.map((u) => assertTestnetRpcUrl(u));
  const archivalEndpoints = (config.archivalRpcEndpoints ?? []).map((u) => assertTestnetRpcUrl(u));
  const fetchFn = deps.fetchFn ?? fetch;
  const rateLimiter = deps.rateLimiter ?? null;
  const shouldStop = deps.shouldStop ?? (() => false);
  const timeoutMs = config.requestTimeoutMs ?? 10_000;
  const apiKey = config.fastnearApiKey ?? null;

  function buildAuthHeaders(extra = {}) {
    const headers = { accept: "application/json", ...extra };
    if (apiKey) {
      headers.authorization = `Bearer ${apiKey}`;
    }
    return headers;
  }

  async function fetchWithTimeout(url, init = {}, { metered = true } = {}) {
    if (metered && rateLimiter) {
      await rateLimiter.acquireSlot(shouldStop);
      if (shouldStop()) {
        throw new BlockSourceError("Indexer shutting down before upstream request.", "ABORTED", {
          retryable: false,
        });
      }
    }
    const startedAt = Date.now();
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
      const response = await fetchFn(url, { ...init, signal: controller.signal });
      if (metered && rateLimiter && response.status !== 429 && response.status < 500) {
        rateLimiter.recordSuccess(Date.now() - startedAt);
      }
      return response;
    } catch (err) {
      if (err?.name === "AbortError") {
        throw new BlockSourceError(
          `Request timed out after ${timeoutMs}ms (${redactUrlForLogs(url)})`,
          "TIMEOUT",
          { retryable: true },
        );
      }
      throw new BlockSourceError(
        `Network request failed for ${redactUrlForLogs(url)}: ${err?.message ?? "unknown error"}`,
        "NETWORK",
        { retryable: true },
      );
    } finally {
      clearTimeout(timer);
    }
  }

  const fastNearTxBaseUrl = "https://tx.test.fastnear.com";

  async function postFastNearTxApi(path, payload) {
    if (!apiKey) {
      throw new BlockSourceError(
        "FASTNEAR_API_KEY is required for Testnet transaction-history backfill.",
        "AUTH_ERROR",
        { retryable: false },
      );
    }
    const url = `${fastNearTxBaseUrl}${path}`;
    const res = await fetchWithTimeout(url, {
      method: "POST",
      headers: buildAuthHeaders({ "content-type": "application/json" }),
      body: JSON.stringify(payload),
    }, { metered: false });

    if (res.status === 429 || res.status >= 500) {
      const retryAfterMs = parseRetryAfterMs(res.headers?.get?.("retry-after"));
      throw new BlockSourceError(
        `FastNear Transactions API temporary HTTP ${res.status} on ${path}`,
        res.status === 429 ? "RATE_LIMIT" : "UPSTREAM_5XX",
        { status: res.status, retryable: true, retryAfterMs },
      );
    }
    if (res.status === 401 || res.status === 403) {
      throw new BlockSourceError(
        `FastNear Transactions API authentication rejected (HTTP ${res.status}). Check FASTNEAR_API_KEY on the server.`,
        "AUTH_ERROR",
        { status: res.status, retryable: false },
      );
    }
    if (!res.ok) {
      throw new BlockSourceError(
        `FastNear Transactions API returned HTTP ${res.status} on ${path}`,
        "UPSTREAM_ERROR",
        { status: res.status, retryable: res.status >= 500 },
      );
    }
    try {
      return await res.json();
    } catch {
      throw new BlockSourceError(
        `FastNear Transactions API returned invalid JSON on ${path}.`,
        "INVALID_JSON",
        { retryable: true },
      );
    }
  }

  async function fetchAccountTransactionPage({
    accountId,
    fromHeight,
    toHeight,
    resumeToken = null,
    limit = 200,
    filters = {},
  }) {
    const validAccountId = assertTestnetAccountId(accountId, "history account");
    if (!Number.isSafeInteger(fromHeight) || fromHeight < 1 ||
        !Number.isSafeInteger(toHeight) || toHeight <= fromHeight) {
      throw new BlockSourceError("Invalid FastNear account-history block range.", "INVALID_RANGE");
    }
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > 200) {
      throw new BlockSourceError("FastNear account-history page limit must be 1..200.", "INVALID_LIMIT");
    }
    if (resumeToken !== null && typeof resumeToken !== "string") {
      throw new BlockSourceError("FastNear resume_token must be an opaque string or null.", "INVALID_CURSOR");
    }

    const payload = {
      account_id: validAccountId,
      from_tx_block_height: fromHeight,
      to_tx_block_height: toHeight,
      desc: false,
      limit,
      ...filters,
    };
    if (resumeToken) payload.resume_token = resumeToken;
    const result = await postFastNearTxApi("/v0/account", payload);
    if (!result || !Array.isArray(result.account_txs)) {
      throw new BlockSourceError(
        "FastNear account-history response is missing account_txs[].",
        "INVALID_RESPONSE",
        { retryable: true },
      );
    }
    if (result.resume_token !== undefined &&
        result.resume_token !== null &&
        typeof result.resume_token !== "string") {
      throw new BlockSourceError(
        "FastNear account-history response contains an invalid resume_token.",
        "INVALID_RESPONSE",
        { retryable: true },
      );
    }
    for (const row of result.account_txs) {
      if (!row || typeof row.transaction_hash !== "string" ||
          !Number.isSafeInteger(row.tx_block_height)) {
        throw new BlockSourceError(
          "FastNear account-history response contains an invalid transaction row.",
          "INVALID_RESPONSE",
          { retryable: true },
        );
      }
    }
    return {
      accountTransactions: result.account_txs,
      resumeToken: result.resume_token ?? null,
      totalTransactions: Number.isSafeInteger(result.txs_count) ? result.txs_count : null,
    };
  }

  async function fetchTransactionsByHash(txHashes) {
    if (!Array.isArray(txHashes) || txHashes.length < 1 || txHashes.length > 20 ||
        txHashes.some((hash) => typeof hash !== "string" || !hash.trim())) {
      throw new BlockSourceError("FastNear transaction lookup requires 1..20 non-empty hashes.", "INVALID_TX_HASHES");
    }
    const result = await postFastNearTxApi("/v0/transactions", { tx_hashes: txHashes });
    if (!result || !Array.isArray(result.transactions)) {
      throw new BlockSourceError(
        "FastNear transaction lookup response is missing transactions[].",
        "INVALID_RESPONSE",
        { retryable: true },
      );
    }
    return result.transactions;
  }

  async function fetchLastFinalizedHeight() {
    const url = `${nearDataBase}/v0/last_block/final`;
    const res = await fetchWithTimeout(url, {
      method: "GET",
      redirect: "manual",
      headers: buildAuthHeaders(),
    });

    if (res.status >= 300 && res.status < 400) {
      const height = extractHeightFromRedirectLocation(res.headers.get("location"));
      if (height !== null) return height;
      throw new BlockSourceError(
        "Unexpected /v0/last_block/final redirect target without block height.",
        "INVALID_RESPONSE",
        { status: res.status, retryable: true },
      );
    }

    if (res.status === 429 || res.status >= 500) {
      const retryAfterMs = parseRetryAfterMs(res.headers?.get?.("retry-after"));
      throw new BlockSourceError(
        `NEAR Data temporary HTTP ${res.status} on /v0/last_block/final`,
        res.status === 429 ? "RATE_LIMIT" : "UPSTREAM_5XX",
        { status: res.status, retryable: true, retryAfterMs },
      );
    }
    if (res.status === 401 || res.status === 403) {
      throw new BlockSourceError(
        `NEAR Data authentication rejected (HTTP ${res.status}). Check FASTNEAR_API_KEY cleanly on server.`,
        "AUTH_ERROR",
        { status: res.status, retryable: false },
      );
    }
    if (!res.ok) {
      throw new BlockSourceError(
        `NEAR Data returned HTTP ${res.status} on /v0/last_block/final`,
        "UPSTREAM_ERROR",
        { status: res.status, retryable: res.status >= 500 },
      );
    }

    const payload = await res.json();
    const height = payload?.block?.header?.height;
    if (!Number.isSafeInteger(height) || height <= 0) {
      throw new BlockSourceError(
        "NEAR Data /v0/last_block/final response missing finalized block.header.height.",
        "INVALID_BLOCK",
        { retryable: true },
      );
    }
    return height;
  }

  async function fetchFinalizedBlock(height) {
    if (!Number.isSafeInteger(height) || height <= 0) {
      throw new BlockSourceError(`Invalid block height "${height}"`, "INVALID_HEIGHT");
    }
    const url = `${nearDataBase}/v0/block/${height}`;
    const res = await fetchWithTimeout(url, {
      method: "GET",
      headers: buildAuthHeaders(),
    });

    if (res.status === 429 || res.status >= 500) {
      const retryAfterMs = parseRetryAfterMs(res.headers?.get?.("retry-after"));
      throw new BlockSourceError(
        `NEAR Data temporary HTTP ${res.status} at finalized block #${height}`,
        res.status === 429 ? "RATE_LIMIT" : "UPSTREAM_5XX",
        { status: res.status, retryable: true, retryAfterMs },
      );
    }
    if (res.status === 401 || res.status === 403) {
      throw new BlockSourceError(
        `NEAR Data authentication failed (HTTP ${res.status}) at block #${height}.`,
        "AUTH_ERROR",
        { status: res.status, retryable: false },
      );
    }
    if (res.status === 404) {
      throw new BlockSourceError(
        `Finalized block #${height} unavailable on NEAR Data (HTTP 404).`,
        "BLOCK_NOT_FOUND",
        { status: 404, retryable: false, gapAtHeight: height },
      );
    }
    if (!res.ok) {
      throw new BlockSourceError(
        `NEAR Data returned HTTP ${res.status} at block #${height}`,
        "UPSTREAM_ERROR",
        { status: res.status, retryable: true },
      );
    }

    const text = await res.text();
    if (!text || text.trim() === "null") {
      throw new BlockSourceError(
        `NEAR Data returned null block payload for height #${height}.`,
        "BLOCK_NULL",
        { retryable: true, gapAtHeight: height },
      );
    }

    let data;
    try {
      data = JSON.parse(text);
    } catch {
      throw new BlockSourceError(
        `Malformed JSON block response from NEAR Data at height #${height}.`,
        "INVALID_JSON",
        { retryable: true },
      );
    }

    const header = data?.block?.header;
    if (!header || header.height !== height || typeof header.hash !== "string" || !header.hash) {
      throw new BlockSourceError(
        `Incomplete or mismatched finalized block header for height #${height}.`,
        "INVALID_BLOCK",
        { retryable: true },
      );
    }
    parseNanosecTimestampSec(header);
    if (!Array.isArray(data?.shards)) {
      throw new BlockSourceError(
        `Block #${height} payload missing shards[] array.`,
        "INVALID_BLOCK",
        { retryable: true },
      );
    }
    return data;
  }

  async function postRpcSingle(endpoint, method, params) {
    const safeUrl = assertTestnetRpcUrl(endpoint);
    const res = await fetchWithTimeout(
      safeUrl,
      {
        method: "POST",
        headers: buildAuthHeaders({
          "content-type": "application/json",
        }),
        body: JSON.stringify({
          jsonrpc: "2.0",
          id: `roastake-indexer-${method}`,
          method,
          params,
        }),
      },
      { metered: false },
    );
    // NEAR archival RPC returns a structured UNKNOWN_BLOCK with HTTP 422.
    if (method === "block" && res.status === 422) {
      const payload = await res.json().catch(() => null);
      if (payload?.jsonrpc === "2.0" &&
          payload.error?.code === -32000 &&
          payload.error?.cause?.name === "UNKNOWN_BLOCK") {
        throw new BlockSourceError("Archival RPC returned UNKNOWN_BLOCK", "UNKNOWN_BLOCK", { retryable: true });
      }
    }
    if (!res.ok) {
      const retryAfterMs = parseRetryAfterMs(res.headers?.get?.("retry-after"));
      throw new BlockSourceError(
        `RPC HTTP ${res.status} calling ${method}`,
        res.status === 429 ? "RATE_LIMIT" : "RPC_HTTP_ERROR",
        { status: res.status, retryable: res.status === 429 || res.status >= 500, retryAfterMs },
      );
    }
    const json = await res.json();
    if (json?.error) {
      throw new BlockSourceError(
        `RPC JSON error on ${method}: ${json.error.message ?? JSON.stringify(json.error)}`,
        method === "block" && json.error.cause?.name === "UNKNOWN_BLOCK" ? "UNKNOWN_BLOCK" : "RPC_ERROR",
        { retryable: true },
      );
    }
    if (!json?.result) {
      throw new BlockSourceError(`RPC returned empty result for ${method}`, "RPC_EMPTY", {
        retryable: true,
      });
    }
    return json.result;
  }

  async function callRpcWithFailover(method, params, endpoints = rpcEndpoints) {
    let lastError = null;
    for (const ep of endpoints) {
      try {
        return await postRpcSingle(ep, method, params);
      } catch (err) {
        lastError = err;
      }
    }
    throw lastError ?? new BlockSourceError(`All NEAR Testnet RPC endpoints failed on ${method}`, "RPC_FAILED");
  }

  /**
   * Fetch and verify M04.6-G backfill transaction finality + block header via RPC
   * (`EXPERIMENTAL_tx_status` with `wait_until: "FINAL"` + `block`), enforcing Testnet policy.
   */
  async function fetchBackfillTxWithBlock(txHash, senderAccountId) {
    const validSender = assertTestnetAccountId(senderAccountId, "backfill sender");
    // Historical seed transactions may have fallen outside regular RPC retention.
    // Try approved archival endpoints after regular RPC providers time out or prune data.
    const historicalRpcEndpoints = [...rpcEndpoints, ...archivalEndpoints];
    const txStatus = await callRpcWithFailover(
      "EXPERIMENTAL_tx_status",
      {
        tx_hash: txHash,
        sender_account_id: validSender,
        wait_until: "FINAL",
      },
      historicalRpcEndpoints,
    );

    const blockHash =
      txStatus?.receipts_outcome?.find((r) => r?.outcome?.executor_id === config.contractId)?.block_hash ??
      txStatus?.transaction_outcome?.block_hash ??
      null;
    if (!blockHash) {
      throw new BlockSourceError(
        `Backfill transaction ${txHash} missing finalized receipt/transaction block_hash.`,
        "BACKFILL_INCOMPLETE",
      );
    }
    const blockResult = await callRpcWithFailover(
      "block",
      { block_id: blockHash },
      historicalRpcEndpoints,
    );
    const header = blockResult?.header;
    if (!header || !Number.isSafeInteger(header.height) || !header.hash) {
      throw new BlockSourceError(
        `Backfill block lookup for ${blockHash} did not return valid block header.`,
        "BACKFILL_INCOMPLETE",
      );
    }
    parseNanosecTimestampSec(header);
    return { txStatus, blockHeader: header };
  }

  /**
   * Fetch a historical transaction once and verify the canonical block header for the
   * transaction plus every Perps-executed receipt. Receipt logs can be emitted in later
   * blocks than the originating transaction, so each receipt keeps its own block metadata.
   */
  async function fetchAccountHistoryTxWithBlocks(txHash, senderAccountId) {
    const validSender = assertTestnetAccountId(senderAccountId, "historical transaction signer");
    const historicalRpcEndpoints = [...rpcEndpoints, ...archivalEndpoints];
    const txStatus = await callRpcWithFailover(
      "EXPERIMENTAL_tx_status",
      {
        tx_hash: txHash,
        sender_account_id: validSender,
        wait_until: "FINAL",
      },
      historicalRpcEndpoints,
    );

    if (txStatus?.final_execution_status !== "FINAL" ||
        txStatus?.transaction?.hash !== txHash ||
        txStatus?.transaction?.signer_id !== validSender) {
      throw new BlockSourceError(
        `RPC final transaction identity/finality check failed for ${txHash}.`,
        "BACKFILL_INCOMPLETE",
        { retryable: true },
      );
    }

    const txBlockHash = txStatus?.transaction_outcome?.block_hash ?? null;
    const perpsReceiptBlockHashes = (txStatus?.receipts_outcome ?? [])
      .filter((receipt) => receipt?.outcome?.executor_id === config.contractId)
      .map((receipt) => receipt?.block_hash)
      .filter((hash) => typeof hash === "string" && hash.length > 0);
    const blockHashes = [...new Set([txBlockHash, ...perpsReceiptBlockHashes].filter(Boolean))];
    if (blockHashes.length === 0) {
      throw new BlockSourceError(
        `Final transaction ${txHash} is missing transaction/Perps receipt block hashes.`,
        "BACKFILL_INCOMPLETE",
        { retryable: true },
      );
    }

    const blockHeadersByHash = {};
    for (const blockHash of blockHashes) {
      const blockResult = await callRpcWithFailover(
        "block",
        { block_id: blockHash },
        historicalRpcEndpoints,
      );
      const header = blockResult?.header;
      if (!header || !Number.isSafeInteger(header.height) ||
          !header.hash || header.hash !== blockHash) {
        throw new BlockSourceError(
          `RPC block lookup for ${blockHash} did not return its matching canonical header.`,
          "BACKFILL_INCOMPLETE",
          { retryable: true },
        );
      }
      parseNanosecTimestampSec(header);
      blockHeadersByHash[blockHash] = header;
    }

    const txBlockHeader = txBlockHash ? blockHeadersByHash[txBlockHash] : null;
    if (!txBlockHeader) {
      throw new BlockSourceError(
        `Historical transaction ${txHash} has no verified transaction block header.`,
        "BACKFILL_INCOMPLETE",
        { retryable: true },
      );
    }
    const firstPerpsReceipt = (txStatus.receipts_outcome ?? [])
      .find((receipt) => receipt?.outcome?.executor_id === config.contractId);
    const blockHeader = firstPerpsReceipt?.block_hash
      ? blockHeadersByHash[firstPerpsReceipt.block_hash]
      : txBlockHeader;

    return {
      txStatus,
      blockHeader,
      txBlockHeader,
      blockHeadersByHash,
    };
  }

  /**
   * Explicitly check whether an archival RPC node is reachable AND retains an exact pruned block.
   * Never silently skips heights if unavailable.
   */
  async function verifyArchivalBlockAvailability(height) {
    let unknownCount = 0;
    for (const ep of archivalEndpoints) {
      try {
        const blk = await postRpcSingle(ep, "block", { block_id: height });
        if (blk?.header?.height === height && blk?.header?.hash) {
          return { available: true, status: "available", endpoint: ep, blockHeader: blk.header };
        }
      } catch (err) {
        if (err?.code === "UNKNOWN_BLOCK") unknownCount += 1;
      }
    }
    const confirmedUnknown = archivalEndpoints.length > 0 && unknownCount === archivalEndpoints.length;
    return {
      available: false,
      status: confirmedUnknown ? "unknown_block" : "unverified",
      blockHeader: null,
      reason: confirmedUnknown
        ? `All configured archival endpoints returned UNKNOWN_BLOCK for #${height}.`
        : `Configured archival RPC endpoints could not verify block #${height}.`,
    };
  }

  return {
    fetchLastFinalizedHeight,
    fetchFinalizedBlock,
    fetchBackfillTxWithBlock,
    fetchAccountHistoryTxWithBlocks,
    fetchAccountTransactionPage,
    fetchTransactionsByHash,
    verifyArchivalBlockAvailability,
    callRpcWithFailover,
  };
}
