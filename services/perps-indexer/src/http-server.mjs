// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import http from "node:http";
import { ORACLE_MAX_AGE_SEC, resolveConfiguredMarket } from "./config.mjs";
import { buildCoverageSummary } from "./coverage.mjs";
import {
  VALID_OHLC_TIMEFRAMES,
  buildAllTimeframeOhlcSummary,
  buildExecutionVolume,
  buildOracleSampleOhlc,
} from "./ohlc-builder.mjs";

const MAX_RESPONSE_LIMIT = 1_000;
const VALID_HISTORY_TIMEFRAMES = new Set(["ALL", ...VALID_OHLC_TIMEFRAMES]);
const MARKET_ACTIONS = new Set([
  "health",
  "observations",
  "executions",
  "candles",
  "volume",
  "history",
]);

function buildCorsHeaders(origin, allowedOrigins) {
  if (!origin) return {};
  if (!allowedOrigins.includes(origin)) return null;
  return {
    "access-control-allow-origin": origin,
    "access-control-allow-methods": "GET, OPTIONS",
    "access-control-allow-headers": "content-type",
    vary: "Origin",
  };
}

function sendJson(res, statusCode, payload, corsHeaders = {}) {
  const body = JSON.stringify(payload);
  res.writeHead(statusCode, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
    ...corsHeaders,
  });
  res.end(body);
}

function parseLimitParam(rawLimit, fallback = 500) {
  if (rawLimit === null || rawLimit === "") return fallback;
  if (!/^\d+$/.test(rawLimit)) {
    throw new Error(`Invalid limit query parameter "${rawLimit}".`);
  }
  const n = Number.parseInt(rawLimit, 10);
  if (!Number.isSafeInteger(n) || n < 1 || n > MAX_RESPONSE_LIMIT) {
    throw new Error(`limit must be an integer between 1 and ${MAX_RESPONSE_LIMIT}.`);
  }
  return n;
}

function computeFreshness(observations, maxAgeSec, nowSecOverride = null) {
  const latest = observations.length > 0 ? observations[observations.length - 1] : null;
  const nowSec = nowSecOverride ?? Math.floor(Date.now() / 1_000);
  if (!latest) {
    return {
      latestObservation: null,
      latestAgeSec: null,
      stale: false,
      maxAgeSec,
      referenceNowSec: nowSec,
    };
  }
  const obsSec = Number(latest.data.observed_at_sec);
  const ageSec = Math.max(0, nowSec - obsSec);
  return {
    latestObservation: latest,
    latestAgeSec: ageSec,
    stale: ageSec > maxAgeSec,
    maxAgeSec,
    referenceNowSec: nowSec,
  };
}

