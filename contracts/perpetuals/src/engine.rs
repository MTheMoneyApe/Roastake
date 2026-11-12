// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

//! Core Perpetuals accounting, execution, funding, liquidation, LP NAV, and emergency settlement engine.

use near_sdk::{env, AccountId};
use roastake_perps_core::{
    advance_funding_elapsed_sec, apply_bad_debt_cascade, apply_profit_cap_usd_e8,
    checked_mul_div_ceil_u128, checked_mul_div_floor_u128, compute_available_margin_usd_e8,
    compute_capped_trader_net_claim_usd_e8, compute_funding_delta_over_interval_e12,
    compute_indicative_liquidation_price_e8, compute_initial_margin_usd_e8,
    compute_liquidation_settlement, compute_lp_available_liquidity_usd_e8, compute_lp_mint_shares,
    compute_lp_mtm_equity_with_insurance_offset_e8, compute_lp_redeem_assets,
    compute_maintenance_margin_usd_e8, compute_margin_ratio_bps, compute_notional_ceil_usd_e8,
    compute_notional_floor_usd_e8, compute_position_equity_usd_e8,
    compute_position_funding_net_usd_e8, compute_position_settlement_e8,
    compute_reserved_liquidity_usd_e8, compute_trading_fee_usd_e8,
    compute_uniform_emergency_settlement_usd_e8, compute_unrealized_pnl_usd_e8,
    compute_utilization_bps, is_liquidatable, PositionSnapshot, BPS_SCALE,
    MIN_FIRST_LP_DEPOSIT_USD_E8,
};

use crate::events::{
    emit_event, FundingAccruedEvent, InsuranceUpdatedEvent, LpDepositedEvent, MarginAdjustedEvent,
    MarketModeChangedEvent, PositionClosedEvent, PositionIncreasedEvent, PositionLiquidatedEvent,
    PositionOpenedEvent, PositionReducedEvent,
};
use crate::oracle::{emergency_reference_observed_sec, evaluate_sandbox_feed_for_execution};
use crate::types::{
    OracleSourceConfig, PerpsMarketState, PositionHealthView, PositionRecord, PositionRecordView,
    StoredMarketMode, StoredPositionSide, WithdrawalKind, DEFAULT_MAX_OPEN_POSITIONS,
    FEE_SPLIT_INSURANCE_BPS, FEE_SPLIT_PROTOCOL_BPS,
};

pub(crate) struct VaultMtmValuation {
    pub lp_mtm_equity_e8: i128,
    pub reserved_liquidity_e8: u128,
    pub lp_available_liquidity_e8: u128,
    pub net_bounded_claim_e8: i128,
    pub total_unrealized_bad_debt_e8: u128,
}

pub(crate) struct LpDepositExecParams<'a> {
    pub lp_account: &'a AccountId,
    pub token_id: &'a AccountId,
    pub raw_units: u128,
    pub deposit_e8: u128,
    pub min_shares: u128,
    pub override_price: Option<(&'a str, u128)>,
    pub now_sec: u64,
}

pub(crate) struct LpWithdrawOutcome {
    pub assets_out_e8: u128,
    pub transfer_units: u128,
    pub transfer_e8: u128,
    pub residue_e8: u128,
}

pub(crate) struct OpenPositionParams<'a> {
    pub trader: &'a AccountId,
    pub market_id: &'a str,
    pub side: StoredPositionSide,
    pub size_e8: u128,
    pub collateral_e8: u128,
    pub price_limit_e8: Option<u128>,
    pub index_price_e8: u128,
    pub now_sec: u64,
}

pub(crate) struct IncreasePositionParams<'a> {
    pub trader: &'a AccountId,
    pub position_id: u64,
    pub add_size_e8: u128,
    pub add_collateral_e8: u128,
    pub price_limit_e8: Option<u128>,
    pub index_price_e8: u128,
    pub now_sec: u64,
}

