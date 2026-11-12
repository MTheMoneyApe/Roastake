// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

/**
 * Pure interval coverage calculator for `seedBackfill` (tx-by-hash M04.6-G evidence) vs
 * contiguous finalized `block_scan` segments (`range_backfill` & `forward_scan`).
 * Never conceals inter-segment gaps or equates tx-only backfill with contiguous block scanning.
 */

function toValidHeight(raw) {
  const n = Number(raw);
  return Number.isSafeInteger(n) && n > 0 ? n : null;
}

/**
 * Normalize & merge overlapping/adjacent intervals cleanly (`[{ startHeight, endHeight }]`).
 * Preserves source attribution when merging within the same source or computes pure union when `ignoreSource` is true.
 */
export function normalizeBlockSegments(segments = [], { ignoreSource = false } = {}) {
  const cleaned = [];
  for (const seg of segments) {
    if (!seg || typeof seg !== "object") continue;
    const startHeight = toValidHeight(seg.startHeight);
    const endHeight = toValidHeight(seg.endHeight);
    if (startHeight === null || endHeight === null || endHeight < startHeight) continue;
    cleaned.push({
      kind: "block_scan",
      source: ignoreSource ? "block_scan" : String(seg.source || "forward_scan"),
      startHeight,
      endHeight,
      contiguousBlockScan: true,
    });
  }

  cleaned.sort(
    (a, b) =>
      a.startHeight - b.startHeight ||
      a.endHeight - b.endHeight ||
      a.source.localeCompare(b.source),
  );

  const merged = [];
  for (const item of cleaned) {
    const prev = merged[merged.length - 1];
    if (
      prev &&
      (ignoreSource || prev.source === item.source) &&
      item.startHeight <= prev.endHeight + 1
    ) {
      prev.endHeight = Math.max(prev.endHeight, item.endHeight);
    } else {
      merged.push({ ...item });
    }
  }

  return merged.map((m) => ({
    ...m,
    blockCount: m.endHeight - m.startHeight + 1,
  }));
}

export function addScannedRangeToSegments(existingSegments = [], range) {
  return normalizeBlockSegments([...existingSegments, range], { ignoreSource: false });
}

export function isHeightCoveredBySegments(segments = [], height) {
  const h = toValidHeight(height);
  if (h === null) return false;
  for (const seg of segments) {
    if (h >= seg.startHeight && h <= seg.endHeight) return true;
  }
  return false;
}

/**
 * Given merged ascending union intervals and `[windowStart..windowEnd]`, return all missing
 * sub-intervals inside `[windowStart..windowEnd]`.
 */
export function computeMissingSubIntervals(unionSegments, windowStart, windowEnd) {
  if (
    !Number.isSafeInteger(windowStart) ||
    !Number.isSafeInteger(windowEnd) ||
    windowEnd < windowStart
  ) {
    return [];
  }

  const missing = [];
  let cursor = windowStart;

  for (const seg of unionSegments) {
    if (seg.endHeight < cursor) continue;
    if (seg.startHeight > windowEnd) break;
    if (seg.startHeight > cursor) {
      missing.push({
        fromHeight: cursor,
        toHeight: Math.min(windowEnd, seg.startHeight - 1),
      });
    }
    cursor = Math.max(cursor, seg.endHeight + 1);
    if (cursor > windowEnd) break;
  }

  if (cursor <= windowEnd) {
    missing.push({ fromHeight: cursor, toHeight: windowEnd });
  }

  return missing.map((g) => ({
    fromHeight: g.fromHeight,
    toHeight: g.toHeight,
    missingBlocks: g.toHeight - g.fromHeight + 1,
  }));
}

/**
 * Migrate legacy v1 cursor checkpoints (before explicit `scannedSegments` tracking) without
 * losing or resetting `coverageStartHeight..indexedThroughHeight` progress or M04.6-G backfill records.
 */
export function extractScannedSegmentsFromCursor(cursor) {
  if (!cursor) return [];
  if (Array.isArray(cursor.scannedSegments) && cursor.scannedSegments.length > 0) {
    return normalizeBlockSegments(cursor.scannedSegments, { ignoreSource: false });
  }
  if (
    Number.isSafeInteger(cursor.coverageStartHeight) &&
    Number.isSafeInteger(cursor.indexedThroughHeight) &&
    cursor.indexedThroughHeight >= cursor.coverageStartHeight
  ) {
    return normalizeBlockSegments(
      [
        {
          startHeight: cursor.coverageStartHeight,
          endHeight: cursor.indexedThroughHeight,
          source: "forward_scan",
        },
      ],
      { ignoreSource: false },
    );
  }
  return [];
}

/**
 * Build complete truthful coverage diagnostics distinguishing discrete `seedBackfill` txs
 * from contiguous `forwardScan` & `rangeBackfill` block scans, and surfacing explicit `coverageGaps`.
 */