function buildMarketTelemetry(ctx, config) {
  const maxAgeSec = config.oracleMaxAgeSec ?? ORACLE_MAX_AGE_SEC;
  const runtime = ctx.getRuntimeState();
  const cursor = ctx.store.getCursor();
  const stats = ctx.store.getStats();
  const observations = ctx.store.getOracleObservations();
  const executions = ctx.store.getExecutions();
  const allEvents = ctx.store.getAllEvents();
  const freshness = computeFreshness(
    observations,
    maxAgeSec,
    runtime.lastFinalizedBlockTimestampSec ?? null,
  );

  const lastFinalizedHeight = runtime.lastFinalizedHeight ?? cursor?.lastFinalizedHeight ?? null;
  const indexedThroughHeight = cursor?.indexedThroughHeight ?? null;
  const lagBlocks =
    lastFinalizedHeight !== null && indexedThroughHeight !== null
      ? Math.max(0, lastFinalizedHeight - indexedThroughHeight)
      : null;

  const coverage = buildCoverageSummary({
    cursor,
    runtime,
    backfillTxCount: ctx.spec.backfillTransactions.length,
  });

  const providerStatus = runtime.providerStatus ?? "initializing";
  const storageStatus = "ok";
  const hasActiveError = Boolean(runtime.lastErrorCode);
  const overallStatus =
    providerStatus === "healthy" &&
    storageStatus === "ok" &&
    !hasActiveError &&
    coverage.coverageComplete
      ? "healthy"
      : "degraded";

  const healthPayload = {
    status: overallStatus,
    providerStatus,
    storageStatus,
    network: config.network,
    contractId: config.contractId,
    marketId: ctx.spec.marketId,
    slug: ctx.spec.slug,
    urlSlug: ctx.spec.urlSlug,
    feedClassification: ctx.spec.feedClassification ?? "TESTNET_SANDBOX_FEED",
    isSynthetic: ctx.spec.isSynthetic ?? true,
    lastFinalizedHeight,
    indexedThroughHeight,
    indexedThroughHash: coverage.indexedThroughHash,
    lagBlocks,
    blocksProcessed: coverage.blocksProcessed,
    coverageStartHeight: coverage.coverageStartHeight,
    coverageComplete: coverage.coverageComplete,
    coverageGaps: coverage.coverageGaps,
    coverageSegments: coverage.coverageSegments,
    seedBackfillRange: coverage.seedBackfillRange,
    forwardScanRange: coverage.forwardScanRange,
    backfillCompleted: coverage.backfillCompleted,
    backfillBlockRange: coverage.backfillBlockRange,
    lastSuccessfulBlockAt:
      runtime.lastSuccessfulBlockAt ?? cursor?.lastSuccessfulBlockAt ?? cursor?.updatedAt ?? null,
    lastIndexedAt: runtime.lastIndexedAt ?? cursor?.updatedAt ?? null,
    lastErrorCode: runtime.lastErrorCode ?? null,
    lastErrorMessage: runtime.lastErrorMessage ?? null,
    lastErrorAt: runtime.lastErrorAt ?? null,
    rateLimit: runtime.rateLimit ?? null,
    gapDetected: coverage.gapDetected,
    gapRange: coverage.gapRange,
    stale: freshness.stale,
    latestObservationAgeSec: freshness.latestAgeSec,
    oracleMaxAgeSec: maxAgeSec,
    eventCount: stats.eventCount,
    observationCount: stats.observationCount,
    executionCount: stats.executionCount,
    rejectedCount: cursor?.rejectedCount ?? runtime.rejectedCount ?? 0,
    rejectionsByReason: cursor?.rejectionsByReason ?? {},
  };

  return {
    ctx,
    runtime,
    cursor,
    stats,
    observations,
    executions,
    allEvents,
    freshness,
    lastFinalizedHeight,
    indexedThroughHeight,
    lagBlocks,
    coverage,
    providerStatus,
    overallStatus,
    maxAgeSec,
    healthPayload,
  };
}

