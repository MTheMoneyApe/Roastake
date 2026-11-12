// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

//! ROASTAKE Perpetuals MVP Contract (NEAR Testnet ONLY).
//!
//! Double-entry isolated-margin Peer-to-Pool Perpetuals engine reusing `roastake_perps_core`
//! math & M04.5A.1 audited security invariants, configured by default for official Circle NEAR
//! Testnet USDC (`3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`,
//! `decimals = 6` scaled 100x to `USD_SCALE = 10^8`), while rejecting unauthorized tokens.

pub mod collateral;
pub mod engine;
pub mod events;
pub mod oracle;
pub mod types;

#[cfg(test)]
mod tests;

use near_sdk::json_types::U128;
use near_sdk::store::{IterableMap, LookupMap};
use near_sdk::{
    env, near, require, AccountId, NearToken, PanicOnDefault, Promise, PromiseError, PromiseOrValue,
};
use roastake_perps_core::{
    compute_position_settlement_e8, compute_uniform_emergency_settlement_usd_e8, PositionSnapshot,
    UniformPositionSettlement, BPS_SCALE, TESTNET_DEFAULT_MAX_PNL_BPS, USD_SCALE,
};

use crate::collateral::ext_ft;
use crate::engine::{
    IncreasePositionParams, LpDepositExecParams, LpWithdrawOutcome, OpenPositionParams,
};
use crate::events::{
    emit_event, LpWithdrawnEvent, MarginAdjustedEvent, MarketModeChangedEvent,
    OracleSandboxTickEvent, SecurityAnomalyEvent,
};
use crate::oracle::{
    assert_market_symbol_not_prohibited_oracle, evaluate_candidate_price_against_policy,
    evaluate_sandbox_feed_for_execution, ext_oracle, validate_external_observation_metadata,
    validate_sandbox_price_bounds, EvaluatedPriceResult,
};
use crate::types::{
    assert_testnet_or_sandbox_account, validate_initial_lp_floor, AccountRecord, AccountView,
    AccountingSnapshotView, CollateralTokenConfig, CollateralTokenView, DepositMemo,
    ExternalPriceObservation, MarketView, OracleSourceConfig, PendingOracleAction,
    PendingOracleActionKind, PerpsMarketState, PositionHealthView, PositionRecord,
    PositionRecordView, StorageBalanceBoundsView, StorageBalanceView, StoredCashBuckets,
    StoredMarketMode, StoredMarketRiskParams, StoredOraclePolicy, StoredPositionSide,
    TestnetSandboxObservation, WithdrawalKind, WithdrawalRecord, ACCOUNT_STORAGE_DEPOSIT_YOCTO,
    DATA_CLASS_TESTNET_SANDBOX, MAX_CALLBACK_PAYLOAD_BYTES, MAX_CLOCK_DRIFT_SEC,
    MAX_METADATA_PAYLOAD_BYTES, METADATA_VERIFY_CALLBACK_GAS, MODE_TESTNET_SANDBOX,
    ORACLE_XCC_CALLBACK_GAS, ORACLE_XCC_VIEW_GAS,
};

/// LP Valuation summary returned by `get_lp_snapshot`.
#[near(serializers = [json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LpSnapshotView {
    pub lp_vault_realized_equity_e8: U128,
    pub lp_mtm_equity_e8: String,
    pub reserved_liquidity_e8: U128,
    pub lp_available_liquidity_e8: U128,
    pub net_bounded_trader_claim_e8: String,
    pub total_unrealized_bad_debt_e8: U128,
    pub lp_total_shares: U128,
    pub lp_locked_seed_shares: U128,
    pub withdrawals_locked_for_emergency: bool,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct PerpsContract {
    pub admin: AccountId,
    pub pending_admin: Option<AccountId>,
    pub treasury: AccountId,
    pub paused_global: bool,
    pub treasury_withdrawal_pending: bool,

    pub tracked_collateral_e8: u128,
    pub buckets: StoredCashBuckets,
    pub lp_total_shares: u128,
    pub lp_locked_seed_shares: u128,

    pub collateral_token_ids: Vec<AccountId>,
    pub collateral_tokens: IterableMap<AccountId, CollateralTokenConfig>,

    pub market_ids: Vec<String>,
    pub markets: IterableMap<String, PerpsMarketState>,

    pub accounts: IterableMap<AccountId, AccountRecord>,

    pub next_position_id: u64,
    pub open_position_ids: Vec<u64>,
    pub positions: IterableMap<u64, PositionRecord>,

    pub next_withdrawal_id: u64,
    pub pending_withdrawals: LookupMap<u64, WithdrawalRecord>,

    pub next_oracle_action_id: u64,
    pub pending_oracle_actions: LookupMap<u64, PendingOracleAction>,
}

#[near]
impl PerpsContract {
    /// Initialize the ROASTAKE Perpetuals Testnet contract with an allowlisted primary NEP-141 collateral asset.
    #[init]
    pub fn init(
        admin: AccountId,
        treasury: AccountId,
        primary_collateral_token: AccountId,
        collateral_symbol: String,
        collateral_decimals: u8,
        non_monetary_testnet_disclosure: String,
    ) -> Self {
        Self::require_testnet_or_sandbox(&env::current_account_id(), "contract_account");
        Self::require_testnet_or_sandbox(&admin, "admin");
        Self::require_testnet_or_sandbox(&treasury, "treasury");
        Self::require_testnet_or_sandbox(&primary_collateral_token, "primary_collateral_token");

        let scale = Self::compute_scale_to_e8(collateral_decimals);
        if collateral_symbol.trim().is_empty() || collateral_symbol.len() > 16 {
            env::panic_str("ERR_INVALID_COLLATERAL_SYMBOL");
        }
        if non_monetary_testnet_disclosure.trim().is_empty() {
            env::panic_str("ERR_DISCLOSURE_REQUIRED");
        }

        let mut collateral_tokens = IterableMap::new(b"c");
        collateral_tokens.insert(
            primary_collateral_token.clone(),
            CollateralTokenConfig {
                token_id: primary_collateral_token.clone(),
                symbol: collateral_symbol,
                decimals: collateral_decimals,
                scale_to_usd_e8: scale,
                non_monetary_testnet_disclosure,
                active: true,
                metadata_verified_on_chain: false,
                tracked_balance_units: 0,
            },
        );

        Self {
            admin,
            pending_admin: None,
            treasury,
            paused_global: false,
            treasury_withdrawal_pending: false,
            tracked_collateral_e8: 0,
            buckets: StoredCashBuckets::default(),
            lp_total_shares: 0,
            lp_locked_seed_shares: 0,
            collateral_token_ids: vec![primary_collateral_token],
            collateral_tokens,
            market_ids: Vec::new(),
            markets: IterableMap::new(b"m"),
            accounts: IterableMap::new(b"a"),
            next_position_id: 0,
            open_position_ids: Vec::new(),
            positions: IterableMap::new(b"p"),
            next_withdrawal_id: 0,
            pending_withdrawals: LookupMap::new(b"w"),
            next_oracle_action_id: 0,
            pending_oracle_actions: LookupMap::new(b"o"),
        }
    }

    // -------------------------------------------------------------------------
    // Admin Governance & Configuration
    // -------------------------------------------------------------------------

    pub fn propose_admin(&mut self, new_admin: AccountId) {
        self.assert_admin();
        Self::require_testnet_or_sandbox(&new_admin, "pending_admin");
        self.pending_admin = Some(new_admin);
    }

    pub fn accept_admin(&mut self) {
        let caller = env::predecessor_account_id();
        let pending = self
            .pending_admin
            .clone()
            .unwrap_or_else(|| env::panic_str("ERR_NO_PENDING_ADMIN"));
        if caller != pending {
            env::panic_str("ERR_UNAUTHORIZED");
        }
        self.admin = pending;
        self.pending_admin = None;
    }

    pub fn cancel_admin_proposal(&mut self) {
        self.assert_admin();
        self.pending_admin = None;
    }

    pub fn set_global_pause(&mut self, paused: bool) {
        self.assert_admin();
        self.paused_global = paused;
    }

    pub fn add_collateral_token(
        &mut self,
        token_id: AccountId,
        symbol: String,
        decimals: u8,
        non_monetary_testnet_disclosure: String,
    ) {
        self.assert_admin();
        Self::require_testnet_or_sandbox(&token_id, "collateral_token");
        let scale = Self::compute_scale_to_e8(decimals);
        if symbol.trim().is_empty() || symbol.len() > 16 {
            env::panic_str("ERR_INVALID_COLLATERAL_SYMBOL");
        }
        if non_monetary_testnet_disclosure.trim().is_empty() {
            env::panic_str("ERR_DISCLOSURE_REQUIRED");
        }
        if self.collateral_tokens.get(&token_id).is_some() {
            env::panic_str("ERR_COLLATERAL_ALREADY_REGISTERED");
        }
        self.collateral_token_ids.push(token_id.clone());
        self.collateral_tokens.insert(
            token_id.clone(),
            CollateralTokenConfig {
                token_id,
                symbol,
                decimals,
                scale_to_usd_e8: scale,
                non_monetary_testnet_disclosure,
                active: true,
                metadata_verified_on_chain: false,
                tracked_balance_units: 0,
            },
        );
    }

    pub fn set_collateral_token_active(&mut self, token_id: AccountId, active: bool) {
        self.assert_admin();
        let mut cfg = self
            .collateral_tokens
            .get(&token_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_COLLATERAL_NOT_FOUND"));
        cfg.active = active;
        self.collateral_tokens.insert(token_id, cfg);
    }

    /// Trigger real on-chain NEP-148 `ft_metadata` verification against allowlisted token expectations.
    pub fn verify_collateral_metadata(&mut self, token_id: AccountId) -> Promise {
        self.assert_admin();
        if self.collateral_tokens.get(&token_id).is_none() {
            env::panic_str("ERR_COLLATERAL_NOT_FOUND");
        }
        ext_ft::ext(token_id.clone())
            .with_static_gas(ORACLE_XCC_VIEW_GAS)
            .ft_metadata()
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(METADATA_VERIFY_CALLBACK_GAS)
                    .on_verify_collateral_metadata(token_id),
            )
    }

