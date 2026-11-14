#!/usr/bin/env node
// Copyright (c) 2026 @MTheMoneyApe. Testnet operator utility. Keep credentials local.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Account, JsonRpcProvider, teraToGas } from "near-api-js";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const envFile = path.join(root, ".env");
const contractId = "perps-v1.roastake-dev-mta.testnet";
const intervalMs = 120_000;
const oraclePingIntervalMs = 15 * 60 * 1000;
const oraclePingTimeoutMs = 120_000;
const markets = [
  { id: "COFFEE-ARABICA-TESTNET/USD", name: "Arabica", price: "1000000000", display: "$10.00/kg" },
  { id: "COFFEE-ROBUSTA-TESTNET/USD", name: "Robusta", price: "500000000", display: "$5.00/kg" },
  { id: "COFFEE-GESHA-TESTNET/USD", name: "Gesha", price: "2500000000", display: "$25.00/kg" },
];
function loadEnv() {
  if (!fs.existsSync(envFile)) return;
  for (const raw of fs.readFileSync(envFile, "utf8").split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const match = line.match(/^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/);
    if (!match || process.env[match[1]] !== undefined) continue;
    let value = match[2].trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) value = value.slice(1, -1);
    else value = value.replace(/\s+#.*$/, "");
    process.env[match[1]] = value;
  }
}
function loadOptionalOracleConfig(primaryOperator) {
  if (String(process.env.ORACLE_ADAPTER_ENABLED || "").trim().toLowerCase() !== "true") return null;

  const contractId = (process.env.ORACLE_ADAPTER_CONTRACT_ID || "roastake-oracle-v1.testnet").trim();
  const market = (process.env.ORACLE_ADAPTER_MARKET || "GESHA-DEMO/USD").trim();
  const operator = (process.env.ORACLE_OPERATOR_ACCOUNT_ID || "").trim();
  const privateKey = (process.env.ORACLE_OPERATOR_PRIVATE_KEY || "").trim();
  const depositRaw = (process.env.ORACLE_ADAPTER_DEPOSIT_YOCTO || "").trim();
  const missing = [];
  if (!operator) missing.push("ORACLE_OPERATOR_ACCOUNT_ID");
  if (!privateKey) missing.push("ORACLE_OPERATOR_PRIVATE_KEY");
  if (!depositRaw) missing.push("ORACLE_ADAPTER_DEPOSIT_YOCTO");
  if (missing.length) {
    process.stderr.write("Oracle Adapter ping disabled: missing " + missing.join(", ") + ". Perps price updates will continue.\\n");
    return null;
  }
  if (!contractId.endsWith(".testnet") || contractId.endsWith(".near")) {
    process.stderr.write("Oracle Adapter ping disabled: contract must be a .testnet account.\\n");
    return null;
  }
  if (!operator.endsWith(".testnet") || operator.endsWith(".near")) {
    process.stderr.write("Oracle Adapter ping disabled: signer must be a .testnet account.\\n");
    return null;
  }
  if (operator === primaryOperator) {
    process.stderr.write("Oracle Adapter ping disabled: use a separate Testnet signer to avoid nonce contention with the Perps keeper.\\n");
    return null;
  }
  if (!market || market.length > 64) {
    process.stderr.write("Oracle Adapter ping disabled: ORACLE_ADAPTER_MARKET must be a non-empty market symbol of at most 64 characters.\\n");
    return null;
  }
  if (!/^\d+$/.test(depositRaw) || BigInt(depositRaw) <= 0n) {
    process.stderr.write("Oracle Adapter ping disabled: ORACLE_ADAPTER_DEPOSIT_YOCTO must be a positive integer.\\n");
    return null;
  }
  return { contractId, market, operator, privateKey, deposit: BigInt(depositRaw) };
}

