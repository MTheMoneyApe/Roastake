// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

/**
 * Pure deterministic exponential backoff with configurable jitter + strict `Retry-After` honoring.
 * Never returns fewer milliseconds than `retryAfterMs` when a server supplies `Retry-After`.
 */
export function computeBackoffDelayMs({
  consecutiveErrors = 1,
  initialBackoffMs = 1_500,
  maxBackoffMs = 60_000,
  retryAfterMs = null,
  jitterRatio = 0.2,
  randomFn = Math.random,
} = {}) {
  const errCount = Math.max(1, Number(consecutiveErrors) || 1);
  const expBase = Math.min(maxBackoffMs, initialBackoffMs * 2 ** Math.min(5, errCount - 1));
  const clampedJitterRatio = Math.max(0, Math.min(0.5, jitterRatio));
  const r = Math.max(0, Math.min(1, Number(randomFn()) || 0));
  // Additive normalized jitter keeps delay strictly >= expBase so exponential floors never invert.
  const expWithJitter = Math.min(
    maxBackoffMs,
    Math.max(initialBackoffMs, Math.round(expBase * (1 + r * clampedJitterRatio))),
  );

  if (Number.isFinite(retryAfterMs) && retryAfterMs > 0) {
    // Wait strictly >= server Retry-After plus a small positive jitter tail (never undercut Retry-After).
    const retryFloor = Math.ceil(retryAfterMs);
    const positiveTail = Math.round(r * Math.min(1_500, Math.max(250, Math.round(retryFloor * 0.1))));
    return Math.min(300_000, Math.max(expWithJitter, retryFloor + positiveTail));
  }

  return expWithJitter;
}

/**
 * Adaptive single-flight request rate governor respecting provider RPM caps and HTTP 429 cooldowns.
 * Measures start-to-start spacing so RTT does not stack unnecessarily on top of safe pacing,
 * and measures rolling 60-second throughput.
 */
export function createRateLimiter(options = {}, deps = {}) {
  const maxRequestsPerMinute = Math.max(1, options.maxRequestsPerMinute ?? 150);
  const baseMinIntervalMs = Math.max(
    0,
    options.minIntervalMs ?? Math.ceil(60_000 / maxRequestsPerMinute),
  );
  const maxAdaptiveIntervalMs = Math.max(baseMinIntervalMs, options.maxAdaptiveIntervalMs ?? 2_500);
  const nowFn = deps.nowFn ?? (() => Date.now());
  const sleepFn = deps.sleepFn ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
  const randomFn = deps.randomFn ?? Math.random;

  let currentIntervalMs = baseMinIntervalMs;
  let nextAllowedAtMs = 0;
  let totalRequests = 0;
  let rateLimitHits = 0;
  let lastRetryAfterMs = null;
  let consecutiveSuccesses = 0;
  let latencySumMs = 0;
  let latencyCount = 0;
  const recentRequestTimestamps = [];

  function pruneWindow(now) {
    const cutoff = now - 60_000;
    while (recentRequestTimestamps.length > 0 && recentRequestTimestamps[0] < cutoff) {
      recentRequestTimestamps.shift();
    }
  }

  async function acquireSlot(shouldStop = () => false) {
    while (!shouldStop()) {
      const now = nowFn();
      pruneWindow(now);

      let waitMs = Math.max(0, nextAllowedAtMs - now);

      // Rolling 60s RPM cap guard in addition to minimum start-to-start interval
      if (recentRequestTimestamps.length >= maxRequestsPerMinute) {
        const oldestWait = Math.max(10, recentRequestTimestamps[0] + 60_000 - now);
        waitMs = Math.max(waitMs, oldestWait);
      }

      if (waitMs <= 0) {
        const stamp = nowFn();
        recentRequestTimestamps.push(stamp);
        totalRequests += 1;
        nextAllowedAtMs = Math.max(nextAllowedAtMs, stamp + currentIntervalMs);
        return;
      }

      // Wait in bounded chunks (<= 500ms) when waiting long so shutdown signals respond promptly.
      const slice = Math.min(waitMs, 500);
      await sleepFn(slice);
    }
  }

  function recordSuccess(durationMs = 0) {
    if (Number.isFinite(durationMs) && durationMs >= 0) {
      latencySumMs += durationMs;
      latencyCount += 1;
    }
    consecutiveSuccesses += 1;
    if (consecutiveSuccesses >= 12 && currentIntervalMs > baseMinIntervalMs) {
      currentIntervalMs = Math.max(
        baseMinIntervalMs,
        Math.floor(currentIntervalMs * 0.85),
      );
      consecutiveSuccesses = 0;
    }
  }

  function recordRateLimit({
    consecutiveErrors = 1,
    retryAfterMs = null,
    isRateLimit = true,
  } = {}) {
    rateLimitHits += 1;
    consecutiveSuccesses = 0;
    if (retryAfterMs !== null) {
      lastRetryAfterMs = retryAfterMs;
    }
    currentIntervalMs = Math.min(
      maxAdaptiveIntervalMs,
      Math.max(baseMinIntervalMs, Math.round(Math.max(currentIntervalMs, 250) * 1.5)),
    );
    const delayMs = computeBackoffDelayMs({
      consecutiveErrors,
      initialBackoffMs: isRateLimit
        ? (options.rateLimitInitialBackoffMs ?? 5_000)
        : (options.initialBackoffMs ?? 1_500),
      maxBackoffMs: options.maxBackoffMs ?? 60_000,
      retryAfterMs,
      jitterRatio: options.jitterRatio ?? 0.2,
      randomFn,
    });
    nextAllowedAtMs = Math.max(nextAllowedAtMs, nowFn() + delayMs);
    return delayMs;
  }

  function getStats() {
    pruneWindow(nowFn());
    return {
      maxRequestsPerMinute,
      baseMinIntervalMs,
      currentIntervalMs,
      requestsLastMinute: recentRequestTimestamps.length,
      totalRequests,
      rateLimitHits,
      lastRetryAfterMs,
      avgRequestLatencyMs: latencyCount > 0 ? Math.round(latencySumMs / latencyCount) : null,
    };
  }

  return {
    acquireSlot,
    recordSuccess,
    recordRateLimit,
    getStats,
  };
}