impl crate::PerpsContract {
    pub(crate) fn side_label(side: StoredPositionSide) -> &'static str {
        match side {
            StoredPositionSide::Long => "Long",
            StoredPositionSide::Short => "Short",
        }
    }

    pub(crate) fn mode_label(mode: StoredMarketMode) -> &'static str {
        match mode {
            StoredMarketMode::Normal => "Normal",
            StoredMarketMode::ReduceOnly => "ReduceOnly",
            StoredMarketMode::Paused => "Paused",
            StoredMarketMode::Emergency => "Emergency",
        }
    }

    /// Conserve every wei across LP (70%), Insurance (10%), and Protocol Treasury (20%).
    pub(crate) fn split_and_credit_fee_e8(&mut self, fee_e8: u128) {
        if fee_e8 == 0 {
            return;
        }
        let ins_cut =
            checked_mul_div_floor_u128(fee_e8, FEE_SPLIT_INSURANCE_BPS as u128, BPS_SCALE)
                .unwrap_or_else(|_| env::panic_str("ERR_FEE_SPLIT_OVERFLOW"));
        let proto_cut =
            checked_mul_div_floor_u128(fee_e8, FEE_SPLIT_PROTOCOL_BPS as u128, BPS_SCALE)
                .unwrap_or_else(|_| env::panic_str("ERR_FEE_SPLIT_OVERFLOW"));
        let lp_cut = fee_e8
            .checked_sub(ins_cut + proto_cut)
            .unwrap_or_else(|| env::panic_str("ERR_FEE_SPLIT_UNDERFLOW"));

        self.buckets.insurance_fund_e8 = self
            .buckets
            .insurance_fund_e8
            .checked_add(ins_cut)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        self.buckets.protocol_fees_e8 = self
            .buckets
            .protocol_fees_e8
            .checked_add(proto_cut)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        self.buckets.lp_vault_realized_equity_e8 = self
            .buckets
            .lp_vault_realized_equity_e8
            .checked_add(lp_cut)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
    }

    /// Compute deterministic taker execution price adjusted symmetrically against trade direction.
    pub(crate) fn compute_exec_price_e8(
        index_price_e8: u128,
        is_buy: bool,
        exec_spread_bps: u32,
    ) -> Result<u128, String> {
        if exec_spread_bps == 0 {
            return Ok(index_price_e8);
        }
        if exec_spread_bps >= BPS_SCALE as u32 {
            return Err("ERR_INVALID_SPREAD_BPS".to_string());
        }
        if is_buy {
            checked_mul_div_ceil_u128(
                index_price_e8,
                BPS_SCALE + exec_spread_bps as u128,
                BPS_SCALE,
            )
            .map_err(|e| format!("ERR_EXEC_PRICE_MATH: {e:?}"))
        } else {
            let p = checked_mul_div_floor_u128(
                index_price_e8,
                BPS_SCALE - exec_spread_bps as u128,
                BPS_SCALE,
            )
            .map_err(|e| format!("ERR_EXEC_PRICE_MATH: {e:?}"))?;
            if p == 0 {
                return Err("ERR_ZERO_EXEC_PRICE".to_string());
            }
            Ok(p)
        }
    }

    pub(crate) fn check_price_slippage(
        exec_price_e8: u128,
        is_buy: bool,
        price_limit_e8: Option<u128>,
    ) -> Result<(), String> {
        let Some(limit) = price_limit_e8 else {
            return Ok(());
        };
        if is_buy && exec_price_e8 > limit {
            return Err(format!(
                "ERR_SLIPPAGE_EXCEEDED: buy exec {exec_price_e8} > max {limit}"
            ));
        }
        if !is_buy && exec_price_e8 < limit {
            return Err(format!(
                "ERR_SLIPPAGE_EXCEEDED: sell exec {exec_price_e8} < min {limit}"
            ));
        }
        Ok(())
    }

    /// Accrue high-precision symmetric funding on `market`, clamped safely to fresh oracle horizon.
    pub(crate) fn internal_accrue_funding_at_price(
        &mut self,
        market_id: &str,
        index_price_e8: u128,
        now_sec: u64,
    ) -> Result<i128, String> {
        let mut market = self
            .markets
            .get(market_id)
            .cloned()
            .ok_or_else(|| format!("ERR_MARKET_NOT_FOUND: {market_id}"))?;

        if now_sec < market.last_funding_sec {
            return Err("ERR_REGRESSIVE_TIMESTAMP".to_string());
        }

        // Clamp accrual cutoff cleanly against staleness boundary so outages never accrue unpriced funding.
        let effective_now_sec = if let Some(obs_sec) = market.last_valid_observed_sec {
            now_sec.min(obs_sec.saturating_add(market.oracle_policy.max_execution_age_sec))
        } else {
            now_sec
        };

        if effective_now_sec <= market.last_funding_sec {
            return Ok(0);
        }

        let elapsed_sec = advance_funding_elapsed_sec(
            market.last_funding_sec,
            effective_now_sec,
            market.oracle_policy.emergency_close_age_sec,
        )
        .map_err(|e| format!("ERR_FUNDING_ELAPSED: {e:?}"))?;

        if elapsed_sec == 0 {
            return Ok(0);
        }

        let delta_e12 = compute_funding_delta_over_interval_e12(
            market.long_oi_e8,
            market.short_oi_e8,
            self.buckets.lp_vault_realized_equity_e8,
            index_price_e8,
            elapsed_sec,
            market.risk.max_hourly_funding_rate_e12.0,
            MIN_FIRST_LP_DEPOSIT_USD_E8,
        )
        .map_err(|e| format!("ERR_FUNDING_CALC: {e:?}"))?;

        market.cum_funding_index_e12 = market
            .cum_funding_index_e12
            .checked_add(delta_e12)
            .ok_or_else(|| "ERR_FUNDING_INDEX_OVERFLOW".to_string())?;
        market.last_funding_sec = effective_now_sec;
        let new_cum = market.cum_funding_index_e12;
        self.markets.insert(market_id.to_string(), market);

        emit_event(
            "funding_accrued",
            &FundingAccruedEvent {
                market_id,
                elapsed_sec,
                index_delta_e12: delta_e12.to_string(),
                new_cum_funding_index_e12: new_cum.to_string(),
            },
        );

        Ok(delta_e12)
    }

    /// Shared zero-duplication valuation kernel computing LP MTM NAV with Insurance offset
    /// (`compute_lp_mtm_equity_with_insurance_offset_e8`) and cash-backed available liquidity.
    pub(crate) fn compute_vault_mtm_from_market_prices(
        &self,
        market_prices: &[(String, u128, i128, Option<u32>)],
        reserved_liquidity_e8: u128,
    ) -> Result<VaultMtmValuation, String> {
        let mut net_bounded_claim_e8 = 0i128;
        let mut total_unrealized_bad_debt_e8 = 0u128;

        for pos_id in &self.open_position_ids {
            let pos = self
                .positions
                .get(pos_id)
                .ok_or_else(|| "ERR_POSITION_INDEX_DESYNC".to_string())?;
            let (_, mark_p, cum_f, max_pnl) = market_prices
                .iter()
                .find(|(id, _, _, _)| id == &pos.market_id)
                .ok_or_else(|| "ERR_MISSING_MARKET_VALUATION_PRICE".to_string())?;

            let raw_pnl = compute_unrealized_pnl_usd_e8(
                pos.side.into(),
                pos.size_e8,
                pos.entry_price_e8,
                *mark_p,
            )
            .map_err(|e| format!("ERR_PNL_MATH: {e:?}"))?;
            let capped_pnl = apply_profit_cap_usd_e8(raw_pnl, pos.notional_e8, *max_pnl)
                .map_err(|e| format!("ERR_PROFIT_CAP: {e:?}"))?;
            let funding_net = compute_position_funding_net_usd_e8(
                pos.side.into(),
                pos.size_e8,
                pos.entry_funding_index_e12,
                *cum_f,
            )
            .map_err(|e| format!("ERR_FUNDING_NET: {e:?}"))?;

            let bounded_claim = compute_capped_trader_net_claim_usd_e8(
                pos.locked_margin_e8,
                capped_pnl,
                funding_net,
            )
            .map_err(|e| format!("ERR_BOUNDED_CLAIM: {e:?}"))?;
            net_bounded_claim_e8 = net_bounded_claim_e8
                .checked_add(bounded_claim)
                .ok_or_else(|| "ERR_BOUNDED_CLAIM_OVERFLOW".to_string())?;

            let eq = compute_position_equity_usd_e8(pos.locked_margin_e8, capped_pnl, funding_net)
                .map_err(|e| format!("ERR_EQUITY_MATH: {e:?}"))?;
            if eq < 0 {
                total_unrealized_bad_debt_e8 = total_unrealized_bad_debt_e8
                    .checked_add(eq.unsigned_abs())
                    .ok_or_else(|| "ERR_BAD_DEBT_SUM_OVERFLOW".to_string())?;
            }
        }

        let (lp_mtm_equity_e8, _ins_offset) = compute_lp_mtm_equity_with_insurance_offset_e8(
            self.buckets.lp_vault_realized_equity_e8,
            net_bounded_claim_e8,
            total_unrealized_bad_debt_e8,
            self.buckets.insurance_fund_e8,
        )
        .map_err(|e| format!("ERR_LP_MTM_EQUITY: {e:?}"))?;

        let lp_available_liquidity_e8 = compute_lp_available_liquidity_usd_e8(
            self.buckets.lp_vault_realized_equity_e8,
            reserved_liquidity_e8,
            net_bounded_claim_e8,
        );

        Ok(VaultMtmValuation {
            lp_mtm_equity_e8,
            reserved_liquidity_e8,
            lp_available_liquidity_e8,
            net_bounded_claim_e8,
            total_unrealized_bad_debt_e8,
        })
    }

    /// Evaluate exact LP MTM NAV with M04.5A.1 Insurance offset + strictly cash-backed available liquidity.
    pub(crate) fn internal_evaluate_vault_mtm(
        &mut self,
        override_price: Option<(&str, u128)>,
        now_sec: u64,
        require_fresh_for_open_markets: bool,
    ) -> Result<VaultMtmValuation, String> {
        let mut reserved_liquidity_e8 = 0u128;
        let mut market_prices: Vec<(String, u128, i128, Option<u32>)> = Vec::new();

        let market_ids: Vec<String> = self.market_ids.clone();
        for m_id in market_ids {
            let mut m = self
                .markets
                .get(&m_id)
                .cloned()
                .ok_or_else(|| "ERR_MARKET_STATE_CORRUPT".to_string())?;
            if m.open_positions == 0 && m.long_oi_e8 == 0 && m.short_oi_e8 == 0 {
                continue;
            }

            let m_res = compute_reserved_liquidity_usd_e8(
                m.long_oi_e8,
                m.short_oi_e8,
                m.risk.matched_gap_buffer_bps,
            )
            .map_err(|e| format!("ERR_RESERVED_LIQ: {e:?}"))?;
            reserved_liquidity_e8 = reserved_liquidity_e8
                .checked_add(m_res)
                .ok_or_else(|| "ERR_RESERVED_LIQ_OVERFLOW".to_string())?;

            let mark_price_e8 = if let Some((ov_id, ov_price)) = override_price {
                if ov_id == m_id {
                    ov_price
                } else {
                    self.resolve_market_mark_for_valuation(
                        &mut m,
                        now_sec,
                        require_fresh_for_open_markets,
                    )?
                }
            } else {
                self.resolve_market_mark_for_valuation(
                    &mut m,
                    now_sec,
                    require_fresh_for_open_markets,
                )?
            };

            let max_pnl = m.risk.max_pnl_bps;
            let cum_f = if m.mode == StoredMarketMode::Emergency {
                m.frozen_funding_index_e12
                    .unwrap_or(m.cum_funding_index_e12)
            } else {
                m.cum_funding_index_e12
            };
            self.markets.insert(m_id.clone(), m);
            market_prices.push((m_id, mark_price_e8, cum_f, max_pnl));
        }

        self.compute_vault_mtm_from_market_prices(&market_prices, reserved_liquidity_e8)
    }

    /// Read-only LP valuation used strictly by `&self` view endpoints without mutating market mode.
    pub(crate) fn internal_readonly_vault_mtm(&self) -> Result<VaultMtmValuation, String> {
        let mut reserved_liquidity_e8 = 0u128;
        let mut market_prices: Vec<(String, u128, i128, Option<u32>)> = Vec::new();
        for m_id in &self.market_ids {
            let Some(m) = self.markets.get(m_id) else {
                continue;
            };
            if m.open_positions == 0 && m.long_oi_e8 == 0 && m.short_oi_e8 == 0 {
                continue;
            }
            let r = compute_reserved_liquidity_usd_e8(
                m.long_oi_e8,
                m.short_oi_e8,
                m.risk.matched_gap_buffer_bps,
            )
            .map_err(|e| format!("{e:?}"))?;
            reserved_liquidity_e8 += r;
            let ref_p = if m.mode == StoredMarketMode::Emergency {
                m.frozen_settlement_price_e8.unwrap_or(0)
            } else {
                m.sandbox_observation
                    .as_ref()
                    .map(|o| o.price_e8)
                    .or(m.last_valid_price_e8)
                    .unwrap_or(0)
            };
            if ref_p == 0 {
                return Err("ERR_NO_REFERENCE_PRICE".to_string());
            }
            let cum_f = if m.mode == StoredMarketMode::Emergency {
                m.frozen_funding_index_e12
                    .unwrap_or(m.cum_funding_index_e12)
            } else {
                m.cum_funding_index_e12
            };
            market_prices.push((m_id.clone(), ref_p, cum_f, m.risk.max_pnl_bps));
        }
        self.compute_vault_mtm_from_market_prices(&market_prices, reserved_liquidity_e8)
    }

    fn resolve_market_mark_for_valuation(
        &self,
        market: &mut PerpsMarketState,
        now_sec: u64,
        require_fresh: bool,
    ) -> Result<u128, String> {
        if market.mode == StoredMarketMode::Emergency {
            return market
                .frozen_settlement_price_e8
                .ok_or_else(|| "ERR_MISSING_EMERGENCY_SNAPSHOT".to_string());
        }
        match &market.oracle_source {
            OracleSourceConfig::TestnetSandboxOperator { .. } => {
                if require_fresh {
                    evaluate_sandbox_feed_for_execution(market, now_sec)
                } else {
                    market
                        .last_valid_price_e8
                        .or_else(|| market.sandbox_observation.as_ref().map(|o| o.price_e8))
                        .ok_or_else(|| "ERR_ORACLE_PRICE_MISSING".to_string())
                }
            }
            OracleSourceConfig::ExternalXcc { .. } => {
                if !require_fresh {
                    return market
                        .last_valid_price_e8
                        .ok_or_else(|| "ERR_ORACLE_PRICE_MISSING".to_string());
                }
                if let (Some(p), Some(ts)) =
                    (market.last_valid_price_e8, market.last_valid_observed_sec)
                {
                    if now_sec.saturating_sub(ts) <= market.oracle_policy.max_execution_age_sec {
                        return Ok(p);
                    }
                }
                Err("ERR_EXTERNAL_ORACLE_STALE_OR_NEEDS_XCC".to_string())
            }
        }
    }

    /// Guard Liquidation-Before-Winner Settlement Ordering (M04.5A.1 Invariant #3).
    fn check_winner_settlement_order(
        &self,
        market_id: &str,
        excluding_position_id: u64,
        payout_needed_from_lp_e8: u128,
        reference_price_e8: u128,
    ) -> Result<(), String> {
        if self.buckets.lp_vault_realized_equity_e8 >= payout_needed_from_lp_e8 {
            return Ok(());
        }

        let market = self
            .markets
            .get(market_id)
            .ok_or_else(|| "ERR_MARKET_NOT_FOUND".to_string())?;
        let mm_bps = market.risk.maintenance_margin_bps;
        let max_pnl = market.risk.max_pnl_bps;
        let cum_f = market.cum_funding_index_e12;

        for &other_id in &self.open_position_ids {
            if other_id == excluding_position_id {
                continue;
            }
            if let Some(other) = self.positions.get(&other_id) {
                if other.market_id != market_id {
                    continue;
                }
                let raw_pnl = compute_unrealized_pnl_usd_e8(
                    other.side.into(),
                    other.size_e8,
                    other.entry_price_e8,
                    reference_price_e8,
                )
                .map_err(|e| format!("ERR_ORDERING_CHECK: {e:?}"))?;
                let capped = apply_profit_cap_usd_e8(raw_pnl, other.notional_e8, max_pnl)
                    .map_err(|e| format!("ERR_ORDERING_CHECK: {e:?}"))?;
                let funding = compute_position_funding_net_usd_e8(
                    other.side.into(),
                    other.size_e8,
                    other.entry_funding_index_e12,
                    cum_f,
                )
                .map_err(|e| format!("ERR_ORDERING_CHECK: {e:?}"))?;
                let eq = compute_position_equity_usd_e8(other.locked_margin_e8, capped, funding)
                    .map_err(|e| format!("ERR_ORDERING_CHECK: {e:?}"))?;
                let notional = compute_notional_ceil_usd_e8(reference_price_e8, other.size_e8)
                    .map_err(|e| format!("ERR_ORDERING_CHECK: {e:?}"))?;
                let liq = is_liquidatable(eq, notional, mm_bps)
                    .map_err(|e| format!("ERR_ORDERING_CHECK: {e:?}"))?;
                if eq < 0 || liq {
                    return Err("ERR_LIQUIDATE_UNDERWATER_POSITIONS_FIRST".to_string());
                }
            }
        }

        Err("ERR_INSUFFICIENT_VAULT_LIQUIDITY".to_string())
    }

    pub(crate) fn internal_execute_lp_deposit(
        &mut self,
        p: LpDepositExecParams<'_>,
    ) -> Result<(), String> {
        let lp_account = p.lp_account;
        let token_id = p.token_id;
        let raw_units = p.raw_units;
        let deposit_e8 = p.deposit_e8;
        let min_shares = p.min_shares;
        let override_price = p.override_price;
        let now_sec = p.now_sec;
        if self.paused_global {
            return Err("ERR_PROTOCOL_PAUSED".to_string());
        }
        // Disallow new LP entries while any market sits in Emergency unwind.
        for m_id in &self.market_ids {
            if let Some(m) = self.markets.get(m_id) {
                if m.mode == StoredMarketMode::Emergency {
                    return Err("ERR_MARKET_EMERGENCY_FROZEN".to_string());
                }
            }
        }

        let acc = self.accounts.get(lp_account).cloned().unwrap_or_default();
        if !acc.registered {
            return Err("ERR_ACCOUNT_NOT_REGISTERED".to_string());
        }

        let lp_mtm_before_e8 = if self.open_position_ids.is_empty() {
            i128::try_from(self.buckets.lp_vault_realized_equity_e8)
                .map_err(|_| "ERR_OVERFLOW".to_string())?
        } else {
            self.internal_evaluate_vault_mtm(override_price, now_sec, true)?
                .lp_mtm_equity_e8
        };

        let mint_res = compute_lp_mint_shares(deposit_e8, self.lp_total_shares, lp_mtm_before_e8)
            .map_err(|e| format!("ERR_LP_MINT: {e:?}"))?;

        if mint_res.minted_to_depositor < min_shares {
            return Err(format!(
                "ERR_LP_SLIPPAGE: minted {} < min_shares {min_shares}",
                mint_res.minted_to_depositor
            ));
        }

        let mut acc_mut = acc;
        acc_mut.lp_shares = acc_mut
            .lp_shares
            .checked_add(mint_res.minted_to_depositor)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        self.accounts.insert(lp_account.clone(), acc_mut);

        let total_minted = mint_res
            .minted_to_depositor
            .checked_add(mint_res.locked_seed_shares)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        self.lp_total_shares = self
            .lp_total_shares
            .checked_add(total_minted)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        self.lp_locked_seed_shares = self
            .lp_locked_seed_shares
            .checked_add(mint_res.locked_seed_shares)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;

        self.buckets.lp_vault_realized_equity_e8 = self
            .buckets
            .lp_vault_realized_equity_e8
            .checked_add(deposit_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        self.tracked_collateral_e8 = self
            .tracked_collateral_e8
            .checked_add(deposit_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;

        let mut token_cfg = self
            .collateral_tokens
            .get(token_id)
            .cloned()
            .ok_or_else(|| "ERR_INVALID_COLLATERAL_TOKEN".to_string())?;
        token_cfg.tracked_balance_units = token_cfg
            .tracked_balance_units
            .checked_add(raw_units)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        self.collateral_tokens.insert(token_id.clone(), token_cfg);

        self.assert_solvency();

        emit_event(
            "lp_deposited",
            &LpDepositedEvent {
                account_id: lp_account.as_str(),
                token_id: token_id.as_str(),
                amount_e8: deposit_e8.to_string(),
                shares_minted: mint_res.minted_to_depositor.to_string(),
                seed_shares_locked: mint_res.locked_seed_shares.to_string(),
                nav_e8: lp_mtm_before_e8.to_string(),
            },
        );

        Ok(())
    }

    pub(crate) fn internal_execute_lp_withdraw(
        &mut self,
        lp_account: &AccountId,
        token_id: &AccountId,
        shares_to_burn: u128,
        min_assets_out_e8: u128,
        override_price: Option<(&str, u128)>,
        now_sec: u64,
    ) -> Result<LpWithdrawOutcome, String> {
        if self.paused_global {
            return Err("ERR_PROTOCOL_PAUSED".to_string());
        }

        // M04.5A.1 Emergency Invariant: LP withdrawals stay LOCKED during Emergency until ALL open positions settle.
        if !self.open_position_ids.is_empty() {
            for m_id in &self.market_ids {
                if let Some(m) = self.markets.get(m_id) {
                    if m.mode == StoredMarketMode::Emergency {
                        return Err("ERR_EMERGENCY_LP_LOCKED".to_string());
                    }
                }
            }
        }

        let mut acc = self
            .accounts
            .get(lp_account)
            .cloned()
            .ok_or_else(|| "ERR_ACCOUNT_NOT_REGISTERED".to_string())?;
        if !acc.registered {
            return Err("ERR_ACCOUNT_NOT_REGISTERED".to_string());
        }
        if acc.has_pending_withdrawal {
            return Err("ERR_WITHDRAWAL_IN_PROGRESS".to_string());
        }
        if shares_to_burn == 0 || acc.lp_shares < shares_to_burn {
            return Err("ERR_INSUFFICIENT_LP_SHARES".to_string());
        }

        let token_cfg = self
            .collateral_tokens
            .get(token_id)
            .cloned()
            .ok_or_else(|| "ERR_INVALID_COLLATERAL_TOKEN".to_string())?;
        if !token_cfg.active {
            return Err("ERR_COLLATERAL_TOKEN_INACTIVE".to_string());
        }

        let valuation = if self.open_position_ids.is_empty() {
            VaultMtmValuation {
                lp_mtm_equity_e8: i128::try_from(self.buckets.lp_vault_realized_equity_e8)
                    .map_err(|_| "ERR_OVERFLOW".to_string())?,
                reserved_liquidity_e8: 0,
                lp_available_liquidity_e8: self.buckets.lp_vault_realized_equity_e8,
                net_bounded_claim_e8: 0,
                total_unrealized_bad_debt_e8: 0,
            }
        } else {
            self.internal_evaluate_vault_mtm(override_price, now_sec, true)?
        };

        let assets_out_e8 = compute_lp_redeem_assets(
            shares_to_burn,
            self.lp_total_shares,
            valuation.lp_mtm_equity_e8,
            valuation.lp_available_liquidity_e8,
        )
        .map_err(|e| format!("ERR_LP_REDEEM: {e:?}"))?;

        if assets_out_e8 < min_assets_out_e8 {
            return Err(format!(
                "ERR_LP_SLIPPAGE: assets {assets_out_e8} < min_assets {min_assets_out_e8}"
            ));
        }

        // Strict Invariant #2: LP withdrawal capped by strictly cash-backed available liquidity.
        if assets_out_e8 > valuation.lp_available_liquidity_e8 {
            return Err(format!(
                "ERR_INSUFFICIENT_LIQUIDITY: requested {assets_out_e8}, available {}",
                valuation.lp_available_liquidity_e8
            ));
        }

        // Lossless 8->6 token split preserving exact sub-micro-USDC residue in free_collateral_e8!
        let (transfer_units, residue_e8) = token_cfg.split_usd_e8_to_token_units(assets_out_e8)?;
        if transfer_units == 0 {
            return Err("ERR_WITHDRAWAL_BELOW_MIN_TOKEN_UNIT".to_string());
        }
        if token_cfg.tracked_balance_units < transfer_units {
            return Err("ERR_INSUFFICIENT_TOKEN_LIQUIDITY".to_string());
        }
        let transfer_e8 = token_cfg.checked_usd_e8(transfer_units)?;

        acc.lp_shares -= shares_to_burn;
        acc.free_collateral_e8 = acc
            .free_collateral_e8
            .checked_add(residue_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        acc.has_pending_withdrawal = true;
        self.accounts.insert(lp_account.clone(), acc);

        self.lp_total_shares -= shares_to_burn;
        self.buckets.lp_vault_realized_equity_e8 = self
            .buckets
            .lp_vault_realized_equity_e8
            .checked_sub(assets_out_e8)
            .ok_or_else(|| "ERR_VAULT_EQUITY_UNDERFLOW".to_string())?;
        self.buckets.total_trader_free_collateral_e8 = self
            .buckets
            .total_trader_free_collateral_e8
            .checked_add(residue_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        self.buckets.total_in_flight_withdrawals_e8 = self
            .buckets
            .total_in_flight_withdrawals_e8
            .checked_add(transfer_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;

        Ok(LpWithdrawOutcome {
            assets_out_e8,
            transfer_units,
            transfer_e8,
            residue_e8,
        })
    }

    pub(crate) fn internal_open_position(
        &mut self,
        p: OpenPositionParams<'_>,
    ) -> Result<u64, String> {
        let trader = p.trader;
        let market_id = p.market_id;
        let side = p.side;
        let size_e8 = p.size_e8;
        let collateral_e8 = p.collateral_e8;
        let price_limit_e8 = p.price_limit_e8;
        let index_price_e8 = p.index_price_e8;
        let now_sec = p.now_sec;
        if self.paused_global {
            return Err("ERR_PROTOCOL_PAUSED".to_string());
        }
        if size_e8 == 0 || collateral_e8 == 0 {
            return Err("ERR_INVALID_SIZE_OR_MARGIN".to_string());
        }
        if self.open_position_ids.len() >= DEFAULT_MAX_OPEN_POSITIONS as usize {
            return Err("ERR_MAX_OPEN_POSITIONS_REACHED".to_string());
        }

        let market = self
            .markets
            .get(market_id)
            .cloned()
            .ok_or_else(|| format!("ERR_MARKET_NOT_FOUND: {market_id}"))?;
        if market.mode != StoredMarketMode::Normal || market.paused_for_insolvency {
            return Err(format!(
                "ERR_MARKET_NOT_NORMAL: mode={}",
                Self::mode_label(market.mode)
            ));
        }

        self.internal_accrue_funding_at_price(market_id, index_price_e8, now_sec)?;

        let mut market = self
            .markets
            .get(market_id)
            .cloned()
            .ok_or_else(|| "ERR_MARKET_NOT_FOUND".to_string())?;

        let is_buy = side == StoredPositionSide::Long;
        let exec_price_e8 =
            Self::compute_exec_price_e8(index_price_e8, is_buy, market.exec_spread_bps)?;
        Self::check_price_slippage(exec_price_e8, is_buy, price_limit_e8)?;

        let entry_notional_floor = compute_notional_floor_usd_e8(exec_price_e8, size_e8)
            .map_err(|e| format!("ERR_NOTIONAL_MATH: {e:?}"))?;
        let entry_notional_ceil = compute_notional_ceil_usd_e8(exec_price_e8, size_e8)
            .map_err(|e| format!("ERR_NOTIONAL_MATH: {e:?}"))?;

        let im_required_e8 =
            compute_initial_margin_usd_e8(entry_notional_ceil, market.risk.initial_margin_bps)
                .map_err(|e| format!("ERR_IM_MATH: {e:?}"))?;
        if collateral_e8 < im_required_e8 {
            return Err(format!(
                "ERR_INSUFFICIENT_MARGIN: provided {collateral_e8} < required IM {im_required_e8}"
            ));
        }

        // Guard against born-liquidatable positions when exec spread differs from mark index price.
        let spread_pnl_at_mark =
            compute_unrealized_pnl_usd_e8(side.into(), size_e8, exec_price_e8, index_price_e8)
                .map_err(|e| format!("ERR_PNL_MATH: {e:?}"))?;
        let eq_at_open = compute_position_equity_usd_e8(collateral_e8, spread_pnl_at_mark, 0)
            .map_err(|e| format!("ERR_EQ_MATH: {e:?}"))?;
        let mark_notional_ceil = compute_notional_ceil_usd_e8(index_price_e8, size_e8)
            .map_err(|e| format!("ERR_NOTIONAL_MATH: {e:?}"))?;
        if is_liquidatable(
            eq_at_open,
            mark_notional_ceil,
            market.risk.maintenance_margin_bps,
        )
        .map_err(|e| format!("ERR_LIQ_CHECK: {e:?}"))?
        {
            return Err("ERR_BORN_LIQUIDATABLE".to_string());
        }

        let fee_e8 = compute_trading_fee_usd_e8(entry_notional_ceil, market.risk.trade_fee_bps)
            .map_err(|e| format!("ERR_FEE_MATH: {e:?}"))?;
        let total_cost_e8 = collateral_e8
            .checked_add(fee_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;

        let mut account = self
            .accounts
            .get(trader)
            .cloned()
            .ok_or_else(|| "ERR_ACCOUNT_NOT_REGISTERED".to_string())?;
        if !account.registered {
            return Err("ERR_ACCOUNT_NOT_REGISTERED".to_string());
        }
        if account.free_collateral_e8 < total_cost_e8 {
            return Err(format!(
                "ERR_INSUFFICIENT_FREE_COLLATERAL: has {}, needs {total_cost_e8}",
                account.free_collateral_e8
            ));
        }

        // Check M04.5A.1 Utilization Limit (`U <= 75%`) with exact post-trade OI.
        let reserved_before_market = compute_reserved_liquidity_usd_e8(
            market.long_oi_e8,
            market.short_oi_e8,
            market.risk.matched_gap_buffer_bps,
        )
        .map_err(|e| format!("ERR_RES_BEFORE: {e:?}"))?;

        let (new_long_oi, new_short_oi) = match side {
            StoredPositionSide::Long => (
                market
                    .long_oi_e8
                    .checked_add(entry_notional_floor)
                    .ok_or_else(|| "ERR_OI_OVERFLOW".to_string())?,
                market.short_oi_e8,
            ),
            StoredPositionSide::Short => (
                market.long_oi_e8,
                market
                    .short_oi_e8
                    .checked_add(entry_notional_floor)
                    .ok_or_else(|| "ERR_OI_OVERFLOW".to_string())?,
            ),
        };
        let reserved_after_market = compute_reserved_liquidity_usd_e8(
            new_long_oi,
            new_short_oi,
            market.risk.matched_gap_buffer_bps,
        )
        .map_err(|e| format!("ERR_RES_AFTER: {e:?}"))?;

        let val_before =
            self.internal_evaluate_vault_mtm(Some((market_id, index_price_e8)), now_sec, false)?;
        let total_reserved_after = val_before
            .reserved_liquidity_e8
            .checked_sub(reserved_before_market)
            .and_then(|v| v.checked_add(reserved_after_market))
            .ok_or_else(|| "ERR_RESERVED_CALC".to_string())?;

        if val_before.lp_mtm_equity_e8 <= 0 {
            return Err("ERR_LP_VAULT_INSOLVENT".to_string());
        }
        let util_after_bps =
            compute_utilization_bps(total_reserved_after, val_before.lp_mtm_equity_e8)
                .map_err(|e| format!("ERR_UTIL_CALC: {e:?}"))?;

        if util_after_bps > (market.risk.max_utilization_bps as u128)
            && reserved_after_market > reserved_before_market
        {
            return Err(format!(
                "ERR_MAX_UTILIZATION_EXCEEDED: util_after {util_after_bps} bps > max {} bps",
                market.risk.max_utilization_bps
            ));
        }

        // Commit atomic ledger transition
        account.free_collateral_e8 -= total_cost_e8;
        self.accounts.insert(trader.clone(), account);

        self.buckets.total_trader_free_collateral_e8 = self
            .buckets
            .total_trader_free_collateral_e8
            .checked_sub(total_cost_e8)
            .ok_or_else(|| "ERR_BUCKET_UNDERFLOW".to_string())?;
        self.buckets.total_trader_locked_margin_e8 = self
            .buckets
            .total_trader_locked_margin_e8
            .checked_add(collateral_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        self.split_and_credit_fee_e8(fee_e8);

        self.next_position_id = self
            .next_position_id
            .checked_add(1)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        let pos_id = self.next_position_id;

        match side {
            StoredPositionSide::Long => {
                market.long_oi_e8 = new_long_oi;
                market.long_size_e8 = market
                    .long_size_e8
                    .checked_add(size_e8)
                    .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
            }
            StoredPositionSide::Short => {
                market.short_oi_e8 = new_short_oi;
                market.short_size_e8 = market
                    .short_size_e8
                    .checked_add(size_e8)
                    .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
            }
        }
        market.open_positions += 1;
        let funding_snapshot = market.cum_funding_index_e12;
        self.markets.insert(market_id.to_string(), market);

        let position = PositionRecord {
            id: pos_id,
            owner: trader.clone(),
            market_id: market_id.to_string(),
            side,
            size_e8,
            notional_e8: entry_notional_floor,
            entry_price_e8: exec_price_e8,
            locked_margin_e8: collateral_e8,
            entry_funding_index_e12: funding_snapshot,
            opened_at_sec: now_sec,
            locked: false,
        };
        self.positions.insert(pos_id, position);
        self.open_position_ids.push(pos_id);

        self.assert_solvency();

        emit_event(
            "position_opened",
            &PositionOpenedEvent {
                position_id: pos_id,
                owner: trader.as_str(),
                market_id,
                side: Self::side_label(side),
                size_e8: size_e8.to_string(),
                notional_e8: entry_notional_floor.to_string(),
                exec_price_e8: exec_price_e8.to_string(),
                margin_e8: collateral_e8.to_string(),
                fee_e8: fee_e8.to_string(),
                funding_index_e12: funding_snapshot.to_string(),
            },
        );

        Ok(pos_id)
    }

    pub(crate) fn internal_increase_position(
        &mut self,
        p: IncreasePositionParams<'_>,
    ) -> Result<(), String> {
        let trader = p.trader;
        let position_id = p.position_id;
        let add_size_e8 = p.add_size_e8;
        let add_collateral_e8 = p.add_collateral_e8;
        let price_limit_e8 = p.price_limit_e8;
        let index_price_e8 = p.index_price_e8;
        let now_sec = p.now_sec;
        if self.paused_global {
            return Err("ERR_PROTOCOL_PAUSED".to_string());
        }
        if add_size_e8 == 0 {
            return Err("ERR_INVALID_SIZE".to_string());
        }

        let pos = self
            .positions
            .get(&position_id)
            .cloned()
            .ok_or_else(|| "ERR_POSITION_NOT_FOUND".to_string())?;
        if &pos.owner != trader {
            return Err("ERR_UNAUTHORIZED".to_string());
        }

        let market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .ok_or_else(|| "ERR_MARKET_NOT_FOUND".to_string())?;
        if market.mode != StoredMarketMode::Normal || market.paused_for_insolvency {
            return Err("ERR_MARKET_NOT_NORMAL".to_string());
        }

        self.internal_accrue_funding_at_price(&pos.market_id, index_price_e8, now_sec)?;

        let mut market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .ok_or_else(|| "ERR_MARKET_NOT_FOUND".to_string())?;

        let is_buy = pos.side == StoredPositionSide::Long;
        let exec_price_e8 =
            Self::compute_exec_price_e8(index_price_e8, is_buy, market.exec_spread_bps)?;
        Self::check_price_slippage(exec_price_e8, is_buy, price_limit_e8)?;

        // First settle existing funding on the prior slice so new average weighted size carries fresh funding baseline.
        let prior_funding_e8 = compute_position_funding_net_usd_e8(
            pos.side.into(),
            pos.size_e8,
            pos.entry_funding_index_e12,
            market.cum_funding_index_e12,
        )
        .map_err(|e| format!("ERR_FUNDING_CALC: {e:?}"))?;

        if prior_funding_e8 > 0 {
            let needed = prior_funding_e8 as u128;
            if self.buckets.lp_vault_realized_equity_e8 < needed {
                self.check_winner_settlement_order(
                    &pos.market_id,
                    position_id,
                    needed,
                    index_price_e8,
                )?;
            }
        }

        let new_locked_before_add_i128 = i128::try_from(pos.locked_margin_e8)
            .map_err(|_| "ERR_OVERFLOW".to_string())?
            .checked_add(prior_funding_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        if new_locked_before_add_i128 < 0 {
            return Err("ERR_UNDERWATER_LIQUIDATE_FIRST".to_string());
        }
        let new_locked_total_e8 = (new_locked_before_add_i128 as u128)
            .checked_add(add_collateral_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;

        let add_notional_floor = compute_notional_floor_usd_e8(exec_price_e8, add_size_e8)
            .map_err(|e| format!("ERR_NOTIONAL: {e:?}"))?;
        let add_notional_ceil = compute_notional_ceil_usd_e8(exec_price_e8, add_size_e8)
            .map_err(|e| format!("ERR_NOTIONAL: {e:?}"))?;
        let fee_e8 = compute_trading_fee_usd_e8(add_notional_ceil, market.risk.trade_fee_bps)
            .map_err(|e| format!("ERR_FEE: {e:?}"))?;

        let new_size_e8 = pos
            .size_e8
            .checked_add(add_size_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        let new_notional_e8 = pos
            .notional_e8
            .checked_add(add_notional_floor)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;

        // Conservative weighted average entry price (ceil for Long, floor for Short).
        let old_cost = pos
            .size_e8
            .checked_mul(pos.entry_price_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        let add_cost = add_size_e8
            .checked_mul(exec_price_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        let sum_cost = old_cost
            .checked_add(add_cost)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        let new_entry_price_e8 = if pos.side == StoredPositionSide::Long {
            roastake_perps_core::ceil_div_u128(sum_cost, new_size_e8)
                .map_err(|e| format!("ERR_AVG_ENTRY: {e:?}"))?
        } else {
            sum_cost / new_size_e8
        };

        let im_after_e8 = compute_initial_margin_usd_e8(
            compute_notional_ceil_usd_e8(index_price_e8, new_size_e8)
                .map_err(|e| format!("ERR_NOTIONAL: {e:?}"))?
                .max(
                    compute_notional_ceil_usd_e8(new_entry_price_e8, new_size_e8)
                        .map_err(|e| format!("ERR_NOTIONAL: {e:?}"))?,
                ),
            market.risk.initial_margin_bps,
        )
        .map_err(|e| format!("ERR_IM: {e:?}"))?;

        let raw_pnl_after = compute_unrealized_pnl_usd_e8(
            pos.side.into(),
            new_size_e8,
            new_entry_price_e8,
            index_price_e8,
        )
        .map_err(|e| format!("ERR_PNL: {e:?}"))?;
        let mark_eq_after = compute_position_equity_usd_e8(new_locked_total_e8, raw_pnl_after, 0)
            .map_err(|e| format!("ERR_EQ: {e:?}"))?;
        let im_after_i128 = i128::try_from(im_after_e8).map_err(|_| "ERR_OVERFLOW".to_string())?;
        if new_locked_total_e8 < im_after_e8 || mark_eq_after < im_after_i128 {
            return Err("ERR_INSUFFICIENT_MARGIN_AFTER_INCREASE".to_string());
        }

        let user_cash_needed = add_collateral_e8
            .checked_add(fee_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        let mut account = self
            .accounts
            .get(trader)
            .cloned()
            .ok_or_else(|| "ERR_ACCOUNT_NOT_REGISTERED".to_string())?;
        if account.free_collateral_e8 < user_cash_needed {
            return Err("ERR_INSUFFICIENT_FREE_COLLATERAL".to_string());
        }

        // Utilization check (`U <= 75%`)
        let reserved_before = compute_reserved_liquidity_usd_e8(
            market.long_oi_e8,
            market.short_oi_e8,
            market.risk.matched_gap_buffer_bps,
        )
        .map_err(|e| format!("ERR_RES: {e:?}"))?;
        let (new_long_oi, new_short_oi) = match pos.side {
            StoredPositionSide::Long => {
                (market.long_oi_e8 + add_notional_floor, market.short_oi_e8)
            }
            StoredPositionSide::Short => {
                (market.long_oi_e8, market.short_oi_e8 + add_notional_floor)
            }
        };
        let reserved_after = compute_reserved_liquidity_usd_e8(
            new_long_oi,
            new_short_oi,
            market.risk.matched_gap_buffer_bps,
        )
        .map_err(|e| format!("ERR_RES: {e:?}"))?;

        let val_before = self.internal_evaluate_vault_mtm(
            Some((&pos.market_id, index_price_e8)),
            now_sec,
            false,
        )?;
        let total_reserved_after = val_before
            .reserved_liquidity_e8
            .checked_sub(reserved_before)
            .and_then(|v| v.checked_add(reserved_after))
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        if val_before.lp_mtm_equity_e8 <= 0 {
            return Err("ERR_LP_VAULT_INSOLVENT".to_string());
        }
        let util_after = compute_utilization_bps(total_reserved_after, val_before.lp_mtm_equity_e8)
            .map_err(|e| format!("ERR_UTIL: {e:?}"))?;
        if util_after > (market.risk.max_utilization_bps as u128)
            && reserved_after > reserved_before
        {
            return Err("ERR_MAX_UTILIZATION_EXCEEDED".to_string());
        }

        // Commit state
        account.free_collateral_e8 -= user_cash_needed;
        self.accounts.insert(trader.clone(), account);
        self.buckets.total_trader_free_collateral_e8 -= user_cash_needed;

        // Realize prior funding between locked margin & LP Vault
        if prior_funding_e8 >= 0 {
            let pf = prior_funding_e8 as u128;
            self.buckets.lp_vault_realized_equity_e8 -= pf;
            self.buckets.total_trader_locked_margin_e8 = self
                .buckets
                .total_trader_locked_margin_e8
                .checked_add(pf + add_collateral_e8)
                .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        } else {
            let pf = prior_funding_e8.unsigned_abs();
            self.buckets.lp_vault_realized_equity_e8 = self
                .buckets
                .lp_vault_realized_equity_e8
                .checked_add(pf)
                .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
            self.buckets.total_trader_locked_margin_e8 = self
                .buckets
                .total_trader_locked_margin_e8
                .checked_sub(pf)
                .and_then(|v| v.checked_add(add_collateral_e8))
                .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        }
        self.split_and_credit_fee_e8(fee_e8);

        match pos.side {
            StoredPositionSide::Long => {
                market.long_oi_e8 = new_long_oi;
                market.long_size_e8 = new_size_e8;
            }
            StoredPositionSide::Short => {
                market.short_oi_e8 = new_short_oi;
                market.short_size_e8 = new_size_e8;
            }
        }
        let cum_f = market.cum_funding_index_e12;
        self.markets.insert(pos.market_id.clone(), market);

        let updated = PositionRecord {
            size_e8: new_size_e8,
            notional_e8: new_notional_e8,
            entry_price_e8: new_entry_price_e8,
            locked_margin_e8: new_locked_total_e8,
            entry_funding_index_e12: cum_f,
            locked: false,
            ..pos
        };
        self.positions.insert(position_id, updated);

        self.assert_solvency();

        emit_event(
            "position_increased",
            &PositionIncreasedEvent {
                position_id,
                owner: trader.as_str(),
                added_size_e8: add_size_e8.to_string(),
                new_size_e8: new_size_e8.to_string(),
                new_entry_price_e8: new_entry_price_e8.to_string(),
                added_margin_e8: add_collateral_e8.to_string(),
                fee_e8: fee_e8.to_string(),
            },
        );

        Ok(())
    }

    pub(crate) fn internal_reduce_or_close_position(
        &mut self,
        trader: &AccountId,
        position_id: u64,
        requested_close_size_e8: Option<u128>,
        price_limit_e8: Option<u128>,
        index_price_e8: u128,
        now_sec: u64,
    ) -> Result<(), String> {
        if self.paused_global {
            return Err("ERR_PROTOCOL_PAUSED".to_string());
        }

        let pos = self
            .positions
            .get(&position_id)
            .cloned()
            .ok_or_else(|| "ERR_POSITION_NOT_FOUND".to_string())?;
        if &pos.owner != trader {
            return Err("ERR_UNAUTHORIZED".to_string());
        }

        let market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .ok_or_else(|| "ERR_MARKET_NOT_FOUND".to_string())?;
        if !matches!(
            market.mode,
            StoredMarketMode::Normal | StoredMarketMode::ReduceOnly
        ) || market.paused_for_insolvency
        {
            return Err(format!(
                "ERR_MARKET_CANNOT_NORMAL_CLOSE: mode={}",
                Self::mode_label(market.mode)
            ));
        }

        self.internal_accrue_funding_at_price(&pos.market_id, index_price_e8, now_sec)?;

        let mut market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .ok_or_else(|| "ERR_MARKET_NOT_FOUND".to_string())?;

        let close_size_e8 = match requested_close_size_e8 {
            Some(s) if s > 0 => s.min(pos.size_e8),
            Some(_) => return Err("ERR_INVALID_CLOSE_SIZE".to_string()),
            None => pos.size_e8,
        };
        let is_full_close = close_size_e8 == pos.size_e8;

        let is_buy = pos.side == StoredPositionSide::Short;
        let exec_price_e8 =
            Self::compute_exec_price_e8(index_price_e8, is_buy, market.exec_spread_bps)?;
        Self::check_price_slippage(exec_price_e8, is_buy, price_limit_e8)?;

        let (slice_margin_e8, slice_notional_e8) = if is_full_close {
            (pos.locked_margin_e8, pos.notional_e8)
        } else {
            let m_slice =
                checked_mul_div_floor_u128(pos.locked_margin_e8, close_size_e8, pos.size_e8)
                    .map_err(|e| format!("ERR_PARTIAL_MARGIN: {e:?}"))?;
            let n_slice = checked_mul_div_floor_u128(pos.notional_e8, close_size_e8, pos.size_e8)
                .map_err(|e| format!("ERR_PARTIAL_NOTIONAL: {e:?}"))?;
            (m_slice, n_slice)
        };

        let snapshot = PositionSnapshot {
            side: pos.side.into(),
            size_e8: close_size_e8,
            locked_margin_e8: slice_margin_e8,
            entry_price_e8: pos.entry_price_e8,
            entry_funding_index_e12: pos.entry_funding_index_e12,
        };

        let settle = compute_position_settlement_e8(
            &snapshot,
            exec_price_e8,
            market.cum_funding_index_e12,
            market.risk.max_pnl_bps,
            market.risk.trade_fee_bps,
        )
        .map_err(|e| format!("ERR_SETTLEMENT_MATH: {e:?}"))?;

        // Liquidation-Before-Winner check whenever LP Vault owes net payout!
        if settle.vault_delta_e8 < 0 {
            let needed = settle.vault_delta_e8.unsigned_abs();
            self.check_winner_settlement_order(
                &pos.market_id,
                position_id,
                needed,
                index_price_e8,
            )?;
        }

        // If partial reduce leaves remaining size, ensure remaining slice stays strictly above MM!
        if !is_full_close {
            let rem_size = pos.size_e8 - close_size_e8;
            let rem_margin = pos.locked_margin_e8 - slice_margin_e8;
            let rem_notional = pos.notional_e8 - slice_notional_e8;
            let raw_rem_pnl = compute_unrealized_pnl_usd_e8(
                pos.side.into(),
                rem_size,
                pos.entry_price_e8,
                index_price_e8,
            )
            .map_err(|e| format!("ERR_REM_PNL: {e:?}"))?;
            let capped_rem_pnl =
                apply_profit_cap_usd_e8(raw_rem_pnl, rem_notional, market.risk.max_pnl_bps)
                    .map_err(|e| format!("ERR_REM_CAP: {e:?}"))?;
            let rem_funding = compute_position_funding_net_usd_e8(
                pos.side.into(),
                rem_size,
                pos.entry_funding_index_e12,
                market.cum_funding_index_e12,
            )
            .map_err(|e| format!("ERR_REM_FUND: {e:?}"))?;
            let rem_eq = compute_position_equity_usd_e8(rem_margin, capped_rem_pnl, rem_funding)
                .map_err(|e| format!("ERR_REM_EQ: {e:?}"))?;
            let rem_mark_notional = compute_notional_ceil_usd_e8(index_price_e8, rem_size)
                .map_err(|e| format!("ERR_REM_NOTIONAL: {e:?}"))?;
            if is_liquidatable(
                rem_eq,
                rem_mark_notional,
                market.risk.maintenance_margin_bps,
            )
            .map_err(|e| format!("ERR_REM_LIQ: {e:?}"))?
            {
                return Err("ERR_PARTIAL_CLOSE_LEAVES_UNHEALTHY".to_string());
            }
        }

        // Commit settlement cash flow
        self.apply_settlement_ledger_update(
            &mut market,
            &pos.owner,
            slice_margin_e8,
            settle.trader_payout_e8,
            settle.fee_e8,
            settle.vault_delta_e8,
            settle.bad_debt_e8,
        );

        match pos.side {
            StoredPositionSide::Long => {
                market.long_oi_e8 -= slice_notional_e8;
                market.long_size_e8 -= close_size_e8;
            }
            StoredPositionSide::Short => {
                market.short_oi_e8 -= slice_notional_e8;
                market.short_size_e8 -= close_size_e8;
            }
        }

        if is_full_close {
            market.open_positions -= 1;
            self.positions.remove(&position_id);
            self.open_position_ids.retain(|id| *id != position_id);
        } else {
            let mut rem_pos = pos.clone();
            rem_pos.size_e8 -= close_size_e8;
            rem_pos.notional_e8 -= slice_notional_e8;
            rem_pos.locked_margin_e8 -= slice_margin_e8;
            rem_pos.locked = false;
            // NOTE: keep `rem_pos.entry_funding_index_e12` untouched so remaining slice accrues identically.
            self.positions.insert(position_id, rem_pos);
        }
        self.markets.insert(pos.market_id.clone(), market);

        self.assert_solvency();

        if is_full_close {
            emit_event(
                "position_closed",
                &PositionClosedEvent {
                    position_id,
                    owner: trader.as_str(),
                    market_id: &pos.market_id,
                    exec_price_e8: exec_price_e8.to_string(),
                    realized_pnl_e8: settle.settlement_pnl_e8.to_string(),
                    funding_settled_e8: settle.settlement_funding_e8.to_string(),
                    fee_e8: settle.fee_e8.to_string(),
                    payout_e8: settle.trader_payout_e8.to_string(),
                    bad_debt_e8: settle.bad_debt_e8.to_string(),
                    emergency: false,
                },
            );
        } else {
            emit_event(
                "position_reduced",
                &PositionReducedEvent {
                    position_id,
                    owner: trader.as_str(),
                    closed_size_e8: close_size_e8.to_string(),
                    remaining_size_e8: (pos.size_e8 - close_size_e8).to_string(),
                    exec_price_e8: exec_price_e8.to_string(),
                    realized_pnl_e8: settle.settlement_pnl_e8.to_string(),
                    funding_settled_e8: settle.settlement_funding_e8.to_string(),
                    fee_e8: settle.fee_e8.to_string(),
                    trader_payout_e8: settle.trader_payout_e8.to_string(),
                },
            );
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_settlement_ledger_update(
        &mut self,
        market: &mut PerpsMarketState,
        owner: &AccountId,
        slice_margin_e8: u128,
        trader_payout_e8: u128,
        fee_e8: u128,
        vault_delta_e8: i128,
        bad_debt_e8: u128,
    ) {
        self.buckets.total_trader_locked_margin_e8 = self
            .buckets
            .total_trader_locked_margin_e8
            .checked_sub(slice_margin_e8)
            .unwrap_or_else(|| env::panic_str("ERR_LOCKED_MARGIN_UNDERFLOW"));

        if trader_payout_e8 > 0 {
            let mut acc = self
                .accounts
                .get(owner)
                .cloned()
                .unwrap_or_else(|| env::panic_str("ERR_OWNER_ACCOUNT_MISSING"));
            acc.free_collateral_e8 = acc
                .free_collateral_e8
                .checked_add(trader_payout_e8)
                .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
            self.accounts.insert(owner.clone(), acc);
            self.buckets.total_trader_free_collateral_e8 = self
                .buckets
                .total_trader_free_collateral_e8
                .checked_add(trader_payout_e8)
                .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        }

        self.split_and_credit_fee_e8(fee_e8);

        if vault_delta_e8 >= 0 {
            self.buckets.lp_vault_realized_equity_e8 = self
                .buckets
                .lp_vault_realized_equity_e8
                .checked_add(vault_delta_e8 as u128)
                .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        } else {
            self.buckets.lp_vault_realized_equity_e8 = self
                .buckets
                .lp_vault_realized_equity_e8
                .checked_sub(vault_delta_e8.unsigned_abs())
                .unwrap_or_else(|| env::panic_str("ERR_VAULT_EQUITY_UNDERFLOW"));
        }

        if bad_debt_e8 > 0 {
            let cascade = apply_bad_debt_cascade(
                bad_debt_e8,
                self.buckets.insurance_fund_e8,
                self.buckets.lp_vault_realized_equity_e8,
            );
            if cascade.absorbed_by_insurance_e8 > 0 {
                self.buckets.insurance_fund_e8 -= cascade.absorbed_by_insurance_e8;
                self.buckets.lp_vault_realized_equity_e8 = self
                    .buckets
                    .lp_vault_realized_equity_e8
                    .checked_add(cascade.absorbed_by_insurance_e8)
                    .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
            }
            if cascade.should_pause_market {
                market.paused_for_insolvency = true;
                if market.mode != StoredMarketMode::Emergency {
                    let prev = Self::mode_label(market.mode);
                    market.mode = StoredMarketMode::Paused;
                    emit_event(
                        "market_paused",
                        &MarketModeChangedEvent {
                            market_id: &market.market_id,
                            triggered_by: env::current_account_id().as_str(),
                            previous_mode: prev,
                            new_mode: "Paused",
                            reason: "bad_debt_cascade_insolvency",
                            frozen_settlement_price_e8: None,
                            frozen_funding_index_e12: None,
                        },
                    );
                }
            }
            emit_event(
                "insurance_updated",
                &InsuranceUpdatedEvent {
                    reason: "bad_debt_absorbed",
                    delta_e8: format!("-{}", cascade.absorbed_by_insurance_e8),
                    new_balance_e8: self.buckets.insurance_fund_e8.to_string(),
                    uncovered_insolvency_e8: cascade.uncovered_insolvency_e8.to_string(),
                },
            );
        }
    }

    pub(crate) fn internal_remove_margin(
        &mut self,
        trader: &AccountId,
        position_id: u64,
        remove_amount_e8: u128,
        index_price_e8: u128,
        now_sec: u64,
    ) -> Result<(), String> {
        if self.paused_global {
            return Err("ERR_PROTOCOL_PAUSED".to_string());
        }
        if remove_amount_e8 == 0 {
            return Err("ERR_INVALID_AMOUNT".to_string());
        }

        let mut pos = self
            .positions
            .get(&position_id)
            .cloned()
            .ok_or_else(|| "ERR_POSITION_NOT_FOUND".to_string())?;
        if &pos.owner != trader {
            return Err("ERR_UNAUTHORIZED".to_string());
        }

        let market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .ok_or_else(|| "ERR_MARKET_NOT_FOUND".to_string())?;
        if market.mode != StoredMarketMode::Normal || market.paused_for_insolvency {
            return Err("ERR_MARKET_NOT_NORMAL".to_string());
        }

        self.internal_accrue_funding_at_price(&pos.market_id, index_price_e8, now_sec)?;
        let market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .ok_or_else(|| "ERR_MARKET_NOT_FOUND".to_string())?;

        let entry_im = compute_initial_margin_usd_e8(
            compute_notional_ceil_usd_e8(pos.entry_price_e8, pos.size_e8)
                .map_err(|e| format!("ERR_NOTIONAL: {e:?}"))?,
            market.risk.initial_margin_bps,
        )
        .map_err(|e| format!("ERR_IM: {e:?}"))?;

        let mark_im = compute_initial_margin_usd_e8(
            compute_notional_ceil_usd_e8(index_price_e8, pos.size_e8)
                .map_err(|e| format!("ERR_NOTIONAL: {e:?}"))?,
            market.risk.initial_margin_bps,
        )
        .map_err(|e| format!("ERR_IM: {e:?}"))?;

        let raw_pnl = compute_unrealized_pnl_usd_e8(
            pos.side.into(),
            pos.size_e8,
            pos.entry_price_e8,
            index_price_e8,
        )
        .map_err(|e| format!("ERR_PNL: {e:?}"))?;
        let capped_pnl = apply_profit_cap_usd_e8(raw_pnl, pos.notional_e8, market.risk.max_pnl_bps)
            .map_err(|e| format!("ERR_CAP: {e:?}"))?;
        let funding_net = compute_position_funding_net_usd_e8(
            pos.side.into(),
            pos.size_e8,
            pos.entry_funding_index_e12,
            market.cum_funding_index_e12,
        )
        .map_err(|e| format!("ERR_FUND: {e:?}"))?;

        let mark_eq = compute_position_equity_usd_e8(pos.locked_margin_e8, capped_pnl, funding_net)
            .map_err(|e| format!("ERR_EQ: {e:?}"))?;
        let max_removable =
            compute_available_margin_usd_e8(pos.locked_margin_e8, entry_im, mark_eq, mark_im)
                .map_err(|e| format!("ERR_AVAIL_MARGIN: {e:?}"))?;

        if remove_amount_e8 > max_removable {
            return Err(format!(
                "ERR_EXCEEDS_AVAILABLE_MARGIN: requested {remove_amount_e8} > max {max_removable}"
            ));
        }

        pos.locked_margin_e8 -= remove_amount_e8;
        pos.locked = false;
        let new_locked = pos.locked_margin_e8;
        self.positions.insert(position_id, pos);

        let mut acc = self
            .accounts
            .get(trader)
            .cloned()
            .ok_or_else(|| "ERR_ACCOUNT_NOT_REGISTERED".to_string())?;
        acc.free_collateral_e8 = acc
            .free_collateral_e8
            .checked_add(remove_amount_e8)
            .ok_or_else(|| "ERR_OVERFLOW".to_string())?;
        self.accounts.insert(trader.clone(), acc);

        self.buckets.total_trader_locked_margin_e8 -= remove_amount_e8;
        self.buckets.total_trader_free_collateral_e8 += remove_amount_e8;
        self.assert_solvency();

        emit_event(
            "margin_removed",
            &MarginAdjustedEvent {
                position_id,
                owner: trader.as_str(),
                delta_margin_e8: format!("-{remove_amount_e8}"),
                new_locked_margin_e8: new_locked.to_string(),
                mark_price_e8: Some(index_price_e8.to_string()),
            },
        );

        Ok(())
    }

    pub(crate) fn internal_liquidate_position(
        &mut self,
        keeper: &AccountId,
        position_id: u64,
        mark_price_e8: u128,
        now_sec: u64,
    ) -> Result<(), String> {
        if self.paused_global {
            return Err("ERR_PROTOCOL_PAUSED".to_string());
        }

        let pos = self
            .positions
            .get(&position_id)
            .cloned()
            .ok_or_else(|| "ERR_POSITION_NOT_FOUND".to_string())?;
        let market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .ok_or_else(|| "ERR_MARKET_NOT_FOUND".to_string())?;
        if market.mode == StoredMarketMode::Emergency {
            return Err("ERR_USE_EMERGENCY_SETTLEMENT".to_string());
        }
        if !self
            .accounts
            .get(keeper)
            .map(|a| a.registered)
            .unwrap_or(false)
        {
            return Err("ERR_KEEPER_NOT_REGISTERED".to_string());
        }

        self.internal_accrue_funding_at_price(&pos.market_id, mark_price_e8, now_sec)?;

        let mut market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .ok_or_else(|| "ERR_MARKET_NOT_FOUND".to_string())?;

        let raw_pnl = compute_unrealized_pnl_usd_e8(
            pos.side.into(),
            pos.size_e8,
            pos.entry_price_e8,
            mark_price_e8,
        )
        .map_err(|e| format!("ERR_PNL: {e:?}"))?;
        let capped_pnl = apply_profit_cap_usd_e8(raw_pnl, pos.notional_e8, market.risk.max_pnl_bps)
            .map_err(|e| format!("ERR_CAP: {e:?}"))?;
        let funding_net = compute_position_funding_net_usd_e8(
            pos.side.into(),
            pos.size_e8,
            pos.entry_funding_index_e12,
            market.cum_funding_index_e12,
        )
        .map_err(|e| format!("ERR_FUND: {e:?}"))?;

        let shared_params = market.risk.to_shared();
        let liq = compute_liquidation_settlement(
            pos.locked_margin_e8,
            capped_pnl,
            funding_net,
            mark_price_e8,
            pos.size_e8,
            &shared_params,
        )
        .map_err(|e| format!("ERR_LIQ_MATH: {e:?}"))?;

        if !liq.liquidatable {
            return Err("ERR_POSITION_NOT_LIQUIDATABLE".to_string());
        }

        let margin_i128 =
            i128::try_from(pos.locked_margin_e8).map_err(|_| "ERR_OVERFLOW".to_string())?;
        let vault_delta_e8 = if liq.equity_before_close_e8 <= 0 {
            margin_i128
        } else {
            margin_i128
                .checked_sub(liq.equity_before_close_e8)
                .ok_or_else(|| "ERR_OVERFLOW".to_string())?
        };
        if vault_delta_e8 < 0
            && self.buckets.lp_vault_realized_equity_e8 < vault_delta_e8.unsigned_abs()
        {
            return Err("ERR_INSUFFICIENT_VAULT_LIQUIDITY".to_string());
        }

        // Deduct locked margin
        self.buckets.total_trader_locked_margin_e8 = self
            .buckets
            .total_trader_locked_margin_e8
            .checked_sub(pos.locked_margin_e8)
            .unwrap_or_else(|| env::panic_str("ERR_LOCKED_MARGIN_UNDERFLOW"));

        if liq.trader_remaining_margin_e8 > 0 {
            let mut owner_acc = self
                .accounts
                .get(&pos.owner)
                .cloned()
                .unwrap_or_else(|| env::panic_str("ERR_OWNER_MISSING"));
            owner_acc.free_collateral_e8 = owner_acc
                .free_collateral_e8
                .checked_add(liq.trader_remaining_margin_e8)
                .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
            self.accounts.insert(pos.owner.clone(), owner_acc);
            self.buckets.total_trader_free_collateral_e8 += liq.trader_remaining_margin_e8;
        }

        if liq.liq_keeper_fee_paid_e8 > 0 {
            let mut keeper_acc = self
                .accounts
                .get(keeper)
                .cloned()
                .unwrap_or_else(|| env::panic_str("ERR_KEEPER_NOT_REGISTERED"));
            keeper_acc.free_collateral_e8 = keeper_acc
                .free_collateral_e8
                .checked_add(liq.liq_keeper_fee_paid_e8)
                .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
            self.accounts.insert(keeper.clone(), keeper_acc);
            self.buckets.total_trader_free_collateral_e8 += liq.liq_keeper_fee_paid_e8;
        }

        if liq.liq_insurance_fee_paid_e8 > 0 {
            self.buckets.insurance_fund_e8 = self
                .buckets
                .insurance_fund_e8
                .checked_add(liq.liq_insurance_fee_paid_e8)
                .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        }

        self.split_and_credit_fee_e8(liq.close_fee_paid_e8);

        if vault_delta_e8 >= 0 {
            self.buckets.lp_vault_realized_equity_e8 = self
                .buckets
                .lp_vault_realized_equity_e8
                .checked_add(vault_delta_e8 as u128)
                .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        } else {
            self.buckets.lp_vault_realized_equity_e8 -= vault_delta_e8.unsigned_abs();
        }

        if liq.bad_debt_e8 > 0 {
            let cascade = apply_bad_debt_cascade(
                liq.bad_debt_e8,
                self.buckets.insurance_fund_e8,
                self.buckets.lp_vault_realized_equity_e8,
            );
            if cascade.absorbed_by_insurance_e8 > 0 {
                self.buckets.insurance_fund_e8 -= cascade.absorbed_by_insurance_e8;
                self.buckets.lp_vault_realized_equity_e8 += cascade.absorbed_by_insurance_e8;
            }
            if cascade.should_pause_market {
                market.paused_for_insolvency = true;
                market.mode = StoredMarketMode::Paused;
            }
            emit_event(
                "insurance_updated",
                &InsuranceUpdatedEvent {
                    reason: "bad_debt_absorbed",
                    delta_e8: format!("-{}", cascade.absorbed_by_insurance_e8),
                    new_balance_e8: self.buckets.insurance_fund_e8.to_string(),
                    uncovered_insolvency_e8: cascade.uncovered_insolvency_e8.to_string(),
                },
            );
        }

        match pos.side {
            StoredPositionSide::Long => {
                market.long_oi_e8 -= pos.notional_e8;
                market.long_size_e8 -= pos.size_e8;
            }
            StoredPositionSide::Short => {
                market.short_oi_e8 -= pos.notional_e8;
                market.short_size_e8 -= pos.size_e8;
            }
        }
        market.open_positions -= 1;
        self.markets.insert(pos.market_id.clone(), market);

        self.positions.remove(&position_id);
        self.open_position_ids.retain(|id| *id != position_id);

        self.assert_solvency();

        emit_event(
            "position_liquidated",
            &PositionLiquidatedEvent {
                position_id,
                owner: pos.owner.as_str(),
                keeper: keeper.as_str(),
                market_id: &pos.market_id,
                mark_price_e8: mark_price_e8.to_string(),
                keeper_fee_e8: liq.liq_keeper_fee_paid_e8.to_string(),
                insurance_fee_e8: liq.liq_insurance_fee_paid_e8.to_string(),
                close_fee_e8: liq.close_fee_paid_e8.to_string(),
                trader_refund_e8: liq.trader_remaining_margin_e8.to_string(),
                bad_debt_e8: liq.bad_debt_e8.to_string(),
            },
        );

        Ok(())
    }

    pub(crate) fn internal_trigger_emergency_settlement(
        &mut self,
        caller: &AccountId,
        market_id: &str,
        now_sec: u64,
    ) -> Result<(), String> {
        let market = self
            .markets
            .get(market_id)
            .cloned()
            .ok_or_else(|| format!("ERR_MARKET_NOT_FOUND: {market_id}"))?;

        if market.mode == StoredMarketMode::Emergency {
            return Err("ERR_EMERGENCY_ALREADY_ACTIVE".to_string());
        }
        if market.paused_for_insolvency {
            return Err("ERR_INSOLVENT_MARKET_REQUIRES_ADMIN_RESOLUTION".to_string());
        }

        let ref_obs_sec = emergency_reference_observed_sec(&market)
            .ok_or_else(|| "ERR_EMERGENCY_NO_REFERENCE_TIMESTAMP".to_string())?;
        let outage_age_sec = now_sec.saturating_sub(ref_obs_sec);
        if outage_age_sec < market.oracle_policy.emergency_close_age_sec {
            return Err(format!(
                "ERR_EMERGENCY_NOT_ELIGIBLE: outage {outage_age_sec}s < {}s",
                market.oracle_policy.emergency_close_age_sec
            ));
        }

        let frozen_price_e8 = market
            .attested_emergency_price_e8
            .or(market.last_valid_price_e8)
            .or_else(|| market.sandbox_observation.as_ref().map(|o| o.price_e8))
            .ok_or_else(|| "ERR_EMERGENCY_NO_VALID_REFERENCE_PRICE".to_string())?;

        // Accrue remaining valid funding up to outage cutoff BEFORE freezing funding index.
        if let Some(last_p) = market.last_valid_price_e8 {
            let _ = self.internal_accrue_funding_at_price(market_id, last_p, now_sec);
        }

        let mut market = self
            .markets
            .get(market_id)
            .cloned()
            .ok_or_else(|| "ERR_MARKET_NOT_FOUND".to_string())?;
        let prev = Self::mode_label(market.mode);
        let frozen_funding = market.cum_funding_index_e12;
        market.mode = StoredMarketMode::Emergency;
        market.frozen_settlement_price_e8 = Some(frozen_price_e8);
        market.frozen_funding_index_e12 = Some(frozen_funding);
        market.emergency_triggered_at_sec = Some(now_sec);
        self.markets.insert(market_id.to_string(), market);

        emit_event(
            "market_paused",
            &MarketModeChangedEvent {
                market_id,
                triggered_by: caller.as_str(),
                previous_mode: prev,
                new_mode: "Emergency",
                reason: "uniform_global_emergency_settlement_frozen",
                frozen_settlement_price_e8: Some(frozen_price_e8.to_string()),
                frozen_funding_index_e12: Some(frozen_funding.to_string()),
            },
        );

        Ok(())
    }

    pub(crate) fn internal_claim_emergency_settlement(
        &mut self,
        position_id: u64,
    ) -> Result<(), String> {
        let pos = self
            .positions
            .get(&position_id)
            .cloned()
            .ok_or_else(|| "ERR_POSITION_NOT_FOUND".to_string())?;
        if pos.locked {
            return Err("ERR_POSITION_LOCKED".to_string());
        }

        let mut market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .ok_or_else(|| "ERR_MARKET_NOT_FOUND".to_string())?;
        if market.mode != StoredMarketMode::Emergency {
            return Err("ERR_MARKET_NOT_IN_EMERGENCY".to_string());
        }

        let frozen_price_e8 = market
            .frozen_settlement_price_e8
            .ok_or_else(|| "ERR_MISSING_FROZEN_EMERGENCY_PRICE".to_string())?;
        let frozen_funding_e12 = market
            .frozen_funding_index_e12
            .ok_or_else(|| "ERR_MISSING_FROZEN_EMERGENCY_FUNDING".to_string())?;

        let snapshot = PositionSnapshot {
            side: pos.side.into(),
            size_e8: pos.size_e8,
            locked_margin_e8: pos.locked_margin_e8,
            entry_price_e8: pos.entry_price_e8,
            entry_funding_index_e12: pos.entry_funding_index_e12,
        };

        let settle = compute_uniform_emergency_settlement_usd_e8(
            &snapshot,
            frozen_price_e8,
            frozen_funding_e12,
            market.risk.max_pnl_bps,
        )
        .map_err(|e| format!("ERR_EMERGENCY_SETTLEMENT_MATH: {e:?}"))?;

        if settle.vault_delta_e8 < 0 {
            let needed = settle.vault_delta_e8.unsigned_abs();
            self.check_winner_settlement_order(
                &pos.market_id,
                position_id,
                needed,
                frozen_price_e8,
            )?;
        }

        self.apply_settlement_ledger_update(
            &mut market,
            &pos.owner,
            pos.locked_margin_e8,
            settle.trader_payout_e8,
            0, // close_fee_bps = 0 enforced symmetrically
            settle.vault_delta_e8,
            settle.bad_debt_e8,
        );

        match pos.side {
            StoredPositionSide::Long => {
                market.long_oi_e8 -= pos.notional_e8;
                market.long_size_e8 -= pos.size_e8;
            }
            StoredPositionSide::Short => {
                market.short_oi_e8 -= pos.notional_e8;
                market.short_size_e8 -= pos.size_e8;
            }
        }
        market.open_positions -= 1;
        self.markets.insert(pos.market_id.clone(), market);
        self.positions.remove(&position_id);
        self.open_position_ids.retain(|id| *id != position_id);

        self.assert_solvency();

        emit_event(
            "position_closed",
            &PositionClosedEvent {
                position_id,
                owner: pos.owner.as_str(),
                market_id: &pos.market_id,
                exec_price_e8: frozen_price_e8.to_string(),
                realized_pnl_e8: settle.settlement_pnl_e8.to_string(),
                funding_settled_e8: settle.settlement_funding_e8.to_string(),
                fee_e8: "0".to_string(),
                payout_e8: settle.trader_payout_e8.to_string(),
                bad_debt_e8: settle.bad_debt_e8.to_string(),
                emergency: true,
            },
        );

        Ok(())
    }

    /// Pure view helper computing indicative health + liquidation price (explicitly NOT binding on-chain).
    pub(crate) fn internal_position_health_view(
        &self,
        pos: &PositionRecord,
        market: &PerpsMarketState,
        ref_price_e8: u128,
    ) -> Result<PositionHealthView, String> {
        let raw_pnl = compute_unrealized_pnl_usd_e8(
            pos.side.into(),
            pos.size_e8,
            pos.entry_price_e8,
            ref_price_e8,
        )
        .map_err(|e| format!("ERR_VIEW_PNL: {e:?}"))?;
        let capped_pnl = apply_profit_cap_usd_e8(raw_pnl, pos.notional_e8, market.risk.max_pnl_bps)
            .map_err(|e| format!("ERR_VIEW_CAP: {e:?}"))?;
        let cum_f = if market.mode == StoredMarketMode::Emergency {
            market
                .frozen_funding_index_e12
                .unwrap_or(market.cum_funding_index_e12)
        } else {
            market.cum_funding_index_e12
        };
        let funding_net = compute_position_funding_net_usd_e8(
            pos.side.into(),
            pos.size_e8,
            pos.entry_funding_index_e12,
            cum_f,
        )
        .map_err(|e| format!("ERR_VIEW_FUND: {e:?}"))?;
        let eq = compute_position_equity_usd_e8(pos.locked_margin_e8, capped_pnl, funding_net)
            .map_err(|e| format!("ERR_VIEW_EQ: {e:?}"))?;
        let mark_notional_ceil = compute_notional_ceil_usd_e8(ref_price_e8, pos.size_e8)
            .map_err(|e| format!("ERR_VIEW_NOTIONAL: {e:?}"))?;
        let mark_notional_floor = compute_notional_floor_usd_e8(ref_price_e8, pos.size_e8)
            .map_err(|e| format!("ERR_VIEW_NOTIONAL: {e:?}"))?;
        let mm_e8 = compute_maintenance_margin_usd_e8(
            mark_notional_ceil,
            market.risk.maintenance_margin_bps,
        )
        .map_err(|e| format!("ERR_VIEW_MM: {e:?}"))?;
        let mr_bps = compute_margin_ratio_bps(eq, mark_notional_floor)
            .map_err(|e| format!("ERR_VIEW_MR: {e:?}"))?;
        let liq = is_liquidatable(eq, mark_notional_ceil, market.risk.maintenance_margin_bps)
            .map_err(|e| format!("ERR_VIEW_LIQ: {e:?}"))?;
        let liq_price = compute_indicative_liquidation_price_e8(
            pos.side.into(),
            pos.size_e8,
            pos.entry_price_e8,
            pos.locked_margin_e8,
            funding_net,
            market.risk.maintenance_margin_bps,
        )
        .map_err(|e| format!("ERR_VIEW_LIQ_PRICE: {e:?}"))?;

        Ok(PositionHealthView {
            position: PositionRecordView::from(pos),
            reference_price_e8: near_sdk::json_types::U128(ref_price_e8),
            unrealized_pnl_e8: raw_pnl.to_string(),
            capped_pnl_e8: capped_pnl.to_string(),
            funding_net_e8: funding_net.to_string(),
            equity_e8: eq.to_string(),
            maintenance_margin_e8: near_sdk::json_types::U128(mm_e8),
            margin_ratio_bps: u32::try_from(mr_bps).unwrap_or(u32::MAX),
            liquidatable_at_reference: liq,
            indicative_liquidation_price_e8: Some(near_sdk::json_types::U128(liq_price)),
        })
    }

    pub(crate) fn internal_dispatch_withdrawal_kind(
        &mut self,
        kind: WithdrawalKind,
        account_id: near_sdk::AccountId,
        token_id: near_sdk::AccountId,
        units: u128,
        usd_e8: u128,
    ) -> near_sdk::Promise {
        self.internal_dispatch_withdrawal(kind, account_id, token_id, units, usd_e8)
    }
}