function handleMarketAction({ action, telemetry, urlObj, res, corsHeaders, config }) {
  const { observations, executions, allEvents, freshness, coverage, runtime, cursor } = telemetry;

  if (action === "health") {
    sendJson(res, 200, telemetry.healthPayload, corsHeaders ?? {});
    return;
  }

  if (action === "observations") {
    let limit;
    try {
      limit = parseLimitParam(urlObj.searchParams.get("limit"), 500);
    } catch (err) {
      sendJson(res, 400, { error: err.message }, corsHeaders ?? {});
      return;
    }
    const sliced = observations.slice(-limit);
    sendJson(
      res,
      200,
      {
        network: config.network,
        contractId: config.contractId,
        marketId: telemetry.ctx.spec.marketId,
        slug: telemetry.ctx.spec.slug,
        urlSlug: telemetry.ctx.spec.urlSlug,
        feedClassification: telemetry.ctx.spec.feedClassification ?? "TESTNET_SANDBOX_FEED",
        observationCount: sliced.length,
        totalObservations: observations.length,
        observations: sliced,
      },
      corsHeaders ?? {},
    );
    return;
  }

  if (action === "executions") {
    let limit;
    try {
      limit = parseLimitParam(urlObj.searchParams.get("limit"), 500);
    } catch (err) {
      sendJson(res, 400, { error: err.message }, corsHeaders ?? {});
      return;
    }
    const sliced = executions.slice(-limit);
    sendJson(
      res,
      200,
      {
        network: config.network,
        contractId: config.contractId,
        marketId: telemetry.ctx.spec.marketId,
        slug: telemetry.ctx.spec.slug,
        urlSlug: telemetry.ctx.spec.urlSlug,
        executionCount: sliced.length,
        totalExecutions: executions.length,
        executions: sliced,
      },
      corsHeaders ?? {},
    );
    return;
  }

  if (action === "candles") {
    const timeframe = urlObj.searchParams.get("timeframe") ?? "5m";
    if (!VALID_OHLC_TIMEFRAMES.includes(timeframe)) {
      sendJson(
        res,
        400,
        { error: `Invalid timeframe "${timeframe}". Allowed: ${VALID_OHLC_TIMEFRAMES.join(", ")}` },
        corsHeaders ?? {},
      );
      return;
    }
    const result = buildOracleSampleOhlc(observations, timeframe);
    sendJson(
      res,
      200,
      {
        network: config.network,
        contractId: config.contractId,
        marketId: telemetry.ctx.spec.marketId,
        slug: telemetry.ctx.spec.slug,
        urlSlug: telemetry.ctx.spec.urlSlug,
        ...result,
      },
      corsHeaders ?? {},
    );
    return;
  }

  if (action === "volume") {
    const timeframe = urlObj.searchParams.get("timeframe") ?? "5m";
    if (!VALID_HISTORY_TIMEFRAMES.has(timeframe)) {
      sendJson(
        res,
        400,
        { error: `Invalid timeframe "${timeframe}".` },
        corsHeaders ?? {},
      );
      return;
    }
    const vol = buildExecutionVolume(allEvents, timeframe);
    sendJson(
      res,
      200,
      {
        network: config.network,
        contractId: config.contractId,
        marketId: telemetry.ctx.spec.marketId,
        slug: telemetry.ctx.spec.slug,
        urlSlug: telemetry.ctx.spec.urlSlug,
        ...vol,
      },
      corsHeaders ?? {},
    );
    return;
  }

  if (action === "history") {
    const timeframe = urlObj.searchParams.get("timeframe") ?? "ALL";
    if (!VALID_HISTORY_TIMEFRAMES.has(timeframe)) {
      sendJson(
        res,
        400,
        {
          error: `Invalid timeframe "${timeframe}". Allowed: ${Array.from(VALID_HISTORY_TIMEFRAMES).join(", ")}`,
        },
        corsHeaders ?? {},
      );
      return;
    }
    let limit;
    try {
      limit = parseLimitParam(urlObj.searchParams.get("limit"), 500);
    } catch (err) {
      sendJson(res, 400, { error: err.message }, corsHeaders ?? {});
      return;
    }

    const candleAvailability = buildAllTimeframeOhlcSummary(observations);
    const requestedCandles =
      timeframe === "ALL" ? null : candleAvailability[timeframe] ?? null;
    const volume = buildExecutionVolume(allEvents, timeframe);

    sendJson(
      res,
      200,
      {
        network: config.network,
        contractId: config.contractId,
        marketId: telemetry.ctx.spec.marketId,
        slug: telemetry.ctx.spec.slug,
        urlSlug: telemetry.ctx.spec.urlSlug,
        feedClassification: telemetry.ctx.spec.feedClassification ?? "TESTNET_SANDBOX_FEED",
        isSynthetic: telemetry.ctx.spec.isSynthetic ?? true,
        sourceMode: "persistent_indexer",
        provider: {
          name: "fastnear_neardata_testnet",
          finality: "FINAL",
          status: telemetry.overallStatus,
          providerStatus: telemetry.providerStatus,
          lastFinalizedHeight: telemetry.lastFinalizedHeight,
          indexedThroughHeight: telemetry.indexedThroughHeight,
          lagBlocks: telemetry.lagBlocks,
          lastIndexedAt: runtime.lastIndexedAt ?? cursor?.updatedAt ?? null,
          lastSuccessfulBlockAt:
            runtime.lastSuccessfulBlockAt ?? cursor?.lastSuccessfulBlockAt ?? cursor?.updatedAt ?? null,
          lastErrorCode: runtime.lastErrorCode ?? null,
          lastErrorAt: runtime.lastErrorAt ?? null,
        },
        coverage: {
          coverageStartHeight: coverage.coverageStartHeight,
          indexedThroughHeight: coverage.indexedThroughHeight,
          indexedThroughHash: coverage.indexedThroughHash,
          backfillCompleted: coverage.backfillCompleted,
          backfillBlockRange: coverage.backfillBlockRange,
          seedBackfillRange: coverage.seedBackfillRange,
          forwardScanRange: coverage.forwardScanRange,
          coverageSegments: coverage.coverageSegments,
          coverageGaps: coverage.coverageGaps,
          coverageComplete: coverage.coverageComplete,
          totalGapBlocks: coverage.totalGapBlocks,
          totalScannedBlocks: coverage.totalScannedBlocks,
          blocksProcessed: coverage.blocksProcessed,
          gapDetected: coverage.gapDetected,
          gapRange: coverage.gapRange,
        },
        observations: observations.slice(-limit),
        executions: executions.slice(-limit),
        latestObservation: freshness.latestObservation,
        referenceNowSec: freshness.referenceNowSec,
        latestAgeSec: freshness.latestAgeSec,
        maxAgeSec: telemetry.maxAgeSec,
        stale: freshness.stale,
        candleAvailability,
        candles: requestedCandles,
        volume,
        rejectedCount: cursor?.rejectedCount ?? runtime.rejectedCount ?? 0,
      },
      corsHeaders ?? {},
    );
  }
}

