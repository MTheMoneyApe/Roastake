// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = path.resolve(__dirname, "../../..");

export const NETWORK_ID = "testnet";
export const PERPS_CONTRACT_ID = "perps-v1.roastake-dev-mta.testnet";
export const ARABICA_MARKET_ID = "COFFEE-ARABICA-TESTNET/USD";
export const ARABICA_URL_SLUG = "COFFEE-ARABICA-TESTNET-USD";
export const ROBUSTA_MARKET_ID = "COFFEE-ROBUSTA-TESTNET/USD";
export const ROBUSTA_URL_SLUG = "COFFEE-ROBUSTA";
export const ROBUSTA_REGISTRATION_HEIGHT = 277191714;
export const GESHA_MARKET_ID = "COFFEE-GESHA-TESTNET/USD";
export const GESHA_URL_SLUG = "COFFEE-GESHA";
export const GESHA_REGISTRATION_HEIGHT = 277197570;
export const CIRCLE_TESTNET_USDC_ID =
  "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af";
export const SANDBOX_OPERATOR_ACCOUNT_ID = "roastake-dev-mta.testnet";

export const APPROVED_NEARDATA_HOSTS = new Set(["testnet.neardata.xyz"]);
export const DEFAULT_NEARDATA_BASE_URL = "https://testnet.neardata.xyz";

export const APPROVED_TESTNET_RPC_HOSTS = new Set([
  "rpc.testnet.fastnear.com",
  "rpc.testnet.near.org",
  "archival-rpc.testnet.near.org",
]);
export const DEFAULT_TESTNET_RPC_ENDPOINTS = [
  "https://rpc.testnet.fastnear.com",
  "https://rpc.testnet.near.org",
];

export const APPROVED_CORS_ORIGINS = [
  "http://127.0.0.1:3000",
  "http://localhost:3000",
];

export const ARABICA_MIN_PRICE_E8 = 100_000_000n; // $1.00/kg
export const ARABICA_MAX_PRICE_E8 = 10_000_000_000n; // $100.00/kg
export const SANDBOX_PRICE_MIN_E8 = 100_000_000n; // $1.00/kg
export const SANDBOX_PRICE_MAX_E8 = 50_000_000_000n; // $500.00/kg
export const ORACLE_MAX_AGE_SEC = 300;
export const ORACLE_MAX_CLOCK_DRIFT_SEC = 60;

export const DEFAULT_POLL_INTERVAL_MS = 2_500;
export const DEFAULT_REQUEST_TIMEOUT_MS = 10_000;
export const DEFAULT_MAX_BLOCKS_PER_CYCLE = 25;
export const DEFAULT_MAX_CATCHUP_BLOCKS_PER_RUN = 500;
export const DEFAULT_MAX_CYCLE_DURATION_MS = 25_000;
export const DEFAULT_MAX_RPM_UNAUTH = 120;
export const DEFAULT_MAX_RPM_AUTH = 600;
export const DEFAULT_BLOCK_PACING_MS = 500;
export const DEFAULT_NEAR_TIP_404_TOLERANCE_BLOCKS = 5;
export const DEFAULT_MAX_TRANSIENT_404_RETRIES = 3;
export const DEFAULT_HTTP_HOST = "127.0.0.1";
export const DEFAULT_HTTP_PORT = 8787;
export const DEFAULT_DATA_DIR = path.join(REPO_ROOT, "data", "perps-indexer");

