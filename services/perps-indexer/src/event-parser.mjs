// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import {
  ARABICA_MAX_PRICE_E8,
  ARABICA_MIN_PRICE_E8,
  ORACLE_MAX_CLOCK_DRIFT_SEC,
  PERPS_CONTRACT_ID,
  ARABICA_MARKET_ID,
  NETWORK_ID,
  isValidTestnetAccountId,
} from "./config.mjs";
import { parseNanosecTimestampSec } from "./block-source.mjs";

export const EXPECTED_STANDARD = "roastake-perps";
export const EXPECTED_VERSION = "1.0.0";
export const EXPECTED_SANDBOX_DATA_CLASS = "TESTNET_SANDBOX_FEED";

const UNSIGNED_INT_REGEX = /^\d+$/;
const SIGNED_INT_REGEX = /^-?\d+$/;

const KNOWN_MARKET_SCOPED_EVENTS = new Set([
  "oracle_sandbox_tick",
  "position_opened",
  "position_closed",
  "position_liquidated",
  "funding_accrued",
  "market_paused",
  "market_resumed",
]);

const KNOWN_CONTRACT_NON_MARKET_EVENTS = new Set([
  "collateral_deposited",
  "collateral_withdrawn",
  "lp_deposited",
  "lp_withdrawn",
  "position_increased",
  "position_reduced",
  "margin_added",
  "margin_removed",
  "insurance_updated",
  "security_anomaly",
]);

export function parseUnsignedBigInt(value) {
  if (typeof value === "bigint") return value >= 0n ? value : null;
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value) || value < 0) return null;
    return BigInt(value);
  }
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  if (!UNSIGNED_INT_REGEX.test(trimmed)) return null;
  return BigInt(trimmed);
}

export function isValidSignedIntString(value) {
  if (typeof value === "number") return Number.isSafeInteger(value);
  if (typeof value !== "string") return false;
  return SIGNED_INT_REGEX.test(value.trim());
}

export function formatE8ToFixedNumber(rawE8) {
  const val = parseUnsignedBigInt(rawE8);
  if (val === null || val <= 0n) return null;
  const whole = val / 100_000_000n;
  const frac = (val % 100_000_000n).toString().padStart(8, "0");
  const num = Number(`${whole}.${frac}`);
  return Number.isFinite(num) && num > 0 ? num : null;
}

export function isReceiptStatusSuccessful(status) {
  if (!status || typeof status !== "object") return false;
  if ("Failure" in status) return false;
  return "SuccessValue" in status || "SuccessReceiptId" in status;
}

function classifyEventCategory(eventType) {
  if (eventType === "oracle_sandbox_tick") return "oracle_observation";
  if (eventType === "position_opened" || eventType === "position_closed" || eventType === "position_liquidated") {
    return "execution";
  }
  if (eventType === "funding_accrued") return "funding";
  if (eventType === "market_paused" || eventType === "market_resumed") return "market_lifecycle";
  if (
    eventType === "collateral_deposited" ||
    eventType === "collateral_withdrawn" ||
    eventType === "lp_deposited" ||
    eventType === "lp_withdrawn"
  ) {
    return "collateral_or_lp";
  }
  return "position_or_system";
}