export function buildCoverageSummary({ cursor, runtime = {}, backfillTxCount = 0 } = {}) {
  const seedRangeRaw = cursor?.backfillBlockRange ?? null;
  const seedHashes = Array.isArray(cursor?.backfillTxHashes) ? cursor.backfillTxHashes : [];
  const seedBlockHeights = Array.isArray(cursor?.backfillBlockHeights)
    ? [...cursor.backfillBlockHeights].filter((n) => Number.isSafeInteger(n) && n > 0).sort((a, b) => a - b)
    : [];

  const seedBackfillRange = seedRangeRaw
    ? {
        minHeight: seedRangeRaw.minHeight,
        maxHeight: seedRangeRaw.maxHeight,
        txCount: seedHashes.length,
        blockHeights: seedBlockHeights,
        contiguousBlockScan: false,
      }
    : null;

  const scannedSegments = extractScannedSegmentsFromCursor(cursor);
  const unionSegments = normalizeBlockSegments(scannedSegments, { ignoreSource: true });

  const forwardSegments = scannedSegments.filter((s) => s.source === "forward_scan");
  const forwardPrimary =
    forwardSegments.length > 0 ? forwardSegments[forwardSegments.length - 1] : null;
  const forwardScanRange = forwardPrimary
    ? {
        startHeight: forwardPrimary.startHeight,
        endHeight: forwardPrimary.endHeight,
        blockCount: forwardPrimary.blockCount,
      }
    : null;

  const coverageSegments = [];
  if (seedBackfillRange) {
    coverageSegments.push({
      kind: "seed_backfill",
      source: "experimental_tx_status",
      startHeight: seedBackfillRange.minHeight,
      endHeight: seedBackfillRange.maxHeight,
      txCount: seedBackfillRange.txCount,
      blockCount:
        seedBlockHeights.length > 0 ? seedBlockHeights.length : seedBackfillRange.txCount,
      blockHeights: seedBlockHeights,
      contiguousBlockScan: false,
    });
  }
  coverageSegments.push(...scannedSegments);

  const coverageGaps = [];

  if (unionSegments.length > 0) {
    const highestScanned = unionSegments[unionSegments.length - 1].endHeight;
    const lowestScanned = unionSegments[0].startHeight;

    // 1. Gap between latest M04.6-G seed tx height (+1) and contiguous block-scan coverage
    if (seedBackfillRange && highestScanned > seedBackfillRange.maxHeight) {
      const gapWindowStart = seedBackfillRange.maxHeight + 1;
      const missingAfterSeed = computeMissingSubIntervals(
        unionSegments,
        gapWindowStart,
        highestScanned,
      );
      for (const gap of missingAfterSeed) {
        coverageGaps.push({
          kind: "unverified_historical_interval",
          ...gap,
          reason:
            "Finalized blocks between M04.6-G seed transactions and forward/range scan have not all been scanned block-by-block.",
        });
      }
    } else if (unionSegments.length > 1) {
      // 2. Interior gaps between disconnected block-scan segments even without seed backfill
      const interiorMissing = computeMissingSubIntervals(
        unionSegments,
        lowestScanned,
        highestScanned,
      );
      for (const gap of interiorMissing) {
        coverageGaps.push({
          kind: "inter_segment_gap",
          ...gap,
          reason: "Unscanned finalized height interval between non-adjacent block segments.",
        });
      }
    }
  }

  // 3. Explicit halted forward gap (e.g., irreversible 404 BLOCK_NOT_FOUND without archival proof)
  if (runtime.gapDetected && runtime.gapRange?.fromHeight) {
    const toH = runtime.gapRange.toHeight ?? runtime.gapRange.fromHeight;
    const alreadyListed = coverageGaps.some(
      (g) => g.fromHeight <= runtime.gapRange.fromHeight && g.toHeight >= runtime.gapRange.fromHeight,
    );
    if (!alreadyListed) {
      coverageGaps.push({
        kind: "halted_historical_gap",
        fromHeight: runtime.gapRange.fromHeight,
        toHeight: toH,
        missingBlocks: Math.max(1, toH - runtime.gapRange.fromHeight + 1),
        reason: runtime.gapRange.reason ?? "Sequential forward scan halted at unavailable finalized block.",
      });
    }
  }

  coverageGaps.sort((a, b) => a.fromHeight - b.fromHeight || a.toHeight - b.toHeight);

  const totalGapBlocks = coverageGaps.reduce((acc, g) => acc + g.missingBlocks, 0);
  const totalScannedBlocks = unionSegments.reduce((acc, s) => acc + s.blockCount, 0);
  const backfillPending =
    backfillTxCount > 0 && !Boolean(cursor?.backfillCompleted);

  const coverageComplete =
    unionSegments.length > 0 &&
    coverageGaps.length === 0 &&
    !Boolean(runtime.gapDetected) &&
    !backfillPending;

  return {
    coverageStartHeight: cursor?.coverageStartHeight ?? forwardScanRange?.startHeight ?? null,
    indexedThroughHeight: cursor?.indexedThroughHeight ?? null,
    indexedThroughHash: cursor?.indexedThroughHash ?? null,
    backfillCompleted: Boolean(cursor?.backfillCompleted),
    backfillBlockRange: cursor?.backfillBlockRange ?? null,
    seedBackfillRange,
    forwardScanRange,
    scannedSegments,
    contiguousIntervals: unionSegments,
    coverageSegments,
    coverageGaps,
    coverageComplete,
    totalGapBlocks,
    totalScannedBlocks,
    blocksProcessed: cursor?.blocksProcessed ?? totalScannedBlocks,
    gapDetected: coverageGaps.length > 0 || Boolean(runtime.gapDetected),
    gapRange: coverageGaps[0] ?? runtime.gapRange ?? null,
  };
}
