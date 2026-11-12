// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

//! State, configuration, collateral, oracle, and view types for ROASTAKE Perpetuals.

use near_sdk::json_types::U128;
use near_sdk::{near, AccountId, Gas, NearToken};
use roastake_perps_core::{
    ContractCashBuckets, MarketMode, MarketRiskParams, OracleValidationPolicy, PositionSide,
    MIN_FIRST_LP_DEPOSIT_USD_E8, TESTNET_DEFAULT_IM_BPS, TESTNET_DEFAULT_MATCHED_GAP_BUFFER_BPS,
    TESTNET_DEFAULT_MAX_PNL_BPS, TESTNET_DEFAULT_MAX_UTILIZATION_BPS, TESTNET_DEFAULT_MM_BPS,
    TESTNET_MAX_LEVERAGE, TESTNET_PROVISIONAL_EMERGENCY_CLOSE_AGE_SEC,
    TESTNET_PROVISIONAL_EXECUTION_MAX_AGE_SEC, TESTNET_PROVISIONAL_LIQ_INSURANCE_FEE_BPS,
    TESTNET_PROVISIONAL_LIQ_KEEPER_FEE_BPS, TESTNET_PROVISIONAL_MAX_JUMP_BPS,
    TESTNET_PROVISIONAL_TRADE_FEE_BPS, USD_SCALE,
};
use roastake_shared::{DEFAULT_MAX_CLOCK_DRIFT_SEC, UNIT_USD_PER_KG};

/// Default NEP-145 style storage deposit required per registered trader/LP/keeper account (0.05 NEAR).
pub const ACCOUNT_STORAGE_DEPOSIT_YOCTO: u128 = 50_000_000_000_000_000_000_000;

/// Maximum open positions permitted concurrently so on-chain MTM LP valuation remains O(N)-gas bounded.
pub const DEFAULT_MAX_OPEN_POSITIONS: u32 = 128;

/// Static gas attached to outbound NEP-141 `ft_transfer` calls.
pub const FT_TRANSFER_GAS: Gas = Gas::from_tgas(25);

/// Static gas attached to private `on_withdraw_transfer` callback.
pub const WITHDRAW_CALLBACK_GAS: Gas = Gas::from_tgas(25);

/// Static gas attached to outbound XCC `ext_oracle::get_price` view calls.
pub const ORACLE_XCC_VIEW_GAS: Gas = Gas::from_tgas(15);

/// Static gas attached to private XCC settlement callbacks.
pub const ORACLE_XCC_CALLBACK_GAS: Gas = Gas::from_tgas(45);

/// Static gas attached to private `ft_metadata` verification callback.
pub const METADATA_VERIFY_CALLBACK_GAS: Gas = Gas::from_tgas(20);

/// Maximum byte size accepted from Oracle XCC callback results.
pub const MAX_CALLBACK_PAYLOAD_BYTES: usize = 4_096;

/// Maximum byte size accepted from NEP-148 `ft_metadata` callbacks (accommodates inline SVG icons).
pub const MAX_METADATA_PAYLOAD_BYTES: usize = 16_384;

/// Official Circle native USDC contract account on NEAR Testnet (`decimals = 6`, validated on-chain).
pub const CIRCLE_TESTNET_USDC_ACCOUNT_ID: &str =
    "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af";

/// Native Circle USDC account on NEAR Mainnet (explicitly rejected by Testnet boundary guards).
pub const CIRCLE_MAINNET_USDC_ACCOUNT_ID: &str =
    "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1";

/// Legacy third-party Tidepool Testnet token account kept only as an alternative fixture in integration tests.
pub const TIDEPOOL_TESTNET_USDC_FIXTURE_ID: &str = "usdc.tidepool-sandbox.testnet";

/// Default Testnet transparency disclosure paired with Circle NEAR Testnet USDC.
pub const CIRCLE_TESTNET_DISCLOSURE: &str =
    "Circle Native Testnet USDC (no monetary value): official 6-decimal NEAR Testnet USDC (3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af); 1 USDC = 1 USD parity is accounting-only.";