function validateAndBuildEntry({
  envelope,
  entry,
  eventIndex,
  context,
  targetContractId,
  targetMarketId,
  minPriceE8 = ARABICA_MIN_PRICE_E8,
  maxPriceE8 = ARABICA_MAX_PRICE_E8,
  ignoredMarketIds = null,
  skipUnscopedProtocolEvents = false,
}) {
  const reject = (reason, detail) => ({
    accepted: false,
    reason,
    detail,
    txHash: context.txHash,
    receiptId: context.receiptId,
    blockHeight: context.blockHeight,
    eventType: envelope.event,
  });

  const eventType = envelope.event;
  if (!KNOWN_MARKET_SCOPED_EVENTS.has(eventType) && !KNOWN_CONTRACT_NON_MARKET_EVENTS.has(eventType)) {
    return reject("unknown_event_type", `Unrecognized Perps event "${eventType}"`);
  }

  if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
    return reject("malformed_event_data", `Event "${eventType}" data item is not an object.`);
  }

  if (KNOWN_MARKET_SCOPED_EVENTS.has(eventType)) {
    if (typeof entry.market_id === "string" && entry.market_id !== targetMarketId) {
      if (ignoredMarketIds && ignoredMarketIds.has(entry.market_id)) {
        return { accepted: false, ignored: true };
      }
      return reject("wrong_market", `Event "${eventType}" targets market "${entry.market_id}"`);
    }
    if (typeof entry.market_id !== "string") {
      return reject("wrong_market", `Event "${eventType}" targets market "missing"`);
    }
  } else if (typeof entry.market_id === "string" && entry.market_id !== targetMarketId) {
    if (ignoredMarketIds && ignoredMarketIds.has(entry.market_id)) {
      return { accepted: false, ignored: true };
    }
    return reject("wrong_market", `Event "${eventType}" targets market "${entry.market_id}"`);
  } else if (skipUnscopedProtocolEvents && typeof entry.market_id !== "string") {
    return { accepted: false, ignored: true };
  }

  // Validate Testnet-only identities whenever present on event payload.
  for (const accField of ["publisher", "owner", "account_id", "keeper", "triggered_by"]) {
    if (entry[accField] !== undefined && entry[accField] !== null) {
      if (!isValidTestnetAccountId(entry[accField])) {
        return reject(
          "mainnet_or_invalid_account",
          `Invalid/Mainnet ${accField} "${entry[accField]}" on "${eventType}".`,
        );
      }
    }
  }

  if (eventType === "oracle_sandbox_tick") {
    const seq = Number(entry.sequence);
    if (!Number.isSafeInteger(seq) || seq < 0) {
      return reject("invalid_oracle_sequence", `Invalid oracle sequence "${entry.sequence}".`);
    }
    const obsSec = Number(entry.observed_at_sec);
    if (!Number.isSafeInteger(obsSec) || obsSec <= 0) {
      return reject("invalid_timestamp", `Invalid observed_at_sec "${entry.observed_at_sec}".`);
    }
    if (obsSec > context.blockTimestamp + ORACLE_MAX_CLOCK_DRIFT_SEC) {
      return reject(
        "future_clock_drift_exceeded",
        `observed_at_sec ${obsSec} exceeds blockTimestamp ${context.blockTimestamp} + ${ORACLE_MAX_CLOCK_DRIFT_SEC}s`,
      );
    }
    const priceE8 = parseUnsignedBigInt(entry.price_e8);
    if (
      priceE8 === null ||
      priceE8 < minPriceE8 ||
      priceE8 > maxPriceE8
    ) {
      return reject(
        "price_out_of_bounds",
        `Oracle price_e8 "${entry.price_e8}" out of valid Testnet bounds.`,
      );
    }
    const dataClass = entry.data_class ?? entry.classification;
    if (dataClass !== EXPECTED_SANDBOX_DATA_CLASS) {
      return reject(
        "unexpected_data_class",
        `Expected ${EXPECTED_SANDBOX_DATA_CLASS}, received "${dataClass}".`,
      );
    }
    if (!entry.publisher) {
      return reject("missing_publisher", "oracle_sandbox_tick missing publisher.");
    }
  } else if (eventType === "position_opened") {
    if (!Number.isSafeInteger(entry.position_id) || entry.position_id < 0) {
      return reject("malformed_execution", "Invalid position_id on position_opened.");
    }
    if (entry.side !== "Long" && entry.side !== "Short") {
      return reject("malformed_execution", `Invalid side "${entry.side}".`);
    }
    const execPrice = parseUnsignedBigInt(entry.exec_price_e8);
    const size = parseUnsignedBigInt(entry.size_e8);
    const notional = parseUnsignedBigInt(entry.notional_e8);
    const margin = parseUnsignedBigInt(entry.margin_e8);
    const fee = parseUnsignedBigInt(entry.fee_e8);
    if (
      execPrice === null ||
      execPrice <= 0n ||
      size === null ||
      size <= 0n ||
      notional === null ||
      notional <= 0n ||
      margin === null ||
      fee === null
    ) {
      return reject("malformed_execution", "Invalid numeric fields on position_opened.");
    }
  } else if (eventType === "position_closed") {
    if (!Number.isSafeInteger(entry.position_id) || entry.position_id < 0) {
      return reject("malformed_execution", "Invalid position_id on position_closed.");
    }
    const execPrice = parseUnsignedBigInt(entry.exec_price_e8);
    if (
      execPrice === null ||
      execPrice <= 0n ||
      !isValidSignedIntString(entry.realized_pnl_e8) ||
      !isValidSignedIntString(entry.funding_settled_e8) ||
      parseUnsignedBigInt(entry.fee_e8) === null ||
      parseUnsignedBigInt(entry.payout_e8) === null ||
      parseUnsignedBigInt(entry.bad_debt_e8) === null
    ) {
      return reject("malformed_execution", "Invalid numeric fields on position_closed.");
    }
  } else if (eventType === "position_liquidated") {
    if (!Number.isSafeInteger(entry.position_id) || entry.position_id < 0) {
      return reject("malformed_execution", "Invalid position_id on position_liquidated.");
    }
    const markPrice = parseUnsignedBigInt(entry.mark_price_e8);
    if (markPrice === null || markPrice <= 0n) {
      return reject("malformed_execution", "Invalid mark_price_e8 on position_liquidated.");
    }
  }

  const normalizedData = { ...entry };
  if (eventType === "oracle_sandbox_tick") {
    normalizedData.oracle_seq = Number(entry.sequence);
    normalizedData.sequence = Number(entry.sequence);
    normalizedData.price_e8 = String(entry.price_e8);
    normalizedData.priceUsd = formatE8ToFixedNumber(entry.price_e8);
    normalizedData.observed_at_sec = Number(entry.observed_at_sec);
    normalizedData.data_class = entry.data_class ?? entry.classification;
    normalizedData.source = context.source;
  }

  const eventId = `${context.txHash ?? "no-tx"}:${context.receiptId}:${eventIndex}`;

  return {
    accepted: true,
    record: {
      eventId,
      network: NETWORK_ID,
      contractId: targetContractId,
      marketId: typeof entry.market_id === "string" ? entry.market_id : targetMarketId,
      category: classifyEventCategory(eventType),
      eventType,
      blockHeight: context.blockHeight,
      blockHash: context.blockHash,
      blockTimestamp: context.blockTimestamp,
      shardId: context.shardId,
      receiptId: context.receiptId,
      transactionHash: context.txHash,
      eventIndex,
      data: normalizedData,
      finality: "FINAL",
      source: context.source,
      indexedAt: context.indexedAt ?? new Date().toISOString(),
    },
  };
}

