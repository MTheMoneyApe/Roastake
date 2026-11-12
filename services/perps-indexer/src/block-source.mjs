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
        headers: { "content-type": "application/json", accept: "application/json" },
        body: JSON.stringify({
          jsonrpc: "2.0",
          id: `roastake-indexer-${method}`,
          method,
          params,
        }),
      },
      { metered: false },
    );
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
        "RPC_ERROR",
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
    const txStatus = await callRpcWithFailover("EXPERIMENTAL_tx_status", {
      tx_hash: txHash,
      sender_account_id: validSender,
      wait_until: "FINAL",
    });

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
    const blockResult = await callRpcWithFailover("block", { block_id: blockHash });
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
   * Explicitly check whether an archival RPC node is reachable AND retains an exact pruned block.
   * Never silently skips heights if unavailable.
   */
  async function verifyArchivalBlockAvailability(height) {
    if (archivalEndpoints.length === 0) {
      return { available: false, block: null, reason: "No approved archival RPC endpoints configured." };
    }
    for (const ep of archivalEndpoints) {
      try {
        const blk = await postRpcSingle(ep, "block", { block_id: height });
        if (blk?.header?.height === height && blk?.header?.hash) {
          return { available: true, endpoint: ep, blockHeader: blk.header };
        }
      } catch {
        // try next configured archival endpoint
      }
    }
    return {
      available: false,
      blockHeader: null,
      reason: `Configured archival RPC endpoints could not verify block #${height}.`,
    };
  }

  return {
    fetchLastFinalizedHeight,
    fetchFinalizedBlock,
    fetchBackfillTxWithBlock,
    verifyArchivalBlockAvailability,
    callRpcWithFailover,
  };
}
