// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import { parseNearDataFinalBlock, parseRpcTxStatusBackfillOutcome } from "./event-parser.mjs";
import {
  addScannedRangeToSegments,
  extractScannedSegmentsFromCursor,
  isHeightCoveredBySegments,
} from "./coverage.mjs";

function tallyRejections(target, rejections) {
  for (const r of rejections) {
    const key = r.reason ?? "unknown";
    target[key] = (target[key] ?? 0) + 1;
  }
}

/**
 * Import the 7 immutable verified M04.6-G Testnet transactions through RPC `EXPERIMENTAL_tx_status`
 * (`wait_until: "FINAL"`) + `block`, enforcing M04.12 receipt verification and persisting exact
 * block heights & timestamps with full idempotency via `HistoryStore`.
 */
export async function importM046GBackfill({
  config,
  marketSpec = null,
  blockSource,
  store,
  ignoredMarketIds = null,
  skipUnscopedProtocolEvents = false,
}) {
  const importedEvents = [];
  const rejected = [];
  const verifiedHashes = [];
  const blockHeights = [];
  let minHeight = null;
  let maxHeight = null;
  const targetMarketId = marketSpec?.marketId ?? config.marketId;
  const backfillTransactions = marketSpec?.backfillTransactions ?? config.backfillTransactions;

  for (const ptr of backfillTransactions) {
    const { txStatus, blockHeader } = await blockSource.fetchBackfillTxWithBlock(
      ptr.txHash,
      ptr.senderAccountId,
    );

    const parsed = parseRpcTxStatusBackfillOutcome({
      txHash: ptr.txHash,
      txStatus,
      blockHeader,
      contractId: config.contractId,
      marketId: targetMarketId,
      minPriceE8: marketSpec?.minPriceE8,
      maxPriceE8: marketSpec?.maxPriceE8,
      ignoredMarketIds,
      skipUnscopedProtocolEvents,
      source: "rpc_experimental_tx_status_backfill",
    });

    rejected.push(...parsed.rejected);
    if (parsed.events.length > 0) {
      verifiedHashes.push(ptr.txHash);
      if (!blockHeights.includes(blockHeader.height)) {
        blockHeights.push(blockHeader.height);
      }
      const { appended, duplicateRejections } = store.appendEvents(parsed.events);
      importedEvents.push(...appended);
      rejected.push(...duplicateRejections);

      if (minHeight === null || blockHeader.height < minHeight) minHeight = blockHeader.height;
      if (maxHeight === null || blockHeader.height > maxHeight) maxHeight = blockHeader.height;
    }
  }

  blockHeights.sort((a, b) => a - b);

  return {
    verifiedHashes,
    blockHeights,
    importedCount: importedEvents.length,
    importedEvents,
    rejected,
    backfillMinHeight: minHeight,
    backfillMaxHeight: maxHeight,
  };
}

/**
 * Execute a resumable, bounded historical finalized-block scan over `[fromHeight..toHeight]`
 * using FastNear NEAR Data (`/v0/block/{height}`):
 * - Skips heights already persisted in `cursor.scannedSegments`.
 * - Persists events (`store.appendEvents`) AND cursor segment progress (`store.saveCursor`)
 *   after every single verified block before advancing.
 * - Deduplicates against existing M04.6-G / forward events by `eventId` & oracle sequence.
 * - Stops without skipping heights if an error, 429 limit, or missing 404 block occurs.
 */