function parseLogsForReceipt({
  logs,
  status,
  executorId,
  receiptId,
  txHash,
  shardId,
  blockHeight,
  blockHash,
  blockTimestamp,
  source,
  targetContractId,
  targetMarketId,
  minPriceE8,
  maxPriceE8,
  ignoredMarketIds,
  skipUnscopedProtocolEvents,
  indexedAt,
}) {
  const accepted = [];
  const rejected = [];

  if (executorId !== targetContractId) {
    return { accepted, rejected };
  }

  if (!isReceiptStatusSuccessful(status)) {
    rejected.push({
      reason: "failed_receipt",
      detail: `Receipt ${receiptId} failed execution on ${executorId}.`,
      txHash,
      receiptId,
      blockHeight,
      eventType: null,
    });
    return { accepted, rejected };
  }

  let perpsEventIdx = 0;
  for (const log of logs ?? []) {
    if (typeof log !== "string" || !log.startsWith("EVENT_JSON:")) continue;

    let parsed;
    try {
      parsed = JSON.parse(log.slice("EVENT_JSON:".length));
    } catch {
      rejected.push({
        reason: "malformed_json",
        detail: `Invalid EVENT_JSON syntax on receipt ${receiptId}.`,
        txHash,
        receiptId,
        blockHeight,
        eventType: null,
      });
      continue;
    }

    if (!parsed || typeof parsed !== "object") {
      rejected.push({
        reason: "malformed_nep297",
        detail: `Non-object NEP-297 log on receipt ${receiptId}.`,
        txHash,
        receiptId,
        blockHeight,
        eventType: null,
      });
      continue;
    }

    if (parsed.standard !== EXPECTED_STANDARD || parsed.version !== EXPECTED_VERSION) {
      rejected.push({
        reason: "invalid_nep297_standard_or_version",
        detail: `Unexpected NEP-297 standard/version (${parsed.standard}@${parsed.version}).`,
        txHash,
        receiptId,
        blockHeight,
        eventType: typeof parsed.event === "string" ? parsed.event : null,
      });
      continue;
    }

    if (typeof parsed.event !== "string" || !parsed.event) {
      rejected.push({
        reason: "missing_event_name",
        detail: `Missing event name on receipt ${receiptId}.`,
        txHash,
        receiptId,
        blockHeight,
        eventType: null,
      });
      continue;
    }

    const entries = Array.isArray(parsed.data) ? parsed.data : [parsed.data];
    if (entries.length === 0) {
      rejected.push({
        reason: "empty_event_data",
        detail: `Empty NEP-297 data array on ${parsed.event}.`,
        txHash,
        receiptId,
        blockHeight,
        eventType: parsed.event,
      });
      continue;
    }

    for (const entry of entries) {
      const outcome = validateAndBuildEntry({
        envelope: parsed,
        entry,
        eventIndex: perpsEventIdx,
        context: {
          txHash,
          receiptId,
          shardId,
          blockHeight,
          blockHash,
          blockTimestamp,
          source,
          indexedAt,
        },
        targetContractId,
        targetMarketId,
        minPriceE8,
        maxPriceE8,
        ignoredMarketIds,
        skipUnscopedProtocolEvents,
      });
      perpsEventIdx += 1;
      if (outcome.accepted) {
        accepted.push(outcome.record);
      } else if (!outcome.ignored) {
        rejected.push(outcome);
      }
    }
  }

  return { accepted, rejected };
}