/// Fee split: 70% LP Vault / 10% Insurance Fund / 20% Protocol Treasury (10,000 bps total).
pub const FEE_SPLIT_LP_BPS: u32 = 7_000;
pub const FEE_SPLIT_INSURANCE_BPS: u32 = 1_000;
pub const FEE_SPLIT_PROTOCOL_BPS: u32 = 2_000;

/// Hard clock-drift guard for future oracle timestamps.
pub const MAX_CLOCK_DRIFT_SEC: u64 = DEFAULT_MAX_CLOCK_DRIFT_SEC;

/// Synthetic Testnet sandbox feed identifiers (strictly forbidden from production claims).
pub const DATA_CLASS_TESTNET_SANDBOX: &str = "TESTNET_SANDBOX_FEED";
pub const MODE_TESTNET_SANDBOX: &str = "testnet_sandbox";

/// Explicit trade direction.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredPositionSide {
    Long,
    Short,
}

impl From<StoredPositionSide> for PositionSide {
    fn from(value: StoredPositionSide) -> Self {
        match value {
            StoredPositionSide::Long => PositionSide::Long,
            StoredPositionSide::Short => PositionSide::Short,
        }
    }
}

impl From<PositionSide> for StoredPositionSide {
    fn from(value: PositionSide) -> Self {
        match value {
            PositionSide::Long => StoredPositionSide::Long,
            PositionSide::Short => StoredPositionSide::Short,
        }
    }
}

/// Operating mode of a perpetuals market.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredMarketMode {
    Normal,
    ReduceOnly,
    Paused,
    Emergency,
}

impl From<StoredMarketMode> for MarketMode {
    fn from(value: StoredMarketMode) -> Self {
        match value {
            StoredMarketMode::Normal => MarketMode::Normal,
            StoredMarketMode::ReduceOnly => MarketMode::ReduceOnly,
            StoredMarketMode::Paused => MarketMode::Paused,
            StoredMarketMode::Emergency => MarketMode::Emergency,
        }
    }
}

impl From<MarketMode> for StoredMarketMode {
    fn from(value: MarketMode) -> Self {
        match value {
            MarketMode::Normal => StoredMarketMode::Normal,
            MarketMode::ReduceOnly => StoredMarketMode::ReduceOnly,
            MarketMode::Paused => StoredMarketMode::Paused,
            MarketMode::Emergency => StoredMarketMode::Emergency,
        }
    }
}

/// Borsh-compatible mirror of `roastake_perps_core::MarketRiskParams`.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMarketRiskParams {
    pub max_leverage: u32,
    pub initial_margin_bps: u32,
    pub maintenance_margin_bps: u32,
    pub trade_fee_bps: u32,
    pub liq_keeper_fee_bps: u32,
    pub liq_insurance_fee_bps: u32,
    pub max_utilization_bps: u32,
    pub matched_gap_buffer_bps: u32,
    pub max_hourly_funding_rate_e12: U128,
    pub max_pnl_bps: Option<u32>,
}

impl Default for StoredMarketRiskParams {
    fn default() -> Self {
        Self::from_shared(&MarketRiskParams::default())
    }
}

impl StoredMarketRiskParams {
    pub fn to_shared(&self) -> MarketRiskParams {
        MarketRiskParams {
            max_leverage: self.max_leverage,
            initial_margin_bps: self.initial_margin_bps,
            maintenance_margin_bps: self.maintenance_margin_bps,
            trade_fee_bps: self.trade_fee_bps,
            liq_keeper_fee_bps: self.liq_keeper_fee_bps,
            liq_insurance_fee_bps: self.liq_insurance_fee_bps,
            max_utilization_bps: self.max_utilization_bps,
            matched_gap_buffer_bps: self.matched_gap_buffer_bps,
            max_hourly_funding_rate_e12: self.max_hourly_funding_rate_e12.0,
            max_pnl_bps: self.max_pnl_bps,
        }
    }

