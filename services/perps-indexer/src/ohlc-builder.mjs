// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

export const ORACLE_SAMPLE_CLASSIFICATION = "ORACLE SAMPLE OHLC";

export const TIMEFRAME_SECONDS = {
  "1m": 60,
  "5m": 300,
  "15m": 900,
  "1H": 3_600,
  "4H": 14_400,
  "1D": 86_400,
};

export const VALID_OHLC_TIMEFRAMES = Object.keys(TIMEFRAME_SECONDS);
export const MIN_OHLC_BUCKETS_FOR_CONTINUITY = 6;
export const MIN_MULTI_SAMPLE_BUCKETS = 2;

function formatE8ScaledDecimal(scaledBigInt, decimals = 8) {
  const neg = scaledBigInt < 0n;
  const abs = neg ? -scaledBigInt : scaledBigInt;
  const scale = 10n ** BigInt(decimals);
  const whole = abs / scale;
  const frac = (abs % scale).toString().padStart(decimals, "0").replace(/0+$/, "");
  const formatted = frac ? `${whole}.${frac}` : `${whole}`;
  return neg ? `-${formatted}` : formatted;
}

/**
 * Aggregate strictly verified `oracle_sandbox_tick` events into ORACLE SAMPLE OHLC intervals.
 * Never creates empty gap buckets, never interpolates prices, and never forward-carries prior values.
 */
export function buildOracleSampleOhlc(observations, timeframe) {
  const bucketSec = TIMEFRAME_SECONDS[timeframe];
  if (!bucketSec) {
    throw new Error(`Unsupported OHLC timeframe "${timeframe}". Expected one of ${VALID_OHLC_TIMEFRAMES.join(", ")}.`);
  }

  const ordered = [...observations].sort(
    (a, b) =>
      Number(a.data?.observed_at_sec ?? a.observedAtSec) -
        Number(b.data?.observed_at_sec ?? b.observedAtSec) ||
      Number(a.data?.sequence ?? a.sequence ?? 0) - Number(b.data?.sequence ?? b.sequence ?? 0),
  );

  if (ordered.length === 0) {
    return {
      seriesClassification: ORACLE_SAMPLE_CLASSIFICATION,
      timeframe,
      bucketSeconds: bucketSec,
      available: false,
      reason: "No verified oracle observations indexed yet.",
      totalObservations: 0,
      bucketCount: 0,
      multiSampleBuckets: 0,
      coverageRatio: 0,
      spanSec: 0,
      sampleBuckets: [],
      candles: [],
    };
  }

  /** @type {Map<number, { time: number, open: number, high: number, low: number, close: number, sampleCount: number }>} */
  const bucketsMap = new Map();

  for (const obs of ordered) {
    const ts = Number(obs.data?.observed_at_sec ?? obs.observedAtSec);
    const priceUsd = Number(obs.data?.priceUsd ?? obs.priceUsd);
    if (!Number.isSafeInteger(ts) || ts <= 0 || !Number.isFinite(priceUsd) || priceUsd <= 0) {
      continue;
    }

    const bucketStart = Math.floor(ts / bucketSec) * bucketSec;
    const existing = bucketsMap.get(bucketStart);
    if (!existing) {
      bucketsMap.set(bucketStart, {
        time: bucketStart,
        open: priceUsd,
        high: priceUsd,
        low: priceUsd,
        close: priceUsd,
        sampleCount: 1,
      });
    } else {
      existing.high = Math.max(existing.high, priceUsd);
      existing.low = Math.min(existing.low, priceUsd);
      existing.close = priceUsd;
      existing.sampleCount += 1;
    }
  }

  const sampleBuckets = Array.from(bucketsMap.values()).sort((a, b) => a.time - b.time);
  const bucketCount = sampleBuckets.length;
  const multiSampleBuckets = sampleBuckets.filter((b) => b.sampleCount >= 2).length;
  const firstBucketTime = sampleBuckets[0]?.time ?? 0;
  const lastBucketTime = sampleBuckets[bucketCount - 1]?.time ?? 0;
  const spanSec = lastBucketTime - firstBucketTime;
  const theoreticalBucketsInSpan = Math.max(1, Math.floor(spanSec / bucketSec) + 1);
  const coverageRatio = Number((bucketCount / theoreticalBucketsInSpan).toFixed(4));

  const available =
    bucketCount >= MIN_OHLC_BUCKETS_FOR_CONTINUITY &&
    multiSampleBuckets >= MIN_MULTI_SAMPLE_BUCKETS;

  const reason = available
    ? null
    : `Insufficient verified oracle observations for ${timeframe} ${ORACLE_SAMPLE_CLASSIFICATION} (${ordered.length} ticks across ${bucketCount} non-empty bucket${bucketCount === 1 ? "" : "s"}, ${multiSampleBuckets} multi-sample; requires >= ${MIN_OHLC_BUCKETS_FOR_CONTINUITY} buckets and >= ${MIN_MULTI_SAMPLE_BUCKETS} multi-sample buckets). Showing reference LineSeries without gap-filling or interpolation.`;

  return {
    seriesClassification: ORACLE_SAMPLE_CLASSIFICATION,
    timeframe,
    bucketSeconds: bucketSec,
    available,
    reason,
    totalObservations: ordered.length,
    bucketCount,
    multiSampleBuckets,
    coverageRatio,
    spanSec,
    sampleBuckets,
    candles: available ? sampleBuckets : [],
  };
}