/**
 * Parse all verified Perps contract events inside a finalized FastNear NEAR Data block payload
 * (`{ block, shards }`), scanning BOTH shard chunk transaction outcomes (same-shard local receipts)
 * AND `receipt_execution_outcomes` (incoming/cross-shard receipts).
 */
export function parseNearDataFinalBlock(blockPayload, options = {}) {
  const targetContractId = options.contractId ?? PERPS_CONTRACT_ID;
  const targetMarketId = options.marketId ?? ARABICA_MARKET_ID;
  const minPriceE8 = options.minPriceE8 ?? ARABICA_MIN_PRICE_E8;
  const maxPriceE8 = options.maxPriceE8 ?? ARABICA_MAX_PRICE_E8;
  const ignoredMarketIds = options.ignoredMarketIds
    ? new Set(options.ignoredMarketIds)
    : null;
  const skipUnscopedProtocolEvents = Boolean(options.skipUnscopedProtocolEvents);
  const source = options.source ?? "neardata_final_block";
  const indexedAt = options.indexedAt ?? new Date().toISOString();

  if (
    blockPayload?.optimistic === true ||
    blockPayload?.final === false ||
    blockPayload?.finality === "optimistic"
  ) {
    throw new Error("Rejected non-finalized optimistic block payload.");
  }

  const header = blockPayload?.block?.header;
  if (!header || !Number.isSafeInteger(header.height) || typeof header.hash !== "string") {
    throw new Error("Non-finalized or incomplete NEAR Data block payload.");
  }

  const blockHeight = header.height;
  const blockHash = header.hash;
  const blockTimestamp = parseNanosecTimestampSec(header);
  const shards = Array.isArray(blockPayload?.shards) ? blockPayload.shards : [];

  const events = [];
  const rejected = [];
  let matchedContractRefs = 0;

  for (const shard of shards) {
    const shardId = Number.isSafeInteger(shard?.shard_id) ? shard.shard_id : null;
    const seenReceiptIdsInShard = new Set();

    // 1. Inspect chunk transactions (direct calls / local receipt execution outcomes)
    const transactions = Array.isArray(shard?.chunk?.transactions) ? shard.chunk.transactions : [];
    for (const txEntry of transactions) {
      const txReceiver = txEntry?.transaction?.receiver_id;
      const execOutcome = txEntry?.outcome?.execution_outcome;
      const executorId = execOutcome?.outcome?.executor_id;

      if (txReceiver === targetContractId || executorId === targetContractId) {
        matchedContractRefs += 1;
      }

      if (execOutcome && executorId === targetContractId) {
        const receiptId = execOutcome.id ?? txEntry?.transaction?.hash ?? "local-tx";
        seenReceiptIdsInShard.add(receiptId);
        const parsed = parseLogsForReceipt({
          logs: execOutcome.outcome?.logs,
          status: execOutcome.outcome?.status,
          executorId,
          receiptId,
          txHash: txEntry?.transaction?.hash ?? null,
          shardId,
          blockHeight,
          blockHash,
          blockTimestamp,
          source,
          targetContractId,
          targetMarketId,
          minPriceE8,
          maxPriceE8,
          ignoredMarketIds,
          skipUnscopedProtocolEvents,
          indexedAt,
        });
        events.push(...parsed.accepted);
        rejected.push(...parsed.rejected);
      }
    }

    // 2. Inspect incoming receipt count in chunk.receipts for observability
    const chunkReceipts = Array.isArray(shard?.chunk?.receipts) ? shard.chunk.receipts : [];
    for (const rcpt of chunkReceipts) {
      if (rcpt?.receiver_id === targetContractId) {
        matchedContractRefs += 1;
      }
    }

    // 3. Inspect shard receipt_execution_outcomes (primary execution home for cross-shard/async receipts)
    const outcomes = Array.isArray(shard?.receipt_execution_outcomes)
      ? shard.receipt_execution_outcomes
      : [];
    for (const ro of outcomes) {
      const eo = ro?.execution_outcome;
      const executorId = eo?.outcome?.executor_id;
      const receiverId = ro?.receipt?.receiver_id;

      if (receiverId === targetContractId || executorId === targetContractId) {
        matchedContractRefs += 1;
      }

      if (!eo || executorId !== targetContractId) continue;
      const receiptId = eo.id ?? ro?.receipt?.receipt_id ?? "unknown-receipt";
      if (seenReceiptIdsInShard.has(receiptId)) continue;
      seenReceiptIdsInShard.add(receiptId);

      const parsed = parseLogsForReceipt({
        logs: eo.outcome?.logs,
        status: eo.outcome?.status,
        executorId,
        receiptId,
        txHash: ro?.tx_hash ?? null,
        shardId,
        blockHeight,
        blockHash,
        blockTimestamp,
        source,
        targetContractId,
        targetMarketId,
        minPriceE8,
        maxPriceE8,
        ignoredMarketIds,
        skipUnscopedProtocolEvents,
        indexedAt,
      });
      events.push(...parsed.accepted);
      rejected.push(...parsed.rejected);
    }
  }

  return {
    blockHeight,
    blockHash,
    blockTimestamp,
    matchedContractRefs,
    events,
    rejected,
  };
}

