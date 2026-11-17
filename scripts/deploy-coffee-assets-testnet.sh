#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
# Copyright © 2026 @MTheMoneyApe. All rights reserved.
#
# M04.18 — Coffee Assets: three NEP-141 Testnet tokens + three RHEA (Ref Finance)
# USDC pools. Run inside WSL2, where near-cli-rs and the legacy keychain live:
#
#   wsl -- bash /mnt/c/Users/M/Documents/Roastake/scripts/deploy-coffee-assets-testnet.sh <step>
#
# Steps (each is idempotent and independently re-runnable):
#   accounts   create the three Testnet sub-accounts (0.15 NEAR each)
#   deploy     deploy + initialize the reusable Coffee Asset token into each account
#   mint       register ROASTAKE admin + RHEA exchange on each token, mint seed supply
#   pools      register admin/tokens on the exchange, create 3 pools, deposit seed funds
#   liquidity  add_liquidity to each pool (1.00 USDC + seed kg per pool)
#   swaps      three small USDC -> coffee swap smoke tests
#   all        accounts + deploy + mint
set -euo pipefail

REPO="/mnt/c/Users/M/Documents/Roastake"
WASM="$REPO/target/m0418-coffee-token-out/roastake_coffee_token.wasm"
STATE="$REPO/target/m0418-coffee-state.env"
ADMIN="roastake-dev-mta.testnet"
REF="ref-finance-101.testnet"
USDC="3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af"
NET="testnet-fastnear"
RPC_FASTNEAR="https://rpc.testnet.fastnear.com"
STEP="${1:-all}"

export PATH="$HOME/.cargo/bin:$PATH"

# variant | account | symbol | name | supply_cap (base units) | reference_usd_e8 | seed (base units)
VARIANTS=(
  "COFFEE-ARABICA|arabica.roastake-dev-mta.testnet|rARABICA|ROASTAKE Testnet Arabica|1000000000|1000000000|10000000"
  "COFFEE-ROBUSTA|robusta.roastake-dev-mta.testnet|rROBUSTA|ROASTAKE Testnet Robusta|2000000000|500000000|20000000"
  "COFFEE-GESHA|gesha.roastake-dev-mta.testnet|rGESHA|ROASTAKE Testnet Gesha|400000000|2500000000|4000000"
)
SEED_USDC="1000000"    # 1.00 USDC (6 decimals) per pool
DEPOSIT_USDC="3150000" # 3.15 USDC: 3.00 liquidity + 0.15 swap buffer

log() { printf '\n=== %s ===\n' "$*"; }

tx() { # account method json gas deposit [signer]
  local account="$1" method="$2" args="$3" gas="$4" deposit="$5" signer="${6:-$ADMIN}"
  near contract call-function as-transaction "$account" "$method" json-args "$args" \
    prepaid-gas "$gas" attached-deposit "$deposit" sign-as "$signer" \
    network-config "$NET" sign-with-legacy-keychain send 2>&1 |
    grep -E 'Transaction ID|"SuccessValue"|Failure|Error:|error:' || true
}

ro() { # account method json -> stdout value only (RPC view call, no CLI rate limit)
  python3 - "$RPC_FASTNEAR" "$1" "$2" "$3" <<'PY'
import sys, json, base64, urllib.request, urllib.error
rpc, acct, method, args = sys.argv[1:5]
body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "query", "params": {
    "request_type": "call_function", "finality": "final", "account_id": acct,
    "method_name": method, "args_base64": base64.b64encode(args.encode()).decode()}}).encode()
req = urllib.request.Request(rpc, data=body, headers={"content-type": "application/json"})
try:
    res = json.load(urllib.request.urlopen(req, timeout=30))
except Exception:
    sys.exit(0)
if "error" in res:
    sys.exit(0)
print(bytes(res["result"]["result"]).decode())
PY
}

account_exists() { near account view-account-summary "$1" network-config "$NET" now >/dev/null 2>&1; }

