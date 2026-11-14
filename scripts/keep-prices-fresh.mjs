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
  let stopping = false;
  let updating = false;
  let resolveStop;
  const stop = new Promise((resolve) => { resolveStop = resolve; });
  const halt = (signal) => { if (!stopping) { stopping = true; process.stdout.write("\nStopping after current update (" + signal + ").\n"); resolveStop(); } };
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
  process.stdout.write("ROASTAKE Testnet price keeper: " + operator + " → " + contractId + "\n");
  process.stdout.write("Immediate refresh, then every 120 seconds. Ctrl+C stops it.\n");
  await refresh();
  while (!stopping) { await pauseOrStop(intervalMs, stop); if (!stopping) await refresh(); }
}
main().catch((error) => { process.stderr.write("Price keeper stopped: " + (error instanceof Error ? error.message : String(error)) + "\n"); process.exitCode = 1; });