    pub fn from_shared(shared: &MarketRiskParams) -> Self {
        Self {
            max_leverage: shared.max_leverage,
            initial_margin_bps: shared.initial_margin_bps,
            maintenance_margin_bps: shared.maintenance_margin_bps,
            trade_fee_bps: shared.trade_fee_bps,
            liq_keeper_fee_bps: shared.liq_keeper_fee_bps,
            liq_insurance_fee_bps: shared.liq_insurance_fee_bps,
            max_utilization_bps: shared.max_utilization_bps,
            matched_gap_buffer_bps: shared.matched_gap_buffer_bps,
            max_hourly_funding_rate_e12: U128(shared.max_hourly_funding_rate_e12),
            max_pnl_bps: shared.max_pnl_bps,
        }
    }

    pub fn testnet_mvp_defaults() -> Self {
        Self {
            max_leverage: TESTNET_MAX_LEVERAGE,
            initial_margin_bps: TESTNET_DEFAULT_IM_BPS,
            maintenance_margin_bps: TESTNET_DEFAULT_MM_BPS,
            trade_fee_bps: TESTNET_PROVISIONAL_TRADE_FEE_BPS,
            liq_keeper_fee_bps: TESTNET_PROVISIONAL_LIQ_KEEPER_FEE_BPS,
            liq_insurance_fee_bps: TESTNET_PROVISIONAL_LIQ_INSURANCE_FEE_BPS,
            max_utilization_bps: TESTNET_DEFAULT_MAX_UTILIZATION_BPS,
            matched_gap_buffer_bps: TESTNET_DEFAULT_MATCHED_GAP_BUFFER_BPS,
            max_hourly_funding_rate_e12: U128(
                MarketRiskParams::default().max_hourly_funding_rate_e12,
            ),
            max_pnl_bps: Some(TESTNET_DEFAULT_MAX_PNL_BPS),
        }
    }
}

/// Borsh-compatible mirror of `roastake_perps_core::OracleValidationPolicy`.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredOraclePolicy {
    pub allow_testnet_sandbox: bool,
    pub max_execution_age_sec: u64,
    pub emergency_close_age_sec: u64,
    pub max_jump_bps: u32,
    pub max_clock_drift_sec: u64,
}

impl Default for StoredOraclePolicy {
    fn default() -> Self {
        Self {
            allow_testnet_sandbox: true,
            max_execution_age_sec: TESTNET_PROVISIONAL_EXECUTION_MAX_AGE_SEC,
            emergency_close_age_sec: TESTNET_PROVISIONAL_EMERGENCY_CLOSE_AGE_SEC,
            max_jump_bps: TESTNET_PROVISIONAL_MAX_JUMP_BPS,
            max_clock_drift_sec: MAX_CLOCK_DRIFT_SEC,
        }
    }
}

impl StoredOraclePolicy {
    pub fn to_shared(&self) -> OracleValidationPolicy {
        OracleValidationPolicy {
            allow_testnet_sandbox: self.allow_testnet_sandbox,
            max_execution_age_sec: self.max_execution_age_sec,
            emergency_close_age_sec: self.emergency_close_age_sec,
            max_jump_bps: self.max_jump_bps,
            max_clock_drift_sec: self.max_clock_drift_sec,
        }
    }
}

/// Borsh-compatible realized cash-backed ledger (`USD_SCALE = 10^8`).
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredCashBuckets {
    pub total_trader_free_collateral_e8: u128,
    pub total_trader_locked_margin_e8: u128,
    pub total_in_flight_withdrawals_e8: u128,
    pub lp_vault_realized_equity_e8: u128,
    pub insurance_fund_e8: u128,
    pub protocol_fees_e8: u128,
}

impl StoredCashBuckets {
    pub fn to_shared(&self) -> ContractCashBuckets {
        ContractCashBuckets {
            total_trader_free_collateral_e8: self.total_trader_free_collateral_e8,
            total_trader_locked_margin_e8: self.total_trader_locked_margin_e8,
            total_in_flight_withdrawals_e8: self.total_in_flight_withdrawals_e8,
            lp_vault_realized_equity_e8: self.lp_vault_realized_equity_e8,
            insurance_fund_e8: self.insurance_fund_e8,
            protocol_fees_e8: self.protocol_fees_e8,
        }
    }

