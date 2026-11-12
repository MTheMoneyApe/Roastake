// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import fs from "node:fs";
import path from "node:path";
import { extractScannedSegmentsFromCursor, normalizeBlockSegments } from "./coverage.mjs";

function atomicWriteJson(filePath, payload) {
  const dir = path.dirname(filePath);
  fs.mkdirSync(dir, { recursive: true });
  const tmpPath = `${filePath}.${process.pid}.${Date.now()}.tmp`;
  const content = `${JSON.stringify(payload, null, 2)}\n`;
  const fd = fs.openSync(tmpPath, "w");
  try {
    fs.writeFileSync(fd, content, "utf8");
    fs.fsyncSync(fd);
  } finally {
    fs.closeSync(fd);
  }
  fs.renameSync(tmpPath, filePath);
}

function isProcessAlive(pid) {
  if (!Number.isSafeInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    // EPERM means process exists without signal permission; ESRCH means not running.
    return err?.code === "EPERM";
  }
}

export class IndexerLockError extends Error {
  constructor(message) {
    super(message);
    this.name = "IndexerLockError";
    this.code = "ERR_INDEXER_LOCKED";
  }
}

/**
 * Append-only durable JSONL event store with single-writer process lock,
 * crash-safe trailing incomplete line truncation, event/sequence deduplication,
 * atomic cursor checkpoints, and rebuildable in-memory indices.
 */