export function buildAllTimeframeOhlcSummary(observations) {
  const summary = {};
  for (const tf of VALID_OHLC_TIMEFRAMES) {
    const built = buildOracleSampleOhlc(observations, tf);
    summary[tf] = {
      seriesClassification: built.seriesClassification,
      timeframe: tf,
      available: built.available,
      reason: built.reason,
      totalObservations: built.totalObservations,
      bucketCount: built.bucketCount,
      multiSampleBuckets: built.multiSampleBuckets,
      coverageRatio: built.coverageRatio,
      candles: built.candles,
    };
  }
  return summary;
}

/**
 * Evaluate trading execution volume strictly from verified on-chain NEP-297 events.
 * Policy derived directly from `contracts/perpetuals/src/events.rs`:
 * - Collateral deposits/withdrawals, LP deposits/withdrawals, margin adjustments, and funding accruals
 *   are NEVER treated as trading volume (`excludedNonVolumeEvents`).
 * - `PositionOpenedEvent` emits `size_e8` & `notional_e8`.
 * - `PositionClosedEvent` & `PositionLiquidatedEvent` OMIT `size_e8` and `notional_e8` on-chain.
 * Consequently, bidirectional trading volume MUST remain `unavailable` whenever any close/liquidation
 * lacks exact executed size/notional evidence rather than guessing or double-counting.
 */