export const M046G_BACKFILL_TRANSACTIONS = [
  {
    txHash: "CGZGrXzCWVPegc8uS3i5LpBqgfMJuUbgeUFRxvnrS4Re",
    senderAccountId: SANDBOX_OPERATOR_ACCOUNT_ID,
    role: "oracle_tick",
    label: "Oracle #1",
  },
  {
    txHash: "GPjS1YZS9vgKGJJhwbe4hnou3MuearryCEXiVgCGtmU",
    senderAccountId: SANDBOX_OPERATOR_ACCOUNT_ID,
    role: "oracle_tick",
    label: "Oracle #2",
  },
  {
    txHash: "8KGqwWiHmFs2TQpDWWPEe8jRHWuBrfpjbe2EDBt47men",
    senderAccountId: SANDBOX_OPERATOR_ACCOUNT_ID,
    role: "oracle_tick",
    label: "Oracle #3",
  },
  {
    txHash: "BNzMzWmVUBpz8nvMNnHvJjajYsnMuDqbVTr7iyQqVqUZ",
    senderAccountId: SANDBOX_OPERATOR_ACCOUNT_ID,
    role: "oracle_tick",
    label: "Oracle #4",
  },
  {
    txHash: "CNgQkfrRpMrU8ywgzFaBdniPfivRDsgEnXcfoFX6gWAp",
    senderAccountId: "majorblog5570.testnet",
    role: "position_open",
    label: "LONG OPEN #2",
  },
  {
    txHash: "Jz7uJnm213s2qLNyBsnkwDPpkNYcuH255wFxPrFeemR",
    senderAccountId: SANDBOX_OPERATOR_ACCOUNT_ID,
    role: "oracle_tick",
    label: "Oracle #5",
  },
  {
    txHash: "2QWybbu1rwxC9in5y8cxAzXzH7PR1bbH6x5WKDJHREnC",
    senderAccountId: "majorblog5570.testnet",
    role: "position_close",
    label: "CLOSE #2",
  },
];

const IMPLICIT_HEX_64_REGEX = /^[0-9a-f]{64}$/;

export class PolicyError extends Error {
  constructor(message) {
    super(message);
    this.name = "PolicyError";
    this.code = "POLICY_ERROR";
  }
}

export function assertTestnetNearDataUrl(rawUrl) {
  let parsed;
  try {
    parsed = new URL(String(rawUrl || "").trim());
  } catch {
    throw new PolicyError(`Invalid NEAR Data URL: "${rawUrl}"`);
  }
  if (parsed.protocol !== "https:") {
    throw new PolicyError(`NEAR Data URL must use HTTPS: "${rawUrl}"`);
  }
  if (!APPROVED_NEARDATA_HOSTS.has(parsed.hostname.toLowerCase())) {
    throw new PolicyError(
      `Testnet-only policy violation: NEAR Data host "${parsed.hostname}" is not approved.`,
    );
  }
  if (parsed.search || parsed.hash) {
    throw new PolicyError("NEAR Data base URL must not contain query parameters or hash fragments.");
  }
  return parsed.origin;
}

export function assertTestnetRpcUrl(rawUrl) {
  let parsed;
  try {
    parsed = new URL(String(rawUrl || "").trim());
  } catch {
    throw new PolicyError(`Invalid NEAR Testnet RPC URL: "${rawUrl}"`);
  }
  if (parsed.protocol !== "https:") {
    throw new PolicyError(`NEAR Testnet RPC URL must use HTTPS: "${rawUrl}"`);
  }
  if (!APPROVED_TESTNET_RPC_HOSTS.has(parsed.hostname.toLowerCase())) {
    throw new PolicyError(
      `Testnet-only policy violation: RPC host "${parsed.hostname}" is not approved.`,
    );
  }
  return parsed.origin;
}

export function isValidTestnetAccountId(accountId) {
  if (typeof accountId !== "string") return false;
  const trimmed = accountId.trim().toLowerCase();
  if (!trimmed || trimmed.endsWith(".near")) return false;
  return trimmed.endsWith(".testnet") || IMPLICIT_HEX_64_REGEX.test(trimmed);
}

export function assertTestnetAccountId(accountId, label = "NEAR account") {
  if (!isValidTestnetAccountId(accountId)) {
    throw new PolicyError(
      `Testnet-only policy violation: ${label} "${accountId}" is not a valid NEAR Testnet account.`,
    );
  }
  return accountId.trim().toLowerCase();
}