/**
 * Parse verified Perps events from RPC `EXPERIMENTAL_tx_status` + `block` header (used solely for
 * M04.6-G immutable evidence backfill, applying identical NEP-297 & market rules).
 */
export function parseRpcTxStatusBackfillOutcome({
  txHash,
  txStatus,
  blockHeader,
  contractId = PERPS_CONTRACT_ID,
  marketId = ARABICA_MARKET_ID,
  minPriceE8 = ARABICA_MIN_PRICE_E8,
  maxPriceE8 = ARABICA_MAX_PRICE_E8,
  ignoredMarketIds = null,
  skipUnscopedProtocolEvents = false,
  source = "rpc_experimental_tx_status_backfill",
  indexedAt = new Date().toISOString(),
}) {
  if (txStatus?.final_execution_status !== "FINAL") {
    return {
      events: [],
      rejected: [
        {
          reason: "non_final_transaction",
          detail: `Backfill tx ${txHash} has final_execution_status="${txStatus?.final_execution_status}".`,
          txHash,
          receiptId: null,
          blockHeight: blockHeader?.height ?? null,
          eventType: null,
        },
      ],
    };
  }

  if (!isReceiptStatusSuccessful(txStatus?.status)) {
    return {
      events: [],
      rejected: [
        {
          reason: "failed_transaction",
          detail: `Backfill tx ${txHash} top-level status failed.`,
          txHash,
          receiptId: null,
          blockHeight: blockHeader?.height ?? null,
          eventType: null,
        },
      ],
    };
  }

  const blockHeight = blockHeader.height;
  const blockHash = blockHeader.hash;
  const blockTimestamp = parseNanosecTimestampSec(blockHeader);
  const receipts = Array.isArray(txStatus?.receipts_outcome) ? txStatus.receipts_outcome : [];

  if (receipts.some((r) => !isReceiptStatusSuccessful(r?.outcome?.status))) {
    return {
      events: [],
      rejected: [
        {
          reason: "failed_receipt",
          detail: `Backfill tx ${txHash} contains a failed sub-receipt.`,
          txHash,
          receiptId: null,
          blockHeight,
          eventType: null,
        },
      ],
    };
  }

  const events = [];
  const rejected = [];

  for (const receipt of receipts) {
    if (receipt?.outcome?.executor_id !== contractId) continue;
    const parsed = parseLogsForReceipt({
      logs: receipt.outcome.logs,
      status: receipt.outcome.status,
      executorId: receipt.outcome.executor_id,
      receiptId: receipt.id,
      txHash: txStatus?.transaction?.hash ?? txHash,
      // RPC EXPERIMENTAL_tx_status does not report shard_id; leave explicitly null rather than guessing.
      shardId: null,
      blockHeight,
      blockHash,
      blockTimestamp,
      source,
      targetContractId: contractId,
      targetMarketId: marketId,
      minPriceE8,
      maxPriceE8,
      ignoredMarketIds: ignoredMarketIds ? new Set(ignoredMarketIds) : null,
      skipUnscopedProtocolEvents: Boolean(skipUnscopedProtocolEvents),
      indexedAt,
    });
    events.push(...parsed.accepted);
    rejected.push(...parsed.rejected);
  }

  return { events, rejected };
}
