// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import path from "node:path";
import { fileURLToPath } from "node:url";
import { loadIndexerConfig, resolveConfiguredMarket } from "./config.mjs";
import { createBlockSource, parseNanosecTimestampSec } from "./block-source.mjs";
import { parseNearDataFinalBlock } from "./event-parser.mjs";
import { createHistoryStore } from "./history-store.mjs";
import { importM046GBackfill, reconcileHistoricalRange } from "./backfill.mjs";
import { createIndexerHttpServer } from "./http-server.mjs";
import {
  addScannedRangeToSegments,
  buildCoverageSummary,
  extractScannedSegmentsFromCursor,
} from "./coverage.mjs";
import { createRateLimiter } from "./rate-limiter.mjs";

function tallyRejections(target, rejections) {
  for (const r of rejections) {
    const key = r.reason ?? "unknown";
    target[key] = (target[key] ?? 0) + 1;
  }
}

/**
 * Orchestrate verified NEAR Testnet multi-market indexer startup, bounded sequential catch-up,
 * per-market isolated HistoryStores, resumable range backfill, adaptive HTTP 429 + Retry-After +
 * jitter backoff, strict temporary-404 vs irreversible-404 handling, and local HTTP serving.
 */
export function createPerpsIndexerService(overrides = {}, deps = {}) {
  const config = loadIndexerConfig(overrides);
  const nowFn = deps.nowFn ?? (() => Date.now());
  const rateLimiter =
    deps.rateLimiter ??
    createRateLimiter(
      {
        maxRequestsPerMinute: config.maxRequestsPerMinute,
        minIntervalMs: config.blockPacingMs,
        jitterRatio: config.jitterRatio,
      },
      {
        randomFn: deps.randomFn,
        nowFn,
        sleepFn: deps.sleepFn,
      },
    );

  const usingInjectedSource = Boolean(deps.blockSource);
  const serviceState = {
    started: false,
    stopped: false,
    pollingActive: false,
    upstreamBackoffUntilMs: 0,
    upstreamConsecutiveErrors: 0,
  };

  const blockSource =
    deps.blockSource ??
    createBlockSource(config, {
      fetchFn: deps.fetchFn,
      rateLimiter,
      shouldStop: () => serviceState.stopped,
    });

  const allMarketIds = config.markets.map((m) => m.marketId);
  const multiMarketMode = config.markets.length > 1;
  let httpServer = null;

  function createPipeline(spec, index) {
    const isPrimary = index === 0;
    const rawStore =
      isPrimary && deps.store ? deps.store : createHistoryStore({ dataDir: spec.dataDir });

    let storeIsOpen = false;
    const wrappedStore = {
      ...rawStore,
      open() {
        rawStore.open();
        storeIsOpen = true;
      },
      close() {
        storeIsOpen = false;
        rawStore.close();
      },
    };

    const pState = {
      started: false,
      stopped: false,
      pollingActive: false,
      lastFinalizedHeight: null,
      lastFinalizedBlockTimestampSec: null,
      lastSuccessfulBlockAt: null,
      lastIndexedAt: null,
      providerStatus: "initializing",
      gapDetected: false,
      gapRange: null,
      consecutiveErrors: 0,
      backoffUntilMs: 0,
      lastErrorCode: null,
      lastErrorMessage: null,
      lastErrorAt: null,
      rejectedCount: 0,
    };

    const transient404Attempts = new Map();
    const ignoredMarketIds = new Set(allMarketIds.filter((id) => id !== spec.marketId));

    return {
      spec,
      isPrimary,
      store: wrappedStore,
      isStoreOpen: () => storeIsOpen,
      state: pState,
      transient404Attempts,
      ignoredMarketIds,
    };
  }

  const pipelines = config.markets.map((spec, idx) => createPipeline(spec, idx));
  const primaryPipeline = pipelines[0];
  let pollTimer = null;

  function resolvePipeline(marketKey) {
    if (!marketKey) return primaryPipeline;
    const matched = resolveConfiguredMarket(String(marketKey), config.markets);
    if (!matched) {
      throw new Error(`Unknown or unconfigured market "${marketKey}".`);
    }
    const found = pipelines.find((p) => p.spec.slug === matched.slug);
    if (!found) {
      throw new Error(`Pipeline not found for market "${marketKey}".`);
    }
    return found;
  }

  function persistHealth(pipeline) {
    const { spec, store, state } = pipeline;
    const cursor = store.getCursor();
    const coverage = buildCoverageSummary({
      cursor,
      runtime: state,
      backfillTxCount: spec.backfillTransactions.length,
    });
    const lagBlocks =
      state.lastFinalizedHeight !== null && coverage.indexedThroughHeight !== null
        ? Math.max(0, state.lastFinalizedHeight - coverage.indexedThroughHeight)
        : null;
    const rateLimit = rateLimiter.getStats();
    const hasActiveError = Boolean(state.lastErrorCode);
    const overallStatus =
      state.providerStatus === "healthy" && !hasActiveError && coverage.coverageComplete
        ? "healthy"
        : "degraded";

    store.saveHealth({
      status: overallStatus,
      providerStatus: state.providerStatus,
      storageStatus: "ok",
      network: config.network,
      contractId: config.contractId,
      marketId: spec.marketId,
      slug: spec.slug,
      urlSlug: spec.urlSlug,
      lastFinalizedHeight: state.lastFinalizedHeight,
      indexedThroughHeight: coverage.indexedThroughHeight,
      coverageStartHeight: coverage.coverageStartHeight,
      lagBlocks,
      blocksProcessed: coverage.blocksProcessed,
      coverageComplete: coverage.coverageComplete,
      coverageGaps: coverage.coverageGaps,
      coverageSegments: coverage.coverageSegments,
      seedBackfillRange: coverage.seedBackfillRange,
      forwardScanRange: coverage.forwardScanRange,
      backfillCompleted: coverage.backfillCompleted,
      backfillBlockRange: coverage.backfillBlockRange,
      lastSuccessfulBlockAt: state.lastSuccessfulBlockAt,
      lastIndexedAt: state.lastIndexedAt,
      lastErrorCode: state.lastErrorCode,
      lastErrorMessage: state.lastErrorMessage,
      lastErrorAt: state.lastErrorAt,
      rateLimit,
      gapDetected: coverage.gapDetected,
      gapRange: coverage.gapRange,
      rejectedCount: state.rejectedCount,
    });
  }

  function makeCycleBlockFetcher(cycleCache) {
    return async function fetchBlockCached(height) {
      if (cycleCache.has(height)) {
        return cycleCache.get(height);
      }
      const startedMs = nowFn();
      if (usingInjectedSource && config.blockPacingMs > 0) {
        await rateLimiter.acquireSlot(() => serviceState.stopped);
      }
      const payload = await blockSource.fetchFinalizedBlock(height);
      if (usingInjectedSource) {
        rateLimiter.recordSuccess(nowFn() - startedMs);
      }
      cycleCache.set(height, payload);
      return payload;
    };
  }

  async function processSingleFinalizedBlock(pipeline, height, cursorSnapshot, fetchBlockCached) {
    const { spec, store, state, transient404Attempts, ignoredMarketIds } = pipeline;
    const blockPayload = await fetchBlockCached(height);

    const parsed = parseNearDataFinalBlock(blockPayload, {
      contractId: config.contractId,
      marketId: spec.marketId,
      minPriceE8: spec.minPriceE8,
      maxPriceE8: spec.maxPriceE8,
      ignoredMarketIds: multiMarketMode ? ignoredMarketIds : null,
      skipUnscopedProtocolEvents: multiMarketMode,
    });

    const { duplicateRejections } = store.appendEvents(parsed.events);
    const allRejections = [...parsed.rejected, ...duplicateRejections];
    const rejectionsByReason = { ...(cursorSnapshot.rejectionsByReason ?? {}) };
    tallyRejections(rejectionsByReason, allRejections);

    const nextRejectedCount = (cursorSnapshot.rejectedCount ?? 0) + allRejections.length;
    const nowIso = new Date(nowFn()).toISOString();
    state.rejectedCount = nextRejectedCount;
    state.lastFinalizedBlockTimestampSec = parsed.blockTimestamp;
    state.lastSuccessfulBlockAt = nowIso;
    state.lastIndexedAt = nowIso;
    transient404Attempts.delete(height);

    const scannedSegments = addScannedRangeToSegments(
      extractScannedSegmentsFromCursor(cursorSnapshot),
      {
        startHeight: parsed.blockHeight,
        endHeight: parsed.blockHeight,
        source: "forward_scan",
      },
    );

    return store.saveCursor({
      ...cursorSnapshot,
      indexedThroughHeight: parsed.blockHeight,
      indexedThroughHash: parsed.blockHash,
      lastFinalizedHeight: state.lastFinalizedHeight ?? parsed.blockHeight,
      scannedSegments,
      blocksProcessed: (cursorSnapshot.blocksProcessed ?? 0) + 1,
      rejectedCount: nextRejectedCount,
      rejectionsByReason,
      lastSuccessfulBlockAt: nowIso,
    });
  }

  async function initializePipelineOnFirstBoot(pipeline, headHeight, fetchBlockCached) {
    const { spec, store, state, ignoredMarketIds } = pipeline;
    state.lastFinalizedHeight = headHeight;

    const headBlock = await fetchBlockCached(headHeight);
    const headTimestampSec = parseNanosecTimestampSec(headBlock.block.header);
    state.lastFinalizedBlockTimestampSec = headTimestampSec;

    const parsedHead = parseNearDataFinalBlock(headBlock, {
      contractId: config.contractId,
      marketId: spec.marketId,
      minPriceE8: spec.minPriceE8,
      maxPriceE8: spec.maxPriceE8,
      ignoredMarketIds: multiMarketMode ? ignoredMarketIds : null,
      skipUnscopedProtocolEvents: multiMarketMode,
    });

    const backfill = await importM046GBackfill({
      config,
      marketSpec: spec,
      blockSource,
      store,
      ignoredMarketIds: multiMarketMode ? ignoredMarketIds : null,
      skipUnscopedProtocolEvents: multiMarketMode,
    });
    const headAppend = store.appendEvents(parsedHead.events);

    const allRejections = [
      ...backfill.rejected,
      ...parsedHead.rejected,
      ...headAppend.duplicateRejections,
    ];
    const rejectionsByReason = {};
    tallyRejections(rejectionsByReason, allRejections);
    const nowIso = new Date(nowFn()).toISOString();
    state.rejectedCount = allRejections.length;
    state.lastSuccessfulBlockAt = nowIso;
    state.lastIndexedAt = nowIso;
    state.providerStatus = "healthy";
    state.lastErrorCode = null;
    state.lastErrorMessage = null;
    state.lastErrorAt = null;

    const initialCursor = {
      version: 2,
      network: config.network,
      contractId: config.contractId,
      marketId: spec.marketId,
      slug: spec.slug,
      coverageStartHeight: headHeight,
      indexedThroughHeight: headHeight,
      indexedThroughHash: headBlock.block.header.hash,
      lastFinalizedHeight: headHeight,
      backfillCompleted: backfill.verifiedHashes.length === spec.backfillTransactions.length,
      backfillTxHashes: backfill.verifiedHashes,
      backfillBlockHeights: backfill.blockHeights,
      backfillBlockRange:
        backfill.backfillMinHeight !== null
          ? { minHeight: backfill.backfillMinHeight, maxHeight: backfill.backfillMaxHeight }
          : null,
      scannedSegments: [
        { startHeight: headHeight, endHeight: headHeight, source: "forward_scan" },
      ],
      blocksProcessed: 1,
      rejectedCount: allRejections.length,
      rejectionsByReason,
      lastSuccessfulBlockAt: nowIso,
    };

    store.saveCursor(initialCursor);
    persistHealth(pipeline);
    return initialCursor;
  }

  function applyProviderErrorState(pipeline, err) {
    serviceState.upstreamConsecutiveErrors += 1;
    pipeline.state.consecutiveErrors += 1;
    const isRateLimit = err?.code === "RATE_LIMIT" || err?.status === 429;
    const delayMs = rateLimiter.recordRateLimit({
      consecutiveErrors: serviceState.upstreamConsecutiveErrors,
      retryAfterMs: err?.retryAfterMs ?? null,
      isRateLimit,
    });
    const untilMs = nowFn() + delayMs;
    serviceState.upstreamBackoffUntilMs = untilMs;
    pipeline.state.backoffUntilMs = untilMs;
    if (!pipeline.state.gapDetected) {
      pipeline.state.providerStatus = "degraded";
    }
    pipeline.state.lastErrorCode = err?.code ?? "UPSTREAM_ERROR";
    pipeline.state.lastErrorMessage = err?.message ?? "Unknown upstream provider error.";
    pipeline.state.lastErrorAt = new Date(nowFn()).toISOString();
    persistHealth(pipeline);
    return delayMs;
  }

  async function pollSinglePipeline(pipeline, finalHeight, fetchBlockCached) {
    const { spec, store, state, transient404Attempts, ignoredMarketIds } = pipeline;
    let cursor = store.getCursor();
    if (!cursor) {
      cursor = await initializePipelineOnFirstBoot(pipeline, finalHeight, fetchBlockCached);
    } else if (!cursor.backfillCompleted && spec.backfillTransactions.length > 0) {
      const backfill = await importM046GBackfill({
        config,
        marketSpec: spec,
        blockSource,
        store,
        ignoredMarketIds: multiMarketMode ? ignoredMarketIds : null,
        skipUnscopedProtocolEvents: multiMarketMode,
      });
      const rejectionsByReason = { ...(cursor.rejectionsByReason ?? {}) };
      tallyRejections(rejectionsByReason, backfill.rejected);
      cursor = store.saveCursor({
        ...cursor,
        backfillCompleted: backfill.verifiedHashes.length === spec.backfillTransactions.length,
        backfillTxHashes: backfill.verifiedHashes,
        backfillBlockHeights: backfill.blockHeights,
        backfillBlockRange:
          backfill.backfillMinHeight !== null
            ? { minHeight: backfill.backfillMinHeight, maxHeight: backfill.backfillMaxHeight }
            : cursor.backfillBlockRange ?? null,
        rejectedCount: (cursor.rejectedCount ?? 0) + backfill.rejected.length,
        rejectionsByReason,
      });
    }

    state.lastFinalizedHeight = finalHeight;
    cursor = store.saveCursor({ ...cursor, lastFinalizedHeight: finalHeight });

    if (finalHeight <= cursor.indexedThroughHeight) {
      state.consecutiveErrors = 0;
      serviceState.upstreamConsecutiveErrors = 0;
      if (!state.gapDetected) {
        state.providerStatus = "healthy";
        state.lastErrorCode = null;
        state.lastErrorMessage = null;
      }
      persistHealth(pipeline);
      return {
        skipped: false,
        processedBlocks: 0,
        indexedThroughHeight: cursor.indexedThroughHeight,
        lastFinalizedHeight: finalHeight,
        lagBlocks: 0,
      };
    }

    const cycleStartMs = nowFn();
    const targetHeight = Math.min(
      finalHeight,
      cursor.indexedThroughHeight + config.maxBlocksPerCycle,
    );
    let processedBlocks = 0;

    for (let h = cursor.indexedThroughHeight + 1; h <= targetHeight; h += 1) {
      if (serviceState.stopped) break;
      if (processedBlocks > 0 && nowFn() - cycleStartMs >= config.maxCycleDurationMs) {
        break;
      }

      try {
        cursor = await processSingleFinalizedBlock(pipeline, h, cursor, fetchBlockCached);
        processedBlocks += 1;
        state.consecutiveErrors = 0;
        serviceState.upstreamConsecutiveErrors = 0;
        state.backoffUntilMs = 0;
        serviceState.upstreamBackoffUntilMs = 0;
        state.gapDetected = false;
        state.gapRange = null;
        state.providerStatus = "healthy";
        state.lastErrorCode = null;
        state.lastErrorMessage = null;
      } catch (err) {
        if (err?.code === "BLOCK_NOT_FOUND") {
          const archival = await blockSource.verifyArchivalBlockAvailability(h);
          const isNearTip =
            state.lastFinalizedHeight !== null &&
            state.lastFinalizedHeight - h <= config.nearTip404ToleranceBlocks;
          const prevAttempts = transient404Attempts.get(h) ?? 0;

          if ((isNearTip || archival.available) && prevAttempts < config.maxTransient404Retries) {
            transient404Attempts.set(h, prevAttempts + 1);
            const tempErr = new Error(
              archival.available
                ? `Temporary NEAR Data 404 at finalized block #${h} (confirmed on archival RPC; attempt ${prevAttempts + 1}/${config.maxTransient404Retries}).`
                : `Temporary tip propagation 404 at finalized block #${h} (attempt ${prevAttempts + 1}/${config.maxTransient404Retries}).`,
            );
            tempErr.code = archival.available ? "UPSTREAM_TEMP_BLOCK_404" : "TEMP_TIP_BLOCK_404";
            const delayMs = applyProviderErrorState(pipeline, tempErr);
            return {
              skipped: false,
              processedBlocks,
              temporaryNotFoundAtHeight: h,
              indexedThroughHeight: cursor.indexedThroughHeight,
              error: tempErr.message,
              errorCode: tempErr.code,
              backoffMs: delayMs,
            };
          }

          state.gapDetected = true;
          state.gapRange = {
            fromHeight: h,
            toHeight: finalHeight,
            reason: archival.reason,
          };
          state.providerStatus = "halted_historical_gap";
          state.lastErrorCode = "BLOCK_NOT_FOUND";
          state.lastErrorMessage = `Finalized block #${h} unavailable (HTTP 404) and unverified via archival RPC.`;
          state.lastErrorAt = new Date(nowFn()).toISOString();
          persistHealth(pipeline);
          return {
            skipped: false,
            processedBlocks,
            haltedAtHeight: h,
            indexedThroughHeight: cursor.indexedThroughHeight,
            gapDetected: true,
          };
        }
        throw err;
      }
    }

    persistHealth(pipeline);
    return {
      skipped: false,
      processedBlocks,
      indexedThroughHeight: cursor.indexedThroughHeight,
      lastFinalizedHeight: finalHeight,
      lagBlocks: Math.max(0, finalHeight - cursor.indexedThroughHeight),
    };
  }

  async function pollOnce(options = {}) {
    if (serviceState.stopped || serviceState.pollingActive) {
      return {
        skipped: true,
        reason: serviceState.stopped ? "stopped" : "concurrent_poll_blocked",
      };
    }
    if (nowFn() < serviceState.upstreamBackoffUntilMs) {
      return {
        skipped: true,
        reason: "backoff_active",
        backoffRemainingMs: serviceState.upstreamBackoffUntilMs - nowFn(),
      };
    }

    const activePipelines = options.market
      ? [resolvePipeline(options.market)].filter((p) => p.isStoreOpen())
      : pipelines.filter((p) => p.isStoreOpen());

    if (activePipelines.length === 0) {
      return { skipped: true, reason: "no_open_market_stores" };
    }

    serviceState.pollingActive = true;
    for (const p of activePipelines) p.state.pollingActive = true;

    try {
      const cycleCache = new Map();
      const fetchBlockCached = makeCycleBlockFetcher(cycleCache);
      const finalHeight = await blockSource.fetchLastFinalizedHeight();
      const byMarket = {};

      for (const pipeline of activePipelines) {
        if (serviceState.stopped) break;
        if (nowFn() < serviceState.upstreamBackoffUntilMs) break;
        try {
          byMarket[pipeline.spec.slug] = await pollSinglePipeline(
            pipeline,
            finalHeight,
            fetchBlockCached,
          );
        } catch (err) {
          const delayMs = applyProviderErrorState(pipeline, err);
          console.error(
            `[perps-indexer:${pipeline.spec.slug}] Poll error #${pipeline.state.consecutiveErrors} (${pipeline.state.lastErrorCode}, backoff ${delayMs}ms): ${err?.message ?? "unknown error"}`,
          );
          byMarket[pipeline.spec.slug] = {
            skipped: false,
            error: err?.message ?? "unknown error",
            errorCode: pipeline.state.lastErrorCode,
            backoffMs: delayMs,
          };
          // Stop advancing sibling pipelines during an upstream outage/rate-limit backoff window.
          break;
        }
      }

      const primaryKey = options.market
        ? resolvePipeline(options.market).spec.slug
        : primaryPipeline.spec.slug;
      const primaryResult =
        byMarket[primaryKey] ?? Object.values(byMarket)[0] ?? { skipped: false, processedBlocks: 0 };
      return {
        ...primaryResult,
        byMarket,
      };
    } catch (err) {
      const target = activePipelines[0] ?? primaryPipeline;
      const delayMs = applyProviderErrorState(target, err);
      for (const p of activePipelines) {
        if (p !== target) {
          p.state.backoffUntilMs = serviceState.upstreamBackoffUntilMs;
          p.state.providerStatus = target.state.providerStatus;
          p.state.lastErrorCode = target.state.lastErrorCode;
          p.state.lastErrorMessage = target.state.lastErrorMessage;
          p.state.lastErrorAt = target.state.lastErrorAt;
          persistHealth(p);
        }
      }
      console.error(
        `[perps-indexer] Poll error #${serviceState.upstreamConsecutiveErrors} (${target.state.lastErrorCode}, backoff ${delayMs}ms): ${err?.message ?? "unknown error"}`,
      );
      return {
        skipped: false,
        error: err?.message ?? "unknown error",
        errorCode: target.state.lastErrorCode,
        backoffMs: delayMs,
      };
    } finally {
      serviceState.pollingActive = false;
      for (const p of activePipelines) p.state.pollingActive = false;
    }
  }

  /**
   * Drain queued finalized blocks sequentially up to configurable `maxBlocks` and `maxDurationMs`
   * without overloading FastNear limits. Returns exact progress and residual lag.
   */
  async function catchUp({
    market = null,
    maxBlocks = config.maxCatchupBlocksPerRun,
    maxDurationMs = 120_000,
    stopOnError = false,
  } = {}) {
    const startedMs = nowFn();
    let totalProcessed = 0;
    let lastResult = null;
    const targetPipelines = market
      ? [resolvePipeline(market)]
      : pipelines.filter((p) => p.isStoreOpen());

    while (!serviceState.stopped && totalProcessed < maxBlocks && nowFn() - startedMs < maxDurationMs) {
      if (nowFn() < serviceState.upstreamBackoffUntilMs) {
        if (stopOnError) break;
        const waitMs = Math.min(
          serviceState.upstreamBackoffUntilMs - nowFn(),
          Math.max(0, maxDurationMs - (nowFn() - startedMs)),
        );
        if (waitMs <= 0) break;
        const sleepFn = deps.sleepFn ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
        await sleepFn(waitMs);
      }

      lastResult = await pollOnce({ market });
      const batchProcessed = Object.values(lastResult.byMarket ?? {}).reduce(
        (acc, r) => acc + (r?.processedBlocks ?? 0),
        0,
      );
      totalProcessed += batchProcessed || (lastResult.processedBlocks ?? 0);
      if (lastResult.gapDetected) break;
      if (lastResult.error) {
        if (stopOnError) break;
        continue;
      }

      const allCaughtUp = targetPipelines.every((p) => {
        const c = p.store.getCursor();
        return (
          c &&
          p.state.lastFinalizedHeight !== null &&
          c.indexedThroughHeight >= p.state.lastFinalizedHeight
        );
      });
      if (allCaughtUp) break;
    }

    const reportPipeline = market ? resolvePipeline(market) : primaryPipeline;
    const finalCursor = reportPipeline.store.getCursor();
    const lagBlocks =
      reportPipeline.state.lastFinalizedHeight !== null && finalCursor?.indexedThroughHeight !== null
        ? Math.max(0, reportPipeline.state.lastFinalizedHeight - finalCursor.indexedThroughHeight)
        : null;

    return {
      totalProcessed,
      indexedThroughHeight: finalCursor?.indexedThroughHeight ?? null,
      lastFinalizedHeight: reportPipeline.state.lastFinalizedHeight,
      lagBlocks,
      gapDetected: reportPipeline.state.gapDetected,
      lastResult,
      durationMs: nowFn() - startedMs,
    };
  }

  /**
   * Resumably scan a historical finalized block interval `[fromHeight..toHeight]` in bounded chunks
   * for a specific market (`market` defaults to primary / Arabica).
   */
  async function runHistoricalRangeBackfill({
    market = null,
    fromHeight,
    toHeight,
    maxBlocks = config.maxCatchupBlocksPerRun,
    maxDurationMs = 90_000,
    onProgress = null,
  }) {
    const pipeline = resolvePipeline(market);
    if (!pipeline.store.getCursor()) {
      await pollOnce({ market: pipeline.spec.slug });
    }
    const res = await reconcileHistoricalRange({
      config,
      marketSpec: pipeline.spec,
      blockSource,
      store: pipeline.store,
      rateLimiter,
      ignoredMarketIds: multiMarketMode ? pipeline.ignoredMarketIds : null,
      skipUnscopedProtocolEvents: multiMarketMode,
      fromHeight,
      toHeight,
      maxBlocks,
      maxDurationMs,
      shouldStop: () => serviceState.stopped,
      onProgress,
    });
    if (res.error) {
      pipeline.state.providerStatus =
        res.error.code === "BLOCK_NOT_FOUND" ? "halted_historical_gap" : "degraded";
      pipeline.state.lastErrorCode = res.error.code;
      pipeline.state.lastErrorMessage = res.error.message;
      pipeline.state.lastErrorAt = new Date(nowFn()).toISOString();
    } else {
      pipeline.state.providerStatus = "healthy";
      pipeline.state.lastErrorCode = null;
      pipeline.state.lastErrorMessage = null;
      if (res.scannedThisRun > 0) {
        pipeline.state.lastSuccessfulBlockAt = new Date(nowFn()).toISOString();
      }
    }
    persistHealth(pipeline);
    return res;
  }

  function scheduleNextPoll() {
    if (serviceState.stopped) return;
    if (pollTimer) clearTimeout(pollTimer);
    const hasBacklog = pipelines.some((p) => {
      if (!p.isStoreOpen()) return false;
      const c = p.store.getCursor();
      return (
        c &&
        p.state.lastFinalizedHeight !== null &&
        c.indexedThroughHeight < p.state.lastFinalizedHeight &&
        !p.state.gapDetected &&
        !p.state.lastErrorCode
      );
    });
    const backoffRemaining = Math.max(0, serviceState.upstreamBackoffUntilMs - nowFn());
    const nextDelayMs =
      backoffRemaining > 0
        ? backoffRemaining + 25
        : hasBacklog
          ? 25
          : config.pollIntervalMs;

    pollTimer = setTimeout(() => {
      void pollOnce().finally(() => {
        if (!serviceState.stopped) scheduleNextPoll();
      });
    }, nextDelayMs);
  }

  async function start({
    startHttp = true,
    startPolling = true,
    skipInitialPoll = false,
    onlyMarket = null,
  } = {}) {
    if (serviceState.started) {
      return { address: httpServer ? httpServer.server.address() : null };
    }
    serviceState.stopped = false;
    const targets = onlyMarket ? [resolvePipeline(onlyMarket)] : pipelines;

    for (const pipeline of targets) {
      pipeline.store.open();
      pipeline.state.stopped = false;
      pipeline.state.started = true;
      const existingCursor = pipeline.store.getCursor();
      if (existingCursor) {
        pipeline.state.rejectedCount = existingCursor.rejectedCount ?? 0;
        pipeline.state.lastFinalizedHeight = existingCursor.lastFinalizedHeight ?? null;
        pipeline.state.lastSuccessfulBlockAt =
          existingCursor.lastSuccessfulBlockAt ?? existingCursor.updatedAt ?? null;
        pipeline.state.lastIndexedAt = existingCursor.updatedAt ?? null;
      }
    }

    if (startHttp) {
      httpServer = createIndexerHttpServer({
        config,
        getMarketContexts: () =>
          targets.map((p) => ({
            spec: p.spec,
            store: p.store,
            getRuntimeState: () => ({
              ...p.state,
              rateLimit: rateLimiter.getStats(),
            }),
          })),
      });
      const address = await httpServer.listen();
      serviceState.started = true;
      if (!skipInitialPoll) {
        await pollOnce({ market: onlyMarket });
      }
      if (startPolling && !serviceState.stopped) {
        scheduleNextPoll();
      }
      return { address };
    }

    serviceState.started = true;
    if (!skipInitialPoll) {
      await pollOnce({ market: onlyMarket });
    }
    if (startPolling && !serviceState.stopped) {
      scheduleNextPoll();
    }
    return { address: null };
  }

  async function stop() {
    serviceState.stopped = true;
    for (const p of pipelines) p.state.stopped = true;
    if (pollTimer) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
    while (serviceState.pollingActive) {
      await new Promise((r) => setTimeout(r, 25));
    }
    if (httpServer) {
      await httpServer.close();
      httpServer = null;
    }
    for (const p of pipelines) {
      if (p.isStoreOpen()) {
        p.store.close();
      }
      p.state.started = false;
    }
    serviceState.started = false;
  }

  return {
    config,
    blockSource,
    store: primaryPipeline.store,
    rateLimiter,
    pollOnce,
    catchUp,
    runHistoricalRangeBackfill,
    start,
    stop,
    getState: () => ({ ...primaryPipeline.state, rateLimit: rateLimiter.getStats() }),
    getMarketStore: (marketKey) => resolvePipeline(marketKey).store,
    getMarketState: (marketKey) => ({
      ...resolvePipeline(marketKey).state,
      rateLimit: rateLimiter.getStats(),
    }),
    getAllMarketStates: () =>
      pipelines.map((p) => ({
        slug: p.spec.slug,
        marketId: p.spec.marketId,
        ...p.state,
        rateLimit: rateLimiter.getStats(),
      })),
  };
}