export const DEFAULT_MARKET_SPECS = [
  {
    slug: "arabica",
    urlSlug: ARABICA_URL_SLUG,
    marketId: ARABICA_MARKET_ID,
    displayName: "Arabica / USD",
    feedClassification: "TESTNET_SANDBOX_FEED",
    isSynthetic: true,
    minPriceE8: ARABICA_MIN_PRICE_E8,
    maxPriceE8: SANDBOX_PRICE_MAX_E8,
    legacyRootDir: true,
    aliases: [
      "arabica",
      ARABICA_URL_SLUG.toLowerCase(),
      ARABICA_MARKET_ID.toLowerCase(),
      "coffee-arabica-testnet",
      "arabica-usd",
    ],
  },
  {
    slug: "robusta",
    urlSlug: ROBUSTA_URL_SLUG,
    marketId: ROBUSTA_MARKET_ID,
    displayName: "Robusta / USD",
    feedClassification: "TESTNET_SANDBOX_FEED",
    isSynthetic: true,
    minPriceE8: SANDBOX_PRICE_MIN_E8,
    maxPriceE8: SANDBOX_PRICE_MAX_E8,
    legacyRootDir: false,
    registrationBlockHeight: ROBUSTA_REGISTRATION_HEIGHT,
    aliases: [
      "robusta",
      ROBUSTA_URL_SLUG.toLowerCase(),
      ROBUSTA_MARKET_ID.toLowerCase(),
      "coffee-robusta-testnet-usd",
      "coffee-robusta-testnet",
      "robusta-usd",
    ],
  },
  {
    slug: "gesha",
    urlSlug: GESHA_URL_SLUG,
    marketId: GESHA_MARKET_ID,
    displayName: "Gesha / USD",
    feedClassification: "TESTNET_SANDBOX_FEED",
    isSynthetic: true,
    minPriceE8: SANDBOX_PRICE_MIN_E8,
    maxPriceE8: SANDBOX_PRICE_MAX_E8,
    legacyRootDir: false,
    registrationBlockHeight: GESHA_REGISTRATION_HEIGHT,
    aliases: [
      "gesha",
      GESHA_URL_SLUG.toLowerCase(),
      GESHA_MARKET_ID.toLowerCase(),
      "coffee-gesha-testnet-usd",
      "coffee-gesha-testnet",
      "gesha-usd",
    ],
  },
];

/**
 * Strictly resolve a user/URL/API market identifier against the allowlisted market catalog.
 * Rejects unindexed/unregistered identifiers (including preview markets or path-traversal probes).
 */
export function resolveConfiguredMarket(rawIdentifier, markets = DEFAULT_MARKET_SPECS) {
  if (typeof rawIdentifier !== "string") return null;
  let normalized = rawIdentifier.trim();
  if (!normalized || normalized.includes("\0") || normalized.includes("..")) {
    return null;
  }
  try {
    normalized = decodeURIComponent(normalized).trim();
  } catch {
    return null;
  }
  if (!normalized || normalized.includes("\0") || normalized.includes("..")) {
    return null;
  }
  const lower = normalized.toLowerCase();
  for (const spec of markets) {
    if (
      spec.slug.toLowerCase() === lower ||
      spec.urlSlug.toLowerCase() === lower ||
      spec.marketId.toLowerCase() === lower ||
      (Array.isArray(spec.aliases) && spec.aliases.includes(lower))
    ) {
      return spec;
    }
  }
  return null;
}

export function parseIntEnv(rawValue, fallback, min, max) {
  if (rawValue === undefined || rawValue === null || String(rawValue).trim() === "") {
    return fallback;
  }
  const n = Number.parseInt(String(rawValue), 10);
  if (!Number.isSafeInteger(n) || n < min || n > max) {
    throw new Error(`Invalid integer configuration "${rawValue}" (expected ${min}..${max})`);
  }
  return n;
}