    pub fn verify_tracked_solvency(&self, tracked_collateral_e8: u128) -> Result<(), String> {
        let _unallocated = self
            .to_shared()
            .verify_solvency(tracked_collateral_e8)
            .map_err(|e| format!("ERR_SOLVENCY_INVARIANT_VIOLATED: {e:?}"))?;
        // Strict equality: every tracked collateral unit must map to exactly one disjoint bucket.
        let sum = self
            .total_trader_free_collateral_e8
            .checked_add(self.total_trader_locked_margin_e8)
            .and_then(|v| v.checked_add(self.total_in_flight_withdrawals_e8))
            .and_then(|v| v.checked_add(self.lp_vault_realized_equity_e8))
            .and_then(|v| v.checked_add(self.insurance_fund_e8))
            .and_then(|v| v.checked_add(self.protocol_fees_e8))
            .ok_or_else(|| "ERR_SOLVENCY_SUM_OVERFLOW".to_string())?;
        if sum != tracked_collateral_e8 {
            return Err(format!(
                "ERR_SOLVENCY_INVARIANT_VIOLATED: sum_buckets ({sum}) != tracked_collateral_e8 ({tracked_collateral_e8})"
            ));
        }
        Ok(())
    }
}

/// Allowlisted external NEP-141 collateral token configuration (default: Circle Testnet USDC `3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af`).
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollateralTokenConfig {
    pub token_id: AccountId,
    pub symbol: String,
    pub decimals: u8,
    /// Factor multiplying token base units to `USD_SCALE = 10^8` (`100` for 6-decimal USDC).
    pub scale_to_usd_e8: u128,
    /// Mandatory Testnet transparency flag disclosing non-monetary / centrally mintable status.
    pub non_monetary_testnet_disclosure: String,
    pub active: bool,
    pub metadata_verified_on_chain: bool,
    pub tracked_balance_units: u128,
}

impl CollateralTokenConfig {
    pub fn checked_usd_e8(&self, raw_units: u128) -> Result<u128, String> {
        raw_units
            .checked_mul(self.scale_to_usd_e8)
            .ok_or_else(|| "ERR_COLLATERAL_CONVERSION_OVERFLOW".to_string())
    }

    /// Split `USD_SCALE = 10^8` into `(whole_token_units, sub_unit_residue_e8)` without losing precision.
    pub fn split_usd_e8_to_token_units(&self, amount_e8: u128) -> Result<(u128, u128), String> {
        if self.scale_to_usd_e8 == 0 {
            return Err("ERR_INVALID_COLLATERAL_SCALE".to_string());
        }
        Ok((
            amount_e8 / self.scale_to_usd_e8,
            amount_e8 % self.scale_to_usd_e8,
        ))
    }
}

/// NEP-148 token metadata returned by `ext_ft::ft_metadata`.
#[near(serializers = [json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FungibleTokenMetadata {
    pub spec: String,
    pub name: String,
    pub symbol: String,
    pub icon: Option<String>,
    pub reference: Option<String>,
    pub reference_hash: Option<String>,
    pub decimals: u8,
}

/// External Oracle observation schema matching `contracts/oracle-adapter` `PriceObservation`.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalPriceObservation {
    pub market: String,
    pub price: U128,
    pub decimals: u8,
    pub unit: String,
    pub source: String,
    pub source_count: u32,
    pub observed_at_sec: u64,
    pub stored_at_sec: u64,
    pub nonce: u64,
    pub data_class: String,
}

/// Oracle source mode bound per perpetuals market.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OracleSourceConfig {
    /// Asynchronous 2-receipt XCC view pull against a verified oracle contract (e.g. `roastake-oracle-v1.testnet`).
    /// Note: strictly rejects `PUBLIC_BENCHMARK_MONTHLY` and `SYNTHETIC_DEMO` data classes.
    ExternalXcc {
        oracle_contract: AccountId,
        oracle_market: String,
    },
    /// Isolated Testnet-only sandbox feed updated by an explicitly authorized operator account
    /// with monotonic sequence numbers, bounds, and full freshness/jump circuit-breakers.
    /// Explicitly tagged `TESTNET_SANDBOX_FEED` and never presented as decentralized market pricing.
    TestnetSandboxOperator {
        oracle_operator: AccountId,
        min_price_e8: U128,
        max_price_e8: U128,
    },
}