/**
 * Create read-only local HTTP JSON server exposing verified per-market history, candles, volume,
 * explicit multi-segment coverage/gaps, and per-market health telemetry.
 */
export function createIndexerHttpServer({
  config,
  store = null,
  getRuntimeState = null,
  getMarketContexts = null,
}) {
  const resolveContexts = () => {
    if (typeof getMarketContexts === "function") {
      return getMarketContexts();
    }
    return [
      {
        spec: {
          slug: "arabica",
          urlSlug: "COFFEE-ARABICA-TESTNET-USD",
          marketId: config.marketId,
          feedClassification: "TESTNET_SANDBOX_FEED",
          isSynthetic: true,
          backfillTransactions: config.backfillTransactions ?? [],
        },
        store,
        getRuntimeState,
      },
    ];
  };

  const server = http.createServer((req, res) => {
    const origin = req.headers.origin;
    const corsHeaders = buildCorsHeaders(origin, config.corsOrigins);

    if (origin && corsHeaders === null) {
      sendJson(res, 403, { error: "Origin not allowed by strict CORS policy." });
      return;
    }

    if (req.method === "OPTIONS") {
      res.writeHead(204, {
        "cache-control": "no-store",
        ...(corsHeaders ?? {}),
      });
      res.end();
      return;
    }

    if (req.method !== "GET") {
      sendJson(
        res,
        405,
        { error: "Read-only API: only GET requests are permitted." },
        corsHeaders ?? {},
      );
      return;
    }

    let urlObj;
    try {
      urlObj = new URL(req.url || "/", `http://${config.httpHost}`);
    } catch {
      sendJson(res, 400, { error: "Malformed request URL." }, corsHeaders ?? {});
      return;
    }

    const pathname = urlObj.pathname;
    const contexts = resolveContexts();
    const marketSpecs = contexts.map((c) => c.spec);
    const telemetryBySlug = new Map();
    const orderedTelemetry = [];
    for (const ctx of contexts) {
      const t = buildMarketTelemetry(ctx, config);
      telemetryBySlug.set(ctx.spec.slug, t);
      orderedTelemetry.push(t);
    }

    const primaryTelemetry = orderedTelemetry[0];

    if (pathname === "/health") {
      const marketsBySlug = {};
      for (const t of orderedTelemetry) {
        marketsBySlug[t.ctx.spec.slug] = t.healthPayload;
      }
      sendJson(
        res,
        200,
        {
          ...primaryTelemetry.healthPayload,
          markets: marketsBySlug,
          marketList: orderedTelemetry.map((t) => t.healthPayload),
        },
        corsHeaders ?? {},
      );
      return;
    }

    if (pathname === "/v1/perps/markets") {
      sendJson(
        res,
        200,
        {
          network: config.network,
          contractId: config.contractId,
          marketCount: orderedTelemetry.length,
          markets: orderedTelemetry.map((t) => t.healthPayload),
        },
        corsHeaders ?? {},
      );
      return;
    }

    // 1. Named slug routes: /v1/perps/<slug>/<action>
    const slugMatch = pathname.match(/^\/v1\/perps\/(arabica|robusta|gesha)\/([a-z-]+)$/);
    if (slugMatch && MARKET_ACTIONS.has(slugMatch[2])) {
      const matchedSpec = resolveConfiguredMarket(slugMatch[1], marketSpecs);
      const telemetry = matchedSpec ? telemetryBySlug.get(matchedSpec.slug) : null;
      if (!telemetry) {
        sendJson(
          res,
          404,
          { error: `Market "${slugMatch[1]}" is not active on this indexer instance.` },
          corsHeaders ?? {},
        );
        return;
      }
      handleMarketAction({
        action: slugMatch[2],
        telemetry,
        urlObj,
        res,
        corsHeaders,
        config,
      });
      return;
    }

    // 2. Generic validated path route: /v1/perps/markets/<marketParam>/<action>
    const genericPathMatch = pathname.match(/^\/v1\/perps\/markets\/([^/]+)\/([a-z-]+)$/);
    if (genericPathMatch && MARKET_ACTIONS.has(genericPathMatch[2])) {
      let decodedParam;
      try {
        decodedParam = decodeURIComponent(genericPathMatch[1]);
      } catch {
        sendJson(res, 400, { error: "Malformed market identifier encoding." }, corsHeaders ?? {});
        return;
      }
      if (!decodedParam || decodedParam.includes("..") || decodedParam.includes("\0")) {
        sendJson(res, 400, { error: "Invalid market identifier." }, corsHeaders ?? {});
        return;
      }
      const matchedSpec = resolveConfiguredMarket(decodedParam, marketSpecs);
      const telemetry = matchedSpec ? telemetryBySlug.get(matchedSpec.slug) : null;
      if (!telemetry) {
        sendJson(
          res,
          404,
          { error: `Market "${decodedParam}" is not registered or indexed.` },
          corsHeaders ?? {},
        );
        return;
      }
      handleMarketAction({
        action: genericPathMatch[2],
        telemetry,
        urlObj,
        res,
        corsHeaders,
        config,
      });
      return;
    }

    // 3. Generic validated query route: /v1/perps/<action>?market=<marketId>
    const genericQueryMatch = pathname.match(/^\/v1\/perps\/([a-z-]+)$/);
    if (genericQueryMatch && MARKET_ACTIONS.has(genericQueryMatch[1])) {
      const rawMarket = urlObj.searchParams.get("market");
      if (!rawMarket || rawMarket.trim().length === 0) {
        sendJson(
          res,
          400,
          { error: 'Query parameter "market" is required on generic /v1/perps endpoints.' },
          corsHeaders ?? {},
        );
        return;
      }
      if (rawMarket.includes("..") || rawMarket.includes("\0")) {
        sendJson(res, 400, { error: "Invalid market query parameter." }, corsHeaders ?? {});
        return;
      }
      const matchedSpec = resolveConfiguredMarket(rawMarket, marketSpecs);
      const telemetry = matchedSpec ? telemetryBySlug.get(matchedSpec.slug) : null;
      if (!telemetry) {
        sendJson(
          res,
          404,
          { error: `Market "${rawMarket}" is not registered or indexed.` },
          corsHeaders ?? {},
        );
        return;
      }
      handleMarketAction({
        action: genericQueryMatch[1],
        telemetry,
        urlObj,
        res,
        corsHeaders,
        config,
      });
      return;
    }

    sendJson(res, 404, { error: `Unknown endpoint "${pathname}".` }, corsHeaders ?? {});
  });

  function listen() {
    return new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(config.httpPort, config.httpHost, () => {
        server.off("error", reject);
        resolve(server.address());
      });
    });
  }

  function closeServer() {
    return new Promise((resolve) => {
      server.close(() => resolve());
    });
  }

  return { server, listen, close: closeServer };
}
