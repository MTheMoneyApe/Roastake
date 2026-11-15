// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import { parseRpcTxStatusBackfillOutcome } from "./event-parser.mjs";
import { assertTestnetAccountId } from "./config.mjs";

const PAGE_LIMIT = 200;
const MAX_HASH_BATCH = 20;

function countRejections(target, rejections) {
  for (const rejection of rejections) {
    const key = rejection.reason ?? "unknown";
    target[key] = (target[key] ?? 0) + 1;
  }
}

function persistState(primaryStore, state) {
  const cursor = primaryStore.getCursor();
  if (!cursor) throw new Error("Account-history backfill requires an initialized primary cursor.");
  return primaryStore.saveCursor({ ...cursor, accountHistoryBackfill: state });
}

function createInitialState({ accountId, fromHeight, toHeight }) {
  return {
    version: 1,
    network: "testnet",
    accountId,
    fromHeight,
    toHeight,
    filter: "is_receiver=true,is_success=true",
    resumeToken: null,
    nextResumeToken: null,
    pendingRows: [],
    pendingIndex: 0,
    pagesFetched: 0,
    pagesCompleted: 0,
    discoveredTransactions: 0,
    processedTransactions: 0,
    verifiedTransactions: 0,
    importedEvents: 0,
    duplicateEvents: 0,
    rejectedCount: 0,
    rejectionsByReason: {},
    completed: false,
    lastTransactionHash: null,
    lastTransactionBlockHeight: null,
    lastSuccessfulAt: null,
    lastError: null,
    consecutiveErrors: 0,
    nextRetryAtMs: null,
    startedAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  };
}

/**
 * Recover contract-receiver transaction history from FastNear's Testnet account index.
 * Every candidate is independently checked through canonical NEAR RPC tx status before
 * any event is appended to a market HistoryStore. Page and row checkpoints are durable;
 * replay after a crash is safe because HistoryStore deduplicates event IDs and oracle sequences.
 *
 * This is transaction/event recovery only. It deliberately does not modify scannedSegments
 * or claim that any unscanned block interval has been inspected.
 */