/// Latest validated observation recorded on an isolated `TESTNET_SANDBOX_FEED` market.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestnetSandboxObservation {
    pub price_e8: u128,
    pub observed_at_sec: u64,
    pub recorded_at_sec: u64,
    pub sequence: u64,
    pub data_class: String,
    pub worker_mode: String,
    pub publisher: AccountId,
}

/// Perpetuals market runtime state.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerpsMarketState {
    pub market_id: String,
    pub unit: String,
    pub mode: StoredMarketMode,
    pub paused_for_insolvency: bool,
    pub risk: StoredMarketRiskParams,
    pub oracle_policy: StoredOraclePolicy,
    pub oracle_source: OracleSourceConfig,
    pub exec_spread_bps: u32,
    pub long_oi_e8: u128,
    pub short_oi_e8: u128,
    pub long_size_e8: u128,
    pub short_size_e8: u128,
    pub open_positions: u64,
    pub cum_funding_index_e12: i128,
    pub last_funding_sec: u64,
    pub last_valid_price_e8: Option<u128>,
    pub last_valid_observed_sec: Option<u64>,
    pub sandbox_observation: Option<TestnetSandboxObservation>,
    pub attested_emergency_price_e8: Option<u128>,
    pub frozen_settlement_price_e8: Option<u128>,
    pub frozen_funding_index_e12: Option<i128>,
    pub emergency_triggered_at_sec: Option<u64>,
}

/// Isolated leveraged coffee perpetual position.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionRecord {
    pub id: u64,
    pub owner: AccountId,
    pub market_id: String,
    pub side: StoredPositionSide,
    pub size_e8: u128,
    pub notional_e8: u128,
    pub entry_price_e8: u128,
    pub locked_margin_e8: u128,
    pub entry_funding_index_e12: i128,
    pub opened_at_sec: u64,
    pub locked: bool,
}

/// Registered trader / LP / keeper ledger account.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountRecord {
    pub registered: bool,
    pub storage_paid_yocto: u128,
    pub free_collateral_e8: u128,
    pub lp_shares: u128,
    pub pending_action_lock: bool,
    pub has_pending_withdrawal: bool,
}

/// Purpose classification for inbound NEP-141 `ft_transfer_call` JSON memo (`msg`).
#[near(serializers = [json])]
#[derive(Debug, Clone, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DepositMemo {
    TraderCollateral,
    LpDeposit { min_shares: Option<U128> },
    InsuranceFund,
    TreasuryTopUp,
}

/// Destination classification for outbound NEP-141 `ft_transfer` operations.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalKind {
    TraderFreeCollateral,
    LpProceeds,
    ProtocolTreasury,
}

/// Active outbound token withdrawal awaiting `#[private] on_withdraw_transfer` confirmation.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithdrawalRecord {
    pub id: u64,
    pub kind: WithdrawalKind,
    pub account_id: AccountId,
    pub token_id: AccountId,
    pub amount_token_units: u128,
    pub amount_usd_e8: u128,
    pub created_at_sec: u64,
}

