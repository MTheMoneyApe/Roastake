// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

//! Structured NEP-297 JSON event emitters for ROASTAKE Perpetuals (`roastake-perps`).

use near_sdk::env;
use serde::Serialize;

pub const EVENT_STANDARD: &str = "roastake-perps";
pub const EVENT_VERSION: &str = "1.0.0";

#[derive(Serialize)]
struct Nep297Envelope<'a, T: Serialize> {
    standard: &'static str,
    version: &'static str,
    event: &'a str,
    data: [&'a T; 1],
}

pub fn emit_event<T: Serialize>(event: &str, payload: &T) {
    let envelope = Nep297Envelope {
        standard: EVENT_STANDARD,
        version: EVENT_VERSION,
        event,
        data: [payload],
    };
    let encoded = serde_json::to_string(&envelope)
        .unwrap_or_else(|e| env::panic_str(&format!("ERR_EVENT_SERIALIZE: {e}")));
    env::log_str(&format!("EVENT_JSON:{encoded}"));
}

#[derive(Serialize)]
pub struct CollateralDepositedEvent<'a> {
    pub account_id: &'a str,
    pub token_id: &'a str,
    pub amount_token_units: String,
    pub amount_e8: String,
    pub new_free_collateral_e8: String,
}

#[derive(Serialize)]
pub struct CollateralWithdrawnEvent<'a> {
    pub account_id: &'a str,
    pub token_id: &'a str,
    pub withdrawal_id: u64,
    pub amount_token_units: String,
    pub amount_e8: String,
    pub status: &'a str,
}

#[derive(Serialize)]
pub struct LpDepositedEvent<'a> {
    pub account_id: &'a str,
    pub token_id: &'a str,
    pub amount_e8: String,
    pub shares_minted: String,
    pub seed_shares_locked: String,
    pub nav_e8: String,
}

#[derive(Serialize)]
pub struct LpWithdrawnEvent<'a> {
    pub account_id: &'a str,
    pub token_id: &'a str,
    pub withdrawal_id: u64,
    pub shares_burned: String,
    pub assets_out_e8: String,
    pub transfer_units: String,
    pub residue_to_free_e8: String,
    pub nav_e8: String,
}

#[derive(Serialize)]
pub struct PositionOpenedEvent<'a> {
    pub position_id: u64,
    pub owner: &'a str,
    pub market_id: &'a str,
    pub side: &'a str,
    pub size_e8: String,
    pub notional_e8: String,
    pub exec_price_e8: String,
    pub margin_e8: String,
    pub fee_e8: String,
    pub funding_index_e12: String,
}

#[derive(Serialize)]
pub struct PositionIncreasedEvent<'a> {
    pub position_id: u64,
    pub owner: &'a str,
    pub added_size_e8: String,
    pub new_size_e8: String,
    pub new_entry_price_e8: String,
    pub added_margin_e8: String,
    pub fee_e8: String,
}

#[derive(Serialize)]
pub struct PositionReducedEvent<'a> {
    pub position_id: u64,
    pub owner: &'a str,
    pub closed_size_e8: String,
    pub remaining_size_e8: String,
    pub exec_price_e8: String,
    pub realized_pnl_e8: String,
    pub funding_settled_e8: String,
    pub fee_e8: String,
    pub trader_payout_e8: String,
}

#[derive(Serialize)]
pub struct PositionClosedEvent<'a> {
    pub position_id: u64,
    pub owner: &'a str,
    pub market_id: &'a str,
    pub exec_price_e8: String,
    pub realized_pnl_e8: String,
    pub funding_settled_e8: String,
    pub fee_e8: String,
    pub payout_e8: String,
    pub bad_debt_e8: String,
    pub emergency: bool,
}

#[derive(Serialize)]
pub struct PositionLiquidatedEvent<'a> {
    pub position_id: u64,
    pub owner: &'a str,
    pub keeper: &'a str,
    pub market_id: &'a str,
    pub mark_price_e8: String,
    pub keeper_fee_e8: String,
    pub insurance_fee_e8: String,
    pub close_fee_e8: String,
    pub trader_refund_e8: String,
    pub bad_debt_e8: String,
}

#[derive(Serialize)]
pub struct MarginAdjustedEvent<'a> {
    pub position_id: u64,
    pub owner: &'a str,
    pub delta_margin_e8: String,
    pub new_locked_margin_e8: String,
    pub mark_price_e8: Option<String>,
}

#[derive(Serialize)]
pub struct FundingAccruedEvent<'a> {
    pub market_id: &'a str,
    pub elapsed_sec: u64,
    pub index_delta_e12: String,
    pub new_cum_funding_index_e12: String,
}

#[derive(Serialize)]
pub struct InsuranceUpdatedEvent<'a> {
    pub reason: &'a str,
    pub delta_e8: String,
    pub new_balance_e8: String,
    pub uncovered_insolvency_e8: String,
}

#[derive(Serialize)]
pub struct MarketModeChangedEvent<'a> {
    pub market_id: &'a str,
    pub triggered_by: &'a str,
    pub previous_mode: &'a str,
    pub new_mode: &'a str,
    pub reason: &'a str,
    pub frozen_settlement_price_e8: Option<String>,
    pub frozen_funding_index_e12: Option<String>,
}

#[derive(Serialize)]
pub struct OracleSandboxTickEvent<'a> {
    pub market_id: &'a str,
    pub publisher: &'a str,
    pub sequence: u64,
    pub price_e8: String,
    pub observed_at_sec: u64,
    pub data_class: &'a str,
}

#[derive(Serialize)]
pub struct SecurityAnomalyEvent<'a> {
    pub kind: &'a str,
    pub actor: &'a str,
    pub detail: &'a str,
}
