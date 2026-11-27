import test from "node:test";
import assert from "node:assert/strict";
import { createRateLimiter } from "../src/rate-limiter.mjs";

test("non-429 provider failures retain backoff but do not inflate rateLimitHits", () => {
  const limiter = createRateLimiter({}, { nowFn: () => 0, randomFn: () => 0 });
  const delay = limiter.recordRateLimit({ isRateLimit: false, consecutiveErrors: 2 });
  assert.ok(delay > 0);
  assert.equal(limiter.getStats().rateLimitHits, 0);
  assert.equal(limiter.getStats().otherProviderErrorHits, 1);
  const retry = limiter.recordRateLimit({ isRateLimit: true, retryAfterMs: 18000 });
  assert.ok(retry >= 18000);
  assert.equal(limiter.getStats().rateLimitHits, 1);
  assert.equal(limiter.getStats().otherProviderErrorHits, 1);
});