/// State persisted across two-receipt XCC `ext_oracle::get_price` calls.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingOracleActionKind {
    OpenPosition {
        trader: AccountId,
        market_id: String,
        side: StoredPositionSide,
        size_e8: u128,
        collateral_e8: u128,
        price_limit_e8: Option<u128>,
    },
    IncreasePosition {
        trader: AccountId,
        position_id: u64,
        add_size_e8: u128,
        add_collateral_e8: u128,
        price_limit_e8: Option<u128>,
    },
    ReducePosition {
        trader: AccountId,
        position_id: u64,
        close_size_e8: u128,
        price_limit_e8: Option<u128>,
    },
    RemoveMargin {
        trader: AccountId,
        position_id: u64,
        amount_e8: u128,
    },
    LiquidatePosition {
        keeper: AccountId,
        position_id: u64,
    },
    AccrueFunding {
        market_id: String,
    },
    DepositLp {
        lp_account: AccountId,
        market_id: String,
        token_id: AccountId,
        raw_amount_units: u128,
        deposit_e8: u128,
        min_shares: u128,
    },
    WithdrawLp {
        lp_account: AccountId,
        market_id: String,
        token_id: AccountId,
        shares_to_burn: u128,
        min_assets_out_e8: u128,
    },
}

/// Tracked in-flight async oracle action awaiting callback resolution.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOracleAction {
    pub id: u64,
    pub initiated_at_sec: u64,
    pub kind: PendingOracleActionKind,
}

/// JSON view snapshot for protocol accounting invariants (`USD_SCALE = 10^8` + physical backing).
#[near(serializers = [json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountingSnapshotView {
    pub total_trader_free_collateral_e8: U128,
    pub total_trader_locked_margin_e8: U128,
    pub total_in_flight_withdrawals_e8: U128,
    pub lp_vault_realized_equity_e8: U128,
    pub insurance_fund_e8: U128,
    pub protocol_fees_e8: U128,
    pub tracked_collateral_e8: U128,
    pub lp_total_shares: U128,
    pub total_open_positions: u64,
    pub solvency_ok: bool,
    pub paused_global: bool,
}

/// JSON view snapshot for a registered collateral token.
#[near(serializers = [json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollateralTokenView {
    pub token_id: AccountId,
    pub symbol: String,
    pub decimals: u8,
    pub scale_to_usd_e8: U128,
    pub active: bool,
    pub metadata_verified_on_chain: bool,
    pub tracked_balance_units: U128,
    pub non_monetary_testnet_disclosure: String,
}

/// JSON view snapshot for an account's balances and locks.
#[near(serializers = [json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountView {
    pub account_id: AccountId,
    pub registered: bool,
    pub storage_paid_yocto: U128,
    pub free_collateral_e8: U128,
    pub withdrawable_token_units_at_scale_100: U128,
    pub sub_micro_unit_residue_e8: U128,
    pub lp_shares: U128,
    pub pending_action_lock: bool,
    pub has_pending_withdrawal: bool,
}

/// JSON view snapshot for a market (including explicit sandbox feed disclosures).
#[near(serializers = [json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketView {
    pub market_id: String,
    pub unit: String,
    pub mode: StoredMarketMode,
    pub paused_for_insolvency: bool,
    pub feed_classification: String,
    pub is_synthetic_testnet_sandbox: bool,
    pub sandbox_operator: Option<AccountId>,
    pub sandbox_min_price_e8: Option<U128>,
    pub sandbox_max_price_e8: Option<U128>,
    pub sandbox_sequence: Option<u64>,
    pub long_oi_e8: U128,
    pub short_oi_e8: U128,
    pub long_size_e8: U128,
    pub short_size_e8: U128,
    pub open_positions: u64,
    pub cum_funding_index_e12: String,
    pub last_funding_sec: u64,
    pub last_valid_price_e8: Option<U128>,
    pub last_valid_observed_sec: Option<u64>,
    pub frozen_settlement_price_e8: Option<U128>,
    pub frozen_funding_index_e12: Option<String>,
    pub risk: StoredMarketRiskParams,
    pub oracle_policy: StoredOraclePolicy,
    pub exec_spread_bps: u32,
}

/// JSON view snapshot for an open position plus indicative health metrics.
#[near(serializers = [json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionHealthView {
    pub position: PositionRecordView,
    pub reference_price_e8: U128,
    pub unrealized_pnl_e8: String,
    pub capped_pnl_e8: String,
    pub funding_net_e8: String,
    pub equity_e8: String,
    pub maintenance_margin_e8: U128,
    pub margin_ratio_bps: u32,
    pub liquidatable_at_reference: bool,
    pub indicative_liquidation_price_e8: Option<U128>,
}