export async function reconcileHistoricalRange({
  config,
  marketSpec = null,
  blockSource,
  store,
  rateLimiter = null,
  ignoredMarketIds = null,
  skipUnscopedProtocolEvents = false,
  fromHeight,
  toHeight,
  maxBlocks = 250,
  maxDurationMs = 60_000,
  maxRateLimitRetries = 3,
  shouldStop = () => false,
  onProgress = null,
}) {
  if (!Number.isSafeInteger(fromHeight) || !Number.isSafeInteger(toHeight) || toHeight < fromHeight) {
    throw new Error(`Invalid backfill range "${fromHeight}..${toHeight}".`);
  }

  let cursor = store.getCursor();
  if (!cursor) {
    throw new Error("Cannot run historical range backfill before initial cursor bootstrap.");
  }

  let scannedSegments = extractScannedSegmentsFromCursor(cursor);
  const startedAtMs = Date.now();
  let scannedThisRun = 0;
  let skippedAlreadyCovered = 0;
  let appendedEventsCount = 0;
  let lastVerifiedHeight = null;
  let errorStop = null;
  let consecutive429s = 0;

  for (let h = fromHeight; h <= toHeight; h += 1) {
    if (shouldStop()) break;
    if (scannedThisRun >= maxBlocks) break;
    if (Date.now() - startedAtMs >= maxDurationMs) break;

    if (isHeightCoveredBySegments(scannedSegments, h)) {
      skippedAlreadyCovered += 1;
      continue;
    }

    let blockProcessed = false;
    while (!blockProcessed && !shouldStop()) {
      try {
        const blockPayload = await blockSource.fetchFinalizedBlock(h);
        const parsed = parseNearDataFinalBlock(blockPayload, {
          contractId: config.contractId,
          marketId: marketSpec?.marketId ?? config.marketId,
          minPriceE8: marketSpec?.minPriceE8,
          maxPriceE8: marketSpec?.maxPriceE8,
          ignoredMarketIds,
          skipUnscopedProtocolEvents,
          source: "neardata_range_backfill",
        });

        const { appended, duplicateRejections } = store.appendEvents(parsed.events);
        const allRejections = [...parsed.rejected, ...duplicateRejections];
        const rejectionsByReason = { ...(cursor.rejectionsByReason ?? {}) };
        tallyRejections(rejectionsByReason, allRejections);

        scannedSegments = addScannedRangeToSegments(scannedSegments, {
          startHeight: parsed.blockHeight,
          endHeight: parsed.blockHeight,
          source: "range_backfill",
        });

        cursor = store.saveCursor({
          ...cursor,
          scannedSegments,
          blocksProcessed: (cursor.blocksProcessed ?? 0) + 1,
          rejectedCount: (cursor.rejectedCount ?? 0) + allRejections.length,
          rejectionsByReason,
        });

        appendedEventsCount += appended.length;
        scannedThisRun += 1;
        lastVerifiedHeight = parsed.blockHeight;
        consecutive429s = 0;
        blockProcessed = true;

        if (typeof onProgress === "function") {
          onProgress({ height: parsed.blockHeight, scannedThisRun, appendedEventsCount });
        }
      } catch (err) {
        if (err?.code === "RATE_LIMIT" && consecutive429s < maxRateLimitRetries) {
          consecutive429s += 1;
          const waitMs = rateLimiter
            ? rateLimiter.recordRateLimit({
                consecutiveErrors: consecutive429s,
                retryAfterMs: err.retryAfterMs ?? null,
              })
            : Math.max(err.retryAfterMs ?? 0, 1_500);
          const deadline = Date.now() + waitMs;
          while (Date.now() < deadline && !shouldStop()) {
            await new Promise((r) => setTimeout(r, Math.min(250, deadline - Date.now())));
          }
          continue;
        }

        // Never advance cursor/scannedSegments past failed/missing height `h`.
        errorStop = {
          height: h,
          code: err?.code ?? "UPSTREAM_ERROR",
          status: err?.status ?? null,
          message: err?.message ?? "Range backfill interrupted.",
          retryAfterMs: err?.retryAfterMs ?? null,
        };
        break;
      }
    }

    if (errorStop) break;
  }

  return {
    fromHeight,
    toHeight,
    scannedThisRun,
    skippedAlreadyCovered,
    appendedEventsCount,
    lastVerifiedHeight,
    completedRange: !errorStop && isHeightCoveredBySegments(scannedSegments, toHeight),
    haltedAtHeight: errorStop?.height ?? null,
    error: errorStop,
    durationMs: Date.now() - startedAtMs,
  };
}