export function loadIndexerConfig(overrides = {}, env = process.env) {
  const nearDataBaseUrl = assertTestnetNearDataUrl(
    overrides.nearDataBaseUrl ?? env.PERPS_INDEXER_NEARDATA_URL ?? DEFAULT_NEARDATA_BASE_URL,
  );
  const rpcListRaw =
    overrides.rpcEndpoints ??
    (env.PERPS_INDEXER_RPC_URLS
      ? env.PERPS_INDEXER_RPC_URLS.split(",").map((s) => s.trim()).filter(Boolean)
      : DEFAULT_TESTNET_RPC_ENDPOINTS);

  const rpcEndpoints = rpcListRaw.map((url) => assertTestnetRpcUrl(url));
  if (rpcEndpoints.length === 0) {
    throw new PolicyError("At least one approved NEAR Testnet RPC endpoint is required.");
  }

  const archivalRpcUrlsRaw =
    overrides.archivalRpcEndpoints ??
    (env.PERPS_INDEXER_ARCHIVAL_RPC_URLS
      ? env.PERPS_INDEXER_ARCHIVAL_RPC_URLS.split(",").map((s) => s.trim()).filter(Boolean)
      : []);
  const archivalRpcEndpoints = archivalRpcUrlsRaw.map((url) => assertTestnetRpcUrl(url));

  const contractId = assertTestnetAccountId(
    overrides.contractId ?? env.PERPS_INDEXER_CONTRACT_ID ?? PERPS_CONTRACT_ID,
    "Perps contract",
  );
  const marketId = String(
    overrides.marketId ?? env.PERPS_INDEXER_MARKET_ID ?? ARABICA_MARKET_ID,
  ).trim();
  if (!marketId) {
    throw new Error("marketId must not be empty.");
  }

  const httpHost = String(overrides.httpHost ?? env.PERPS_INDEXER_HOST ?? DEFAULT_HTTP_HOST).trim();
  if (httpHost !== "127.0.0.1" && httpHost !== "localhost") {
    throw new PolicyError(
      `Local API safety guard: refusing non-loopback HTTP bind host "${httpHost}".`,
    );
  }

  const corsRaw = overrides.corsOrigins ??
    (env.PERPS_INDEXER_CORS_ORIGINS
      ? env.PERPS_INDEXER_CORS_ORIGINS.split(",").map((s) => s.trim()).filter(Boolean)
      : APPROVED_CORS_ORIGINS);
  if (corsRaw.includes("*")) {
    throw new PolicyError("CORS wildcard '*' is strictly forbidden.");
  }

  // Optional FastNear API key — kept strictly in server memory, never sent to frontend or logs.
  const rawApiKey = overrides.fastnearApiKey ?? env.FASTNEAR_API_KEY ?? "";
  const fastnearApiKey = typeof rawApiKey === "string" && rawApiKey.trim().length > 0
    ? rawApiKey.trim()
    : null;

  const defaultMaxRpm = fastnearApiKey ? DEFAULT_MAX_RPM_AUTH : DEFAULT_MAX_RPM_UNAUTH;
  const maxRequestsPerMinute = parseIntEnv(
    overrides.maxRequestsPerMinute ?? env.PERPS_INDEXER_MAX_RPM,
    defaultMaxRpm,
    10,
    6_000,
  );
  const derivedPacingMs = fastnearApiKey
    ? Math.max(50, Math.floor(60_000 / maxRequestsPerMinute))
    : DEFAULT_BLOCK_PACING_MS;

  const resolvedDataDir = path.resolve(overrides.dataDir ?? env.PERPS_INDEXER_DATA_DIR ?? DEFAULT_DATA_DIR);
  const resolvedBackfillTxs = overrides.backfillTransactions ?? M046G_BACKFILL_TRANSACTIONS;
  const hasExplicitMarketOverride =
    overrides.marketId !== undefined || Boolean(env.PERPS_INDEXER_MARKET_ID);

  let configuredMarkets;
  if (Array.isArray(overrides.markets) && overrides.markets.length > 0) {
    configuredMarkets = overrides.markets.map((m) => ({
      ...m,
      dataDir: path.resolve(
        m.dataDir ??
          (m.legacyRootDir ? resolvedDataDir : path.join(resolvedDataDir, "markets", m.slug)),
      ),
      minPriceE8: m.minPriceE8 ?? SANDBOX_PRICE_MIN_E8,
      maxPriceE8: m.maxPriceE8 ?? SANDBOX_PRICE_MAX_E8,
      backfillTransactions: m.backfillTransactions ?? [],
    }));
  } else if (hasExplicitMarketOverride || overrides.singleMarketOnly) {
    const matched = resolveConfiguredMarket(marketId);
    const singleSlug = matched?.slug ?? marketId.toLowerCase().replace(/[^a-z0-9]+/g, "-");
    configuredMarkets = [
      {
        slug: singleSlug,
        urlSlug: matched?.urlSlug ?? singleSlug.toUpperCase(),
        marketId,
        displayName: matched?.displayName ?? marketId,
        feedClassification: matched?.feedClassification ?? "TESTNET_SANDBOX_FEED",
        isSynthetic: matched?.isSynthetic ?? true,
        minPriceE8: matched?.minPriceE8 ?? ARABICA_MIN_PRICE_E8,
        maxPriceE8: matched?.maxPriceE8 ?? SANDBOX_PRICE_MAX_E8,
        legacyRootDir: true,
        aliases: matched?.aliases ?? [marketId.toLowerCase()],
        dataDir: resolvedDataDir,
        backfillTransactions:
          marketId === ARABICA_MARKET_ID ? resolvedBackfillTxs : [],
      },
    ];
  } else {
    const backfillOverridesByMarket = {
      [ARABICA_MARKET_ID]: resolvedBackfillTxs,
      [ROBUSTA_MARKET_ID]: overrides.robustaBackfillTransactions ?? [],
      [GESHA_MARKET_ID]: overrides.geshaBackfillTransactions ?? [],
    };
    configuredMarkets = DEFAULT_MARKET_SPECS.map((spec) => ({
      ...spec,
      dataDir: spec.legacyRootDir
        ? resolvedDataDir
        : path.join(resolvedDataDir, "markets", spec.slug),
      backfillTransactions: backfillOverridesByMarket[spec.marketId] ?? [],
    }));
  }

  return {
    network: NETWORK_ID,
    contractId,
    marketId,
    markets: configuredMarkets,
    nearDataBaseUrl,
    rpcEndpoints,
    archivalRpcEndpoints,
    fastnearApiKey,
    dataDir: resolvedDataDir,
    pollIntervalMs: parseIntEnv(
      overrides.pollIntervalMs ?? env.PERPS_INDEXER_POLL_MS,
      DEFAULT_POLL_INTERVAL_MS,
      250,
      300_000,
    ),
    requestTimeoutMs: parseIntEnv(
      overrides.requestTimeoutMs ?? env.PERPS_INDEXER_TIMEOUT_MS,
      DEFAULT_REQUEST_TIMEOUT_MS,
      1_000,
      60_000,
    ),
    maxBlocksPerCycle: parseIntEnv(
      overrides.maxBlocksPerCycle ?? env.PERPS_INDEXER_MAX_BLOCKS_PER_CYCLE,
      DEFAULT_MAX_BLOCKS_PER_CYCLE,
      1,
      5_000,
    ),
    maxCatchupBlocksPerRun: parseIntEnv(
      overrides.maxCatchupBlocksPerRun ?? env.PERPS_INDEXER_MAX_CATCHUP_BLOCKS,
      DEFAULT_MAX_CATCHUP_BLOCKS_PER_RUN,
      1,
      20_000,
    ),
    maxCycleDurationMs: parseIntEnv(
      overrides.maxCycleDurationMs ?? env.PERPS_INDEXER_MAX_CYCLE_MS,
      DEFAULT_MAX_CYCLE_DURATION_MS,
      1_000,
      300_000,
    ),
    maxRequestsPerMinute,
    blockPacingMs: parseIntEnv(
      overrides.blockPacingMs ?? env.PERPS_INDEXER_BLOCK_PACING_MS,
      derivedPacingMs,
      0,
      10_000,
    ),
    nearTip404ToleranceBlocks: parseIntEnv(
      overrides.nearTip404ToleranceBlocks ?? env.PERPS_INDEXER_NEAR_TIP_404_TOLERANCE,
      DEFAULT_NEAR_TIP_404_TOLERANCE_BLOCKS,
      0,
      50,
    ),
    maxTransient404Retries: parseIntEnv(
      overrides.maxTransient404Retries ?? env.PERPS_INDEXER_MAX_TRANSIENT_404_RETRIES,
      DEFAULT_MAX_TRANSIENT_404_RETRIES,
      0,
      10,
    ),
    jitterRatio: overrides.jitterRatio ?? 0.2,
    httpHost,
    httpPort: parseIntEnv(
      overrides.httpPort ?? env.PERPS_INDEXER_PORT,
      DEFAULT_HTTP_PORT,
      0,
      65_535,
    ),
    corsOrigins: corsRaw,
    oracleMaxAgeSec: ORACLE_MAX_AGE_SEC,
    backfillTransactions: resolvedBackfillTxs,
  };
}