export function buildExecutionVolume(allEvents, timeframe = "ALL") {
  if (timeframe !== "ALL" && !TIMEFRAME_SECONDS[timeframe]) {
    throw new Error(`Unsupported volume timeframe "${timeframe}".`);
  }

  const seenEventIds = new Set();
  let openedExecutions = 0;
  let closedExecutions = 0;
  let liquidatedExecutions = 0;
  let excludedNonVolumeEvents = 0;
  let missingSizeOnCloseOrLiq = 0;
  let openedSizeSumE8 = 0n;
  let openedNotionalSumE8 = 0n;

  for (const ev of allEvents) {
    if (!ev?.eventId || seenEventIds.has(ev.eventId)) continue;
    seenEventIds.add(ev.eventId);

    if (
      ev.eventType === "collateral_deposited" ||
      ev.eventType === "collateral_withdrawn" ||
      ev.eventType === "lp_deposited" ||
      ev.eventType === "lp_withdrawn" ||
      ev.eventType === "margin_added" ||
      ev.eventType === "margin_removed" ||
      ev.eventType === "funding_accrued"
    ) {
      excludedNonVolumeEvents += 1;
      continue;
    }

    if (ev.eventType === "position_opened") {
      openedExecutions += 1;
      const sz = typeof ev.data?.size_e8 === "string" && /^\d+$/.test(ev.data.size_e8)
        ? BigInt(ev.data.size_e8)
        : null;
      const not = typeof ev.data?.notional_e8 === "string" && /^\d+$/.test(ev.data.notional_e8)
        ? BigInt(ev.data.notional_e8)
        : null;
      if (sz !== null) openedSizeSumE8 += sz;
      if (not !== null) openedNotionalSumE8 += not;
    } else if (ev.eventType === "position_closed") {
      closedExecutions += 1;
      const hasExactCloseSize =
        typeof ev.data?.size_e8 === "string" &&
        /^\d+$/.test(ev.data.size_e8) &&
        typeof ev.data?.notional_e8 === "string" &&
        /^\d+$/.test(ev.data.notional_e8);
      if (!hasExactCloseSize) {
        missingSizeOnCloseOrLiq += 1;
      }
    } else if (ev.eventType === "position_liquidated") {
      liquidatedExecutions += 1;
      const hasExactLiqSize =
        typeof ev.data?.size_e8 === "string" &&
        /^\d+$/.test(ev.data.size_e8) &&
        typeof ev.data?.notional_e8 === "string" &&
        /^\d+$/.test(ev.data.notional_e8);
      if (!hasExactLiqSize) {
        missingSizeOnCloseOrLiq += 1;
      }
    }
  }

  const totalExecutions = openedExecutions + closedExecutions + liquidatedExecutions;

  if (totalExecutions === 0) {
    return {
      available: false,
      status: "unavailable",
      timeframe,
      reason: "No verified position execution events indexed yet; volume unavailable.",
      baseVolumeKg: null,
      notionalVolumeUsd: null,
      executionCount: 0,
      openedExecutions: 0,
      closedExecutions: 0,
      liquidatedExecutions: 0,
      verifiedOpenedBaseKg: null,
      verifiedOpenedNotionalUsd: null,
      excludedNonVolumeEvents,
      source: "contracts/perpetuals/src/events.rs",
    };
  }

  if (missingSizeOnCloseOrLiq > 0 || totalExecutions < 4) {
    const schemaReason =
      missingSizeOnCloseOrLiq > 0
        ? "PositionClosedEvent and PositionLiquidatedEvent in contracts/perpetuals/src/events.rs do not emit executed size_e8/notional_e8 on-chain; bidirectional volume cannot be constructed reliably without inferring close sizes."
        : `Insufficient execution depth (${totalExecutions} verified executions) to form meaningful volume bars.`;

    return {
      available: false,
      status: "unavailable",
      timeframe,
      reason: `Volume unavailable — ${schemaReason}`,
      baseVolumeKg: null,
      notionalVolumeUsd: null,
      executionCount: totalExecutions,
      openedExecutions,
      closedExecutions,
      liquidatedExecutions,
      verifiedOpenedBaseKg: openedExecutions > 0 ? formatE8ScaledDecimal(openedSizeSumE8, 8) : null,
      verifiedOpenedNotionalUsd:
        openedExecutions > 0 ? formatE8ScaledDecimal(openedNotionalSumE8, 8) : null,
      excludedNonVolumeEvents,
      source: "contracts/perpetuals/src/events.rs (deposits/withdrawals/LP/margin strictly excluded)",
    };
  }

  return {
    available: true,
    status: "verified",
    timeframe,
    reason: null,
    baseVolumeKg: formatE8ScaledDecimal(openedSizeSumE8, 8),
    notionalVolumeUsd: formatE8ScaledDecimal(openedNotionalSumE8, 8),
    executionCount: totalExecutions,
    openedExecutions,
    closedExecutions,
    liquidatedExecutions,
    verifiedOpenedBaseKg: formatE8ScaledDecimal(openedSizeSumE8, 8),
    verifiedOpenedNotionalUsd: formatE8ScaledDecimal(openedNotionalSumE8, 8),
    excludedNonVolumeEvents,
    source: "verified_nep297_execution_events",
  };
}