export function createHistoryStore(options) {
  const dataDir = path.resolve(options.dataDir);
  const eventsPath = path.join(dataDir, "events.jsonl");
  const cursorPath = path.join(dataDir, "cursor.json");
  const healthPath = path.join(dataDir, "health.json");
  const lockPath = path.join(dataDir, "writer.lock");

  let lockAcquired = false;
  let closed = false;
  let recoveredTruncatedBytes = 0;

  /** @type {Map<string, any>} */
  const eventsById = new Map();
  /** @type {Set<number>} */
  const seenOracleSequences = new Set();
  /** @type {any[]} */
  const allEvents = [];
  let cursor = null;
  let health = null;

  function acquireLock() {
    fs.mkdirSync(dataDir, { recursive: true });
    for (let attempt = 0; attempt < 2; attempt += 1) {
      try {
        const fd = fs.openSync(lockPath, "wx");
        try {
          fs.writeFileSync(
            fd,
            JSON.stringify({ pid: process.pid, startedAt: new Date().toISOString() }) + "\n",
            "utf8",
          );
          fs.fsyncSync(fd);
        } finally {
          fs.closeSync(fd);
        }
        lockAcquired = true;
        return;
      } catch (err) {
        if (err?.code !== "EEXIST") throw err;
        let existing = null;
        try {
          existing = JSON.parse(fs.readFileSync(lockPath, "utf8"));
        } catch {
          // Malformed or active partial lock file -> check below.
        }
        if (existing?.pid && isProcessAlive(existing.pid)) {
          throw new IndexerLockError(
            `Another perps-indexer process (PID ${existing.pid}) holds ${lockPath}. Concurrent writers are forbidden.`,
          );
        }
        // Stale lock left behind by a crashed dead PID: unlink safely once and retry.
        try {
          fs.unlinkSync(lockPath);
        } catch {
          throw new IndexerLockError(`Unable to clear active lock at ${lockPath}.`);
        }
      }
    }
    throw new IndexerLockError(`Failed to acquire exclusive writer lock at ${lockPath}.`);
  }

  function loadAndRecoverEvents() {
    if (!fs.existsSync(eventsPath)) {
      fs.writeFileSync(eventsPath, "", "utf8");
      return;
    }

    const rawBuf = fs.readFileSync(eventsPath);
    if (rawBuf.length === 0) return;

    const rawText = rawBuf.toString("utf8");
    const rawLines = rawText.split("\n");
    const endsWithNewline = rawText.endsWith("\n");
    const candidateCount = endsWithNewline ? rawLines.length - 1 : rawLines.length;

    let validByteLength = 0;
    for (let i = 0; i < candidateCount; i += 1) {
      const line = rawLines[i];
      const isLastPartialLine = i === candidateCount - 1 && !endsWithNewline;
      const clean = line.replace(/\r$/, "");
      if (!clean.trim()) {
        if (!isLastPartialLine) {
          validByteLength += Buffer.byteLength(line + "\n", "utf8");
        }
        continue;
      }

      let record = null;
      try {
        record = JSON.parse(clean);
      } catch {
        // Only recover cleanly if corruption is on the incomplete trailing line caused by abrupt power loss.
        if (i === candidateCount - 1) {
          recoveredTruncatedBytes = rawBuf.length - validByteLength;
          fs.truncateSync(eventsPath, validByteLength);
          break;
        }
        throw new Error(
          `Unrecoverable JSONL corruption at line ${i + 1} in ${eventsPath}.`,
        );
      }

      if (isLastPartialLine) {
        // Trailing line parsed as valid JSON before '\n' was flushed: heal newline cleanly on disk.
        const appendFd = fs.openSync(eventsPath, "a");
        try {
          fs.writeFileSync(appendFd, "\n", "utf8");
          fs.fsyncSync(appendFd);
        } finally {
          fs.closeSync(appendFd);
        }
      }

      validByteLength += Buffer.byteLength(line + "\n", "utf8");

      if (record?.eventId && !eventsById.has(record.eventId)) {
        if (record.eventType === "oracle_sandbox_tick") {
          const seq = Number(record?.data?.sequence ?? record?.data?.oracle_seq);
          if (Number.isSafeInteger(seq)) {
            const seqKey = `${record.marketId ?? "__default__"}:${seq}`;
            if (seenOracleSequences.has(seqKey)) continue;
            seenOracleSequences.add(seqKey);
          }
        }
        eventsById.set(record.eventId, record);
        allEvents.push(record);
      }
    }
  }

  function loadCursorAndHealth() {
    if (fs.existsSync(cursorPath)) {
      const parsed = JSON.parse(fs.readFileSync(cursorPath, "utf8"));
      if (
        !parsed ||
        !Number.isSafeInteger(parsed.coverageStartHeight) ||
        !Number.isSafeInteger(parsed.indexedThroughHeight)
      ) {
        throw new Error(`Invalid cursor checkpoint format in ${cursorPath}.`);
      }
      // Migrate legacy v1 cursor cleanly without discarding prior M04.6-G backfill or forward scan range.
      const scannedSegments = extractScannedSegmentsFromCursor(parsed);
      const inferredSeedHeights =
        Array.isArray(parsed.backfillBlockHeights) && parsed.backfillBlockHeights.length > 0
          ? parsed.backfillBlockHeights
          : Array.from(
              new Set(
                allEvents
                  .filter((e) => e.source === "rpc_experimental_tx_status_backfill")
                  .map((e) => Number(e.blockHeight))
                  .filter((n) => Number.isSafeInteger(n) && n > 0),
              ),
            ).sort((a, b) => a - b);
      cursor = {
        ...parsed,
        version: Math.max(2, Number(parsed.version) || 1),
        scannedSegments,
        backfillBlockHeights: inferredSeedHeights,
        blocksProcessed:
          parsed.blocksProcessed ??
          scannedSegments.reduce((acc, s) => acc + s.blockCount, 0),
      };
    }
    if (fs.existsSync(healthPath)) {
      try {
        health = JSON.parse(fs.readFileSync(healthPath, "utf8"));
      } catch {
        health = null;
      }
    }
  }

  function open() {
    if (closed) throw new Error("Cannot reopen closed HistoryStore instance.");
    acquireLock();
    loadAndRecoverEvents();
    loadCursorAndHealth();
  }

  /**
   * Append deduplicated validated event records atomically to `events.jsonl`.
   * Duplicate `eventId` OR duplicate `oracle_sandbox_tick.sequence` are rejected cleanly without disk duplication.
   */
  function appendEvents(candidateRecords) {
    if (!lockAcquired || closed) {
      throw new Error("HistoryStore writer lock is not active.");
    }
    const appended = [];
    const duplicateRejections = [];
    const linesToAppend = [];

    for (const record of candidateRecords) {
      if (!record?.eventId) continue;
      if (eventsById.has(record.eventId)) {
        duplicateRejections.push({
          reason: "duplicate_event_id",
          eventId: record.eventId,
          blockHeight: record.blockHeight,
        });
        continue;
      }
      if (record.eventType === "oracle_sandbox_tick") {
        const seq = Number(record?.data?.sequence ?? record?.data?.oracle_seq);
        const seqKey = `${record.marketId ?? "__default__"}:${seq}`;
        if (Number.isSafeInteger(seq) && seenOracleSequences.has(seqKey)) {
          duplicateRejections.push({
            reason: "duplicate_oracle_sequence",
            eventId: record.eventId,
            sequence: seq,
            marketId: record.marketId ?? null,
            blockHeight: record.blockHeight,
          });
          continue;
        }
        if (Number.isSafeInteger(seq)) seenOracleSequences.add(seqKey);
      }

      eventsById.set(record.eventId, record);
      allEvents.push(record);
      appended.push(record);
      linesToAppend.push(JSON.stringify(record));
    }

    if (linesToAppend.length > 0) {
      const fd = fs.openSync(eventsPath, "a");
      try {
        fs.writeFileSync(fd, `${linesToAppend.join("\n")}\n`, "utf8");
        fs.fsyncSync(fd);
      } finally {
        fs.closeSync(fd);
      }
    }

    return { appended, duplicateRejections };
  }

  function saveCursor(nextCursor) {
    if (!lockAcquired || closed) {
      throw new Error("HistoryStore writer lock is not active.");
    }
    const scannedSegments = Array.isArray(nextCursor.scannedSegments)
      ? normalizeBlockSegments(nextCursor.scannedSegments, { ignoreSource: false })
      : extractScannedSegmentsFromCursor(nextCursor);
    cursor = {
      ...nextCursor,
      version: 2,
      scannedSegments,
      updatedAt: new Date().toISOString(),
    };
    atomicWriteJson(cursorPath, cursor);
    return cursor;
  }

  function saveHealth(nextHealth) {
    if (!lockAcquired || closed) {
      throw new Error("HistoryStore writer lock is not active.");
    }
    health = { ...nextHealth, updatedAt: new Date().toISOString() };
    atomicWriteJson(healthPath, health);
    return health;
  }

  function getCursor() {
    return cursor ? { ...cursor } : null;
  }

  function getHealth() {
    return health ? { ...health } : null;
  }

  function getOracleObservations() {
    return allEvents
      .filter((e) => e.eventType === "oracle_sandbox_tick")
      .sort(
        (a, b) =>
          Number(a.data.observed_at_sec) - Number(b.data.observed_at_sec) ||
          Number(a.data.sequence) - Number(b.data.sequence) ||
          a.eventId.localeCompare(b.eventId),
      );
  }

  function getExecutions() {
    return allEvents
      .filter((e) => e.category === "execution")
      .sort(
        (a, b) =>
          Number(a.blockTimestamp) - Number(b.blockTimestamp) ||
          Number(a.blockHeight) - Number(b.blockHeight) ||
          a.eventId.localeCompare(b.eventId),
      );
  }

  function getAllEvents() {
    return [...allEvents];
  }

  function close() {
    if (closed) return;
    closed = true;
    if (lockAcquired && fs.existsSync(lockPath)) {
      try {
        fs.unlinkSync(lockPath);
      } catch {
        // Ignore cleanup failure during teardown.
      }
    }
    lockAcquired = false;
  }

  return {
    open,
    close,
    appendEvents,
    saveCursor,
    saveHealth,
    getCursor,
    getHealth,
    getOracleObservations,
    getExecutions,
    getAllEvents,
    getStats: () => ({
      dataDir,
      eventsPath,
      cursorPath,
      healthPath,
      eventCount: allEvents.length,
      observationCount: allEvents.filter((e) => e.eventType === "oracle_sandbox_tick").length,
      executionCount: allEvents.filter((e) => e.category === "execution").length,
      recoveredTruncatedBytes,
    }),
  };
}