function parseCliFlagValue(argv, prefix) {
  const hit = argv.find((a) => a.startsWith(prefix));
  return hit ? hit.slice(prefix.length) : null;
}

const isDirectEntry =
  Boolean(process.argv[1]) &&
  path.resolve(process.argv[1]) === path.resolve(fileURLToPath(import.meta.url));

if (isDirectEntry || process.argv.includes("--run-cli")) {
  const argv = process.argv.slice(2);
  const onceMode = argv.includes("--once");
  const catchUpMode = argv.includes("--catch-up");
  const targetMarketArg = parseCliFlagValue(argv, "--market=");
  const backfillRangeRaw = parseCliFlagValue(argv, "--backfill-range=");
  const maxBlocksArg = parseCliFlagValue(argv, "--max-blocks=");
  const maxDurationArg = parseCliFlagValue(argv, "--max-duration-ms=");

  const service = createPerpsIndexerService();

  const handleSignal = async (signal) => {
    console.log(`[perps-indexer] Received ${signal}, flushing cursors & shutting down...`);
    await service.stop();
    process.exit(0);
  };

  process.on("SIGINT", () => void handleSignal("SIGINT"));
  process.on("SIGTERM", () => void handleSignal("SIGTERM"));

  const cliOneShot = onceMode || Boolean(backfillRangeRaw);
  const skipInitialPoll = Boolean(backfillRangeRaw && !catchUpMode);

  service
    .start({
      startHttp: !cliOneShot,
      startPolling: !cliOneShot,
      skipInitialPoll,
      onlyMarket: cliOneShot ? targetMarketArg : null,
    })
    .then(async ({ address }) => {
      if (backfillRangeRaw) {
        const match = backfillRangeRaw.match(/^(\d+)\.\.(HEAD|\d+)$/i);
        if (!match) {
          throw new Error(`Invalid --backfill-range="${backfillRangeRaw}". Expected START..END or START..HEAD.`);
        }
        const fromHeight = Number.parseInt(match[1], 10);
        const toHeight =
          match[2].toUpperCase() === "HEAD"
            ? await service.blockSource.fetchLastFinalizedHeight()
            : Number.parseInt(match[2], 10);
        const maxBlocks = maxBlocksArg ? Number.parseInt(maxBlocksArg, 10) : 250;
        const maxDurationMs = maxDurationArg ? Number.parseInt(maxDurationArg, 10) : 120_000;
        const rangeRes = await service.runHistoricalRangeBackfill({
          market: targetMarketArg,
          fromHeight,
          toHeight,
          maxBlocks,
          maxDurationMs,
        });
        console.log(`[perps-indexer] Backfill batch result: ${JSON.stringify(rangeRes)}`);
      }

      if (catchUpMode && cliOneShot) {
        const maxBlocks = maxBlocksArg ? Number.parseInt(maxBlocksArg, 10) : service.config.maxCatchupBlocksPerRun;
        const maxDurationMs = maxDurationArg ? Number.parseInt(maxDurationArg, 10) : 180_000;
        const cuRes = await service.catchUp({
          market: targetMarketArg,
          maxBlocks,
          maxDurationMs,
        });
        console.log(`[perps-indexer] Catch-up result: ${JSON.stringify(cuRes)}`);
      }

      const activeMarkets = cliOneShot && targetMarketArg
        ? [resolveConfiguredMarket(targetMarketArg, service.config.markets)].filter(Boolean)
        : service.config.markets;
      for (const spec of activeMarkets) {
        const mStore = service.getMarketStore(spec.slug);
        const mState = service.getMarketState(spec.slug);
        const c = mStore.getCursor();
        const s = mStore.getStats();
        const cov = buildCoverageSummary({
          cursor: c,
          runtime: mState,
          backfillTxCount: spec.backfillTransactions.length,
        });
        console.log(
          `[perps-indexer:${spec.slug}] ${cliOneShot ? "Completed" : `Started on http://${address.address}:${address.port}`} | forward=${cov.forwardScanRange?.startHeight}..${cov.forwardScanRange?.endHeight} | gaps=${cov.coverageGaps.length} (${cov.totalGapBlocks} blk) | lag=${mState.lastFinalizedHeight !== null && c ? Math.max(0, mState.lastFinalizedHeight - c.indexedThroughHeight) : "?"} | events=${s.eventCount} | obs=${s.observationCount} | execs=${s.executionCount}`,
        );
      }

      if (cliOneShot) {
        await service.stop();
      }
    })
    .catch(async (err) => {
      console.error(`[perps-indexer] Fatal startup error: ${err?.message ?? err}`);
      await service.stop();
      process.exit(1);
    });
}