    #[private]
    pub fn on_verify_collateral_metadata(&mut self, token_id: AccountId) -> bool {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "Method on_verify_collateral_metadata is private"
        );
        let res = env::promise_result_checked(0, MAX_METADATA_PAYLOAD_BYTES);
        self.internal_handle_verify_metadata_callback(token_id, res)
    }

    /// Register a new perpetuals market with explicit M04.5A.1 risk invariants and Oracle source binding.
    pub fn register_market(
        &mut self,
        market_id: String,
        oracle_source: OracleSourceConfig,
        risk: Option<StoredMarketRiskParams>,
        oracle_policy: Option<StoredOraclePolicy>,
        exec_spread_bps: Option<u32>,
    ) {
        self.assert_admin();
        if market_id.trim().is_empty() || market_id.len() > 64 {
            env::panic_str("ERR_INVALID_MARKET_ID");
        }
        if self.markets.get(&market_id).is_some() {
            env::panic_str("ERR_MARKET_ALREADY_EXISTS");
        }
        assert_market_symbol_not_prohibited_oracle(&market_id)
            .unwrap_or_else(|e| env::panic_str(&e));

        match &oracle_source {
            OracleSourceConfig::ExternalXcc {
                oracle_contract,
                oracle_market,
            } => {
                Self::require_testnet_or_sandbox(oracle_contract, "oracle_contract");
                assert_market_symbol_not_prohibited_oracle(oracle_market)
                    .unwrap_or_else(|e| env::panic_str(&e));
            }
            OracleSourceConfig::TestnetSandboxOperator {
                oracle_operator,
                min_price_e8,
                max_price_e8,
            } => {
                Self::require_testnet_or_sandbox(oracle_operator, "oracle_operator");
                validate_sandbox_price_bounds(min_price_e8.0, max_price_e8.0)
                    .unwrap_or_else(|e| env::panic_str(&e));
            }
        }

        let risk = risk.unwrap_or_else(StoredMarketRiskParams::testnet_mvp_defaults);
        // M04.5A.1 Invariant: require explicit finite max_pnl_bps cap (`Some(cap_bps)`).
        if risk.max_pnl_bps.is_none() {
            env::panic_str("ERR_MAX_PNL_CAP_REQUIRED");
        }
        risk.to_shared()
            .validate()
            .unwrap_or_else(|e| env::panic_str(&format!("ERR_INVALID_RISK_PARAMS: {e:?}")));

        let spread = exec_spread_bps.unwrap_or(0);
        if spread >= BPS_SCALE as u32 || spread > 100 {
            env::panic_str("ERR_INVALID_EXEC_SPREAD");
        }

        let policy = oracle_policy.unwrap_or_default();
        if policy.max_execution_age_sec == 0
            || policy.emergency_close_age_sec <= policy.max_execution_age_sec
        {
            env::panic_str("ERR_INVALID_ORACLE_POLICY_AGES");
        }

        let now_sec = env::block_timestamp_ms() / 1_000;
        self.market_ids.push(market_id.clone());
        self.markets.insert(
            market_id.clone(),
            PerpsMarketState {
                market_id,
                unit: roastake_shared::UNIT_USD_PER_KG.to_string(),
                mode: StoredMarketMode::Normal,
                paused_for_insolvency: false,
                risk,
                oracle_policy: policy,
                oracle_source,
                exec_spread_bps: spread,
                long_oi_e8: 0,
                short_oi_e8: 0,
                long_size_e8: 0,
                short_size_e8: 0,
                open_positions: 0,
                cum_funding_index_e12: 0,
                last_funding_sec: now_sec,
                last_valid_price_e8: None,
                last_valid_observed_sec: None,
                sandbox_observation: None,
                attested_emergency_price_e8: None,
                frozen_settlement_price_e8: None,
                frozen_funding_index_e12: None,
                emergency_triggered_at_sec: None,
            },
        );
    }

    pub fn update_risk_params(&mut self, market_id: String, risk: StoredMarketRiskParams) {
        self.assert_admin();
        let mut market = self
            .markets
            .get(&market_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_MARKET_NOT_FOUND"));
        if market.mode == StoredMarketMode::Emergency {
            env::panic_str("ERR_MARKET_EMERGENCY_FROZEN");
        }
        if risk.max_pnl_bps.is_none() {
            env::panic_str("ERR_MAX_PNL_CAP_REQUIRED");
        }
        risk.to_shared()
            .validate()
            .unwrap_or_else(|e| env::panic_str(&format!("ERR_INVALID_RISK_PARAMS: {e:?}")));
        market.risk = risk;
        self.markets.insert(market_id, market);
    }

    pub fn set_market_mode(&mut self, market_id: String, new_mode: StoredMarketMode) {
        self.assert_admin();
        if new_mode == StoredMarketMode::Emergency {
            env::panic_str("ERR_USE_TRIGGER_EMERGENCY_SETTLEMENT");
        }
        let mut market = self
            .markets
            .get(&market_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_MARKET_NOT_FOUND"));
        if market.mode == StoredMarketMode::Emergency {
            env::panic_str("ERR_EMERGENCY_MODE_IRREVERSIBLE");
        }
        if market.paused_for_insolvency && new_mode == StoredMarketMode::Normal {
            env::panic_str("ERR_INSOLVENCY_REQUIRES_RECAPITALIZATION");
        }

        // If admin resumes after a jump circuit-breaker on Sandbox feed, synchronize last_valid_price to current verified sandbox obs.
        if new_mode == StoredMarketMode::Normal {
            if let Some(obs) = market.sandbox_observation.clone() {
                market.last_valid_price_e8 = Some(obs.price_e8);
                market.last_valid_observed_sec = Some(obs.observed_at_sec);
            }
        }

        let prev = Self::mode_label(market.mode);
        let next = Self::mode_label(new_mode);
        market.mode = new_mode;
        self.markets.insert(market_id.clone(), market);

        let evt = if new_mode == StoredMarketMode::Normal {
            "market_resumed"
        } else {
            "market_paused"
        };
        emit_event(
            evt,
            &MarketModeChangedEvent {
                market_id: &market_id,
                triggered_by: self.admin.as_str(),
                previous_mode: prev,
                new_mode: next,
                reason: "admin_mode_transition",
                frozen_settlement_price_e8: None,
                frozen_funding_index_e12: None,
            },
        );
    }

    /// Governance option BEFORE emergency activation: strictly forbidden once frozen in Emergency.
    pub fn set_emergency_recovery_price(&mut self, market_id: String, recovery_price_e8: U128) {
        self.assert_admin();
        if recovery_price_e8.0 == 0 {
            env::panic_str("ERR_ZERO_PRICE");
        }
        let mut market = self
            .markets
            .get(&market_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_MARKET_NOT_FOUND"));
        if market.mode == StoredMarketMode::Emergency || market.frozen_settlement_price_e8.is_some()
        {
            env::panic_str("ERR_EMERGENCY_PRICE_FROZEN");
        }
        market.attested_emergency_price_e8 = Some(recovery_price_e8.0);
        self.markets.insert(market_id, market);
    }

    pub fn push_sandbox_oracle_price(
        &mut self,
        market_id: String,
        price_e8: U128,
        observed_at_sec: u64,
        sequence: u64,
    ) {
        let caller = env::predecessor_account_id();
        let now_sec = env::block_timestamp_ms() / 1_000;
        let mut market = self
            .markets
            .get(&market_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_MARKET_NOT_FOUND"));

        if market.mode == StoredMarketMode::Emergency {
            env::panic_str("ERR_EMERGENCY_PRICE_FROZEN");
        }

        let (operator, min_p, max_p) = match &market.oracle_source {
            OracleSourceConfig::TestnetSandboxOperator {
                oracle_operator,
                min_price_e8,
                max_price_e8,
            } => (oracle_operator.clone(), min_price_e8.0, max_price_e8.0),
            OracleSourceConfig::ExternalXcc { .. } => {
                env::panic_str("ERR_MARKET_USES_EXTERNAL_XCC_ORACLE")
            }
        };

        if caller != operator {
            env::panic_str("ERR_UNAUTHORIZED_ORACLE_OPERATOR");
        }
        if price_e8.0 < min_p || price_e8.0 > max_p {
            env::panic_str("ERR_ORACLE_PRICE_OUT_OF_BOUNDS");
        }
        if observed_at_sec == 0 || observed_at_sec > now_sec.saturating_add(MAX_CLOCK_DRIFT_SEC) {
            env::panic_str("ERR_ORACLE_TIMESTAMP_INVALID");
        }
        if let Some(prev) = &market.sandbox_observation {
            if sequence <= prev.sequence {
                env::panic_str("ERR_ORACLE_NON_MONOTONIC_SEQUENCE");
            }
            if observed_at_sec < prev.observed_at_sec {
                env::panic_str("ERR_ORACLE_REGRESSIVE_TIMESTAMP");
            }
        } else if sequence == 0 {
            env::panic_str("ERR_ORACLE_NON_MONOTONIC_SEQUENCE");
        }

        market.sandbox_observation = Some(TestnetSandboxObservation {
            price_e8: price_e8.0,
            observed_at_sec,
            recorded_at_sec: now_sec,
            sequence,
            data_class: DATA_CLASS_TESTNET_SANDBOX.to_string(),
            worker_mode: MODE_TESTNET_SANDBOX.to_string(),
            publisher: caller.clone(),
        });

        let prev_mode = market.mode;
        let eval = evaluate_candidate_price_against_policy(
            &mut market,
            price_e8.0,
            observed_at_sec,
            DATA_CLASS_TESTNET_SANDBOX,
            MODE_TESTNET_SANDBOX,
            now_sec,
        );
        let new_mode = market.mode;
        self.markets.insert(market_id.clone(), market);

        emit_event(
            "oracle_sandbox_tick",
            &OracleSandboxTickEvent {
                market_id: &market_id,
                publisher: caller.as_str(),
                sequence,
                price_e8: price_e8.0.to_string(),
                observed_at_sec,
                data_class: DATA_CLASS_TESTNET_SANDBOX,
            },
        );

        if prev_mode != new_mode {
            emit_event(
                "market_paused",
                &MarketModeChangedEvent {
                    market_id: &market_id,
                    triggered_by: caller.as_str(),
                    previous_mode: Self::mode_label(prev_mode),
                    new_mode: Self::mode_label(new_mode),
                    reason: match eval {
                        EvaluatedPriceResult::TriggerReduceOnly { code } => {
                            if code.contains("JUMP") {
                                "jump_circuit_breaker"
                            } else {
                                "stale_reduce_only"
                            }
                        }
                        _ => "oracle_policy",
                    },
                    frozen_settlement_price_e8: None,
                    frozen_funding_index_e12: None,
                },
            );
        }
    }

    // -------------------------------------------------------------------------
    // Wallet Registration & NEP-145 Storage Flow
    // -------------------------------------------------------------------------

    #[payable]
    pub fn register_account(&mut self, account_id: Option<AccountId>) -> StorageBalanceView {
        self.storage_deposit(account_id, Some(true))
    }

    #[payable]
    pub fn storage_deposit(
        &mut self,
        account_id: Option<AccountId>,
        _registration_only: Option<bool>,
    ) -> StorageBalanceView {
        let target = account_id.unwrap_or_else(env::predecessor_account_id);
        Self::require_testnet_or_sandbox(&target, "registered_account");
        let attached = env::attached_deposit().as_yoctonear();

        if let Some(existing) = self.accounts.get(&target).cloned() {
            if existing.registered {
                if attached > 0 {
                    Promise::new(env::predecessor_account_id())
                        .transfer(NearToken::from_yoctonear(attached))
                        .detach();
                }
                return StorageBalanceView {
                    total: U128(existing.storage_paid_yocto),
                    available: U128(0),
                };
            }
        }

        if attached < ACCOUNT_STORAGE_DEPOSIT_YOCTO {
            env::panic_str("ERR_INSUFFICIENT_STORAGE_DEPOSIT");
        }

        let refund = attached - ACCOUNT_STORAGE_DEPOSIT_YOCTO;
        if refund > 0 {
            Promise::new(env::predecessor_account_id())
                .transfer(NearToken::from_yoctonear(refund))
                .detach();
        }

        let rec = AccountRecord {
            registered: true,
            storage_paid_yocto: ACCOUNT_STORAGE_DEPOSIT_YOCTO,
            free_collateral_e8: 0,
            lp_shares: 0,
            pending_action_lock: false,
            has_pending_withdrawal: false,
        };
        self.accounts.insert(target, rec);

        StorageBalanceView {
            total: U128(ACCOUNT_STORAGE_DEPOSIT_YOCTO),
            available: U128(0),
        }
    }

    // -------------------------------------------------------------------------
    // NEP-141 Inbound Deposits & Outbound Async Withdrawals
    // -------------------------------------------------------------------------

    pub fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        let token_id = env::predecessor_account_id();
        let token_cfg = self
            .collateral_tokens
            .get(&token_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_INVALID_COLLATERAL_TOKEN"));
        if !token_cfg.active {
            env::panic_str("ERR_COLLATERAL_TOKEN_INACTIVE");
        }
        if amount.0 == 0 {
            env::panic_str("ERR_INVALID_DEPOSIT_AMOUNT");
        }
        let amount_e8 = token_cfg
            .checked_usd_e8(amount.0)
            .unwrap_or_else(|e| env::panic_str(&e));

        let memo: DepositMemo = serde_json::from_str(&msg)
            .unwrap_or_else(|_| env::panic_str("ERR_INVALID_DEPOSIT_MEMO"));

        match memo {
            DepositMemo::TraderCollateral => {
                if self.paused_global {
                    env::panic_str("ERR_PROTOCOL_PAUSED");
                }
                self.internal_credit_free_collateral_deposit(
                    &sender_id, &token_id, amount.0, amount_e8,
                )
            }
            DepositMemo::InsuranceFund => {
                self.internal_credit_insurance_deposit(&token_id, amount.0, amount_e8)
            }
            DepositMemo::TreasuryTopUp => {
                self.internal_credit_treasury_deposit(&token_id, amount.0, amount_e8)
            }
            DepositMemo::LpDeposit { min_shares } => {
                if self.paused_global {
                    env::panic_str("ERR_PROTOCOL_PAUSED");
                }
                validate_initial_lp_floor(amount_e8).unwrap_or_else(|e| env::panic_str(&e));

                let mut acc = self
                    .accounts
                    .get(&sender_id)
                    .cloned()
                    .unwrap_or_else(|| env::panic_str("ERR_ACCOUNT_NOT_REGISTERED"));
                if !acc.registered {
                    env::panic_str("ERR_ACCOUNT_NOT_REGISTERED");
                }
                if acc.pending_action_lock {
                    env::panic_str("ERR_ACCOUNT_LOCKED");
                }

                let now_sec = env::block_timestamp_ms() / 1_000;
                if let Some((xcc_m_id, oracle_contract, oracle_market)) =
                    self.find_open_external_xcc_market()
                {
                    acc.pending_action_lock = true;
                    self.accounts.insert(sender_id.clone(), acc);

                    let action_id =
                        self.push_pending_oracle_action(PendingOracleActionKind::DepositLp {
                            lp_account: sender_id,
                            market_id: xcc_m_id,
                            token_id,
                            raw_amount_units: amount.0,
                            deposit_e8: amount_e8,
                            min_shares: min_shares.map(|v| v.0).unwrap_or(1),
                        });
                    let promise = ext_oracle::ext(oracle_contract)
                        .with_static_gas(ORACLE_XCC_VIEW_GAS)
                        .get_price(oracle_market)
                        .then(
                            Self::ext(env::current_account_id())
                                .with_static_gas(ORACLE_XCC_CALLBACK_GAS)
                                .on_oracle_lp_deposit(action_id),
                        );
                    PromiseOrValue::Promise(promise)
                } else {
                    self.internal_execute_lp_deposit(LpDepositExecParams {
                        lp_account: &sender_id,
                        token_id: &token_id,
                        raw_units: amount.0,
                        deposit_e8: amount_e8,
                        min_shares: min_shares.map(|v| v.0).unwrap_or(1),
                        override_price: None,
                        now_sec,
                    })
                    .unwrap_or_else(|e| env::panic_str(&e));
                    PromiseOrValue::Value(U128(0))
                }
            }
        }
    }

    pub fn withdraw_collateral(
        &mut self,
        token_id: AccountId,
        amount_token_units: Option<U128>,
    ) -> Promise {
        if self.paused_global {
            env::panic_str("ERR_PROTOCOL_PAUSED");
        }
        let caller = env::predecessor_account_id();
        let token_cfg = self
            .collateral_tokens
            .get(&token_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_INVALID_COLLATERAL_TOKEN"));
        if !token_cfg.active {
            env::panic_str("ERR_COLLATERAL_TOKEN_INACTIVE");
        }

        let mut acc = self
            .accounts
            .get(&caller)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_ACCOUNT_NOT_REGISTERED"));
        if !acc.registered {
            env::panic_str("ERR_ACCOUNT_NOT_REGISTERED");
        }
        if acc.pending_action_lock {
            env::panic_str("ERR_ACCOUNT_LOCKED");
        }
        if acc.has_pending_withdrawal {
            env::panic_str("ERR_WITHDRAWAL_IN_PROGRESS");
        }

        let max_whole_units = acc.free_collateral_e8 / token_cfg.scale_to_usd_e8;
        let units = match amount_token_units {
            Some(u) => u.0,
            None => max_whole_units,
        };
        if units == 0 {
            env::panic_str("ERR_WITHDRAWAL_BELOW_MIN_TOKEN_UNIT");
        }
        let usd_e8 = token_cfg
            .checked_usd_e8(units)
            .unwrap_or_else(|e| env::panic_str(&e));
        if acc.free_collateral_e8 < usd_e8 {
            env::panic_str("ERR_INSUFFICIENT_FREE_COLLATERAL");
        }
        if token_cfg.tracked_balance_units < units {
            env::panic_str("ERR_INSUFFICIENT_TOKEN_LIQUIDITY");
        }

        // Deduct eagerly and preserve any sub-micro-USDC residue (`acc.free_collateral_e8 % 100`) intact.
        acc.free_collateral_e8 -= usd_e8;
        acc.has_pending_withdrawal = true;
        self.accounts.insert(caller.clone(), acc);

        self.buckets.total_trader_free_collateral_e8 -= usd_e8;
        self.buckets.total_in_flight_withdrawals_e8 += usd_e8;

        self.internal_dispatch_withdrawal_kind(
            WithdrawalKind::TraderFreeCollateral,
            caller,
            token_id,
            units,
            usd_e8,
        )
    }

    pub fn withdraw_lp(
        &mut self,
        token_id: AccountId,
        shares_to_burn: U128,
        min_assets_out_e8: Option<U128>,
    ) -> PromiseOrValue<()> {
        if self.paused_global {
            env::panic_str("ERR_PROTOCOL_PAUSED");
        }
        let caller = env::predecessor_account_id();
        let min_out = min_assets_out_e8.map(|v| v.0).unwrap_or(1);

        let mut acc = self
            .accounts
            .get(&caller)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_ACCOUNT_NOT_REGISTERED"));
        if !acc.registered {
            env::panic_str("ERR_ACCOUNT_NOT_REGISTERED");
        }
        if acc.pending_action_lock {
            env::panic_str("ERR_ACCOUNT_LOCKED");
        }
        if acc.has_pending_withdrawal {
            env::panic_str("ERR_WITHDRAWAL_IN_PROGRESS");
        }
        if shares_to_burn.0 == 0 || acc.lp_shares < shares_to_burn.0 {
            env::panic_str("ERR_INSUFFICIENT_LP_SHARES");
        }

        if let Some((xcc_m_id, oracle_contract, oracle_market)) =
            self.find_open_external_xcc_market()
        {
            acc.pending_action_lock = true;
            self.accounts.insert(caller.clone(), acc);
            let action_id = self.push_pending_oracle_action(PendingOracleActionKind::WithdrawLp {
                lp_account: caller,
                market_id: xcc_m_id,
                token_id,
                shares_to_burn: shares_to_burn.0,
                min_assets_out_e8: min_out,
            });
            let p = ext_oracle::ext(oracle_contract)
                .with_static_gas(ORACLE_XCC_VIEW_GAS)
                .get_price(oracle_market)
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(ORACLE_XCC_CALLBACK_GAS)
                        .on_oracle_action(action_id),
                );
            return PromiseOrValue::Promise(p);
        }

        let now_sec = env::block_timestamp_ms() / 1_000;
        let outcome = self
            .internal_execute_lp_withdraw(
                &caller,
                &token_id,
                shares_to_burn.0,
                min_out,
                None,
                now_sec,
            )
            .unwrap_or_else(|e| env::panic_str(&e));

        self.emit_lp_withdrawn_and_dispatch(caller, token_id, shares_to_burn.0, outcome)
    }

    pub fn claim_protocol_fees(
        &mut self,
        token_id: AccountId,
        amount_token_units: Option<U128>,
    ) -> Promise {
        let caller = env::predecessor_account_id();
        if caller != self.admin && caller != self.treasury {
            env::panic_str("ERR_UNAUTHORIZED");
        }
        if self.treasury_withdrawal_pending {
            env::panic_str("ERR_WITHDRAWAL_IN_PROGRESS");
        }
        let token_cfg = self
            .collateral_tokens
            .get(&token_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_INVALID_COLLATERAL_TOKEN"));
        if !token_cfg.active {
            env::panic_str("ERR_COLLATERAL_TOKEN_INACTIVE");
        }

        let max_units = self.buckets.protocol_fees_e8 / token_cfg.scale_to_usd_e8;
        let units = amount_token_units.map(|v| v.0).unwrap_or(max_units);
        if units == 0 {
            env::panic_str("ERR_WITHDRAWAL_BELOW_MIN_TOKEN_UNIT");
        }
        let usd_e8 = token_cfg
            .checked_usd_e8(units)
            .unwrap_or_else(|e| env::panic_str(&e));
        if self.buckets.protocol_fees_e8 < usd_e8 {
            env::panic_str("ERR_INSUFFICIENT_PROTOCOL_FEES");
        }
        if token_cfg.tracked_balance_units < units {
            env::panic_str("ERR_INSUFFICIENT_TOKEN_LIQUIDITY");
        }

        self.treasury_withdrawal_pending = true;
        self.buckets.protocol_fees_e8 -= usd_e8;
        self.buckets.total_in_flight_withdrawals_e8 += usd_e8;

        self.internal_dispatch_withdrawal_kind(
            WithdrawalKind::ProtocolTreasury,
            self.treasury.clone(),
            token_id,
            units,
            usd_e8,
        )
    }

    #[private]
    pub fn on_withdraw_transfer(&mut self, withdrawal_id: u64) {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "Method on_withdraw_transfer is private"
        );
        let succeeded = env::promise_result_checked(0, 1024).is_ok();
        self.internal_handle_withdraw_callback(withdrawal_id, succeeded);
    }

    // -------------------------------------------------------------------------
    // Position Lifecycle, Funding & Liquidation
    // -------------------------------------------------------------------------

    pub fn open_position(
        &mut self,
        market_id: String,
        side: StoredPositionSide,
        size_e8: U128,
        collateral_e8: U128,
        price_limit_e8: Option<U128>,
    ) -> PromiseOrValue<Option<u64>> {
        if self.paused_global {
            env::panic_str("ERR_PROTOCOL_PAUSED");
        }
        let trader = env::predecessor_account_id();
        let mut acc = self
            .accounts
            .get(&trader)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_ACCOUNT_NOT_REGISTERED"));
        if !acc.registered {
            env::panic_str("ERR_ACCOUNT_NOT_REGISTERED");
        }
        if acc.pending_action_lock {
            env::panic_str("ERR_ACCOUNT_LOCKED");
        }

        let mut market = self
            .markets
            .get(&market_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_MARKET_NOT_FOUND"));
        if market.mode != StoredMarketMode::Normal || market.paused_for_insolvency {
            env::panic_str("ERR_MARKET_NOT_NORMAL");
        }

        let now_sec = env::block_timestamp_ms() / 1_000;
        match market.oracle_source.clone() {
            OracleSourceConfig::TestnetSandboxOperator { .. } => {
                let prev_mode = market.mode;
                let price_e8 = match evaluate_sandbox_feed_for_execution(&mut market, now_sec) {
                    Ok(p) => p,
                    Err(e) => {
                        self.markets.insert(market_id.clone(), market);
                        if prev_mode == StoredMarketMode::Normal {
                            emit_event(
                                "market_paused",
                                &MarketModeChangedEvent {
                                    market_id: &market_id,
                                    triggered_by: trader.as_str(),
                                    previous_mode: "Normal",
                                    new_mode: "ReduceOnly",
                                    reason: &e,
                                    frozen_settlement_price_e8: None,
                                    frozen_funding_index_e12: None,
                                },
                            );
                        }
                        env::panic_str(&e);
                    }
                };
                self.markets.insert(market_id.clone(), market);
                let pos_id = self
                    .internal_open_position(OpenPositionParams {
                        trader: &trader,
                        market_id: &market_id,
                        side,
                        size_e8: size_e8.0,
                        collateral_e8: collateral_e8.0,
                        price_limit_e8: price_limit_e8.map(|v| v.0),
                        index_price_e8: price_e8,
                        now_sec,
                    })
                    .unwrap_or_else(|e| env::panic_str(&e));
                PromiseOrValue::Value(Some(pos_id))
            }
            OracleSourceConfig::ExternalXcc {
                oracle_contract,
                oracle_market,
            } => {
                acc.pending_action_lock = true;
                self.accounts.insert(trader.clone(), acc);
                let action_id =
                    self.push_pending_oracle_action(PendingOracleActionKind::OpenPosition {
                        trader,
                        market_id,
                        side,
                        size_e8: size_e8.0,
                        collateral_e8: collateral_e8.0,
                        price_limit_e8: price_limit_e8.map(|v| v.0),
                    });
                let p = ext_oracle::ext(oracle_contract)
                    .with_static_gas(ORACLE_XCC_VIEW_GAS)
                    .get_price(oracle_market)
                    .then(
                        Self::ext(env::current_account_id())
                            .with_static_gas(ORACLE_XCC_CALLBACK_GAS)
                            .on_oracle_action(action_id),
                    );
                PromiseOrValue::Promise(p)
            }
        }
    }

    pub fn increase_position(
        &mut self,
        position_id: u64,
        add_size_e8: U128,
        add_collateral_e8: U128,
        price_limit_e8: Option<U128>,
    ) -> PromiseOrValue<()> {
        if self.paused_global {
            env::panic_str("ERR_PROTOCOL_PAUSED");
        }
        let trader = env::predecessor_account_id();
        let mut acc = self
            .accounts
            .get(&trader)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_ACCOUNT_NOT_REGISTERED"));
        if acc.pending_action_lock {
            env::panic_str("ERR_ACCOUNT_LOCKED");
        }
        let mut pos = self
            .positions
            .get(&position_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_POSITION_NOT_FOUND"));
        if pos.owner != trader {
            env::panic_str("ERR_UNAUTHORIZED");
        }
        if pos.locked {
            env::panic_str("ERR_POSITION_LOCKED");
        }
        let mut market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_MARKET_NOT_FOUND"));
        if market.mode != StoredMarketMode::Normal || market.paused_for_insolvency {
            env::panic_str("ERR_MARKET_NOT_NORMAL");
        }

        let now_sec = env::block_timestamp_ms() / 1_000;
        match market.oracle_source.clone() {
            OracleSourceConfig::TestnetSandboxOperator { .. } => {
                let price_e8 = evaluate_sandbox_feed_for_execution(&mut market, now_sec)
                    .unwrap_or_else(|e| env::panic_str(&e));
                self.markets.insert(pos.market_id.clone(), market);
                self.internal_increase_position(IncreasePositionParams {
                    trader: &trader,
                    position_id,
                    add_size_e8: add_size_e8.0,
                    add_collateral_e8: add_collateral_e8.0,
                    price_limit_e8: price_limit_e8.map(|v| v.0),
                    index_price_e8: price_e8,
                    now_sec,
                })
                .unwrap_or_else(|e| env::panic_str(&e));
                PromiseOrValue::Value(())
            }
            OracleSourceConfig::ExternalXcc {
                oracle_contract,
                oracle_market,
            } => {
                acc.pending_action_lock = true;
                pos.locked = true;
                self.accounts.insert(trader.clone(), acc);
                self.positions.insert(position_id, pos);
                let action_id =
                    self.push_pending_oracle_action(PendingOracleActionKind::IncreasePosition {
                        trader,
                        position_id,
                        add_size_e8: add_size_e8.0,
                        add_collateral_e8: add_collateral_e8.0,
                        price_limit_e8: price_limit_e8.map(|v| v.0),
                    });
                let p = ext_oracle::ext(oracle_contract)
                    .with_static_gas(ORACLE_XCC_VIEW_GAS)
                    .get_price(oracle_market)
                    .then(
                        Self::ext(env::current_account_id())
                            .with_static_gas(ORACLE_XCC_CALLBACK_GAS)
                            .on_oracle_action(action_id),
                    );
                PromiseOrValue::Promise(p)
            }
        }
    }

    pub fn reduce_position(
        &mut self,
        position_id: u64,
        close_size_e8: U128,
        price_limit_e8: Option<U128>,
    ) -> PromiseOrValue<()> {
        self.dispatch_reduce_or_close(position_id, Some(close_size_e8.0), price_limit_e8)
    }

    pub fn close_position(
        &mut self,
        position_id: u64,
        price_limit_e8: Option<U128>,
    ) -> PromiseOrValue<()> {
        self.dispatch_reduce_or_close(position_id, None, price_limit_e8)
    }

    fn dispatch_reduce_or_close(
        &mut self,
        position_id: u64,
        requested_close_size_e8: Option<u128>,
        price_limit_e8: Option<U128>,
    ) -> PromiseOrValue<()> {
        if self.paused_global {
            env::panic_str("ERR_PROTOCOL_PAUSED");
        }
        let trader = env::predecessor_account_id();
        let mut acc = self
            .accounts
            .get(&trader)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_ACCOUNT_NOT_REGISTERED"));
        if acc.pending_action_lock {
            env::panic_str("ERR_ACCOUNT_LOCKED");
        }
        let mut pos = self
            .positions
            .get(&position_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_POSITION_NOT_FOUND"));
        if pos.owner != trader {
            env::panic_str("ERR_UNAUTHORIZED");
        }
        if pos.locked {
            env::panic_str("ERR_POSITION_LOCKED");
        }

        let mut market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_MARKET_NOT_FOUND"));
        if !matches!(
            market.mode,
            StoredMarketMode::Normal | StoredMarketMode::ReduceOnly
        ) || market.paused_for_insolvency
        {
            env::panic_str("ERR_MARKET_CANNOT_NORMAL_CLOSE");
        }

        let now_sec = env::block_timestamp_ms() / 1_000;
        let resolved_close_size = requested_close_size_e8.unwrap_or(pos.size_e8);
        match market.oracle_source.clone() {
            OracleSourceConfig::TestnetSandboxOperator { .. } => {
                let price_e8 = evaluate_sandbox_feed_for_execution(&mut market, now_sec)
                    .unwrap_or_else(|e| env::panic_str(&e));
                self.markets.insert(pos.market_id.clone(), market);
                self.internal_reduce_or_close_position(
                    &trader,
                    position_id,
                    Some(resolved_close_size),
                    price_limit_e8.map(|v| v.0),
                    price_e8,
                    now_sec,
                )
                .unwrap_or_else(|e| env::panic_str(&e));
                PromiseOrValue::Value(())
            }
            OracleSourceConfig::ExternalXcc {
                oracle_contract,
                oracle_market,
            } => {
                acc.pending_action_lock = true;
                pos.locked = true;
                self.accounts.insert(trader.clone(), acc);
                self.positions.insert(position_id, pos);
                let action_id =
                    self.push_pending_oracle_action(PendingOracleActionKind::ReducePosition {
                        trader,
                        position_id,
                        close_size_e8: resolved_close_size,
                        price_limit_e8: price_limit_e8.map(|v| v.0),
                    });
                let p = ext_oracle::ext(oracle_contract)
                    .with_static_gas(ORACLE_XCC_VIEW_GAS)
                    .get_price(oracle_market)
                    .then(
                        Self::ext(env::current_account_id())
                            .with_static_gas(ORACLE_XCC_CALLBACK_GAS)
                            .on_oracle_action(action_id),
                    );
                PromiseOrValue::Promise(p)
            }
        }
    }

    pub fn add_margin(&mut self, position_id: u64, amount_e8: U128) {
        if self.paused_global {
            env::panic_str("ERR_PROTOCOL_PAUSED");
        }
        if amount_e8.0 == 0 {
            env::panic_str("ERR_INVALID_AMOUNT");
        }
        let trader = env::predecessor_account_id();
        let mut acc = self
            .accounts
            .get(&trader)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_ACCOUNT_NOT_REGISTERED"));
        if acc.pending_action_lock {
            env::panic_str("ERR_ACCOUNT_LOCKED");
        }
        if acc.free_collateral_e8 < amount_e8.0 {
            env::panic_str("ERR_INSUFFICIENT_FREE_COLLATERAL");
        }
        let mut pos = self
            .positions
            .get(&position_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_POSITION_NOT_FOUND"));
        if pos.owner != trader {
            env::panic_str("ERR_UNAUTHORIZED");
        }
        if pos.locked {
            env::panic_str("ERR_POSITION_LOCKED");
        }
        let market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_MARKET_NOT_FOUND"));
        if market.mode == StoredMarketMode::Emergency {
            env::panic_str("ERR_MARKET_EMERGENCY_FROZEN");
        }

        acc.free_collateral_e8 -= amount_e8.0;
        pos.locked_margin_e8 = pos
            .locked_margin_e8
            .checked_add(amount_e8.0)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        let new_locked = pos.locked_margin_e8;
        self.accounts.insert(trader.clone(), acc);
        self.positions.insert(position_id, pos);

        self.buckets.total_trader_free_collateral_e8 -= amount_e8.0;
        self.buckets.total_trader_locked_margin_e8 += amount_e8.0;
        self.assert_solvency();

        emit_event(
            "margin_added",
            &MarginAdjustedEvent {
                position_id,
                owner: trader.as_str(),
                delta_margin_e8: amount_e8.0.to_string(),
                new_locked_margin_e8: new_locked.to_string(),
                mark_price_e8: None,
            },
        );
    }

    pub fn remove_margin(&mut self, position_id: u64, amount_e8: U128) -> PromiseOrValue<()> {
        if self.paused_global {
            env::panic_str("ERR_PROTOCOL_PAUSED");
        }
        let trader = env::predecessor_account_id();
        let mut acc = self
            .accounts
            .get(&trader)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_ACCOUNT_NOT_REGISTERED"));
        if acc.pending_action_lock {
            env::panic_str("ERR_ACCOUNT_LOCKED");
        }
        let mut pos = self
            .positions
            .get(&position_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_POSITION_NOT_FOUND"));
        if pos.owner != trader {
            env::panic_str("ERR_UNAUTHORIZED");
        }
        if pos.locked {
            env::panic_str("ERR_POSITION_LOCKED");
        }
        let mut market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_MARKET_NOT_FOUND"));
        if market.mode != StoredMarketMode::Normal || market.paused_for_insolvency {
            env::panic_str("ERR_MARKET_NOT_NORMAL");
        }

        let now_sec = env::block_timestamp_ms() / 1_000;
        match market.oracle_source.clone() {
            OracleSourceConfig::TestnetSandboxOperator { .. } => {
                let price_e8 = evaluate_sandbox_feed_for_execution(&mut market, now_sec)
                    .unwrap_or_else(|e| env::panic_str(&e));
                self.markets.insert(pos.market_id.clone(), market);
                self.internal_remove_margin(&trader, position_id, amount_e8.0, price_e8, now_sec)
                    .unwrap_or_else(|e| env::panic_str(&e));
                PromiseOrValue::Value(())
            }
            OracleSourceConfig::ExternalXcc {
                oracle_contract,
                oracle_market,
            } => {
                acc.pending_action_lock = true;
                pos.locked = true;
                self.accounts.insert(trader.clone(), acc);
                self.positions.insert(position_id, pos);
                let action_id =
                    self.push_pending_oracle_action(PendingOracleActionKind::RemoveMargin {
                        trader,
                        position_id,
                        amount_e8: amount_e8.0,
                    });
                let p = ext_oracle::ext(oracle_contract)
                    .with_static_gas(ORACLE_XCC_VIEW_GAS)
                    .get_price(oracle_market)
                    .then(
                        Self::ext(env::current_account_id())
                            .with_static_gas(ORACLE_XCC_CALLBACK_GAS)
                            .on_oracle_action(action_id),
                    );
                PromiseOrValue::Promise(p)
            }
        }
    }

    pub fn accrue_funding(&mut self, market_id: String) -> PromiseOrValue<()> {
        let mut market = self
            .markets
            .get(&market_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_MARKET_NOT_FOUND"));
        if market.mode == StoredMarketMode::Emergency {
            env::panic_str("ERR_MARKET_EMERGENCY_FROZEN");
        }
        let now_sec = env::block_timestamp_ms() / 1_000;
        match market.oracle_source.clone() {
            OracleSourceConfig::TestnetSandboxOperator { .. } => {
                let price_e8 = evaluate_sandbox_feed_for_execution(&mut market, now_sec)
                    .unwrap_or_else(|e| env::panic_str(&e));
                self.markets.insert(market_id.clone(), market);
                self.internal_accrue_funding_at_price(&market_id, price_e8, now_sec)
                    .unwrap_or_else(|e| env::panic_str(&e));
                PromiseOrValue::Value(())
            }
            OracleSourceConfig::ExternalXcc {
                oracle_contract,
                oracle_market,
            } => {
                let action_id =
                    self.push_pending_oracle_action(PendingOracleActionKind::AccrueFunding {
                        market_id,
                    });
                let p = ext_oracle::ext(oracle_contract)
                    .with_static_gas(ORACLE_XCC_VIEW_GAS)
                    .get_price(oracle_market)
                    .then(
                        Self::ext(env::current_account_id())
                            .with_static_gas(ORACLE_XCC_CALLBACK_GAS)
                            .on_oracle_action(action_id),
                    );
                PromiseOrValue::Promise(p)
            }
        }
    }

    pub fn liquidate_position(&mut self, position_id: u64) -> PromiseOrValue<()> {
        if self.paused_global {
            env::panic_str("ERR_PROTOCOL_PAUSED");
        }
        let keeper = env::predecessor_account_id();
        if !self
            .accounts
            .get(&keeper)
            .map(|a| a.registered)
            .unwrap_or(false)
        {
            env::panic_str("ERR_KEEPER_NOT_REGISTERED");
        }
        let mut pos = self
            .positions
            .get(&position_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_POSITION_NOT_FOUND"));
        if pos.locked {
            env::panic_str("ERR_POSITION_LOCKED");
        }
        let mut market = self
            .markets
            .get(&pos.market_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_MARKET_NOT_FOUND"));
        if market.mode == StoredMarketMode::Emergency {
            env::panic_str("ERR_USE_EMERGENCY_SETTLEMENT");
        }

        let now_sec = env::block_timestamp_ms() / 1_000;
        match market.oracle_source.clone() {
            OracleSourceConfig::TestnetSandboxOperator { .. } => {
                let price_e8 = evaluate_sandbox_feed_for_execution(&mut market, now_sec)
                    .unwrap_or_else(|e| env::panic_str(&e));
                self.markets.insert(pos.market_id.clone(), market);
                self.internal_liquidate_position(&keeper, position_id, price_e8, now_sec)
                    .unwrap_or_else(|e| env::panic_str(&e));
                PromiseOrValue::Value(())
            }
            OracleSourceConfig::ExternalXcc {
                oracle_contract,
                oracle_market,
            } => {
                pos.locked = true;
                self.positions.insert(position_id, pos);
                let action_id =
                    self.push_pending_oracle_action(PendingOracleActionKind::LiquidatePosition {
                        keeper,
                        position_id,
                    });
                let p = ext_oracle::ext(oracle_contract)
                    .with_static_gas(ORACLE_XCC_VIEW_GAS)
                    .get_price(oracle_market)
                    .then(
                        Self::ext(env::current_account_id())
                            .with_static_gas(ORACLE_XCC_CALLBACK_GAS)
                            .on_oracle_action(action_id),
                    );
                PromiseOrValue::Promise(p)
            }
        }
    }

    pub fn trigger_emergency_settlement(&mut self, market_id: String) {
        let caller = env::predecessor_account_id();
        let now_sec = env::block_timestamp_ms() / 1_000;
        self.internal_trigger_emergency_settlement(&caller, &market_id, now_sec)
            .unwrap_or_else(|e| env::panic_str(&e));
    }

    pub fn claim_emergency_settlement(&mut self, position_id: u64) {
        self.internal_claim_emergency_settlement(position_id)
            .unwrap_or_else(|e| env::panic_str(&e));
    }

    // -------------------------------------------------------------------------
    // Private Non-Panicking XCC Oracle Callbacks
    // -------------------------------------------------------------------------

    #[private]
    pub fn on_oracle_lp_deposit(&mut self, action_id: u64) -> U128 {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "Method on_oracle_lp_deposit is private"
        );
        let Some(action) = self.pending_oracle_actions.remove(&action_id) else {
            emit_event(
                "security_anomaly",
                &SecurityAnomalyEvent {
                    kind: "duplicate_or_unknown_oracle_lp_deposit_callback",
                    actor: env::predecessor_account_id().as_str(),
                    detail: &format!("action_id={action_id}"),
                },
            );
            return U128(0);
        };

        let PendingOracleActionKind::DepositLp {
            lp_account,
            market_id,
            token_id,
            raw_amount_units,
            deposit_e8,
            min_shares,
        } = action.kind
        else {
            env::log_str("ERR_CALLBACK_KIND_MISMATCH");
            return U128(0);
        };

        // Always release account lock cleanly.
        if let Some(mut acc) = self.accounts.get(&lp_account).cloned() {
            acc.pending_action_lock = false;
            self.accounts.insert(lp_account.clone(), acc);
        }

        let now_sec = env::block_timestamp_ms() / 1_000;
        let Ok(price_e8) = self.resolve_xcc_callback_price(&market_id, now_sec) else {
            return U128(raw_amount_units);
        };

        match self.internal_execute_lp_deposit(LpDepositExecParams {
            lp_account: &lp_account,
            token_id: &token_id,
            raw_units: raw_amount_units,
            deposit_e8,
            min_shares,
            override_price: Some((&market_id, price_e8)),
            now_sec,
        }) {
            Ok(()) => U128(0),
            Err(err) => {
                env::log_str(&format!("ERR_ASYNC_LP_DEPOSIT_REFUNDED: {err}"));
                U128(raw_amount_units)
            }
        }
    }

    #[private]
    pub fn on_oracle_action(&mut self, action_id: u64) -> PromiseOrValue<()> {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "Method on_oracle_action is private"
        );
        let Some(action) = self.pending_oracle_actions.remove(&action_id) else {
            emit_event(
                "security_anomaly",
                &SecurityAnomalyEvent {
                    kind: "duplicate_or_unknown_oracle_callback",
                    actor: env::predecessor_account_id().as_str(),
                    detail: &format!("action_id={action_id}"),
                },
            );
            return PromiseOrValue::Value(());
        };

        let now_sec = env::block_timestamp_ms() / 1_000;

        // Release locks first cleanly before any early return; if mutation succeeds, position removal or update applies below.
        self.release_action_locks(&action.kind);

        let market_id = match &action.kind {
            PendingOracleActionKind::OpenPosition { market_id, .. }
            | PendingOracleActionKind::AccrueFunding { market_id }
            | PendingOracleActionKind::WithdrawLp { market_id, .. } => market_id.clone(),
            PendingOracleActionKind::IncreasePosition { position_id, .. }
            | PendingOracleActionKind::ReducePosition { position_id, .. }
            | PendingOracleActionKind::RemoveMargin { position_id, .. }
            | PendingOracleActionKind::LiquidatePosition { position_id, .. } => {
                let Some(p) = self.positions.get(position_id) else {
                    env::log_str("ERR_CALLBACK_POSITION_MISSING");
                    return PromiseOrValue::Value(());
                };
                p.market_id.clone()
            }
            PendingOracleActionKind::DepositLp { .. } => {
                env::log_str("ERR_CALLBACK_KIND_MISMATCH");
                return PromiseOrValue::Value(());
            }
        };

        let price_e8 = match self.resolve_xcc_callback_price(&market_id, now_sec) {
            Ok(p) => p,
            Err(err) => {
                env::log_str(&format!("ERR_ASYNC_ORACLE_REJECTED: {err}"));
                return PromiseOrValue::Value(());
            }
        };

        match action.kind {
            PendingOracleActionKind::OpenPosition {
                trader,
                market_id,
                side,
                size_e8,
                collateral_e8,
                price_limit_e8,
            } => {
                if let Err(err) = self.internal_open_position(OpenPositionParams {
                    trader: &trader,
                    market_id: &market_id,
                    side,
                    size_e8,
                    collateral_e8,
                    price_limit_e8,
                    index_price_e8: price_e8,
                    now_sec,
                }) {
                    env::log_str(&format!("ERR_ASYNC_ACTION_ABORTED: {err}"));
                }
                PromiseOrValue::Value(())
            }
            PendingOracleActionKind::IncreasePosition {
                trader,
                position_id,
                add_size_e8,
                add_collateral_e8,
                price_limit_e8,
            } => {
                if let Err(err) = self.internal_increase_position(IncreasePositionParams {
                    trader: &trader,
                    position_id,
                    add_size_e8,
                    add_collateral_e8,
                    price_limit_e8,
                    index_price_e8: price_e8,
                    now_sec,
                }) {
                    env::log_str(&format!("ERR_ASYNC_ACTION_ABORTED: {err}"));
                }
                PromiseOrValue::Value(())
            }
            PendingOracleActionKind::ReducePosition {
                trader,
                position_id,
                close_size_e8,
                price_limit_e8,
            } => {
                if let Err(err) = self.internal_reduce_or_close_position(
                    &trader,
                    position_id,
                    Some(close_size_e8),
                    price_limit_e8,
                    price_e8,
                    now_sec,
                ) {
                    env::log_str(&format!("ERR_ASYNC_ACTION_ABORTED: {err}"));
                }
                PromiseOrValue::Value(())
            }
            PendingOracleActionKind::RemoveMargin {
                trader,
                position_id,
                amount_e8,
            } => {
                if let Err(err) =
                    self.internal_remove_margin(&trader, position_id, amount_e8, price_e8, now_sec)
                {
                    env::log_str(&format!("ERR_ASYNC_ACTION_ABORTED: {err}"));
                }
                PromiseOrValue::Value(())
            }
            PendingOracleActionKind::LiquidatePosition {
                keeper,
                position_id,
            } => {
                if let Err(err) =
                    self.internal_liquidate_position(&keeper, position_id, price_e8, now_sec)
                {
                    env::log_str(&format!("ERR_ASYNC_ACTION_ABORTED: {err}"));
                }
                PromiseOrValue::Value(())
            }
            PendingOracleActionKind::AccrueFunding { market_id } => {
                if let Err(err) =
                    self.internal_accrue_funding_at_price(&market_id, price_e8, now_sec)
                {
                    env::log_str(&format!("ERR_ASYNC_ACTION_ABORTED: {err}"));
                }
                PromiseOrValue::Value(())
            }
            PendingOracleActionKind::WithdrawLp {
                lp_account,
                market_id,
                token_id,
                shares_to_burn,
                min_assets_out_e8,
            } => match self.internal_execute_lp_withdraw(
                &lp_account,
                &token_id,
                shares_to_burn,
                min_assets_out_e8,
                Some((&market_id, price_e8)),
                now_sec,
            ) {
                Ok(outcome) => self.emit_lp_withdrawn_and_dispatch(
                    lp_account,
                    token_id,
                    shares_to_burn,
                    outcome,
                ),
                Err(err) => {
                    env::log_str(&format!("ERR_ASYNC_ACTION_ABORTED: {err}"));
                    PromiseOrValue::Value(())
                }
            },
            PendingOracleActionKind::DepositLp { .. } => PromiseOrValue::Value(()),
        }
    }

    // -------------------------------------------------------------------------
    // View Methods
    // -------------------------------------------------------------------------

    pub fn get_admin(&self) -> AccountId {
        self.admin.clone()
    }

    pub fn get_pending_admin(&self) -> Option<AccountId> {
        self.pending_admin.clone()
    }

    pub fn get_treasury(&self) -> AccountId {
        self.treasury.clone()
    }

    pub fn is_paused_global(&self) -> bool {
        self.paused_global
    }

    pub fn get_collateral_token(&self, token_id: AccountId) -> Option<CollateralTokenView> {
        self.collateral_tokens
            .get(&token_id)
            .map(|c| CollateralTokenView {
                token_id: c.token_id.clone(),
                symbol: c.symbol.clone(),
                decimals: c.decimals,
                scale_to_usd_e8: U128(c.scale_to_usd_e8),
                active: c.active,
                metadata_verified_on_chain: c.metadata_verified_on_chain,
                tracked_balance_units: U128(c.tracked_balance_units),
                non_monetary_testnet_disclosure: c.non_monetary_testnet_disclosure.clone(),
            })
    }

    pub fn list_collateral_tokens(&self) -> Vec<CollateralTokenView> {
        self.collateral_token_ids
            .iter()
            .filter_map(|id| self.get_collateral_token(id.clone()))
            .collect()
    }

    pub fn storage_balance_bounds(&self) -> StorageBalanceBoundsView {
        StorageBalanceBoundsView {
            min: U128(ACCOUNT_STORAGE_DEPOSIT_YOCTO),
            max: U128(ACCOUNT_STORAGE_DEPOSIT_YOCTO),
        }
    }

    pub fn storage_balance_of(&self, account_id: AccountId) -> Option<StorageBalanceView> {
        self.accounts.get(&account_id).and_then(|acc| {
            if acc.registered {
                Some(StorageBalanceView {
                    total: U128(acc.storage_paid_yocto),
                    available: U128(0),
                })
            } else {
                None
            }
        })
    }

    pub fn is_account_registered(&self, account_id: AccountId) -> bool {
        self.accounts
            .get(&account_id)
            .map(|a| a.registered)
            .unwrap_or(false)
    }

    pub fn get_account(&self, account_id: AccountId) -> Option<AccountView> {
        let acc = self.accounts.get(&account_id)?;
        let scale = self
            .collateral_token_ids
            .first()
            .and_then(|id| self.collateral_tokens.get(id))
            .map(|c| c.scale_to_usd_e8)
            .unwrap_or(100);
        Some(AccountView {
            account_id,
            registered: acc.registered,
            storage_paid_yocto: U128(acc.storage_paid_yocto),
            free_collateral_e8: U128(acc.free_collateral_e8),
            withdrawable_token_units_at_scale_100: U128(acc.free_collateral_e8 / scale),
            sub_micro_unit_residue_e8: U128(acc.free_collateral_e8 % scale),
            lp_shares: U128(acc.lp_shares),
            pending_action_lock: acc.pending_action_lock,
            has_pending_withdrawal: acc.has_pending_withdrawal,
        })
    }

    pub fn get_market(&self, market_id: String) -> Option<MarketView> {
        let m = self.markets.get(&market_id)?;
        let (feed_classification, is_sandbox, sandbox_operator, sandbox_min_p, sandbox_max_p) =
            match &m.oracle_source {
                OracleSourceConfig::TestnetSandboxOperator {
                    oracle_operator,
                    min_price_e8,
                    max_price_e8,
                } => (
                    DATA_CLASS_TESTNET_SANDBOX.to_string(),
                    true,
                    Some(oracle_operator.clone()),
                    Some(*min_price_e8),
                    Some(*max_price_e8),
                ),
                OracleSourceConfig::ExternalXcc {
                    oracle_contract, ..
                } => (
                    format!("EXTERNAL_XCC:{oracle_contract}"),
                    m.oracle_policy.allow_testnet_sandbox,
                    None,
                    None,
                    None,
                ),
            };
        Some(MarketView {
            market_id: m.market_id.clone(),
            unit: m.unit.clone(),
            mode: m.mode,
            paused_for_insolvency: m.paused_for_insolvency,
            feed_classification,
            is_synthetic_testnet_sandbox: is_sandbox,
            sandbox_operator,
            sandbox_min_price_e8: sandbox_min_p,
            sandbox_max_price_e8: sandbox_max_p,
            sandbox_sequence: m.sandbox_observation.as_ref().map(|o| o.sequence),
            long_oi_e8: U128(m.long_oi_e8),
            short_oi_e8: U128(m.short_oi_e8),
            long_size_e8: U128(m.long_size_e8),
            short_size_e8: U128(m.short_size_e8),
            open_positions: m.open_positions,
            cum_funding_index_e12: m.cum_funding_index_e12.to_string(),
            last_funding_sec: m.last_funding_sec,
            last_valid_price_e8: m.last_valid_price_e8.map(U128),
            last_valid_observed_sec: m.last_valid_observed_sec,
            frozen_settlement_price_e8: m.frozen_settlement_price_e8.map(U128),
            frozen_funding_index_e12: m.frozen_funding_index_e12.map(|v| v.to_string()),
            risk: m.risk.clone(),
            oracle_policy: m.oracle_policy.clone(),
            exec_spread_bps: m.exec_spread_bps,
        })
    }

    pub fn list_markets(&self) -> Vec<String> {
        self.market_ids.clone()
    }

    pub fn get_position(&self, position_id: u64) -> Option<PositionRecordView> {
        self.positions
            .get(&position_id)
            .map(PositionRecordView::from)
    }

    pub fn get_positions_by_owner(&self, owner: AccountId) -> Vec<PositionRecordView> {
        self.open_position_ids
            .iter()
            .filter_map(|id| self.positions.get(id))
            .filter(|p| p.owner == owner)
            .map(PositionRecordView::from)
            .collect()
    }

    pub fn get_position_health(&self, position_id: u64) -> Option<PositionHealthView> {
        let pos = self.positions.get(&position_id)?;
        let market = self.markets.get(&pos.market_id)?;
        let ref_price = if market.mode == StoredMarketMode::Emergency {
            market.frozen_settlement_price_e8?
        } else {
            market
                .sandbox_observation
                .as_ref()
                .map(|o| o.price_e8)
                .or(market.last_valid_price_e8)?
        };
        self.internal_position_health_view(pos, market, ref_price)
            .ok()
    }

    pub fn preview_emergency_settlement(
        &self,
        position_id: u64,
    ) -> Option<UniformPositionSettlement> {
        let pos = self.positions.get(&position_id)?;
        let market = self.markets.get(&pos.market_id)?;
        let frozen_price = market
            .frozen_settlement_price_e8
            .or(market.attested_emergency_price_e8)
            .or(market.last_valid_price_e8)?;
        let frozen_funding = market
            .frozen_funding_index_e12
            .unwrap_or(market.cum_funding_index_e12);
        let snap = PositionSnapshot {
            side: pos.side.into(),
            size_e8: pos.size_e8,
            locked_margin_e8: pos.locked_margin_e8,
            entry_price_e8: pos.entry_price_e8,
            entry_funding_index_e12: pos.entry_funding_index_e12,
        };
        compute_uniform_emergency_settlement_usd_e8(
            &snap,
            frozen_price,
            frozen_funding,
            market.risk.max_pnl_bps,
        )
        .ok()
    }

    pub fn get_lp_snapshot(&self) -> LpSnapshotView {
        let emergencies_with_oi = !self.open_position_ids.is_empty()
            && self.market_ids.iter().any(|id| {
                self.markets
                    .get(id)
                    .map(|m| m.mode == StoredMarketMode::Emergency)
                    .unwrap_or(false)
            });

        let clone_self_valuation = self.internal_readonly_vault_mtm().ok();

        if let Some(val) = clone_self_valuation {
            LpSnapshotView {
                lp_vault_realized_equity_e8: U128(self.buckets.lp_vault_realized_equity_e8),
                lp_mtm_equity_e8: val.lp_mtm_equity_e8.to_string(),
                reserved_liquidity_e8: U128(val.reserved_liquidity_e8),
                lp_available_liquidity_e8: U128(val.lp_available_liquidity_e8),
                net_bounded_trader_claim_e8: val.net_bounded_claim_e8.to_string(),
                total_unrealized_bad_debt_e8: U128(val.total_unrealized_bad_debt_e8),
                lp_total_shares: U128(self.lp_total_shares),
                lp_locked_seed_shares: U128(self.lp_locked_seed_shares),
                withdrawals_locked_for_emergency: emergencies_with_oi,
            }
        } else {
            LpSnapshotView {
                lp_vault_realized_equity_e8: U128(self.buckets.lp_vault_realized_equity_e8),
                lp_mtm_equity_e8: self.buckets.lp_vault_realized_equity_e8.to_string(),
                reserved_liquidity_e8: U128(0),
                lp_available_liquidity_e8: U128(self.buckets.lp_vault_realized_equity_e8),
                net_bounded_trader_claim_e8: "0".to_string(),
                total_unrealized_bad_debt_e8: U128(0),
                lp_total_shares: U128(self.lp_total_shares),
                lp_locked_seed_shares: U128(self.lp_locked_seed_shares),
                withdrawals_locked_for_emergency: emergencies_with_oi,
            }
        }
    }

    pub fn get_accounting_snapshot(&self) -> AccountingSnapshotView {
        AccountingSnapshotView {
            total_trader_free_collateral_e8: U128(self.buckets.total_trader_free_collateral_e8),
            total_trader_locked_margin_e8: U128(self.buckets.total_trader_locked_margin_e8),
            total_in_flight_withdrawals_e8: U128(self.buckets.total_in_flight_withdrawals_e8),
            lp_vault_realized_equity_e8: U128(self.buckets.lp_vault_realized_equity_e8),
            insurance_fund_e8: U128(self.buckets.insurance_fund_e8),
            protocol_fees_e8: U128(self.buckets.protocol_fees_e8),
            tracked_collateral_e8: U128(self.tracked_collateral_e8),
            lp_total_shares: U128(self.lp_total_shares),
            total_open_positions: self.open_position_ids.len() as u64,
            solvency_ok: self.verify_solvency_invariant(),
            paused_global: self.paused_global,
        }
    }

    pub fn verify_solvency_invariant(&self) -> bool {
        if self
            .buckets
            .verify_tracked_solvency(self.tracked_collateral_e8)
            .is_err()
        {
            return false;
        }
        // Check tracked collateral matches sum across all registered collateral token units.
        let mut expected_e8 = 0u128;
        for id in &self.collateral_token_ids {
            if let Some(cfg) = self.collateral_tokens.get(id) {
                let Ok(val) = cfg.checked_usd_e8(cfg.tracked_balance_units) else {
                    return false;
                };
                let Some(next) = expected_e8.checked_add(val) else {
                    return false;
                };
                expected_e8 = next;
            }
        }
        expected_e8 == self.tracked_collateral_e8
    }

    // -------------------------------------------------------------------------
    // Internal Guards & Helpers
    // -------------------------------------------------------------------------

    fn assert_admin(&self) {
        if env::predecessor_account_id() != self.admin {
            env::panic_str("ERR_UNAUTHORIZED");
        }
    }

    fn require_testnet_or_sandbox(account_id: &AccountId, context: &str) {
        assert_testnet_or_sandbox_account(account_id, context)
            .unwrap_or_else(|e| env::panic_str(&e));
    }

    fn compute_scale_to_e8(decimals: u8) -> u128 {
        if decimals > 8 {
            env::panic_str("ERR_COLLATERAL_DECIMALS_GT_8_UNSUPPORTED");
        }
        10u128.pow((8 - decimals) as u32)
    }

    pub(crate) fn assert_solvency(&self) {
        self.buckets
            .verify_tracked_solvency(self.tracked_collateral_e8)
            .unwrap_or_else(|e| env::panic_str(&e));
        let mut expected_e8 = 0u128;
        for id in &self.collateral_token_ids {
            if let Some(cfg) = self.collateral_tokens.get(id) {
                let val = cfg
                    .checked_usd_e8(cfg.tracked_balance_units)
                    .unwrap_or_else(|e| env::panic_str(&e));
                expected_e8 = expected_e8
                    .checked_add(val)
                    .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
            }
        }
        if expected_e8 != self.tracked_collateral_e8 {
            env::panic_str("ERR_COLLATERAL_TOKEN_LEDGER_DESYNC");
        }
    }

    fn push_pending_oracle_action(&mut self, kind: PendingOracleActionKind) -> u64 {
        self.next_oracle_action_id = self
            .next_oracle_action_id
            .checked_add(1)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        let id = self.next_oracle_action_id;
        let rec = PendingOracleAction {
            id,
            initiated_at_sec: env::block_timestamp_ms() / 1_000,
            kind,
        };
        self.pending_oracle_actions.insert(id, rec);
        id
    }

    fn release_action_locks(&mut self, kind: &PendingOracleActionKind) {
        match kind {
            PendingOracleActionKind::OpenPosition { trader, .. }
            | PendingOracleActionKind::DepositLp {
                lp_account: trader, ..
            }
            | PendingOracleActionKind::WithdrawLp {
                lp_account: trader, ..
            } => {
                if let Some(mut acc) = self.accounts.get(trader).cloned() {
                    acc.pending_action_lock = false;
                    self.accounts.insert(trader.clone(), acc);
                }
            }
            PendingOracleActionKind::IncreasePosition {
                trader,
                position_id,
                ..
            }
            | PendingOracleActionKind::ReducePosition {
                trader,
                position_id,
                ..
            }
            | PendingOracleActionKind::RemoveMargin {
                trader,
                position_id,
                ..
            } => {
                if let Some(mut acc) = self.accounts.get(trader).cloned() {
                    acc.pending_action_lock = false;
                    self.accounts.insert(trader.clone(), acc);
                }
                if let Some(mut pos) = self.positions.get(position_id).cloned() {
                    pos.locked = false;
                    self.positions.insert(*position_id, pos);
                }
            }
            PendingOracleActionKind::LiquidatePosition { position_id, .. } => {
                if let Some(mut pos) = self.positions.get(position_id).cloned() {
                    pos.locked = false;
                    self.positions.insert(*position_id, pos);
                }
            }
            PendingOracleActionKind::AccrueFunding { .. } => {}
        }
    }

    fn resolve_xcc_callback_price(
        &mut self,
        market_id: &str,
        now_sec: u64,
    ) -> Result<u128, String> {
        let raw = env::promise_result_checked(0, MAX_CALLBACK_PAYLOAD_BYTES)
            .map_err(|e: PromiseError| format!("ERR_ORACLE_XCC_PROMISE_FAILED: {e:?}"))?;
        let obs_opt: Option<ExternalPriceObservation> = serde_json::from_slice(&raw)
            .map_err(|_| "ERR_ORACLE_XCC_MALFORMED_JSON".to_string())?;
        let obs = obs_opt.ok_or_else(|| "ERR_ORACLE_PRICE_MISSING".to_string())?;

        let mut market = self
            .markets
            .get(market_id)
            .cloned()
            .ok_or_else(|| "ERR_MARKET_NOT_FOUND".to_string())?;

        let OracleSourceConfig::ExternalXcc { oracle_market, .. } = market.oracle_source.clone()
        else {
            return Err("ERR_INVALID_ORACLE_MODE".to_string());
        };

        let worker_mode = validate_external_observation_metadata(&obs, &oracle_market)?;
        let prev_mode = market.mode;
        let eval = evaluate_candidate_price_against_policy(
            &mut market,
            obs.price.0,
            obs.observed_at_sec,
            &obs.data_class,
            worker_mode,
            now_sec,
        );
        let new_mode = market.mode;
        self.markets.insert(market_id.to_string(), market);

        if prev_mode != new_mode {
            emit_event(
                "market_paused",
                &MarketModeChangedEvent {
                    market_id,
                    triggered_by: env::current_account_id().as_str(),
                    previous_mode: Self::mode_label(prev_mode),
                    new_mode: Self::mode_label(new_mode),
                    reason: "xcc_oracle_policy_transition",
                    frozen_settlement_price_e8: None,
                    frozen_funding_index_e12: None,
                },
            );
        }

        match eval {
            EvaluatedPriceResult::Valid { price_e8, .. } => Ok(price_e8),
            EvaluatedPriceResult::TriggerReduceOnly { code }
            | EvaluatedPriceResult::Rejected { code } => Err(code),
        }
    }

    fn find_open_external_xcc_market(&self) -> Option<(String, AccountId, String)> {
        for m_id in &self.market_ids {
            if let Some(m) = self.markets.get(m_id) {
                if m.open_positions > 0 && m.mode != StoredMarketMode::Emergency {
                    if let OracleSourceConfig::ExternalXcc {
                        oracle_contract,
                        oracle_market,
                    } = &m.oracle_source
                    {
                        return Some((
                            m_id.clone(),
                            oracle_contract.clone(),
                            oracle_market.clone(),
                        ));
                    }
                }
            }
        }
        None
    }

    fn emit_lp_withdrawn_and_dispatch(
        &mut self,
        caller: AccountId,
        token_id: AccountId,
        shares_to_burn: u128,
        outcome: LpWithdrawOutcome,
    ) -> PromiseOrValue<()> {
        let nav_e8 = self
            .internal_readonly_vault_mtm()
            .map(|v| v.lp_mtm_equity_e8)
            .unwrap_or(self.buckets.lp_vault_realized_equity_e8 as i128);
        let promise = self.internal_dispatch_withdrawal_kind(
            WithdrawalKind::LpProceeds,
            caller.clone(),
            token_id.clone(),
            outcome.transfer_units,
            outcome.transfer_e8,
        );
        emit_event(
            "lp_withdrawn",
            &LpWithdrawnEvent {
                account_id: caller.as_str(),
                token_id: token_id.as_str(),
                withdrawal_id: self.next_withdrawal_id,
                shares_burned: shares_to_burn.to_string(),
                assets_out_e8: outcome.assets_out_e8.to_string(),
                transfer_units: outcome.transfer_units.to_string(),
                residue_to_free_e8: outcome.residue_e8.to_string(),
                nav_e8: nav_e8.to_string(),
            },
        );
        PromiseOrValue::Promise(promise)
    }

    /// Reference helper computing exact settlement preview for an existing open position.
    pub fn preview_position_settlement_at(
        &self,
        position_id: u64,
        settlement_price_e8: U128,
        settlement_funding_index_e12: Option<String>,
        close_fee_bps: Option<u32>,
    ) -> Option<UniformPositionSettlement> {
        let pos = self.positions.get(&position_id)?;
        let market = self.markets.get(&pos.market_id)?;
        let settle_f = match settlement_funding_index_e12 {
            Some(s) => s.parse::<i128>().ok()?,
            None => market.cum_funding_index_e12,
        };
        compute_position_settlement_e8(
            &PositionSnapshot {
                side: pos.side.into(),
                size_e8: pos.size_e8,
                locked_margin_e8: pos.locked_margin_e8,
                entry_price_e8: pos.entry_price_e8,
                entry_funding_index_e12: pos.entry_funding_index_e12,
            },
            settlement_price_e8.0,
            settle_f,
            market
                .risk
                .max_pnl_bps
                .or(Some(TESTNET_DEFAULT_MAX_PNL_BPS)),
            close_fee_bps.unwrap_or(market.risk.trade_fee_bps),
        )
        .ok()
    }

    /// Unused constant suppression guard.
    pub fn get_internal_usd_scale(&self) -> U128 {
        U128(USD_SCALE)
    }
}