/// JSON-safe mirror of `PositionRecord`.
#[near(serializers = [json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionRecordView {
    pub id: u64,
    pub owner: AccountId,
    pub market_id: String,
    pub side: StoredPositionSide,
    pub size_e8: U128,
    pub notional_e8: U128,
    pub entry_price_e8: U128,
    pub locked_margin_e8: U128,
    pub entry_funding_index_e12: String,
    pub opened_at_sec: u64,
    pub locked: bool,
}

impl From<&PositionRecord> for PositionRecordView {
    fn from(pos: &PositionRecord) -> Self {
        Self {
            id: pos.id,
            owner: pos.owner.clone(),
            market_id: pos.market_id.clone(),
            side: pos.side,
            size_e8: U128(pos.size_e8),
            notional_e8: U128(pos.notional_e8),
            entry_price_e8: U128(pos.entry_price_e8),
            locked_margin_e8: U128(pos.locked_margin_e8),
            entry_funding_index_e12: pos.entry_funding_index_e12.to_string(),
            opened_at_sec: pos.opened_at_sec,
            locked: pos.locked,
        }
    }
}

/// Return storage registration bounds mirroring NEP-145 conventions.
#[near(serializers = [json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageBalanceBoundsView {
    pub min: U128,
    pub max: U128,
}

/// Storage balance summary mirroring NEP-145 conventions.
#[near(serializers = [json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageBalanceView {
    pub total: U128,
    pub available: U128,
}

/// Compile-time / runtime constant assertions for key M04.5A.1 invariants.
pub fn validate_initial_lp_floor(amount_e8: u128) -> Result<(), String> {
    if amount_e8 < MIN_FIRST_LP_DEPOSIT_USD_E8 {
        return Err(format!(
            "ERR_FIRST_LP_DEPOSIT_TOO_SMALL: minimum first LP seed is {MIN_FIRST_LP_DEPOSIT_USD_E8} e8"
        ));
    }
    Ok(())
}

/// Default unit label expected across coffee perpetuals feeds.
pub fn expected_feed_unit() -> &'static str {
    UNIT_USD_PER_KG
}

/// Enforce standard NEAR Testnet / Sandbox account boundaries (`.testnet`, `.test.near`, or non-Mainnet 64-hex implicit accounts ONLY).
pub fn assert_testnet_or_sandbox_account(
    account_id: &AccountId,
    context: &str,
) -> Result<(), String> {
    let raw = account_id.as_str();
    let lower = raw.to_ascii_lowercase();
    if lower == CIRCLE_MAINNET_USDC_ACCOUNT_ID {
        return Err(format!(
            "ERR_MAINNET_REJECTED: mainnet Circle USDC account '{raw}' forbidden for {context}"
        ));
    }
    if lower.contains("mainnet") {
        return Err(format!(
            "ERR_MAINNET_REJECTED: mainnet reference in account '{raw}' forbidden for {context}"
        ));
    }
    if lower.ends_with(".near") && !lower.ends_with(".test.near") {
        return Err(format!(
            "ERR_MAINNET_REJECTED: mainnet account '{raw}' forbidden for {context}"
        ));
    }
    if !(lower.ends_with(".testnet") || lower.ends_with(".test.near") || is_implicit_hex_64(&lower))
    {
        // Explicit check via roastake_shared when standard named .testnet account
        roastake_shared::validate_testnet_account_id(raw)
            .map_err(|e| format!("ERR_MAINNET_REJECTED ({context}): {e}"))?;
    }
    Ok(())
}

fn is_implicit_hex_64(account_id: &str) -> bool {
    account_id.len() == 64
        && account_id
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Storage margin returned on `NearToken` formatting helper.
pub fn account_storage_deposit_near() -> NearToken {
    NearToken::from_yoctonear(ACCOUNT_STORAGE_DEPOSIT_YOCTO)
}

/// Avoid dead_code warning if USD_SCALE constant is referenced across docs/tests.
pub const INTERNAL_USD_SCALE: u128 = USD_SCALE;