export async function importAccountHistoryBackfill({
  config,
  blockSource,
  primaryStore,
  marketPipelines,
  finalHeight,
  maxTransactions = 3,
  shouldStop = () => false,
}) {
  if (!config.accountHistoryBackfillEnabled) {
    return { skipped: true, reason: "disabled" };
  }
  if (!config.fastnearApiKey) {
    return { skipped: true, reason: "missing_fastnear_api_key" };
  }
  if (!Number.isSafeInteger(finalHeight) || finalHeight < 1) {
    throw new Error("Account-history backfill requires a valid finalized chain height.");
  }
  if (!Number.isSafeInteger(maxTransactions) || maxTransactions < 1 || maxTransactions > MAX_HASH_BATCH) {
    throw new Error(`maxTransactions must be between 1 and ${MAX_HASH_BATCH}.`);
  }

  const initialCursor = primaryStore.getCursor();
  if (!initialCursor) {
    throw new Error("Account-history backfill cannot start before primary cursor bootstrap.");
  }

  const accountId = assertTestnetAccountId(config.contractId, "history contract");
  const fromHeight = Number.isSafeInteger(config.accountHistoryBackfillFromHeight)
    ? config.accountHistoryBackfillFromHeight
    : 277174473;
  let state = initialCursor.accountHistoryBackfill ?? createInitialState({
    accountId,
    fromHeight,
    toHeight: finalHeight + 1,
  });

  if (!initialCursor.accountHistoryBackfill) {
    persistState(primaryStore, state);
  }

  if (state.accountId !== accountId || state.fromHeight !== fromHeight) {
    throw new Error(
      "Persisted account-history backfill scope does not match current contract/from-height configuration; refusing to reset its checkpoint.",
    );
  }
  if (!Number.isSafeInteger(state.toHeight) || state.toHeight <= state.fromHeight) {
    throw new Error("Persisted account-history backfill range is invalid.");
  }
  if (state.completed) {
    return { skipped: true, reason: "already_completed", state };
  }
  if (Number.isFinite(state.nextRetryAtMs) && state.nextRetryAtMs > Date.now()) {
    return { skipped: true, reason: "retry_backoff", retryAt: state.nextRetryAtMs, state };
  }

  let processedThisRun = 0;
  let appendedThisRun = 0;
  let verifiedThisRun = 0;

  while (!shouldStop() && processedThisRun < maxTransactions && !state.completed) {
    if (state.pendingIndex >= state.pendingRows.length) {
      if (state.pendingRows.length > 0) {
        const nextToken = state.nextResumeToken;
        if (nextToken && nextToken === state.resumeToken) {
          throw new Error("FastNear returned a repeated resume_token; refusing an infinite pagination loop.");
        }
        state = {
          ...state,
          resumeToken: nextToken,
          nextResumeToken: null,
          pendingRows: [],
          pendingIndex: 0,
          pagesCompleted: state.pagesCompleted + 1,
          completed: !nextToken,
          updatedAt: new Date().toISOString(),
          lastError: null,
          consecutiveErrors: 0,
          nextRetryAtMs: null,
        };
        persistState(primaryStore, state);
        if (state.completed) break;
      }

      if (state.pendingRows.length === 0 && !state.completed) {
        const page = await blockSource.fetchAccountTransactionPage({
          accountId,
          fromHeight: state.fromHeight,
          toHeight: state.toHeight,
          resumeToken: state.resumeToken,
          limit: PAGE_LIMIT,
          filters: { is_receiver: true, is_success: true },
        });

        const pendingRows = page.accountTransactions.map((row) => {
          if (row.tx_block_height < state.fromHeight || row.tx_block_height >= state.toHeight) {
            throw new Error("FastNear returned a transaction outside the requested historical height range.");
          }
          return {
            transactionHash: row.transaction_hash,
            transactionBlockHeight: row.tx_block_height,
          };
        });
        state = {
          ...state,
          pendingRows,
          pendingIndex: 0,
          nextResumeToken: page.resumeToken,
          pagesFetched: state.pagesFetched + 1,
          discoveredTransactions: state.discoveredTransactions + pendingRows.length,
          updatedAt: new Date().toISOString(),
          lastError: null,
        };
        persistState(primaryStore, state);

        if (pendingRows.length === 0) {
          if (page.resumeToken && page.resumeToken === state.resumeToken) {
            throw new Error("FastNear returned an empty page with an unchanged resume_token.");
          }
          state = {
            ...state,
            resumeToken: page.resumeToken,
            nextResumeToken: null,
            pagesCompleted: state.pagesCompleted + 1,
            completed: !page.resumeToken,
            updatedAt: new Date().toISOString(),
          };
          persistState(primaryStore, state);
          continue;
        }
      }
    }

    const batchRows = state.pendingRows.slice(
      state.pendingIndex,
      Math.min(state.pendingRows.length, state.pendingIndex + (maxTransactions - processedThisRun)),
    );
    const hashes = batchRows.map((row) => row.transactionHash);
    const indexedTransactions = await blockSource.fetchTransactionsByHash(hashes);
    const transactionDetails = new Map();
    for (const indexed of indexedTransactions) {
      const transaction = indexed?.transaction;
      if (typeof transaction?.hash === "string") {
        transactionDetails.set(transaction.hash, transaction);
      }
    }

    for (const row of batchRows) {
      if (shouldStop() || processedThisRun >= maxTransactions) break;

      const txHash = row.transactionHash;
      const detail = transactionDetails.get(txHash);
      if (!detail) {
        throw new Error(`FastNear transaction lookup did not return requested hash ${txHash}.`);
      }
      const signerAccountId = assertTestnetAccountId(detail.signer_id, "historical transaction signer");
      const {
        txStatus,
        blockHeader,
        txBlockHeader,
        blockHeadersByHash,
      } = await blockSource.fetchAccountHistoryTxWithBlocks(
        txHash,
        signerAccountId,
      );

      if (txBlockHeader?.height !== row.transactionBlockHeight) {
        throw new Error(
          `FastNear/RPC transaction block-height mismatch for ${txHash}: index=${row.transactionBlockHeight}, rpc=${txBlockHeader?.height ?? "missing"}.`,
        );
      }

      if (txStatus?.transaction?.hash !== txHash) {
        throw new Error(`RPC transaction hash mismatch for historical transaction ${txHash}.`);
      }
      if (txStatus?.transaction?.signer_id !== signerAccountId) {
        throw new Error(`RPC signer mismatch for historical transaction ${txHash}.`);
      }

      let transactionVerified = false;
      let transactionImported = 0;
      let transactionDuplicates = 0;
      const transactionRejections = [];

      for (const pipeline of marketPipelines) {
        const marketIds = marketPipelines
          .map((candidate) => candidate.spec.marketId)
          .filter((id) => id !== pipeline.spec.marketId);
        const parsed = parseRpcTxStatusBackfillOutcome({
          txHash,
          txStatus,
          blockHeader,
          blockHeadersByHash,
          contractId: config.contractId,
          marketId: pipeline.spec.marketId,
          minPriceE8: pipeline.spec.minPriceE8,
          maxPriceE8: pipeline.spec.maxPriceE8,
          ignoredMarketIds: marketIds,
          skipUnscopedProtocolEvents: marketPipelines.length > 1,
          source: "fastnear_account_history_rpc_verified",
        });

        if (parsed.rejected.some((item) => item.reason === "non_final_transaction")) {
          throw new Error(`Historical transaction ${txHash} is not FINAL on RPC; leaving it pending for retry.`);
        }
        transactionVerified = transactionVerified || parsed.events.length > 0 ||
          (txStatus?.final_execution_status === "FINAL");
        const appendResult = pipeline.store.appendEvents(parsed.events);
        transactionImported += appendResult.appended.length;
        transactionDuplicates += appendResult.duplicateRejections.length;
        transactionRejections.push(...parsed.rejected);
        transactionRejections.push(...appendResult.duplicateRejections);
      }

      const rejectionsByReason = { ...(state.rejectionsByReason ?? {}) };
      countRejections(rejectionsByReason, transactionRejections);
      state = {
        ...state,
        pendingIndex: state.pendingIndex + 1,
        processedTransactions: state.processedTransactions + 1,
        verifiedTransactions: state.verifiedTransactions + (transactionVerified ? 1 : 0),
        importedEvents: state.importedEvents + transactionImported,
        duplicateEvents: state.duplicateEvents + transactionDuplicates,
        rejectedCount: state.rejectedCount + transactionRejections.length,
        rejectionsByReason,
        lastTransactionHash: txHash,
        lastTransactionBlockHeight: row.transactionBlockHeight,
        lastSuccessfulAt: new Date().toISOString(),
        updatedAt: new Date().toISOString(),
        lastError: null,
        consecutiveErrors: 0,
        nextRetryAtMs: null,
      };
      persistState(primaryStore, state);
      processedThisRun += 1;
      verifiedThisRun += transactionVerified ? 1 : 0;
      appendedThisRun += transactionImported;
    }
  }

  if (!state.completed && state.pendingRows.length > 0 &&
      state.pendingIndex >= state.pendingRows.length) {
    const nextToken = state.nextResumeToken;
    if (nextToken && nextToken === state.resumeToken) {
      throw new Error("FastNear returned a repeated resume_token; refusing an infinite pagination loop.");
    }
    state = {
      ...state,
      resumeToken: nextToken,
      nextResumeToken: null,
      pendingRows: [],
      pendingIndex: 0,
      pagesCompleted: state.pagesCompleted + 1,
      completed: !nextToken,
      updatedAt: new Date().toISOString(),
      lastError: null,
    };
    persistState(primaryStore, state);
  }

  return {
    skipped: false,
    completed: state.completed,
    fromHeight: state.fromHeight,
    toHeightExclusive: state.toHeight,
    pagesFetched: state.pagesFetched,
    pagesCompleted: state.pagesCompleted,
    discoveredTransactions: state.discoveredTransactions,
    processedTransactions: state.processedTransactions,
    verifiedTransactions: state.verifiedTransactions,
    importedEvents: state.importedEvents,
    duplicateEvents: state.duplicateEvents,
    processedThisRun,
    verifiedThisRun,
    appendedThisRun,
    pendingPageTransactions: Math.max(0, state.pendingRows.length - state.pendingIndex),
    resumePending: Boolean(state.nextResumeToken || state.resumeToken || state.pendingRows.length),
    state,
  };
}