function required(name) {
  const value = process.env[name] && process.env[name].trim();
  if (!value) throw new Error("Missing " + name + " in " + envFile + ".");
  return value;
}
async function pauseOrStop(ms, stop) {
  let timer;
  try {
    await Promise.race([
      new Promise((resolve) => {
        timer = setTimeout(resolve, ms);
      }),
      stop,
    ]);
  } finally {
    // Do not leave the interval timer alive after SIGINT/SIGTERM.
    if (timer) clearTimeout(timer);
  }
}
async function main() {
  loadEnv();
  const operator = required("NEAR_OPERATOR_ACCOUNT_ID");
  const key = required("NEAR_OPERATOR_PRIVATE_KEY");
  const rpcUrl = process.env.NEAR_TESTNET_RPC_URL || "https://rpc.testnet.fastnear.com";
  const configuredContract = process.env.PERPS_CONTRACT_ID || contractId;
  if (!operator.endsWith(".testnet") || operator.endsWith(".near")) throw new Error("Operator must be a .testnet account.");
  if (configuredContract !== contractId) throw new Error("This helper is locked to " + contractId + ".");
  const rpc = new URL(rpcUrl);
  if (rpc.protocol !== "https:" || !["rpc.testnet.fastnear.com", "rpc.testnet.near.org"].includes(rpc.hostname)) throw new Error("Use an approved HTTPS NEAR Testnet RPC host.");
  const provider = new JsonRpcProvider({ url: rpcUrl });
  const account = new Account(operator, provider, key);
  const oracleConfig = loadOptionalOracleConfig(operator);
  const oracleAccount = oracleConfig ? new Account(oracleConfig.operator, provider, oracleConfig.privateKey) : null;
  let stopping = false;
  let updating = false;
  let oraclePingInFlight = false;
  let oraclePingTimer = null;
  let resolveStop;
  const stop = new Promise((resolve) => { resolveStop = resolve; });
  const halt = (signal) => {
    if (!stopping) {
      stopping = true;
      if (oraclePingTimer) clearInterval(oraclePingTimer);
      process.stdout.write("\nStopping after current update (" + signal + ").\n");
      resolveStop();
    }
  };
  process.on("SIGINT", () => halt("SIGINT"));
  process.on("SIGTERM", () => halt("SIGTERM"));
  async function refresh() {
    if (updating) return;
    updating = true;
    try {
      for (const market of markets) {
        if (stopping) break;
        try {
          const state = await provider.callFunction({ contractId, method: "get_market", args: { market_id: market.id }, blockQuery: { finality: "final" } });
          if (!state) throw new Error("Market is not registered.");
          if (state.feed_classification !== "TESTNET_SANDBOX_FEED" || state.is_synthetic_testnet_sandbox !== true) throw new Error("Unexpected feed type.");
          if (state.sandbox_operator !== operator) throw new Error("Operator mismatch; refusing to sign.");
          if (state.mode === "Emergency") throw new Error("Market is in Emergency mode.");
          const price = BigInt(market.price);
          if (price < BigInt(state.sandbox_min_price_e8 || "0") || price > BigInt(state.sandbox_max_price_e8 || "0")) throw new Error("Price outside on-chain bounds.");
          const previous = Number(state.sandbox_sequence || 0);
          if (!Number.isSafeInteger(previous) || previous < 0 || previous >= Number.MAX_SAFE_INTEGER) throw new Error("Invalid or exhausted market sequence.");
          const sequence = previous + 1;
          const result = await account.callFunction({
            contractId, methodName: "push_sandbox_oracle_price",
            args: { market_id: market.id, price_e8: market.price, observed_at_sec: Math.floor(Date.now() / 1000), sequence },
            gas: teraToGas("30"), deposit: 0n, waitUntil: "FINAL"
          });
          const tx = result?.transaction_outcome?.id || result?.transaction?.hash || "finalized";
          process.stdout.write("[" + new Date().toISOString() + "] " + market.name + " " + market.display + " seq=" + sequence + " tx=" + tx + "\n");
        } catch (error) {
          process.stderr.write(market.name + " refresh failed: " + (error instanceof Error ? error.message : String(error)) + "\n");
        }
      }
    } finally { updating = false; }
  }
  async function performOraclePing() {
    const [admin, isRequester, marketConfig, minimumCostRaw] = await Promise.all([
      provider.callFunction({ contractId: oracleConfig.contractId, method: "get_admin", args: {}, blockQuery: { finality: "final" } }),
      provider.callFunction({ contractId: oracleConfig.contractId, method: "is_requester", args: { account_id: oracleConfig.operator }, blockQuery: { finality: "final" } }),
      provider.callFunction({ contractId: oracleConfig.contractId, method: "get_market_config", args: { market: oracleConfig.market }, blockQuery: { finality: "final" } }),
      // Supported WorkerInput variants are far below 1 KiB; use 1 KiB as a conservative
      // input-size bound while retaining the contract's default ResourceLimits.
      provider.callFunction({ contractId: oracleConfig.contractId, method: "estimate_execution_cost", args: { resource_limits: null, input_len: 1024 }, blockQuery: { finality: "final" } }),
    ]);

    if (admin !== oracleConfig.operator && isRequester !== true) {
      throw new Error("Oracle signer is not the adapter admin or an authorized requester.");
    }
    if (!marketConfig || marketConfig.active !== true) {
      throw new Error("Oracle market is missing or inactive: " + oracleConfig.market);
    }

    const minimumValue = minimumCostRaw && typeof minimumCostRaw === "object" && "value" in minimumCostRaw
      ? minimumCostRaw.value
      : minimumCostRaw;
    const minimumDeposit = BigInt(String(minimumValue));
    if (oracleConfig.deposit < minimumDeposit) {
      throw new Error("Oracle deposit is below on-chain estimated minimum (" + minimumDeposit + " yoctoNEAR).");
    }

    const result = await oracleAccount.callFunction({
      contractId: oracleConfig.contractId,
      methodName: "request_price_update",
      args: { market: oracleConfig.market },
      gas: teraToGas("200"),
      deposit: oracleConfig.deposit,
      waitUntil: "FINAL",
    });
    const tx = result?.transaction_outcome?.id || result?.transaction?.hash || "finalized";
    process.stdout.write("[" + new Date().toISOString() + "] Oracle Adapter " + oracleConfig.market + " request finalized tx=" + tx + "\n");
  }

  async function runOraclePing(reason) {
    if (!oracleConfig || stopping || oraclePingInFlight) return;
    oraclePingInFlight = true;
    const task = performOraclePing();
    // Keep the in-flight guard until the underlying RPC/transaction settles, even if the
    // local timeout fires. This prevents duplicate paid requests after an ambiguous timeout.
    task.finally(() => { oraclePingInFlight = false; }).catch(() => {});
    let timeout;
    try {
      await Promise.race([
        task,
        new Promise((_, reject) => {
          timeout = setTimeout(() => reject(new Error("Oracle request exceeded " + oraclePingTimeoutMs + " ms; the Perps keeper continues independently.")), oraclePingTimeoutMs);
        }),
      ]);
    } catch (error) {
      process.stderr.write("[" + new Date().toISOString() + "] Oracle Adapter ping (" + reason + ") failed; Perps updates continue: " + (error instanceof Error ? error.message : String(error)) + "\n");
    } finally {
      if (timeout) clearTimeout(timeout);
    }
  }

  process.stdout.write("ROASTAKE Testnet price keeper: " + operator + " → " + contractId + "\n");
  process.stdout.write("Immediate Perps refresh, then every 120 seconds. Ctrl+C stops it.\n");
  if (oracleConfig) {
    process.stdout.write("Oracle Adapter requests enabled: " + oracleConfig.operator + " → " + oracleConfig.contractId + " / " + oracleConfig.market + " (startup + every 900 seconds).\n");
  } else {
    process.stdout.write("Oracle Adapter requests disabled. Set ORACLE_ADAPTER_ENABLED=true and its dedicated signer/deposit variables to opt in.\n");
  }
  await refresh();
  if (oracleConfig) {
    void runOraclePing("startup");
    oraclePingTimer = setInterval(() => { void runOraclePing("15-minute timer"); }, oraclePingIntervalMs);
  }
  while (!stopping) { await pauseOrStop(intervalMs, stop); if (!stopping) await refresh(); }
}
main().catch((error) => { process.stderr.write("Price keeper stopped: " + (error instanceof Error ? error.message : String(error)) + "\n"); process.exitCode = 1; });