find_pool_id() { # token_a token_b -> prints pool id or nothing
  local a="$1" b="$2" n i raw
  n=$(ro "$REF" get_number_of_pools '{}')
  n=${n//[!0-9]/}
  [[ -z "$n" ]] && return 1
  for ((i = n - 40; i < n; i++)); do
    raw=$(ro "$REF" get_pool "{\"pool_id\":$i}")
    if [[ "$raw" == *"$a"* && "$raw" == *"$b"* ]]; then
      echo "$i"
      return 0
    fi
  done
  return 1
}

step_accounts() {
  local row variant account symbol name cap price seed
  for row in "${VARIANTS[@]}"; do
    IFS='|' read -r variant account symbol name cap price seed <<<"$row"
    if account_exists "$account"; then
      log "SKIP create $account (exists)"
    else
      log "CREATE $account"
      near account create-account fund-myself "$account" '0.15 NEAR' autogenerate-new-keypair \
        save-to-legacy-keychain sign-as "$ADMIN" network-config "$NET" sign-with-legacy-keychain send 2>&1 |
        grep -E 'successfully created|Transaction ID|Error:' || true
    fi
  done
}

step_deploy() {
  local row variant account symbol name cap price seed init_args
  for row in "${VARIANTS[@]}"; do
    IFS='|' read -r variant account symbol name cap price seed <<<"$row"
    log "DEPLOY $account ($symbol)"
    init_args=$(printf '{"owner_id":"%s","minter_id":"%s","variant":"%s","name":"%s","symbol":"%s","supply_cap":"%s","reference_price_usd_e8":"%s","icon":null,"reference":"Synthetic NEAR Testnet reference price. SIMULATED_TESTNET_INVENTORY only; no physical custody, warehouse or redemption claim."}' \
      "$ADMIN" "$ADMIN" "$variant" "$name" "$symbol" "$cap" "$price")
    near contract deploy "$account" use-file "$WASM" with-init-call new json-args "$init_args" \
      prepaid-gas '100 Tgas' attached-deposit '0 NEAR' network-config "$NET" \
      sign-with-legacy-keychain send 2>&1 | grep -E 'Transaction ID|Error:|error:' || true
  done
}

step_mint() {
  local row variant account symbol name cap price seed
  for row in "${VARIANTS[@]}"; do
    IFS='|' read -r variant account symbol name cap price seed <<<"$row"
    log "REGISTER ADMIN on $symbol"
    tx "$account" storage_deposit "{\"account_id\":\"$ADMIN\",\"registration_only\":true}" '30 Tgas' '0.00125 NEAR'
    log "REGISTER RHEA EXCHANGE on $symbol"
    tx "$account" storage_deposit "{\"account_id\":\"$REF\",\"registration_only\":true}" '30 Tgas' '0.00125 NEAR'
    log "MINT seed $symbol ($seed base units)"
    tx "$account" mint "{\"account_id\":\"$ADMIN\",\"amount\":\"$seed\"}" '30 Tgas' '0 NEAR'
  done
}

step_pools() {
  local row variant account symbol name cap price seed
  log "REGISTER ADMIN IN RHEA EXCHANGE"
  tx "$REF" storage_deposit "{\"account_id\":\"$ADMIN\",\"registration_only\":true}" '30 Tgas' '1.02 NEAR'
  STORAGE_MARK="$REPO/target/m0418-coffee-storage.marker"
  if [[ ! -f "$STORAGE_MARK" ]]; then
    log "TOP UP EXCHANGE STORAGE (+0.25 NEAR free balance for token registration)"
    tx "$REF" storage_deposit "{\"account_id\":\"$ADMIN\",\"registration_only\":false}" '30 Tgas' '0.25 NEAR'
    touch "$STORAGE_MARK"
  fi
  for row in "${VARIANTS[@]}"; do
    IFS='|' read -r variant account symbol name cap price seed <<<"$row"
    log "REGISTER $symbol IN EXCHANGE"
    tx "$REF" register_tokens "{\"token_ids\":[\"$account\"]}" '50 Tgas' '1 yoctoNEAR'
  done
  : >"$STATE"
  for row in "${VARIANTS[@]}"; do
    IFS='|' read -r variant account symbol name cap price seed <<<"$row"
    pool_id=$(find_pool_id "$account" "$USDC" || true)
    if [[ -n "${pool_id:-}" ]]; then
      log "SKIP create $symbol / USDC (existing pool $pool_id)"
    else
      log "CREATE POOL $symbol / USDC (fee 30)"
      tx "$REF" add_simple_pool "{\"tokens\":[\"$account\",\"$USDC\"],\"fee\":30}" '100 Tgas' '0.1 NEAR'
      pool_id=$(find_pool_id "$account" "$USDC" || true)
    fi
    if [[ -z "${pool_id:-}" ]]; then
      echo "POOL_ID_NOT_FOUND for $symbol" >&2
      exit 1
    fi
    echo "$symbol=$pool_id" >>"$STATE"
    log "POOL $symbol = $pool_id"
  done
  log "DEPOSIT USDC INTO EXCHANGE (target $DEPOSIT_USDC base units)"
  cur_usdc=$(ro "$REF" get_deposit "{\"account_id\":\"$ADMIN\",\"token_id\":\"$USDC\"}")
  cur_usdc=${cur_usdc//[!0-9]/}
  cur_usdc=${cur_usdc:-0}
  if (($cur_usdc < DEPOSIT_USDC)); then
    tx "$USDC" ft_transfer_call "{\"receiver_id\":\"$REF\",\"amount\":\"$((DEPOSIT_USDC - cur_usdc))\",\"msg\":\"\"}" '100 Tgas' '1 yoctoNEAR'
  else
    log "SKIP USDC deposit (already $cur_usdc)"
  fi
  for row in "${VARIANTS[@]}"; do
    IFS='|' read -r variant account symbol name cap price seed <<<"$row"
    cur=$(ro "$REF" get_deposit "{\"account_id\":\"$ADMIN\",\"token_id\":\"$account\"}")
    cur=${cur//[!0-9]/}
    cur=${cur:-0}
    if (($cur < seed)); then
      log "DEPOSIT seed $symbol INTO EXCHANGE ($((seed - cur)))"
      tx "$account" ft_transfer_call "{\"receiver_id\":\"$REF\",\"amount\":\"$((seed - cur))\",\"msg\":\"\"}" '100 Tgas' '1 yoctoNEAR'
    else
      log "SKIP seed deposit $symbol (already $cur)"
    fi
  done
  cat "$STATE"
}

step_liquidity() {
  local row variant account symbol name cap price seed pool_id
  source "$STATE"
  for row in "${VARIANTS[@]}"; do
    IFS='|' read -r variant account symbol name cap price seed <<<"$row"
    pool_id="${!symbol}"
    log "ADD LIQUIDITY $symbol pool=$pool_id [$seed, $SEED_USDC]"
    tx "$REF" add_liquidity "{\"pool_id\":$pool_id,\"amounts\":[\"$seed\",\"$SEED_USDC\"]}" '100 Tgas' '0.001 NEAR'
  done
}

step_swaps() {
  local row variant account symbol name cap price seed pool_id quote min_out
  source "$STATE"
  for row in "${VARIANTS[@]}"; do
    IFS='|' read -r variant account symbol name cap price seed <<<"$row"
    pool_id="${!symbol}"
    quote=$(ro "$REF" get_return "{\"pool_id\":$pool_id,\"token_in\":\"$USDC\",\"amount_in\":\"50000\",\"token_out\":\"$account\"}")
    quote=${quote//[!0-9]/}
    if [[ -z "$quote" || "$quote" == "0" ]]; then
      echo "QUOTE_UNAVAILABLE for $symbol — refusing to swap" >&2
      exit 1
    fi
    min_out=$(awk -v q="$quote" 'BEGIN { printf "%d", (q * 99) / 100 }')
    log "SWAP 0.05 USDC -> $symbol (pool $pool_id, quote $quote, min $min_out)"
    tx "$USDC" ft_transfer_call \
      "{\"receiver_id\":\"$REF\",\"amount\":\"50000\",\"msg\":\"{\\\"force\\\":0,\\\"actions\\\":[{\\\"pool_id\\\":$pool_id,\\\"token_in\\\":\\\"$USDC\\\",\\\"token_out\\\":\\\"$account\\\",\\\"amount_in\\\":\\\"50000\\\",\\\"min_amount_out\\\":\\\"$min_out\\\"}]}\"}" \
      '100 Tgas' '1 yoctoNEAR'
  done
}

case "$STEP" in
accounts) step_accounts ;;
deploy) step_deploy ;;
mint) step_mint ;;
pools) step_pools ;;
liquidity) step_liquidity ;;
swaps) step_swaps ;;
all | tokens)
  step_accounts
  step_deploy
  step_mint
  ;;
*)
  echo "usage: $0 [accounts|deploy|mint|pools|liquidity|swaps|all]" >&2
  exit 2
  ;;
esac
