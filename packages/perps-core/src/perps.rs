// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

//! Shared Perpetuals constants, domain types, checked fixed-point primitives,
//! margin/PnL/funding/liquidation math, LP share pricing, solvency checks, and oracle policy helpers.
//!
//! ## Design Rules
//! - Strictly deterministic integer fixed-point arithmetic (`u128` / `i128` with dependency-free
//!   exact 256-bit `mul_div` intermediates). **Zero** floating-point (`f32`/`f64`) operations.
//! - Every quantity carries documented units and scales:
//!   - Prices: `D8{USD/kg}` scaled by [`PRICE_SCALE`] (`10^8`).
//!   - Position Sizes: `D8{kg}` scaled by [`SIZE_SCALE`] (`10^8`).
//!   - Internal USD / `rUSD` Collateral: `D8{rUSD}` scaled by [`USD_SCALE`] (`10^8`).
//!   - Basis Points: `bps` scaled by [`BPS_SCALE`] (`10_000`, where `10_000 = 100%`).
//!   - Funding Rates (`D12{1/s}`) and Cumulative Funding Index (`D12{USD/kg}`): scaled by
//!     [`FUNDING_SCALE`] (`10^12`).
//!   - LP Vault Shares: `D8{share}` scaled by [`LP_SHARE_SCALE`] (`10^8`).
//! - All math favors **protocol solvency** under fractional rounding:
//!   - Protocol charges, IM/MM, and fees round **up** (`ceil`).
//!   - User credits, payouts, and shares minted/redeemed round **down** (`floor`).
//!   - Signed trader net PnL / funding rounds toward **negative infinity** (`floor toward -inf`).

use core::fmt;
use roastake_shared::{
    calculate_divergence_bps, DATA_CLASS_LIVE_AGGREGATED, DATA_CLASS_LIVE_VERIFIED,
    DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY, DATA_CLASS_SYNTHETIC, FIXED_POINT_SCALE_E12,
    FIXED_POINT_SCALE_E8, MODE_FIXTURE, MODE_LIVE_INDEX, MODE_PUBLIC_BENCHMARK,
};
use serde::{Deserialize, Serialize};

// -----------------------------------------------------------------------------
// Canonical Fixed-Point Scales
// -----------------------------------------------------------------------------

/// Fixed-point scaling factor for coffee prices in `USD/kg` (`10^8`).
pub const PRICE_SCALE: u128 = FIXED_POINT_SCALE_E8;

/// Fixed-point scaling factor for contract sizes in `kg` (`10^8`, i.e., `10^-8 kg` precision).
pub const SIZE_SCALE: u128 = FIXED_POINT_SCALE_E8;

/// Fixed-point scaling factor for `rUSD` collateral & accounting USD amounts (`10^8`).
pub const USD_SCALE: u128 = FIXED_POINT_SCALE_E8;

/// Basis point denominator (`10_000 = 100.00%`, `1 bps = 0.01%`).
pub const BPS_SCALE: u128 = 10_000;

/// Fixed-point scaling factor (`10^12`) for per-second funding rates (`D12{1/s}`)
/// and signed cumulative funding indices (`D12{USD/kg}`).
///
/// Reuses [`FIXED_POINT_SCALE_E12`] (`10^12`) from `fixed_point.rs`:
/// - At `1 bps / 8h` (`~3.472e-9 / s`), `r_sec * 10^12 = 3472`, keeping single-second truncation
///   error below `0.03%`.
/// - Across `$500/kg` coffee and `10^8 kg` (`100,000 MT`), `size_e8 * delta_cum_e12 <= 10^32`,
///   leaving `> 10^6x` headroom below `i128::MAX` (`~1.7e38`) alongside exact 256-bit `mul_div`.
pub const FUNDING_SCALE: u128 = FIXED_POINT_SCALE_E12;

/// Signed equivalent of [`FUNDING_SCALE`] (`10^12`).
pub const FUNDING_SCALE_I128: i128 = 1_000_000_000_000;

/// Fixed-point scaling factor for LP Vault shares (`10^8`, matching `USD_SCALE`).
pub const LP_SHARE_SCALE: u128 = USD_SCALE;

/// Seconds per hour (`3,600 s`).
pub const SECONDS_PER_HOUR: u64 = 3_600;

/// Seconds per standard 8-hour funding interval (`28,800 s`).
pub const SECONDS_PER_8H: u64 = 8 * SECONDS_PER_HOUR;

// -----------------------------------------------------------------------------
// Normative Protocol Limits & Initial Testnet Parameters (M04.5A Specification)
// -----------------------------------------------------------------------------

/// Minimum LP seed shares permanently locked on the very first LP Vault deposit (`0.0001` shares).
///
/// Defeats first-depositor share-inflation / donation attacks and prevents the pool share
/// denominator from collapsing back to zero when the last external LP exits.
pub const MINIMUM_LP_SEED_SHARES: u128 = 10_000;

/// Minimum first LP deposit in `USD_SCALE` (`1 rUSD = 10^8`) so seed shares are negligible (`0.01%`).
pub const MIN_FIRST_LP_DEPOSIT_USD_E8: u128 = USD_SCALE;

/// Approved Testnet specification maximum leverage (`5x`, corresponding to `20%` IM).
pub const TESTNET_MAX_LEVERAGE: u32 = 5;

/// Approved Testnet initial margin ratio (`2000 bps = 20.00%`).
pub const TESTNET_DEFAULT_IM_BPS: u32 = 2_000;

/// Approved Testnet maintenance margin ratio (`800 bps = 8.00%`).
pub const TESTNET_DEFAULT_MM_BPS: u32 = 800;

/// Provisional Testnet taker/open/close fee (`10 bps = 0.10%` of notional).
pub const TESTNET_PROVISIONAL_TRADE_FEE_BPS: u32 = 10;

/// Refined provisional Testnet keeper liquidation incentive (`50 bps = 0.50%` of mark notional).
pub const TESTNET_PROVISIONAL_LIQ_KEEPER_FEE_BPS: u32 = 50;

/// Refined provisional Testnet insurance fee on liquidation (`50 bps = 0.50%` of mark notional).
pub const TESTNET_PROVISIONAL_LIQ_INSURANCE_FEE_BPS: u32 = 50;

/// Approved Testnet maximum LP utilization (`7500 bps = 75.00%`).
pub const TESTNET_DEFAULT_MAX_UTILIZATION_BPS: u32 = 7_500;

/// Provisional Testnet matched-OI gap buffer (`2500 bps = 25.00%` of matched OI).
pub const TESTNET_DEFAULT_MATCHED_GAP_BUFFER_BPS: u32 = 2_500;

/// Provisional Testnet execution freshness threshold (`300 s = 5 minutes`) for `TestnetSandbox` / `ProductionLive`.
/// Incompatible with `ARABICA-MONTHLY-IMF/USD` (`90d` policy) and `GESHA-DEMO/USD` (`observed_at = 0`).
pub const TESTNET_PROVISIONAL_EXECUTION_MAX_AGE_SEC: u64 = 300;

/// Provisional Testnet emergency outage exit threshold (`3,600 s = 1 hour`) past last valid price.
pub const TESTNET_PROVISIONAL_EMERGENCY_CLOSE_AGE_SEC: u64 = 3_600;

/// Provisional Testnet single-update price jump circuit breaker (`500 bps = 5.00%`).
pub const TESTNET_PROVISIONAL_MAX_JUMP_BPS: u32 = 500;

/// Execution mode for Testnet sandbox feeds (distinct from `live_index`, `public_benchmark`, `fixture`).
pub const MODE_TESTNET_SANDBOX: &str = "testnet_sandbox";

/// Explicit oracle data classification for non-commercial Testnet sandbox feeds.
pub const DATA_CLASS_TESTNET_SANDBOX_FEED: &str = "TESTNET_SANDBOX_FEED";

// -----------------------------------------------------------------------------
// Domain Enums & Configuration Types
// -----------------------------------------------------------------------------

/// Position directional side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PositionSide {
    Long,
    Short,
}

/// Position margin isolation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarginMode {
    /// Phase 1 MVP uses strictly isolated per-position margin to bound blast radius.
    Isolated,
}

/// Per-market operational state machine governing order flow and outage protection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketMode {
    /// Full trading allowed within freshness, jump, and utilization limits.
    Normal,
    /// Only operations that shrink net exposure or add collateral (`close`, `reduce`, `add_margin`, `liquidate` if price fresh).
    ReduceOnly,
    /// Trading halted (circuit breaker, governance pause, or insolvent pool state); deposits/safe withdrawals remain guarded.
    Paused,
    /// Prolonged oracle outage past `emergency_close_age_sec`; triggers irreversible uniform snapshot settlement
    /// across all open positions (freezing funding at cutoff and preventing opportunistic cherry-picking).
    Emergency,
}

/// Oracle feed classification separating real live quotes, Testnet sandbox feeds, benchmarks, and fixtures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OracleFeedTier {
    /// Verified single or multi-source live price feed (`LIVE_VERIFIED` / `LIVE_AGGREGATED`).
    ProductionLive,
    /// Explicit non-commercial Testnet sandbox feed (`TESTNET_SANDBOX_FEED`) for controlled E2E tests.
    TestnetSandbox,
    /// Historical monthly macro benchmark (`PUBLIC_BENCHMARK_MONTHLY`, e.g., IMF PCPS). Forbidden for Perps execution.
    HistoricalBenchmark,
    /// Deterministic static demo fixture (`SYNTHETIC_DEMO`). Forbidden for Perps execution.
    SyntheticFixture,
    /// Unknown or unrecognized classification (fail closed).
    Unknown,
}

/// Approved Testnet default max profit ratio relative to entry notional (`10_000 bps = 100%`,
/// i.e., `+500%` ROI at `5x` leverage) so reserved skew backing `1:1` strictly bounds vault payout liability.
pub const TESTNET_DEFAULT_MAX_PNL_BPS: u32 = 10_000;

/// Risk parameter profile for a Perpetuals market.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketRiskParams {
    pub max_leverage: u32,
    pub initial_margin_bps: u32,
    pub maintenance_margin_bps: u32,
    pub trade_fee_bps: u32,
    pub liq_keeper_fee_bps: u32,
    pub liq_insurance_fee_bps: u32,
    pub max_utilization_bps: u32,
    pub matched_gap_buffer_bps: u32,
    pub max_hourly_funding_rate_e12: u128,
    /// Optional positive PnL cap in bps of entry notional (e.g., `Some(10_000)` = `100%` of notional).
    pub max_pnl_bps: Option<u32>,
}

impl Default for MarketRiskParams {
    fn default() -> Self {
        Self {
            max_leverage: TESTNET_MAX_LEVERAGE,
            initial_margin_bps: TESTNET_DEFAULT_IM_BPS,
            maintenance_margin_bps: TESTNET_DEFAULT_MM_BPS,
            trade_fee_bps: TESTNET_PROVISIONAL_TRADE_FEE_BPS,
            liq_keeper_fee_bps: TESTNET_PROVISIONAL_LIQ_KEEPER_FEE_BPS,
            liq_insurance_fee_bps: TESTNET_PROVISIONAL_LIQ_INSURANCE_FEE_BPS,
            max_utilization_bps: TESTNET_DEFAULT_MAX_UTILIZATION_BPS,
            matched_gap_buffer_bps: TESTNET_DEFAULT_MATCHED_GAP_BUFFER_BPS,
            // Default cap: 5 bps / hour = 5e-4 * 1e12 = 500_000_000
            max_hourly_funding_rate_e12: 500_000_000,
            max_pnl_bps: Some(TESTNET_DEFAULT_MAX_PNL_BPS),
        }
    }
}

impl MarketRiskParams {
    /// Validate internal economic consistency (bounds, leverage vs IM, MM < IM, liquidation + close fees < MM).
    pub fn validate(&self) -> Result<(), PerpMathError> {
        if self.max_leverage == 0 {
            return Err(PerpMathError::InvalidMarginParams {
                reason: "max_leverage must be >= 1",
            });
        }
        let min_im_for_lev = BPS_SCALE
            .checked_div(self.max_leverage as u128)
            .ok_or(PerpMathError::ZeroDenominator)?;
        if (self.initial_margin_bps as u128) > BPS_SCALE
            || (self.initial_margin_bps as u128) < min_im_for_lev
            || self.initial_margin_bps == 0
        {
            return Err(PerpMathError::InvalidMarginParams {
                reason: "initial_margin_bps must lie in [10_000 / max_leverage, 10_000]",
            });
        }
        if self.maintenance_margin_bps == 0
            || self.maintenance_margin_bps >= self.initial_margin_bps
        {
            return Err(PerpMathError::InvalidMarginParams {
                reason: "maintenance_margin_bps must satisfy 0 < mm_bps < im_bps",
            });
        }
        if (self.trade_fee_bps as u128) >= BPS_SCALE
            || (self.max_utilization_bps as u128) > BPS_SCALE
            || (self.matched_gap_buffer_bps as u128) > BPS_SCALE
        {
            return Err(PerpMathError::InvalidBps);
        }
        let total_liq_outflow_bps = self
            .liq_keeper_fee_bps
            .checked_add(self.liq_insurance_fee_bps)
            .and_then(|v| v.checked_add(self.trade_fee_bps))
            .ok_or(PerpMathError::Overflow)?;
        if total_liq_outflow_bps >= self.maintenance_margin_bps {
            return Err(PerpMathError::InvalidMarginParams {
                reason: "combined liquidation + close fees must remain strictly below maintenance_margin_bps",
            });
        }
        if let Some(cap_bps) = self.max_pnl_bps {
            if cap_bps == 0 {
                return Err(PerpMathError::InvalidMarginParams {
                    reason: "max_pnl_bps when Some must be > 0",
                });
            }
        }
        Ok(())
    }
}

/// Policy thresholds applied by the Perps engine when evaluating an Oracle observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OracleValidationPolicy {
    pub allow_testnet_sandbox: bool,
    pub max_execution_age_sec: u64,
    pub emergency_close_age_sec: u64,
    pub max_jump_bps: u32,
    pub max_clock_drift_sec: u64,
}

impl Default for OracleValidationPolicy {
    fn default() -> Self {
        Self {
            allow_testnet_sandbox: true,
            max_execution_age_sec: TESTNET_PROVISIONAL_EXECUTION_MAX_AGE_SEC,
            emergency_close_age_sec: TESTNET_PROVISIONAL_EMERGENCY_CLOSE_AGE_SEC,
            max_jump_bps: TESTNET_PROVISIONAL_MAX_JUMP_BPS,
            max_clock_drift_sec: roastake_shared::DEFAULT_MAX_CLOCK_DRIFT_SEC,
        }
    }
}

/// Liquidation settlement output decomposing every outgoing dollar to preserve zero value creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiquidationSettlement {
    pub mark_notional_e8: u128,
    pub equity_before_close_e8: i128,
    pub maintenance_margin_e8: u128,
    pub liquidatable: bool,
    pub liq_keeper_fee_paid_e8: u128,
    pub liq_insurance_fee_paid_e8: u128,
    pub close_fee_paid_e8: u128,
    pub trader_remaining_margin_e8: u128,
    pub bad_debt_e8: u128,
}

/// Outcome of routing realized bad debt through Insurance Fund and LP Vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BadDebtCascadeOutcome {
    pub absorbed_by_insurance_e8: u128,
    pub absorbed_by_lp_vault_e8: u128,
    pub uncovered_insolvency_e8: u128,
    pub insurance_depleted: bool,
    pub should_pause_market: bool,
}

/// Outcome of validating an on-chain Oracle observation for Perps execution or liquidation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OracleHealthOutcome {
    /// Observation is valid and within execution age and jump policy.
    ValidForExecution {
        price_e8: u128,
        age_sec: u64,
        tier: OracleFeedTier,
    },
    /// Jump exceeded `max_jump_bps`; contract should enter `ReduceOnly` or `Paused` circuit breaker.
    JumpCircuitBreaker {
        previous_price_e8: u128,
        observed_price_e8: u128,
        jump_bps: u32,
    },
    /// Observation age exceeds `max_execution_age_sec` but has not crossed `emergency_close_age_sec`.
    StaleReduceOnly { age_sec: u64 },
    /// Observation age crossed `emergency_close_age_sec`; eligible for `MarketMode::Emergency`.
    EmergencyEligible { age_sec: u64 },
}

/// Realized cash-backed accounts partitioning the contract's `rUSD` token balance (no double counting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContractCashBuckets {
    pub total_trader_free_collateral_e8: u128,
    pub total_trader_locked_margin_e8: u128,
    pub total_in_flight_withdrawals_e8: u128,
    pub lp_vault_realized_equity_e8: u128,
    pub insurance_fund_e8: u128,
    pub protocol_fees_e8: u128,
}

impl ContractCashBuckets {
    /// Total accounted internal liabilities + equity in `D8{rUSD}`.
    pub fn total_accounted_e8(&self) -> Result<u128, PerpMathError> {
        self.total_trader_free_collateral_e8
            .checked_add(self.total_trader_locked_margin_e8)
            .and_then(|v| v.checked_add(self.total_in_flight_withdrawals_e8))
            .and_then(|v| v.checked_add(self.lp_vault_realized_equity_e8))
            .and_then(|v| v.checked_add(self.insurance_fund_e8))
            .and_then(|v| v.checked_add(self.protocol_fees_e8))
            .ok_or(PerpMathError::Overflow)
    }

    /// Check the realized solvency invariant: physical token balance covers all internal cash buckets,
    /// returning any unallocated external transfer surplus (`actual - accounted`).
    pub fn verify_solvency(&self, physical_token_balance_e8: u128) -> Result<u128, PerpMathError> {
        let accounted = self.total_accounted_e8()?;
        physical_token_balance_e8
            .checked_sub(accounted)
            .ok_or(PerpMathError::InsolvencyDetected {
                required_e8: accounted,
                actual_e8: physical_token_balance_e8,
            })
    }
}

// -----------------------------------------------------------------------------
// Typed Errors
// -----------------------------------------------------------------------------

/// Typed error set for shared Perpetuals arithmetic and validation helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PerpMathError {
    ZeroDenominator,
    ZeroPrice,
    ZeroSize,
    Overflow,
    Underflow,
    InvalidBps,
    InvalidMarginParams { reason: &'static str },
    RegressiveTimestamp,
    InsufficientMargin,
    InsufficientLiquidity,
    ZeroSharesResult,
    ZeroAssetsResult,
    OracleClassRejected,
    FutureTimestampDrift,
    InsolvencyDetected { required_e8: u128, actual_e8: u128 },
}

impl fmt::Display for PerpMathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDenominator => write!(f, "ERR_ZERO_DENOMINATOR: divisor cannot be zero"),
            Self::ZeroPrice => write!(f, "ERR_ZERO_PRICE: price must be greater than zero"),
            Self::ZeroSize => write!(f, "ERR_ZERO_SIZE: size must be greater than zero"),
            Self::Overflow => write!(f, "ERR_OVERFLOW: fixed-point integer overflow"),
            Self::Underflow => write!(f, "ERR_UNDERFLOW: fixed-point integer underflow"),
            Self::InvalidBps => write!(f, "ERR_INVALID_BPS: bps cannot exceed 10_000"),
            Self::InvalidMarginParams { reason } => {
                write!(f, "ERR_INVALID_MARGIN_PARAMS: {reason}")
            }
            Self::RegressiveTimestamp => {
                write!(
                    f,
                    "ERR_REGRESSIVE_TIMESTAMP: now_sec cannot precede last_sec"
                )
            }
            Self::InsufficientMargin => {
                write!(
                    f,
                    "ERR_INSUFFICIENT_MARGIN: collateral below required threshold"
                )
            }
            Self::InsufficientLiquidity => {
                write!(
                    f,
                    "ERR_INSUFFICIENT_LIQUIDITY: available LP liquidity exceeded"
                )
            }
            Self::ZeroSharesResult => {
                write!(f, "ERR_ZERO_SHARES: deposit rounds to zero LP shares")
            }
            Self::ZeroAssetsResult => {
                write!(f, "ERR_ZERO_ASSETS: share redemption rounds to zero assets")
            }
            Self::OracleClassRejected => {
                write!(
                    f,
                    "ERR_ORACLE_CLASS_REJECTED: oracle data_class cannot execute perpetuals"
                )
            }
            Self::FutureTimestampDrift => {
                write!(
                    f,
                    "ERR_FUTURE_TIMESTAMP: oracle observation exceeds allowed clock drift"
                )
            }
            Self::InsolvencyDetected {
                required_e8,
                actual_e8,
            } => write!(
                f,
                "ERR_INSOLVENT: accounted buckets ({required_e8}) exceed balance ({actual_e8})"
            ),
        }
    }
}

impl std::error::Error for PerpMathError {}

// -----------------------------------------------------------------------------
// Dependency-Free 256-Bit Intermediate Arithmetic Primitives
// -----------------------------------------------------------------------------

/// Compute exact 256-bit unsigned product `(hi, lo) = a * b`.
#[inline]
fn mul_wide_u128(a: u128, b: u128) -> (u128, u128) {
    let a_lo = a & 0xFFFF_FFFF_FFFF_FFFF;
    let a_hi = a >> 64;
    let b_lo = b & 0xFFFF_FFFF_FFFF_FFFF;
    let b_hi = b >> 64;

    let p0 = a_lo * b_lo;
    let p1 = a_lo * b_hi;
    let p2 = a_hi * b_lo;
    let p3 = a_hi * b_hi;

    let mid = (p0 >> 64) + (p1 & 0xFFFF_FFFF_FFFF_FFFF) + (p2 & 0xFFFF_FFFF_FFFF_FFFF);
    let lo = (p0 & 0xFFFF_FFFF_FFFF_FFFF) | (mid << 64);
    let hi = p3 + (p1 >> 64) + (p2 >> 64) + (mid >> 64);
    (hi, lo)
}

/// Divide 256-bit unsigned `(hi, lo)` by `denom` returning `(quotient, remainder)`.
///
/// Fails with [`PerpMathError::ZeroDenominator`] when `denom == 0`, and [`PerpMathError::Overflow`]
/// when the mathematical quotient exceeds `u128::MAX` (`hi >= denom`).
fn div_rem_256_by_128(hi: u128, lo: u128, denom: u128) -> Result<(u128, u128), PerpMathError> {
    if denom == 0 {
        return Err(PerpMathError::ZeroDenominator);
    }
    if hi >= denom {
        return Err(PerpMathError::Overflow);
    }
    if hi == 0 {
        return Ok((lo / denom, lo % denom));
    }

    let mut rem = hi;
    let mut quot = 0u128;

    for bit_idx in (0..128).rev() {
        let next_bit = (lo >> bit_idx) & 1;
        let carry = rem >> 127;
        rem = (rem << 1) | next_bit;

        if carry == 1 || rem >= denom {
            // Safe when `carry == 1` because `rem_before < denom` ensures `(2^128 + rem) - denom < denom <= u128::MAX`.
            rem = rem.wrapping_sub(denom);
            quot |= 1u128 << bit_idx;
        }
    }

    Ok((quot, rem))
}

/// Overflow-safe exact multiplication and floor division: `floor((a * b) / denom)`.
///
/// Uses pure 256-bit intermediates without external crate dependencies. Fails closed if `denom == 0`
/// or if the final mathematical quotient exceeds `u128::MAX`.
pub fn checked_mul_div_floor_u128(a: u128, b: u128, denom: u128) -> Result<u128, PerpMathError> {
    if denom == 0 {
        return Err(PerpMathError::ZeroDenominator);
    }
    if a == 0 || b == 0 {
        return Ok(0);
    }
    if let Some(prod) = a.checked_mul(b) {
        return Ok(prod / denom);
    }
    let (hi, lo) = mul_wide_u128(a, b);
    let (q, _) = div_rem_256_by_128(hi, lo, denom)?;
    Ok(q)
}

/// Overflow-safe exact multiplication and ceiling division: `ceil((a * b) / denom)`.
///
/// Rounds **up** whenever `(a * b) % denom != 0`. Never wraps or saturates on overflow.
pub fn checked_mul_div_ceil_u128(a: u128, b: u128, denom: u128) -> Result<u128, PerpMathError> {
    if denom == 0 {
        return Err(PerpMathError::ZeroDenominator);
    }
    if a == 0 || b == 0 {
        return Ok(0);
    }
    let (q, r) = if let Some(prod) = a.checked_mul(b) {
        (prod / denom, prod % denom)
    } else {
        let (hi, lo) = mul_wide_u128(a, b);
        div_rem_256_by_128(hi, lo, denom)?
    };
    if r > 0 {
        q.checked_add(1).ok_or(PerpMathError::Overflow)
    } else {
        Ok(q)
    }
}

/// Unsigned ceiling division: `ceil(numerator / denominator)`.
pub fn ceil_div_u128(numerator: u128, denominator: u128) -> Result<u128, PerpMathError> {
    if denominator == 0 {
        return Err(PerpMathError::ZeroDenominator);
    }
    let q = numerator / denominator;
    let r = numerator % denominator;
    if r > 0 {
        q.checked_add(1).ok_or(PerpMathError::Overflow)
    } else {
        Ok(q)
    }
}

/// Signed integer division rounding strictly toward negative infinity (`floor(n / d)`),
/// rather than Rust's `/` which truncates toward zero.
///
/// Used so positive gains round down (`+10.9 -> +10`) and negative losses round more
/// negative (`-10.1 -> -11`), ensuring matched positions can never extract rounding dust.
pub fn floor_div_i128(numerator: i128, denominator: i128) -> Result<i128, PerpMathError> {
    if denominator == 0 {
        return Err(PerpMathError::ZeroDenominator);
    }
    if numerator == i128::MIN && denominator == -1 {
        return Err(PerpMathError::Overflow);
    }
    let q = numerator / denominator;
    let r = numerator % denominator;
    if r != 0 && ((numerator < 0) ^ (denominator < 0)) {
        q.checked_sub(1).ok_or(PerpMathError::Overflow)
    } else {
        Ok(q)
    }
}

/// Exact signed `floor((a * b) / denom)` rounding strictly toward negative infinity (`denom > 0`).
///
/// Uses 256-bit unsigned magnitude arithmetic via `unsigned_abs()`, preventing `i128::MIN`
/// negation panics and premature intermediate overflows.
pub fn checked_signed_mul_div_floor_i128(
    a: i128,
    b: i128,
    denom: u128,
) -> Result<i128, PerpMathError> {
    if denom == 0 {
        return Err(PerpMathError::ZeroDenominator);
    }
    if a == 0 || b == 0 {
        return Ok(0);
    }

    let negative = (a < 0) ^ (b < 0);
    let a_abs = a.unsigned_abs();
    let b_abs = b.unsigned_abs();

    let (q, r) = if let Some(prod) = a_abs.checked_mul(b_abs) {
        (prod / denom, prod % denom)
    } else {
        let (hi, lo) = mul_wide_u128(a_abs, b_abs);
        div_rem_256_by_128(hi, lo, denom)?
    };

    if !negative {
        i128::try_from(q).map_err(|_| PerpMathError::Overflow)
    } else {
        let mag = if r > 0 {
            q.checked_add(1).ok_or(PerpMathError::Overflow)?
        } else {
            q
        };
        let min_mag = i128::MIN.unsigned_abs();
        if mag == min_mag {
            Ok(i128::MIN)
        } else if mag < min_mag {
            Ok(-(mag as i128))
        } else {
            Err(PerpMathError::Overflow)
        }
    }
}

// -----------------------------------------------------------------------------
// Notional, PnL, Margin, Fees & Liquidation Math
// -----------------------------------------------------------------------------

/// Compute floor position notional in `D8{rUSD}`: `floor(price_e8 * size_e8 / PRICE_SCALE)`.
pub fn compute_notional_floor_usd_e8(price_e8: u128, size_e8: u128) -> Result<u128, PerpMathError> {
    if price_e8 == 0 {
        return Err(PerpMathError::ZeroPrice);
    }
    checked_mul_div_floor_u128(price_e8, size_e8, PRICE_SCALE)
}

/// Compute conservative ceiling position notional in `D8{rUSD}`: `ceil(price_e8 * size_e8 / PRICE_SCALE)`.
///
/// Preferred when computing required margin (`IM`, `MM`) or trading fees so fractional dust
/// never reduces collateral requirements.
pub fn compute_notional_ceil_usd_e8(price_e8: u128, size_e8: u128) -> Result<u128, PerpMathError> {
    if price_e8 == 0 {
        return Err(PerpMathError::ZeroPrice);
    }
    checked_mul_div_ceil_u128(price_e8, size_e8, PRICE_SCALE)
}

/// Compute signed unrealized PnL in `D8{rUSD}` using conservative rounding toward `-infinity`.
///
/// - **Units / Scales**: `size_e8` (`D8{kg}`), `entry_price_e8` / `mark_price_e8` (`D8{USD/kg}`)
///   -> returns `i128` (`D8{rUSD}`).
/// - **Rounding**: positive trader gains round down (`floor`); negative trader losses round
///   more negative (`-ceil(|loss|)`).
/// - **Zero-Sum Symmetry**: For equal `(size, entry, mark)`, `pnl_long + pnl_short` equals `0`
///   when exact, or `-1` (`<= 0`) when fractional remainder exists — never creates phantom value.
pub fn compute_unrealized_pnl_usd_e8(
    side: PositionSide,
    size_e8: u128,
    entry_price_e8: u128,
    mark_price_e8: u128,
) -> Result<i128, PerpMathError> {
    if entry_price_e8 == 0 || mark_price_e8 == 0 {
        return Err(PerpMathError::ZeroPrice);
    }
    if size_e8 == 0 || entry_price_e8 == mark_price_e8 {
        return Ok(0);
    }

    let price_diff = mark_price_e8.abs_diff(entry_price_e8);
    let is_profit = match side {
        PositionSide::Long => mark_price_e8 > entry_price_e8,
        PositionSide::Short => mark_price_e8 < entry_price_e8,
    };

    if is_profit {
        let gain = checked_mul_div_floor_u128(size_e8, price_diff, PRICE_SCALE)?;
        i128::try_from(gain).map_err(|_| PerpMathError::Overflow)
    } else {
        let loss_mag = checked_mul_div_ceil_u128(size_e8, price_diff, PRICE_SCALE)?;
        let min_mag = i128::MIN.unsigned_abs();
        if loss_mag == min_mag {
            Ok(i128::MIN)
        } else if loss_mag < min_mag {
            Ok(-(loss_mag as i128))
        } else {
            Err(PerpMathError::Overflow)
        }
    }
}

/// Compute bounded signed MTM net trader claim against LP Vault:
/// `max(-locked_margin_e8, raw_pnl_e8 + funding_net_e8)`.
///
/// Under Isolated Margin, the LP Vault can never recover more than `locked_margin_e8` from a
/// losing position. Clamping losses at `-locked_margin_e8` prevents phantom recovery above
/// posted margin from inflating LP Vault NAV during gap moves.
pub fn compute_capped_trader_net_claim_usd_e8(
    locked_margin_e8: u128,
    raw_pnl_e8: i128,
    funding_net_e8: i128,
) -> Result<i128, PerpMathError> {
    let margin_i128 = i128::try_from(locked_margin_e8).map_err(|_| PerpMathError::Overflow)?;
    let total_mtm = raw_pnl_e8
        .checked_add(funding_net_e8)
        .ok_or(PerpMathError::Overflow)?;
    let neg_margin = margin_i128.checked_neg().ok_or(PerpMathError::Overflow)?;
    Ok(total_mtm.max(neg_margin))
}

/// Compute Initial Margin required in `D8{rUSD}` (ceiling division).
pub fn compute_initial_margin_usd_e8(
    notional_usd_e8: u128,
    im_bps: u32,
) -> Result<u128, PerpMathError> {
    if im_bps == 0 || (im_bps as u128) > BPS_SCALE {
        return Err(PerpMathError::InvalidBps);
    }
    checked_mul_div_ceil_u128(notional_usd_e8, im_bps as u128, BPS_SCALE)
}

/// Compute Maintenance Margin required in `D8{rUSD}` (ceiling division).
pub fn compute_maintenance_margin_usd_e8(
    notional_usd_e8: u128,
    mm_bps: u32,
) -> Result<u128, PerpMathError> {
    if mm_bps == 0 || (mm_bps as u128) > BPS_SCALE {
        return Err(PerpMathError::InvalidBps);
    }
    checked_mul_div_ceil_u128(notional_usd_e8, mm_bps as u128, BPS_SCALE)
}

/// Compute trading fee in `D8{rUSD}` (ceiling division so small trades still pay `>= 1` wei when `fee_bps > 0` and `notional > 0`).
pub fn compute_trading_fee_usd_e8(
    notional_usd_e8: u128,
    fee_bps: u32,
) -> Result<u128, PerpMathError> {
    if (fee_bps as u128) > BPS_SCALE {
        return Err(PerpMathError::InvalidBps);
    }
    if fee_bps == 0 || notional_usd_e8 == 0 {
        return Ok(0);
    }
    checked_mul_div_ceil_u128(notional_usd_e8, fee_bps as u128, BPS_SCALE)
}

/// Compute current signed position equity (`locked_margin + pnl + net_funding`) in `D8{rUSD}`.
pub fn compute_position_equity_usd_e8(
    locked_margin_e8: u128,
    pnl_e8: i128,
    funding_net_e8: i128,
) -> Result<i128, PerpMathError> {
    let margin_i128 = i128::try_from(locked_margin_e8).map_err(|_| PerpMathError::Overflow)?;
    margin_i128
        .checked_add(pnl_e8)
        .and_then(|v| v.checked_add(funding_net_e8))
        .ok_or(PerpMathError::Overflow)
}

/// Compute current margin ratio in `bps` (`floor(max(0, equity) * 10_000 / mark_notional)`).
///
/// Returns `0` when `equity_usd_e8 <= 0`. Rejects `mark_notional_usd_e8 == 0`.
pub fn compute_margin_ratio_bps(
    equity_usd_e8: i128,
    mark_notional_usd_e8: u128,
) -> Result<u128, PerpMathError> {
    if mark_notional_usd_e8 == 0 {
        return Err(PerpMathError::ZeroDenominator);
    }
    if equity_usd_e8 <= 0 {
        return Ok(0);
    }
    checked_mul_div_floor_u128(equity_usd_e8 as u128, BPS_SCALE, mark_notional_usd_e8)
}

/// Compute safely withdrawable isolated margin from an open position (`D8{rUSD}`).
///
/// Bounded by BOTH:
/// 1. `locked_margin_e8 - entry_im_e8` (traders cannot extract unrealized profit without realizing/closing), and
/// 2. `mark_equity_e8 - mark_im_e8` (post-removal equity must remain `>= mark_im_e8`).
pub fn compute_available_margin_usd_e8(
    locked_margin_e8: u128,
    entry_im_e8: u128,
    mark_equity_e8: i128,
    mark_im_e8: u128,
) -> Result<u128, PerpMathError> {
    let from_locked = locked_margin_e8.saturating_sub(entry_im_e8);
    let mark_im_i128 = i128::try_from(mark_im_e8).map_err(|_| PerpMathError::Overflow)?;
    let from_equity = mark_equity_e8
        .checked_sub(mark_im_i128)
        .ok_or(PerpMathError::Overflow)?;
    if from_equity <= 0 {
        return Ok(0);
    }
    Ok(from_locked.min(from_equity as u128))
}

/// Check whether a position is eligible for liquidation (`equity_e8 <= maintenance_margin_e8`).
pub fn is_liquidatable(
    equity_usd_e8: i128,
    mark_notional_usd_e8: u128,
    mm_bps: u32,
) -> Result<bool, PerpMathError> {
    let mm_e8 = compute_maintenance_margin_usd_e8(mark_notional_usd_e8, mm_bps)?;
    let mm_i128 = i128::try_from(mm_e8).map_err(|_| PerpMathError::Overflow)?;
    Ok(equity_usd_e8 <= mm_i128)
}

/// Compute complete liquidation settlement decomposition from validated mark price and risk params.
///
/// Strictly conserves margin: all liquidation fees are drawn **only** from remaining positive
/// equity, never fabricated on underwater positions (`bad_debt_e8 > 0`).
pub fn compute_liquidation_settlement(
    locked_margin_e8: u128,
    raw_pnl_e8: i128,
    funding_net_e8: i128,
    mark_price_e8: u128,
    size_e8: u128,
    params: &MarketRiskParams,
) -> Result<LiquidationSettlement, PerpMathError> {
    params.validate()?;
    if size_e8 == 0 {
        return Err(PerpMathError::ZeroSize);
    }
    let mark_notional_floor = compute_notional_floor_usd_e8(mark_price_e8, size_e8)?;
    let mark_notional_ceil = compute_notional_ceil_usd_e8(mark_price_e8, size_e8)?;
    let mm_e8 =
        compute_maintenance_margin_usd_e8(mark_notional_ceil, params.maintenance_margin_bps)?;
    let eq_e8 = compute_position_equity_usd_e8(locked_margin_e8, raw_pnl_e8, funding_net_e8)?;
    let mm_i128 = i128::try_from(mm_e8).map_err(|_| PerpMathError::Overflow)?;
    let liquidatable = eq_e8 <= mm_i128;

    if !liquidatable {
        return Ok(LiquidationSettlement {
            mark_notional_e8: mark_notional_floor,
            equity_before_close_e8: eq_e8,
            maintenance_margin_e8: mm_e8,
            liquidatable: false,
            liq_keeper_fee_paid_e8: 0,
            liq_insurance_fee_paid_e8: 0,
            close_fee_paid_e8: 0,
            trader_remaining_margin_e8: eq_e8.max(0) as u128,
            bad_debt_e8: 0,
        });
    }

    let bad_debt_e8 = if eq_e8 < 0 { eq_e8.unsigned_abs() } else { 0 };
    let mut rem = if eq_e8 > 0 { eq_e8 as u128 } else { 0 };

    let desired_keeper = compute_trading_fee_usd_e8(mark_notional_ceil, params.liq_keeper_fee_bps)?;
    let desired_ins = compute_trading_fee_usd_e8(mark_notional_ceil, params.liq_insurance_fee_bps)?;
    let desired_close = compute_trading_fee_usd_e8(mark_notional_ceil, params.trade_fee_bps)?;

    let liq_keeper_fee_paid_e8 = rem.min(desired_keeper);
    rem -= liq_keeper_fee_paid_e8;

    let liq_insurance_fee_paid_e8 = rem.min(desired_ins);
    rem -= liq_insurance_fee_paid_e8;

    let close_fee_paid_e8 = rem.min(desired_close);
    rem -= close_fee_paid_e8;

    Ok(LiquidationSettlement {
        mark_notional_e8: mark_notional_floor,
        equity_before_close_e8: eq_e8,
        maintenance_margin_e8: mm_e8,
        liquidatable: true,
        liq_keeper_fee_paid_e8,
        liq_insurance_fee_paid_e8,
        close_fee_paid_e8,
        trader_remaining_margin_e8: rem,
        bad_debt_e8,
    })
}

/// Apply an explicit maximum positive profit cap in `bps` of entry notional (`D8{rUSD}`).
///
/// Prevents unbounded Long tail payouts (`> 100%` of reserved entry notional when `max_pnl_bps = 10_000`),
/// strictly bounding LP Vault exposure alongside utilization limits. Negative PnL is never capped here.
pub fn apply_profit_cap_usd_e8(
    raw_pnl_e8: i128,
    entry_notional_e8: u128,
    max_pnl_bps: Option<u32>,
) -> Result<i128, PerpMathError> {
    if raw_pnl_e8 <= 0 {
        return Ok(raw_pnl_e8);
    }
    match max_pnl_bps {
        None => Ok(raw_pnl_e8),
        Some(cap_bps) => {
            if cap_bps == 0 {
                return Err(PerpMathError::InvalidBps);
            }
            let max_gain =
                checked_mul_div_floor_u128(entry_notional_e8, cap_bps as u128, BPS_SCALE)?;
            let max_gain_i128 = i128::try_from(max_gain).map_err(|_| PerpMathError::Overflow)?;
            Ok(raw_pnl_e8.min(max_gain_i128))
        }
    }
}

/// Compact position snapshot needed for deterministic settlement (normal close or emergency).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionSnapshot {
    pub side: PositionSide,
    pub size_e8: u128,
    pub locked_margin_e8: u128,
    pub entry_price_e8: u128,
    pub entry_funding_index_e12: i128,
}

/// Outcome of uniform settlement for either normal close or frozen-snapshot Emergency Settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniformPositionSettlement {
    pub settlement_pnl_e8: i128,
    pub settlement_funding_e8: i128,
    pub fee_e8: u128,
    pub net_equity_e8: i128,
    pub trader_payout_e8: u128,
    pub vault_delta_e8: i128,
    pub bad_debt_e8: u128,
}

/// Settle a position cleanly at a given price and accrued funding index (used identically for
/// normal closes and uniform global Emergency Settlement at frozen `P_last_valid`).
///
/// Guarantees exact cash conservation: `locked_margin_e8 - trader_payout_e8 - fee_e8` equals
/// the net transfer between trader collateral and LP Vault (+ bad debt when equity is negative),
/// preventing double-counting or phantom money creation.
pub fn compute_position_settlement_e8(
    position: &PositionSnapshot,
    settlement_price_e8: u128,
    settlement_funding_index_e12: i128,
    max_pnl_bps: Option<u32>,
    close_fee_bps: u32,
) -> Result<UniformPositionSettlement, PerpMathError> {
    if position.size_e8 == 0 {
        return Err(PerpMathError::ZeroSize);
    }
    let entry_notional = compute_notional_floor_usd_e8(position.entry_price_e8, position.size_e8)?;
    let raw_pnl = compute_unrealized_pnl_usd_e8(
        position.side,
        position.size_e8,
        position.entry_price_e8,
        settlement_price_e8,
    )?;
    let capped_pnl = apply_profit_cap_usd_e8(raw_pnl, entry_notional, max_pnl_bps)?;
    let funding_net = compute_position_funding_net_usd_e8(
        position.side,
        position.size_e8,
        position.entry_funding_index_e12,
        settlement_funding_index_e12,
    )?;

    let equity_before_fee =
        compute_position_equity_usd_e8(position.locked_margin_e8, capped_pnl, funding_net)?;
    let settle_notional_ceil = compute_notional_ceil_usd_e8(settlement_price_e8, position.size_e8)?;
    let desired_fee = compute_trading_fee_usd_e8(settle_notional_ceil, close_fee_bps)?;
    let fee_i128 = i128::try_from(desired_fee).map_err(|_| PerpMathError::Overflow)?;

    let equity_after_fee = equity_before_fee
        .checked_sub(fee_i128)
        .ok_or(PerpMathError::Overflow)?;

    if equity_before_fee <= 0 {
        // Already underwater before close fee: trader receives 0, bad_debt = -equity_before_fee, zero unbacked fee fabricated.
        let bad_debt_e8 = equity_before_fee.unsigned_abs();
        let vault_delta_e8 =
            i128::try_from(position.locked_margin_e8).map_err(|_| PerpMathError::Overflow)?;
        Ok(UniformPositionSettlement {
            settlement_pnl_e8: capped_pnl,
            settlement_funding_e8: funding_net,
            fee_e8: 0,
            net_equity_e8: equity_before_fee,
            trader_payout_e8: 0,
            vault_delta_e8,
            bad_debt_e8,
        })
    } else if equity_after_fee <= 0 {
        // Positive equity absorbed fully by close fee (fee capped at equity_before_fee; zero bad debt created).
        let fee_paid = equity_before_fee as u128;
        let margin_i128 =
            i128::try_from(position.locked_margin_e8).map_err(|_| PerpMathError::Overflow)?;
        let fee_paid_i128 = i128::try_from(fee_paid).map_err(|_| PerpMathError::Overflow)?;
        let vault_delta_e8 = margin_i128
            .checked_sub(fee_paid_i128)
            .ok_or(PerpMathError::Overflow)?;
        Ok(UniformPositionSettlement {
            settlement_pnl_e8: capped_pnl,
            settlement_funding_e8: funding_net,
            fee_e8: fee_paid,
            net_equity_e8: 0,
            trader_payout_e8: 0,
            vault_delta_e8,
            bad_debt_e8: 0,
        })
    } else {
        let trader_payout_e8 = equity_after_fee as u128;
        let margin_i128 =
            i128::try_from(position.locked_margin_e8).map_err(|_| PerpMathError::Overflow)?;
        // LP receives `locked_margin - trader_payout - fee_e8` (= `-capped_pnl - funding_net`)
        let payout_plus_fee = trader_payout_e8
            .checked_add(desired_fee)
            .ok_or(PerpMathError::Overflow)?;
        let payout_plus_fee_i128 =
            i128::try_from(payout_plus_fee).map_err(|_| PerpMathError::Overflow)?;
        let vault_delta_e8 = margin_i128
            .checked_sub(payout_plus_fee_i128)
            .ok_or(PerpMathError::Overflow)?;
        Ok(UniformPositionSettlement {
            settlement_pnl_e8: capped_pnl,
            settlement_funding_e8: funding_net,
            fee_e8: desired_fee,
            net_equity_e8: equity_after_fee,
            trader_payout_e8,
            vault_delta_e8,
            bad_debt_e8: 0,
        })
    }
}

/// Indicative UI liquidation price estimate incorporating accrued `funding_net_e8`.
///
/// **SECURITY INVARIANT:** This value is for UX display only and MUST NEVER replace exact
/// on-chain [`is_liquidatable`] / [`compute_liquidation_settlement`] recomputation.
pub fn compute_indicative_liquidation_price_e8(
    side: PositionSide,
    size_e8: u128,
    entry_price_e8: u128,
    locked_margin_e8: u128,
    funding_net_e8: i128,
    mm_bps: u32,
) -> Result<u128, PerpMathError> {
    if entry_price_e8 == 0 {
        return Err(PerpMathError::ZeroPrice);
    }
    if size_e8 == 0 {
        return Err(PerpMathError::ZeroSize);
    }
    if mm_bps == 0 || (mm_bps as u128) >= BPS_SCALE {
        return Err(PerpMathError::InvalidBps);
    }
    let margin_i128 = i128::try_from(locked_margin_e8).map_err(|_| PerpMathError::Overflow)?;
    let effective_margin = margin_i128
        .checked_add(funding_net_e8)
        .ok_or(PerpMathError::Overflow)?;

    let margin_abs_per_kg =
        checked_mul_div_floor_u128(effective_margin.unsigned_abs(), SIZE_SCALE, size_e8)?;

    match side {
        PositionSide::Long => {
            if effective_margin >= 0 {
                if entry_price_e8 <= margin_abs_per_kg {
                    return Ok(0);
                }
                let num = entry_price_e8 - margin_abs_per_kg;
                checked_mul_div_ceil_u128(num, BPS_SCALE, BPS_SCALE - (mm_bps as u128))
            } else {
                let num = entry_price_e8
                    .checked_add(margin_abs_per_kg)
                    .ok_or(PerpMathError::Overflow)?;
                checked_mul_div_ceil_u128(num, BPS_SCALE, BPS_SCALE - (mm_bps as u128))
            }
        }
        PositionSide::Short => {
            if effective_margin >= 0 {
                let num = entry_price_e8
                    .checked_add(margin_abs_per_kg)
                    .ok_or(PerpMathError::Overflow)?;
                checked_mul_div_floor_u128(num, BPS_SCALE, BPS_SCALE + (mm_bps as u128))
            } else {
                if entry_price_e8 <= margin_abs_per_kg {
                    return Ok(0);
                }
                let num = entry_price_e8 - margin_abs_per_kg;
                checked_mul_div_floor_u128(num, BPS_SCALE, BPS_SCALE + (mm_bps as u128))
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Funding Rate & Cumulative Funding Index Primitives
// -----------------------------------------------------------------------------

/// Compute verified non-negative elapsed accrual duration, rejecting regressive timestamps
/// and optionally clamping runaway intervals (`max_accrual_window_sec > 0`).
pub fn advance_funding_elapsed_sec(
    last_funding_sec: u64,
    now_sec: u64,
    max_accrual_window_sec: u64,
) -> Result<u64, PerpMathError> {
    if now_sec < last_funding_sec {
        return Err(PerpMathError::RegressiveTimestamp);
    }
    let raw_dt = now_sec - last_funding_sec;
    if max_accrual_window_sec > 0 {
        Ok(raw_dt.min(max_accrual_window_sec))
    } else {
        Ok(raw_dt)
    }
}

/// Compute skew-weighted signed per-second funding rate in `D12{1/s}` (`FUNDING_SCALE = 10^12`).
///
/// Sign convention:
/// - `> 0` when `long_oi_e8 > short_oi_e8` (Longs pay Shorts & LP Vault).
/// - `< 0` when `short_oi_e8 > long_oi_e8` (Shorts pay Longs & LP Vault).
/// - `0` when `long_oi_e8 == short_oi_e8` (zero skew).
///
/// Magnitude is clamped at `max_hourly_rate_e12 / 3_600` to prevent extreme skew spikes.
pub fn compute_funding_rate_per_sec_e12(
    long_oi_e8: u128,
    short_oi_e8: u128,
    lp_equity_e8: u128,
    max_hourly_rate_e12: u128,
    min_skew_base_e8: u128,
) -> Result<i128, PerpMathError> {
    let skew_denom = lp_equity_e8.max(min_skew_base_e8);
    if skew_denom == 0 {
        return Err(PerpMathError::ZeroDenominator);
    }
    if long_oi_e8 == short_oi_e8 || max_hourly_rate_e12 == 0 {
        return Ok(0);
    }

    let abs_skew = long_oi_e8.abs_diff(short_oi_e8);
    let max_sec_rate = max_hourly_rate_e12 / (SECONDS_PER_HOUR as u128);
    let denom = skew_denom
        .checked_mul(SECONDS_PER_HOUR as u128)
        .ok_or(PerpMathError::Overflow)?;
    let raw_rate = checked_mul_div_floor_u128(abs_skew, max_hourly_rate_e12, denom)?;
    let clamped_mag = raw_rate.min(max_sec_rate);
    let mag_i128 = i128::try_from(clamped_mag).map_err(|_| PerpMathError::Overflow)?;

    if long_oi_e8 > short_oi_e8 {
        Ok(mag_i128)
    } else {
        Ok(-mag_i128)
    }
}

/// Compute cumulative funding index delta (`D12{USD/kg}`) symmetrically from explicit `rate_per_sec_e12`
/// (`sign * floor(|index_price * rate * dt| / PRICE_SCALE)`), preventing sign asymmetry.
pub fn compute_funding_index_delta_e12(
    index_price_e8: u128,
    rate_per_sec_e12: i128,
    elapsed_sec: u64,
) -> Result<i128, PerpMathError> {
    if index_price_e8 == 0 {
        return Err(PerpMathError::ZeroPrice);
    }
    if rate_per_sec_e12 == 0 || elapsed_sec == 0 {
        return Ok(0);
    }
    let rate_mag = rate_per_sec_e12.unsigned_abs();
    let rate_dt = rate_mag
        .checked_mul(elapsed_sec as u128)
        .ok_or(PerpMathError::Overflow)?;
    let mag = checked_mul_div_floor_u128(index_price_e8, rate_dt, PRICE_SCALE)?;
    let mag_i128 = i128::try_from(mag).map_err(|_| PerpMathError::Overflow)?;
    if rate_per_sec_e12 < 0 {
        Ok(-mag_i128)
    } else {
        Ok(mag_i128)
    }
}

/// Recommended high-precision interval funding accrual (`D12{USD/kg}`) avoiding `/ 3600` pre-truncation loss.
///
/// Computes hourly skew rate clamped at `max_hourly_rate_e12`, scales across `(index_price_e8 * elapsed_sec)`,
/// and divides cleanly by `(SECONDS_PER_HOUR * PRICE_SCALE)` with exact sign symmetry.
pub fn compute_funding_delta_over_interval_e12(
    long_oi_e8: u128,
    short_oi_e8: u128,
    lp_equity_e8: u128,
    index_price_e8: u128,
    elapsed_sec: u64,
    max_hourly_rate_e12: u128,
    min_skew_base_e8: u128,
) -> Result<i128, PerpMathError> {
    if index_price_e8 == 0 {
        return Err(PerpMathError::ZeroPrice);
    }
    let skew_denom = lp_equity_e8.max(min_skew_base_e8);
    if skew_denom == 0 {
        return Err(PerpMathError::ZeroDenominator);
    }
    if long_oi_e8 == short_oi_e8 || elapsed_sec == 0 || max_hourly_rate_e12 == 0 {
        return Ok(0);
    }
    let abs_skew = long_oi_e8.abs_diff(short_oi_e8).min(skew_denom);
    let hourly_rate_e12 = checked_mul_div_floor_u128(abs_skew, max_hourly_rate_e12, skew_denom)?;
    let rate_times_dt = hourly_rate_e12
        .checked_mul(elapsed_sec as u128)
        .ok_or(PerpMathError::Overflow)?;
    let denom = (SECONDS_PER_HOUR as u128)
        .checked_mul(PRICE_SCALE)
        .ok_or(PerpMathError::Overflow)?;
    let mag = checked_mul_div_floor_u128(index_price_e8, rate_times_dt, denom)?;
    let mag_i128 = i128::try_from(mag).map_err(|_| PerpMathError::Overflow)?;
    if long_oi_e8 > short_oi_e8 {
        Ok(mag_i128)
    } else {
        Ok(-mag_i128)
    }
}

/// Compute signed net funding payment (`D8{rUSD}`) for a position since its `entry_funding_index_e12`.
///
/// Sign convention:
/// - **Positive**: trader **receives** funding (increases equity).
/// - **Negative**: trader **pays** funding (decreases equity).
///
/// Rounded strictly toward `-infinity`, guaranteeing `long_net + short_net <= 0` for matched sizes.
pub fn compute_position_funding_net_usd_e8(
    side: PositionSide,
    size_e8: u128,
    entry_funding_index_e12: i128,
    current_funding_index_e12: i128,
) -> Result<i128, PerpMathError> {
    if size_e8 == 0 || entry_funding_index_e12 == current_funding_index_e12 {
        return Ok(0);
    }
    let delta_index = current_funding_index_e12
        .checked_sub(entry_funding_index_e12)
        .ok_or(PerpMathError::Overflow)?;
    let size_i128 = i128::try_from(size_e8).map_err(|_| PerpMathError::Overflow)?;

    let signed_delta = match side {
        // Positive `delta_index` means Longs pay (`-delta`), Shorts receive (`+delta`).
        PositionSide::Long => delta_index.checked_neg().ok_or(PerpMathError::Overflow)?,
        PositionSide::Short => delta_index,
    };

    checked_signed_mul_div_floor_i128(size_i128, signed_delta, FUNDING_SCALE)
}

// -----------------------------------------------------------------------------
// LP Vault NAV, Reserved Liquidity, Utilization & Bad Debt Waterfall
// -----------------------------------------------------------------------------

/// Compute Mark-to-Market LP Vault Equity (`D8{rUSD}`) excluding pre-liquidation insurance reimbursement:
/// `lp_realized_equity_e8 - net_bounded_trader_mtm_claim_e8`.
pub fn compute_lp_mtm_equity_e8(
    lp_realized_equity_e8: u128,
    net_bounded_trader_mtm_claim_e8: i128,
) -> Result<i128, PerpMathError> {
    let realized_i128 =
        i128::try_from(lp_realized_equity_e8).map_err(|_| PerpMathError::Overflow)?;
    realized_i128
        .checked_sub(net_bounded_trader_mtm_claim_e8)
        .ok_or(PerpMathError::Overflow)
}

/// Compute Mark-to-Market LP Vault Equity adjusted for expected Insurance Fund coverage of
/// open underwater bad debt (`D8{rUSD}`):
/// `lp_realized - net_bounded_trader_mtm + min(insurance_fund, sum_unrealized_bad_debt)`.
///
/// Defeats pre-liquidation LP share dilution arbitrage where an attacker deposits right before
/// an underwater position is liquidated to capture `Insurance -> LP Vault` reimbursement!
pub fn compute_lp_mtm_equity_with_insurance_offset_e8(
    lp_realized_equity_e8: u128,
    net_bounded_trader_mtm_claim_e8: i128,
    total_unrealized_bad_debt_e8: u128,
    insurance_fund_e8: u128,
) -> Result<(i128, u128), PerpMathError> {
    let base_mtm =
        compute_lp_mtm_equity_e8(lp_realized_equity_e8, net_bounded_trader_mtm_claim_e8)?;
    let covered_offset = total_unrealized_bad_debt_e8.min(insurance_fund_e8);
    let offset_i128 = i128::try_from(covered_offset).map_err(|_| PerpMathError::Overflow)?;
    let adjusted = base_mtm
        .checked_add(offset_i128)
        .ok_or(PerpMathError::Overflow)?;
    Ok((adjusted, covered_offset))
}

/// Compute conservative LP liquidity reserve required against Gross Long & Short OI (`D8{rUSD}`):
/// `net_skew_oi + matched_oi * matched_gap_buffer_bps / BPS_SCALE` (ceiling division).
///
/// Ensures that even when Longs and Shorts net to zero skew, LP Vault reserves `matched_gap_buffer_bps`
/// across matched exposure to cover sudden gap insolvency on leveraged liquidations.
pub fn compute_reserved_liquidity_usd_e8(
    long_oi_e8: u128,
    short_oi_e8: u128,
    matched_gap_buffer_bps: u32,
) -> Result<u128, PerpMathError> {
    if (matched_gap_buffer_bps as u128) > BPS_SCALE {
        return Err(PerpMathError::InvalidBps);
    }
    let skew_oi = long_oi_e8.abs_diff(short_oi_e8);
    let matched_side = long_oi_e8.min(short_oi_e8);
    let two_matched = matched_side.checked_mul(2).ok_or(PerpMathError::Overflow)?;
    let matched_reserve =
        checked_mul_div_ceil_u128(two_matched, matched_gap_buffer_bps as u128, BPS_SCALE)?;
    skew_oi
        .checked_add(matched_reserve)
        .ok_or(PerpMathError::Overflow)
}

/// Compute actual withdrawable `rUSD` in LP Vault (`lp_realized_equity_e8 - max(reserved_oi, net_positive_trader_mtm)`).
///
/// Guarantees LPs can never withdraw cash required to pay winning open positions OR back OI reserves,
/// and never pay out unrealized losing-trader gains or pending Insurance offsets before cash realizes.
pub fn compute_lp_available_liquidity_usd_e8(
    lp_realized_equity_e8: u128,
    reserved_oi_e8: u128,
    net_bounded_trader_mtm_claim_e8: i128,
) -> u128 {
    let positive_trader_claim = if net_bounded_trader_mtm_claim_e8 > 0 {
        net_bounded_trader_mtm_claim_e8 as u128
    } else {
        0
    };
    let encumbered = reserved_oi_e8.max(positive_trader_claim);
    lp_realized_equity_e8.saturating_sub(encumbered)
}

/// Compute conservative LP utilization ratio in basis points (`ceil(reserved_e8 * 10_000 / effective_lp_equity_e8)`).
///
/// Returns `0` if `reserved_e8 == 0`, and saturates to `u32::MAX` (`> 10_000`) if `effective_lp_equity_e8 <= 0`
/// while exposure remains open (triggering utilization gate fail-closed).
pub fn compute_utilization_bps(
    reserved_e8: u128,
    effective_lp_equity_e8: i128,
) -> Result<u128, PerpMathError> {
    if reserved_e8 == 0 {
        return Ok(0);
    }
    if effective_lp_equity_e8 <= 0 {
        return Ok(u32::MAX as u128);
    }
    checked_mul_div_ceil_u128(reserved_e8, BPS_SCALE, effective_lp_equity_e8 as u128)
}

/// Output from an LP deposit share mint calculation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LpDepositShares {
    pub minted_to_depositor: u128,
    pub locked_seed_shares: u128,
    pub new_total_shares: u128,
}

/// Compute LP Vault shares minted on deposit (`D8{share}`), rounding **down**.
///
/// - On empty pool (`total_shares == 0`): requires `deposit_e8 >= MIN_FIRST_LP_DEPOSIT_USD_E8`
///   and permanently locks [`MINIMUM_LP_SEED_SHARES`] (`0.0001` shares) against share-inflation attacks.
/// - On existing pool (`total_shares > 0`): requires strictly positive `lp_mtm_equity_e8 > 0`
///   and `shares_out > 0`.
pub fn compute_lp_mint_shares(
    deposit_e8: u128,
    total_shares: u128,
    lp_mtm_equity_e8: i128,
) -> Result<LpDepositShares, PerpMathError> {
    if deposit_e8 == 0 {
        return Err(PerpMathError::ZeroSharesResult);
    }
    if total_shares == 0 {
        if deposit_e8 < MIN_FIRST_LP_DEPOSIT_USD_E8 {
            return Err(PerpMathError::InsufficientMargin);
        }
        let user_shares = deposit_e8
            .checked_sub(MINIMUM_LP_SEED_SHARES)
            .ok_or(PerpMathError::Underflow)?;
        if user_shares == 0 {
            return Err(PerpMathError::ZeroSharesResult);
        }
        return Ok(LpDepositShares {
            minted_to_depositor: user_shares,
            locked_seed_shares: MINIMUM_LP_SEED_SHARES,
            new_total_shares: deposit_e8,
        });
    }

    if lp_mtm_equity_e8 <= 0 {
        return Err(PerpMathError::InsufficientLiquidity);
    }

    let minted = checked_mul_div_floor_u128(deposit_e8, total_shares, lp_mtm_equity_e8 as u128)?;
    if minted == 0 {
        return Err(PerpMathError::ZeroSharesResult);
    }
    let new_total = total_shares
        .checked_add(minted)
        .ok_or(PerpMathError::Overflow)?;
    Ok(LpDepositShares {
        minted_to_depositor: minted,
        locked_seed_shares: 0,
        new_total_shares: new_total,
    })
}

/// Compute `rUSD` redeemed when an LP burns `shares_to_burn`, rounding **down** and enforcing
/// `assets_out <= lp_available_liquidity_e8`.
///
/// Rejects burning into permanently locked seed shares (`total_shares - shares_to_burn < MINIMUM_LP_SEED_SHARES` when `total_shares >= MINIMUM_LP_SEED_SHARES`).
pub fn compute_lp_redeem_assets(
    shares_to_burn: u128,
    total_shares: u128,
    lp_mtm_equity_e8: i128,
    lp_available_liquidity_e8: u128,
) -> Result<u128, PerpMathError> {
    if total_shares == 0 || shares_to_burn == 0 {
        return Err(PerpMathError::ZeroAssetsResult);
    }
    if shares_to_burn > total_shares {
        return Err(PerpMathError::Underflow);
    }
    if total_shares >= MINIMUM_LP_SEED_SHARES
        && total_shares - shares_to_burn < MINIMUM_LP_SEED_SHARES
    {
        return Err(PerpMathError::InsufficientLiquidity);
    }
    if lp_mtm_equity_e8 <= 0 {
        return Err(PerpMathError::InsufficientLiquidity);
    }

    let assets =
        checked_mul_div_floor_u128(shares_to_burn, lp_mtm_equity_e8 as u128, total_shares)?;
    if assets == 0 {
        return Err(PerpMathError::ZeroAssetsResult);
    }
    if assets > lp_available_liquidity_e8 {
        return Err(PerpMathError::InsufficientLiquidity);
    }
    Ok(assets)
}

/// Route realized bad debt across the loss waterfall:
/// `Insurance Fund -> LP Vault Realized Equity -> Insolvency Pause`.
///
/// Conserves total loss: `absorbed_by_insurance + absorbed_by_lp_vault + uncovered_insolvency == bad_debt_e8`.
pub fn apply_bad_debt_cascade(
    bad_debt_e8: u128,
    insurance_fund_e8: u128,
    lp_realized_equity_e8: u128,
) -> BadDebtCascadeOutcome {
    let absorbed_by_insurance_e8 = bad_debt_e8.min(insurance_fund_e8);
    let rem = bad_debt_e8 - absorbed_by_insurance_e8;
    let absorbed_by_lp_vault_e8 = rem.min(lp_realized_equity_e8);
    let uncovered_insolvency_e8 = rem - absorbed_by_lp_vault_e8;

    BadDebtCascadeOutcome {
        absorbed_by_insurance_e8,
        absorbed_by_lp_vault_e8,
        uncovered_insolvency_e8,
        insurance_depleted: rem > 0 && absorbed_by_insurance_e8 == insurance_fund_e8,
        should_pause_market: uncovered_insolvency_e8 > 0,
    }
}

// -----------------------------------------------------------------------------
// Oracle Feed Classification, Health Evaluation & Conservative Emergency Close
// -----------------------------------------------------------------------------

/// Map Oracle `data_class` + `worker_mode` into strict Perps execution tiers.
pub fn classify_oracle_feed(data_class: &str, worker_mode: &str) -> OracleFeedTier {
    match (data_class, worker_mode) {
        (DATA_CLASS_LIVE_VERIFIED | DATA_CLASS_LIVE_AGGREGATED, MODE_LIVE_INDEX) => {
            OracleFeedTier::ProductionLive
        }
        (DATA_CLASS_TESTNET_SANDBOX_FEED, MODE_TESTNET_SANDBOX) => OracleFeedTier::TestnetSandbox,
        (DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY, MODE_PUBLIC_BENCHMARK) => {
            OracleFeedTier::HistoricalBenchmark
        }
        (DATA_CLASS_SYNTHETIC, MODE_FIXTURE) => OracleFeedTier::SyntheticFixture,
        _ => OracleFeedTier::Unknown,
    }
}

/// Evaluate an Oracle observation for Perpetuals trading / liquidation eligibility.
///
/// **Fail-Closed Guarantees:**
/// - Rejects `HistoricalBenchmark` (`ARABICA-MONTHLY-IMF/USD`) and `SyntheticFixture` (`GESHA-DEMO/USD`).
/// - Rejects `observed_at_sec == 0` and future drift exceeding `policy.max_clock_drift_sec`.
/// - Never falls back silently to an old price when `age_sec > max_execution_age_sec` or jump > `max_jump_bps`.
pub fn evaluate_oracle_for_perps(
    price_e8: u128,
    observed_at_sec: u64,
    data_class: &str,
    worker_mode: &str,
    previous_valid_price_e8: Option<u128>,
    now_sec: u64,
    policy: &OracleValidationPolicy,
) -> Result<OracleHealthOutcome, PerpMathError> {
    if price_e8 == 0 {
        return Err(PerpMathError::ZeroPrice);
    }

    let tier = classify_oracle_feed(data_class, worker_mode);
    match tier {
        OracleFeedTier::ProductionLive => {}
        OracleFeedTier::TestnetSandbox if policy.allow_testnet_sandbox => {}
        OracleFeedTier::TestnetSandbox
        | OracleFeedTier::HistoricalBenchmark
        | OracleFeedTier::SyntheticFixture
        | OracleFeedTier::Unknown => {
            return Err(PerpMathError::OracleClassRejected);
        }
    }

    if observed_at_sec == 0 {
        return Err(PerpMathError::OracleClassRejected);
    }
    if observed_at_sec > now_sec.saturating_add(policy.max_clock_drift_sec) {
        return Err(PerpMathError::FutureTimestampDrift);
    }

    let age_sec = now_sec.saturating_sub(observed_at_sec);
    if age_sec >= policy.emergency_close_age_sec {
        return Ok(OracleHealthOutcome::EmergencyEligible { age_sec });
    }
    if age_sec > policy.max_execution_age_sec {
        return Ok(OracleHealthOutcome::StaleReduceOnly { age_sec });
    }

    if let Some(prev) = previous_valid_price_e8 {
        if prev > 0 && policy.max_jump_bps > 0 {
            let jump_bps = calculate_divergence_bps(prev, price_e8);
            if jump_bps > policy.max_jump_bps {
                return Ok(OracleHealthOutcome::JumpCircuitBreaker {
                    previous_price_e8: prev,
                    observed_price_e8: price_e8,
                    jump_bps,
                });
            }
        }
    }

    Ok(OracleHealthOutcome::ValidForExecution {
        price_e8,
        age_sec,
        tier,
    })
}

/// Compute deterministic **Uniform Emergency Settlement** outcome for any open position when a market
/// locks into [`MarketMode::Emergency`] after `emergency_close_age_sec`.
///
/// ## Definitive M04.5A.1 Anti-Exploit Design
/// Instead of voluntary selective exits that penalize ITM traders or grant OTM traders a free put
/// option when off-chain prices diverge during an outage, `MarketMode::Emergency` freezes **both**:
/// 1. The global market settlement price at `frozen_settlement_price_e8` (either `last_valid_price_e8`
///    snapshotted irreversibly at outage cutoff, or a multi-sig attested final recovery price), AND
/// 2. The global cumulative funding index at `frozen_funding_index_e12` (`last_valid_ts + max_exec_age`),
///    settling ALL Long and Short positions symmetrically with `close_fee_bps = 0` via [`compute_position_settlement_e8`].
///
/// Zero cherry-picking, zero Long/Short asymmetry, zero trapped margin, and exact solvency conservation.
pub fn compute_uniform_emergency_settlement_usd_e8(
    position: &PositionSnapshot,
    frozen_settlement_price_e8: u128,
    frozen_funding_index_e12: i128,
    max_pnl_bps: Option<u32>,
) -> Result<UniformPositionSettlement, PerpMathError> {
    compute_position_settlement_e8(
        position,
        frozen_settlement_price_e8,
        frozen_funding_index_e12,
        max_pnl_bps,
        0, // Zero close penalty during uniform emergency unwind
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pnl_long_short_symmetry_zero_and_invalid_inputs() {
        let size = 100 * SIZE_SCALE; // 100 kg
        let entry = 8 * PRICE_SCALE; // $8.00/kg
        let up = 9 * PRICE_SCALE; // $9.00/kg (+$1.00/kg -> +$100)
        let down = 7 * PRICE_SCALE; // $7.00/kg (-$1.00/kg -> -$100)

        // Exact Long & Short winners/losers
        let long_win = compute_unrealized_pnl_usd_e8(PositionSide::Long, size, entry, up).unwrap();
        let short_lose =
            compute_unrealized_pnl_usd_e8(PositionSide::Short, size, entry, up).unwrap();
        assert_eq!(long_win, 100 * USD_SCALE as i128);
        assert_eq!(short_lose, -(100 * USD_SCALE as i128));
        assert_eq!(long_win + short_lose, 0);

        let long_lose =
            compute_unrealized_pnl_usd_e8(PositionSide::Long, size, entry, down).unwrap();
        let short_win =
            compute_unrealized_pnl_usd_e8(PositionSide::Short, size, entry, down).unwrap();
        assert_eq!(long_lose, -(100 * USD_SCALE as i128));
        assert_eq!(short_win, 100 * USD_SCALE as i128);
        assert_eq!(long_lose + short_win, 0);

        // Identical price -> 0
        assert_eq!(
            compute_unrealized_pnl_usd_e8(PositionSide::Long, size, entry, entry).unwrap(),
            0
        );
        assert_eq!(
            compute_unrealized_pnl_usd_e8(PositionSide::Short, size, entry, entry).unwrap(),
            0
        );

        // Size zero -> 0
        assert_eq!(
            compute_unrealized_pnl_usd_e8(PositionSide::Long, 0, entry, up).unwrap(),
            0
        );

        // Fractional rounding conserves vault (`long + short <= 0`, never positive!)
        let small_size = 3u128; // 3e-8 kg
        let odd_up = entry + 10_000_000u128; // +$0.10/kg => 3e-9 rUSD = 0.3 raw units
        let l_frac =
            compute_unrealized_pnl_usd_e8(PositionSide::Long, small_size, entry, odd_up).unwrap();
        let s_frac =
            compute_unrealized_pnl_usd_e8(PositionSide::Short, small_size, entry, odd_up).unwrap();
        assert_eq!(l_frac, 0);
        assert_eq!(s_frac, -1);
        assert!(l_frac + s_frac <= 0);

        // Zero prices rejected fail-closed
        assert_eq!(
            compute_unrealized_pnl_usd_e8(PositionSide::Long, size, 0, up).unwrap_err(),
            PerpMathError::ZeroPrice
        );
        assert_eq!(
            compute_unrealized_pnl_usd_e8(PositionSide::Short, size, entry, 0).unwrap_err(),
            PerpMathError::ZeroPrice
        );
    }

    #[test]
    fn test_margin_requirements_monotonicity_and_liquidation_boundaries() {
        let params = MarketRiskParams::default();
        assert!(params.validate().is_ok());

        // Notional $1,000 ($10/kg * 100 kg)
        let size = 100 * SIZE_SCALE;
        let entry = 10 * PRICE_SCALE;
        let notional = compute_notional_ceil_usd_e8(entry, size).unwrap();
        assert_eq!(notional, 1_000 * USD_SCALE);

        // IM (20% = $200), MM (8% = $80)
        let im = compute_initial_margin_usd_e8(notional, params.initial_margin_bps).unwrap();
        let mm =
            compute_maintenance_margin_usd_e8(notional, params.maintenance_margin_bps).unwrap();
        assert_eq!(im, 200 * USD_SCALE);
        assert_eq!(mm, 80 * USD_SCALE);

        // Monotonicity: larger size or higher IM bps strictly raises IM
        let im_bigger =
            compute_initial_margin_usd_e8(notional + USD_SCALE, params.initial_margin_bps).unwrap();
        assert!(im_bigger > im);

        // Opening insufficient margin detected via equity < im
        let eq_below_im = compute_position_equity_usd_e8(im - 1, 0, 0).unwrap();
        assert!(eq_below_im < im as i128);

        // Remove margin check: excess above IM is withdrawable only when mark equity stays >= IM
        assert_eq!(
            compute_available_margin_usd_e8(250 * USD_SCALE, im, 250 * USD_SCALE as i128, im)
                .unwrap(),
            50 * USD_SCALE
        );
        // Unrealized profit ($100) cannot be extracted out of initial margin
        assert_eq!(
            compute_available_margin_usd_e8(im, im, (im + 100 * USD_SCALE) as i128, im).unwrap(),
            0
        );
        // Margin drop prevents full withdrawal below mark IM
        assert_eq!(
            compute_available_margin_usd_e8(250 * USD_SCALE, im, 190 * USD_SCALE as i128, im)
                .unwrap(),
            0
        );

        // Price drops -$1.20/kg ($8.80/kg -> mark notional $880, MM = $70.40)
        let mark_drop = 880_000_000u128; // $8.80
        let pnl_drop =
            compute_unrealized_pnl_usd_e8(PositionSide::Long, size, entry, mark_drop).unwrap();
        assert_eq!(pnl_drop, -(120 * USD_SCALE as i128));
        let eq_drop = compute_position_equity_usd_e8(im, pnl_drop, 0).unwrap();
        assert_eq!(eq_drop, 80 * USD_SCALE as i128); // $80 > $70.40 MM -> healthy
        assert!(!is_liquidatable(eq_drop, 880 * USD_SCALE, params.maintenance_margin_bps).unwrap());

        // Drop further to $8.69/kg -> equity $69 <= MM $69.52 -> liquidatable!
        let mark_liq = 869_000_000u128;
        let pnl_liq =
            compute_unrealized_pnl_usd_e8(PositionSide::Long, size, entry, mark_liq).unwrap();
        let settlement =
            compute_liquidation_settlement(im, pnl_liq, 0, mark_liq, size, &params).unwrap();
        assert!(settlement.liquidatable);
        assert_eq!(settlement.bad_debt_e8, 0);
        // Conservation: keeper + insurance + close + trader remainder == equity_before_close
        assert_eq!(
            settlement.liq_keeper_fee_paid_e8
                + settlement.liq_insurance_fee_paid_e8
                + settlement.close_fee_paid_e8
                + settlement.trader_remaining_margin_e8,
            settlement.equity_before_close_e8 as u128
        );

        // Max leverage validation rejects 0 or unbacked IM
        let bad_params = MarketRiskParams {
            max_leverage: 10,
            initial_margin_bps: 500, // 5% < 10% for 10x
            ..params
        };
        assert!(bad_params.validate().is_err());
    }

    #[test]
    fn test_funding_directions_conservation_zero_skew_and_regressive_time() {
        let lp_equity = 100_000 * USD_SCALE;
        let max_hourly_rate = 500_000_000u128; // 5 bps / hr
        let min_base = 10_000 * USD_SCALE;

        // Zero skew -> zero funding
        assert_eq!(
            compute_funding_rate_per_sec_e12(
                50_000 * USD_SCALE,
                50_000 * USD_SCALE,
                lp_equity,
                max_hourly_rate,
                min_base
            )
            .unwrap(),
            0
        );

        // Long skewed -> positive funding rate
        let r_pos = compute_funding_rate_per_sec_e12(
            80_000 * USD_SCALE,
            20_000 * USD_SCALE,
            lp_equity,
            max_hourly_rate,
            min_base,
        )
        .unwrap();
        assert!(r_pos > 0);

        // Short skewed -> negative funding rate
        let r_neg = compute_funding_rate_per_sec_e12(
            20_000 * USD_SCALE,
            80_000 * USD_SCALE,
            lp_equity,
            max_hourly_rate,
            min_base,
        )
        .unwrap();
        assert_eq!(r_neg, -r_pos);

        // Accrual over 1 hour at $10/kg
        let delta_pos = compute_funding_index_delta_e12(10 * PRICE_SCALE, r_pos, 3_600).unwrap();
        assert!(delta_pos > 0);
        // Zero elapsed -> 0
        assert_eq!(
            compute_funding_index_delta_e12(10 * PRICE_SCALE, r_pos, 0).unwrap(),
            0
        );
        // Zero rate -> 0
        assert_eq!(
            compute_funding_index_delta_e12(10 * PRICE_SCALE, 0, 3_600).unwrap(),
            0
        );

        // Matched Long/Short conservation: Long pays <= Short receives? Never creates funds: Long + Short <= 0!
        let size = 123_456_789u128; // Fractional size 1.23456789 kg
        let long_net =
            compute_position_funding_net_usd_e8(PositionSide::Long, size, 0, delta_pos).unwrap();
        let short_net =
            compute_position_funding_net_usd_e8(PositionSide::Short, size, 0, delta_pos).unwrap();
        assert!(long_net < 0);
        assert!(short_net > 0);
        assert!(
            long_net + short_net <= 0,
            "Funding rounding must never create net positive funds"
        );

        // Negative rate: Short pays (<0), Long receives (>0), sum <= 0
        let delta_neg = compute_funding_index_delta_e12(10 * PRICE_SCALE, r_neg, 3_600).unwrap();
        let l_neg =
            compute_position_funding_net_usd_e8(PositionSide::Long, size, 0, delta_neg).unwrap();
        let s_neg =
            compute_position_funding_net_usd_e8(PositionSide::Short, size, 0, delta_neg).unwrap();
        assert!(l_neg > 0);
        assert!(s_neg < 0);
        assert!(l_neg + s_neg <= 0);

        // Regressive timestamp fail-closed & extreme window clamp
        assert_eq!(
            advance_funding_elapsed_sec(1_000, 999, 86_400).unwrap_err(),
            PerpMathError::RegressiveTimestamp
        );
        assert_eq!(
            advance_funding_elapsed_sec(1_000, 10_000_000, 604_800).unwrap(),
            604_800
        );
    }

    #[test]
    fn test_lp_accounting_dilution_protection_profit_loss_and_locked_withdrawals() {
        // 1. Initial seed deposit ($10,000) locks 0.0001 seed shares against inflation attacks
        let dep1 = 10_000 * USD_SCALE;
        let mint1 = compute_lp_mint_shares(dep1, 0, 0).unwrap();
        assert_eq!(mint1.locked_seed_shares, MINIMUM_LP_SEED_SHARES);
        assert_eq!(mint1.new_total_shares, dep1);
        assert_eq!(mint1.minted_to_depositor, dep1 - MINIMUM_LP_SEED_SHARES);

        // 2. Pool in MTM profit (+$2,000, i.e., traders lost $2,000 within margin -> total LP MTM $12,000)
        // New $6,000 deposit receives 5,000 shares (pro-rata at $1.20/share), no dilution!
        let lp_mtm_profit = compute_lp_mtm_equity_e8(dep1, -(2_000 * USD_SCALE as i128)).unwrap();
        assert_eq!(lp_mtm_profit, 12_000 * USD_SCALE as i128);
        let dep2 = 6_000 * USD_SCALE;
        let mint2 = compute_lp_mint_shares(dep2, mint1.new_total_shares, lp_mtm_profit).unwrap();
        assert_eq!(mint2.minted_to_depositor, 5_000 * LP_SHARE_SCALE);

        // 3. Pool in MTM loss (-$3,000 -> total LP MTM $13,000 across 15,000 shares & realized $16,000)
        let realized = dep1 + dep2;
        let lp_mtm_loss = compute_lp_mtm_equity_e8(realized, 3_000 * USD_SCALE as i128).unwrap();
        assert_eq!(lp_mtm_loss, 13_000 * USD_SCALE as i128);

        // 4. Encumbered OI reserve ($12,000) limits withdrawable cash to $4,000
        let reserved = compute_reserved_liquidity_usd_e8(12_000 * USD_SCALE, 0, 2_500).unwrap();
        assert_eq!(reserved, 12_000 * USD_SCALE);
        let avail =
            compute_lp_available_liquidity_usd_e8(realized, reserved, 3_000 * USD_SCALE as i128);
        assert_eq!(avail, 4_000 * USD_SCALE);

        // Trying to redeem 6,000 shares (~$5,200 MTM > $4,000 available) fails closed
        assert_eq!(
            compute_lp_redeem_assets(
                6_000 * LP_SHARE_SCALE,
                mint2.new_total_shares,
                lp_mtm_loss,
                avail
            )
            .unwrap_err(),
            PerpMathError::InsufficientLiquidity
        );

        // Redeeming 3,000 shares ($2,600 MTM <= $4,000 available) succeeds cleanly
        let out = compute_lp_redeem_assets(
            3_000 * LP_SHARE_SCALE,
            mint2.new_total_shares,
            lp_mtm_loss,
            avail,
        )
        .unwrap();
        assert_eq!(out, 2_600 * USD_SCALE);

        // Rounding no-profit check: immediate deposit & redeem never exceeds deposit
        let tiny_dep = 777_777_777u128;
        let tiny_mint =
            compute_lp_mint_shares(tiny_dep, mint2.new_total_shares, lp_mtm_loss).unwrap();
        let immediate_out = compute_lp_redeem_assets(
            tiny_mint.minted_to_depositor,
            tiny_mint.new_total_shares,
            lp_mtm_loss + tiny_dep as i128,
            realized + tiny_dep,
        )
        .unwrap();
        assert!(immediate_out <= tiny_dep);
    }

    #[test]
    fn test_solvency_invariants_bad_debt_cascade_and_insolvency_pause() {
        let mut buckets = ContractCashBuckets {
            total_trader_free_collateral_e8: 5_000 * USD_SCALE,
            total_trader_locked_margin_e8: 2_000 * USD_SCALE,
            total_in_flight_withdrawals_e8: 500 * USD_SCALE,
            lp_vault_realized_equity_e8: 20_000 * USD_SCALE,
            insurance_fund_e8: 1_500 * USD_SCALE,
            protocol_fees_e8: 250 * USD_SCALE,
        };
        let physical = buckets.total_accounted_e8().unwrap();
        // Exact solvency with zero unallocated surplus
        assert_eq!(buckets.verify_solvency(physical).unwrap(), 0);
        // External airdrop doesn't break invariant; shows unallocated surplus separately
        assert_eq!(
            buckets.verify_solvency(physical + 100 * USD_SCALE).unwrap(),
            100 * USD_SCALE
        );
        // Deficit fails solvency fail-closed
        assert!(matches!(
            buckets.verify_solvency(physical - 1),
            Err(PerpMathError::InsolvencyDetected { .. })
        ));

        // Bad debt cascade scenarios:
        // (a) Fully absorbed by Insurance Fund
        let c1 = apply_bad_debt_cascade(
            800 * USD_SCALE,
            buckets.insurance_fund_e8,
            buckets.lp_vault_realized_equity_e8,
        );
        assert_eq!(c1.absorbed_by_insurance_e8, 800 * USD_SCALE);
        assert_eq!(c1.absorbed_by_lp_vault_e8, 0);
        assert_eq!(c1.uncovered_insolvency_e8, 0);
        assert!(!c1.should_pause_market);

        // (b) Exhausts Insurance ($1,500), hits LP Vault ($2,500) -> still solvent
        let c2 = apply_bad_debt_cascade(
            4_000 * USD_SCALE,
            buckets.insurance_fund_e8,
            buckets.lp_vault_realized_equity_e8,
        );
        assert_eq!(c2.absorbed_by_insurance_e8, 1_500 * USD_SCALE);
        assert_eq!(c2.absorbed_by_lp_vault_e8, 2_500 * USD_SCALE);
        assert_eq!(c2.uncovered_insolvency_e8, 0);
        assert!(c2.insurance_depleted);
        assert!(!c2.should_pause_market);

        // Simulate cash transfer from Insurance to LP on c2 -> total physical balance unchanged!
        buckets.insurance_fund_e8 -= c2.absorbed_by_insurance_e8;
        buckets.lp_vault_realized_equity_e8 += c2.absorbed_by_insurance_e8;
        assert_eq!(buckets.total_accounted_e8().unwrap(), physical);

        // (c) Exceeds Insurance + LP Vault -> triggers market insolvency pause!
        let c3 = apply_bad_debt_cascade(
            50_000 * USD_SCALE,
            buckets.insurance_fund_e8,
            buckets.lp_vault_realized_equity_e8,
        );
        assert!(c3.uncovered_insolvency_e8 > 0);
        assert!(c3.should_pause_market);
        assert_eq!(
            c3.absorbed_by_insurance_e8 + c3.absorbed_by_lp_vault_e8 + c3.uncovered_insolvency_e8,
            50_000 * USD_SCALE
        );
    }

    #[test]
    fn test_oracle_policy_and_conservative_emergency_close() {
        let policy = OracleValidationPolicy::default();
        let now = 1_800_000_000u64;

        // IMF benchmark & synthetic fixtures rejected for Perps
        assert_eq!(
            evaluate_oracle_for_perps(
                738_863_238,
                now - 60,
                DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY,
                MODE_PUBLIC_BENCHMARK,
                None,
                now,
                &policy,
            )
            .unwrap_err(),
            PerpMathError::OracleClassRejected
        );
        assert_eq!(
            evaluate_oracle_for_perps(
                1_850_000_000,
                0,
                DATA_CLASS_SYNTHETIC,
                MODE_FIXTURE,
                None,
                now,
                &policy,
            )
            .unwrap_err(),
            PerpMathError::OracleClassRejected
        );

        // Testnet sandbox feed within 300s & 5% jump passes
        assert_eq!(
            evaluate_oracle_for_perps(
                800_000_000,
                now - 10,
                DATA_CLASS_TESTNET_SANDBOX_FEED,
                MODE_TESTNET_SANDBOX,
                Some(780_000_000),
                now,
                &policy,
            )
            .unwrap(),
            OracleHealthOutcome::ValidForExecution {
                price_e8: 800_000_000,
                age_sec: 10,
                tier: OracleFeedTier::TestnetSandbox,
            }
        );

        // > 5% jump triggers circuit breaker
        assert!(matches!(
            evaluate_oracle_for_perps(
                900_000_000,
                now - 10,
                DATA_CLASS_LIVE_AGGREGATED,
                MODE_LIVE_INDEX,
                Some(800_000_000),
                now,
                &policy,
            )
            .unwrap(),
            OracleHealthOutcome::JumpCircuitBreaker { .. }
        ));

        // 301s..3599s triggers StaleReduceOnly; >= 3600s enters EmergencyEligible
        assert!(matches!(
            evaluate_oracle_for_perps(
                800_000_000,
                now - 600,
                DATA_CLASS_LIVE_VERIFIED,
                MODE_LIVE_INDEX,
                None,
                now,
                &policy,
            )
            .unwrap(),
            OracleHealthOutcome::StaleReduceOnly { age_sec: 600 }
        ));
        assert!(matches!(
            evaluate_oracle_for_perps(
                800_000_000,
                now - 4_000,
                DATA_CLASS_LIVE_VERIFIED,
                MODE_LIVE_INDEX,
                None,
                now,
                &policy,
            )
            .unwrap(),
            OracleHealthOutcome::EmergencyEligible { age_sec: 4_000 }
        ));

        // Uniform Emergency Settlement at frozen P_last_valid ($9.00/kg) for both ITM Long and OTM Short
        // Symmetric across winners and losers, zero closing penalty, zero cherry-picking!
        let size = 100 * SIZE_SCALE; // 100 kg
        let entry = 8 * PRICE_SCALE; // $8/kg -> $800 notional
        let margin = 200 * USD_SCALE;
        let frozen_p = 9 * PRICE_SCALE; // +$100 to Long, -$100 to Short

        let em_long = compute_uniform_emergency_settlement_usd_e8(
            &PositionSnapshot {
                side: PositionSide::Long,
                size_e8: size,
                locked_margin_e8: margin,
                entry_price_e8: entry,
                entry_funding_index_e12: 0,
            },
            frozen_p,
            0,
            Some(TESTNET_DEFAULT_MAX_PNL_BPS),
        )
        .unwrap();
        let em_short = compute_uniform_emergency_settlement_usd_e8(
            &PositionSnapshot {
                side: PositionSide::Short,
                size_e8: size,
                locked_margin_e8: margin,
                entry_price_e8: entry,
                entry_funding_index_e12: 0,
            },
            frozen_p,
            0,
            Some(TESTNET_DEFAULT_MAX_PNL_BPS),
        )
        .unwrap();

        assert_eq!(em_long.fee_e8, 0);
        assert_eq!(em_short.fee_e8, 0);
        assert_eq!(em_long.trader_payout_e8, 300 * USD_SCALE);
        assert_eq!(em_short.trader_payout_e8, 100 * USD_SCALE);
        // Net pool transfer across matched Long + Short settles identically to 0
        assert_eq!(em_long.vault_delta_e8 + em_short.vault_delta_e8, 0);
    }

    #[test]
    fn test_extreme_arithmetic_overflow_underflow_div_zero_and_i128_min() {
        assert_eq!(
            checked_mul_div_floor_u128(10, 10, 0).unwrap_err(),
            PerpMathError::ZeroDenominator
        );
        assert_eq!(
            ceil_div_u128(10, 0).unwrap_err(),
            PerpMathError::ZeroDenominator
        );
        assert_eq!(
            floor_div_i128(-10, 0).unwrap_err(),
            PerpMathError::ZeroDenominator
        );
        assert_eq!(
            floor_div_i128(i128::MIN, -1).unwrap_err(),
            PerpMathError::Overflow
        );
        assert_eq!(floor_div_i128(-11, 10).unwrap(), -2);
        assert_eq!(floor_div_i128(11, 10).unwrap(), 1);
        assert_eq!(floor_div_i128(-10, 10).unwrap(), -1);

        // Exact 256-bit intermediate succeeds where naive u128 mul overflows
        let big = 1u128 << 100;
        assert_eq!(checked_mul_div_floor_u128(big, big, big).unwrap(), big);
        assert_eq!(
            checked_mul_div_floor_u128(u128::MAX, u128::MAX, u128::MAX).unwrap(),
            u128::MAX
        );
        // Carry bit case with denominator > 2^127
        let large_den = (1u128 << 127) + 12345;
        assert_eq!(
            checked_mul_div_floor_u128(u128::MAX, 2, large_den).unwrap(),
            3
        );
        assert_eq!(
            checked_mul_div_ceil_u128(u128::MAX, 2, large_den).unwrap(),
            4
        );

        // Exact boundary where floor == u128::MAX succeeds while ceil == u128::MAX + 1 overflows!
        // Note: 2^129 - 1 = 2 * u128::MAX + 1 is divisible by 7 (`k = (2^129 - 1) / 7` < 2^127).
        let k_2_129_minus_1_div_7: u128 = 97_223_533_405_982_418_132_392_744_980_505_203_273;
        assert_eq!(
            checked_mul_div_floor_u128(7, k_2_129_minus_1_div_7, 2).unwrap(),
            u128::MAX
        );
        assert_eq!(
            checked_mul_div_ceil_u128(7, k_2_129_minus_1_div_7, 2).unwrap_err(),
            PerpMathError::Overflow
        );

        // True quotient overflow past u128::MAX fails cleanly
        assert_eq!(
            checked_mul_div_floor_u128(u128::MAX, 2, 1).unwrap_err(),
            PerpMathError::Overflow
        );
        assert_eq!(
            checked_mul_div_ceil_u128(u128::MAX, u128::MAX, u128::MAX - 1).unwrap_err(),
            PerpMathError::Overflow
        );

        // Signed i128::MIN exact boundary without panic
        assert_eq!(
            checked_signed_mul_div_floor_i128(i128::MIN, 1, 1).unwrap(),
            i128::MIN
        );
        assert_eq!(
            checked_signed_mul_div_floor_i128(i128::MIN, -1, 1).unwrap_err(),
            PerpMathError::Overflow
        );
        assert_eq!(
            checked_signed_mul_div_floor_i128(i128::MIN, 1, 2).unwrap(),
            i128::MIN / 2
        );
        // Signed negative fractional rounding toward -infinity
        assert_eq!(checked_signed_mul_div_floor_i128(-1, 1, 2).unwrap(), -1);
    }

    /// Independent 256-bit schoolbook + bit-by-bit long-division oracle (zero shared logic with production math).
    fn ref_mul_div_256(a: u128, b: u128, d: u128) -> Option<(u128, u128)> {
        if d == 0 {
            return None;
        }
        let a0 = a as u64 as u128;
        let a1 = (a >> 64) as u64 as u128;
        let b0 = b as u64 as u128;
        let b1 = (b >> 64) as u64 as u128;

        let p00 = a0 * b0;
        let p01 = a0 * b1;
        let p10 = a1 * b0;
        let p11 = a1 * b1;
        let (mid1, c1) = (p00 >> 64).overflowing_add(p01);
        let (mid, c2) = mid1.overflowing_add(p10);
        let expected_lo = (p00 & 0xFFFF_FFFF_FFFF_FFFF) | ((mid & 0xFFFF_FFFF_FFFF_FFFF) << 64);
        let expected_hi = p11 + (mid >> 64) + ((u128::from(c1) + u128::from(c2)) << 64);

        assert_eq!(mul_wide_u128(a, b), (expected_hi, expected_lo));

        // Independent 256-by-128 shift-subtract long division (`rem < d <= u128::MAX` at each step start)
        let mut q: u128 = 0;
        let mut rem_lo: u128 = 0;
        for i in (0..256).rev() {
            let bit = if i >= 128 {
                (expected_hi >> (i - 128)) & 1
            } else {
                (expected_lo >> i) & 1
            };
            let rem_hi = rem_lo >> 127;
            rem_lo = (rem_lo << 1) | bit;
            if rem_hi > 0 || rem_lo >= d {
                let (next_lo, borrow) = rem_lo.overflowing_sub(d);
                rem_lo = next_lo;
                debug_assert_eq!(rem_hi, u128::from(borrow));
                if i >= 128 {
                    return None;
                }
                q |= 1u128 << i;
            }
        }
        Some((q, rem_lo))
    }

    #[test]
    fn test_256bit_mul_div_against_independent_reference_property_grid() {
        let vals: [u128; 10] = [
            0,
            1,
            2,
            3,
            10_000,
            PRICE_SCALE,
            FUNDING_SCALE,
            1u128 << 64,
            1u128 << 127,
            u128::MAX,
        ];
        let denoms: [u128; 8] = [
            1,
            2,
            7,
            10_000,
            PRICE_SCALE,
            (1u128 << 100) + 77,
            (1u128 << 127) + 12_345,
            u128::MAX,
        ];

        for &a in &vals {
            for &b in &vals {
                for &d in &denoms {
                    let expected = ref_mul_div_256(a, b, d);
                    let got_floor = checked_mul_div_floor_u128(a, b, d);
                    match expected {
                        Some((q, r)) => {
                            assert_eq!(got_floor.unwrap(), q);
                            // Property: reconstruct exact product `q * d + r == (a * b)` in 256-bit
                            let (qd_hi, qd_lo) = mul_wide_u128(q, d);
                            let (sum_lo, carry) = qd_lo.overflowing_add(r);
                            let sum_hi = qd_hi + u128::from(carry);
                            assert_eq!((sum_hi, sum_lo), mul_wide_u128(a, b));
                            assert!(r < d);

                            let got_ceil = checked_mul_div_ceil_u128(a, b, d);
                            if r == 0 {
                                assert_eq!(got_ceil.unwrap(), q);
                            } else if q == u128::MAX {
                                assert_eq!(got_ceil.unwrap_err(), PerpMathError::Overflow);
                            } else {
                                assert_eq!(got_ceil.unwrap(), q + 1);
                            }
                        }
                        None => {
                            assert_eq!(got_floor.unwrap_err(), PerpMathError::Overflow);
                            assert_eq!(
                                checked_mul_div_ceil_u128(a, b, d).unwrap_err(),
                                PerpMathError::Overflow
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_pnl_and_notional_large_configured_coffee_bounds() {
        // $500/kg coffee across 100,000 metric tons (100,000,000 kg) = $50B notional
        let max_coffee_price = 500 * PRICE_SCALE;
        let giant_size = 100_000_000 * SIZE_SCALE;
        let notional = compute_notional_ceil_usd_e8(max_coffee_price, giant_size).unwrap();
        assert_eq!(notional, 50_000_000_000 * USD_SCALE);

        // 20% price swing stays well within i128 without external U256 crates
        let swing_up = 600 * PRICE_SCALE;
        let pnl = compute_unrealized_pnl_usd_e8(
            PositionSide::Long,
            giant_size,
            max_coffee_price,
            swing_up,
        )
        .unwrap();
        assert_eq!(pnl, 10_000_000_000 * USD_SCALE as i128);
    }

    #[test]
    fn test_funding_high_precision_interval_and_symmetry() {
        // Verify single-interval funding avoids per-second truncation loss on modest 1% skew
        let lp_equity = 1_000_000 * USD_SCALE;
        let long_oi = 10_000 * USD_SCALE; // 1% of LP
        let short_oi = 0;
        let max_hourly = 50_000_000u128; // 0.5 bps / hr
        let min_base = 10_000 * USD_SCALE;

        // With old `/3600` first, floor(0.01 * 50_000_000 / 3600) = 138; over 24h vs single div loses precision
        let delta_pos = compute_funding_delta_over_interval_e12(
            long_oi,
            short_oi,
            lp_equity,
            10 * PRICE_SCALE,
            86_400,
            max_hourly,
            min_base,
        )
        .unwrap();
        let delta_neg = compute_funding_delta_over_interval_e12(
            short_oi,
            long_oi,
            lp_equity,
            10 * PRICE_SCALE,
            86_400,
            max_hourly,
            min_base,
        )
        .unwrap();
        assert!(delta_pos > 0);
        // Exact magnitude symmetry!
        assert_eq!(delta_neg, -delta_pos);
    }

    #[test]
    fn test_funding_skew_direction_flip_and_extreme_durations() {
        let lp_equity = 50_000 * USD_SCALE;
        let max_hourly = 500_000_000u128; // 5 bps / hr
        let min_base = 10_000 * USD_SCALE;
        let mut cum_idx = 0i128;

        // Step 1: Long-dominated (+$30k net skew) for 4 hours -> cum_idx rises
        let r1 = compute_funding_rate_per_sec_e12(
            40_000 * USD_SCALE,
            10_000 * USD_SCALE,
            lp_equity,
            max_hourly,
            min_base,
        )
        .unwrap();
        let d1 = compute_funding_index_delta_e12(10 * PRICE_SCALE, r1, 4 * 3_600).unwrap();
        cum_idx += d1;
        assert!(cum_idx > 0);

        // Step 2: Balanced (skew = 0) -> no change
        let r2 = compute_funding_rate_per_sec_e12(
            25_000 * USD_SCALE,
            25_000 * USD_SCALE,
            lp_equity,
            max_hourly,
            min_base,
        )
        .unwrap();
        assert_eq!(r2, 0);

        // Step 3: Skew flips Short-dominated (-$30k net skew) for 8 hours -> cum_idx drops negative
        let r3 = compute_funding_rate_per_sec_e12(
            10_000 * USD_SCALE,
            40_000 * USD_SCALE,
            lp_equity,
            max_hourly,
            min_base,
        )
        .unwrap();
        assert_eq!(r3, -r1);
        let d3 = compute_funding_index_delta_e12(10 * PRICE_SCALE, r3, 8 * 3_600).unwrap();
        cum_idx += d3;
        assert!(cum_idx < 0);

        // Across the entire flip, matched 10 kg Long + Short never create positive total value
        let size = 10 * SIZE_SCALE;
        let l_net =
            compute_position_funding_net_usd_e8(PositionSide::Long, size, 0, cum_idx).unwrap();
        let s_net =
            compute_position_funding_net_usd_e8(PositionSide::Short, size, 0, cum_idx).unwrap();
        assert!(l_net + s_net <= 0);

        // Multi-day extreme un-accrued interval fits safely without overflow
        let d_extreme =
            compute_funding_index_delta_e12(500 * PRICE_SCALE, r1, 365 * 24 * 3_600).unwrap();
        assert!(d_extreme > 0);
    }

    #[test]
    fn test_lp_simultaneous_deposits_last_withdrawer_and_zero_nav() {
        // Seed initial vault with $1,000
        let s0 = compute_lp_mint_shares(1_000 * USD_SCALE, 0, 0).unwrap();
        let mut total_shares = s0.new_total_shares;
        let mut nav = 1_000 * USD_SCALE as i128;

        // Simultaneous identical deposits of $500 each at identical NAV snapshot mint equal shares
        let dep = 500 * USD_SCALE;
        let s_a = compute_lp_mint_shares(dep, total_shares, nav).unwrap();
        total_shares = s_a.new_total_shares;
        nav += dep as i128;

        let s_b = compute_lp_mint_shares(dep, total_shares, nav).unwrap();
        assert_eq!(s_a.minted_to_depositor, s_b.minted_to_depositor);
        total_shares = s_b.new_total_shares;
        nav += dep as i128;

        // Last withdrawer removes 100% of all user shares, leaving locked seed shares intact cleanly
        let user_shares =
            s0.minted_to_depositor + s_a.minted_to_depositor + s_b.minted_to_depositor;
        assert_eq!(total_shares - user_shares, MINIMUM_LP_SEED_SHARES);
        let withdrawn_all =
            compute_lp_redeem_assets(user_shares, total_shares, nav, nav as u128).unwrap();
        assert_eq!(withdrawn_all, (nav as u128) - MINIMUM_LP_SEED_SHARES);

        // Attempting to burn locked seed shares OR burn more than total fails cleanly
        assert_eq!(
            compute_lp_redeem_assets(total_shares, total_shares, nav, nav as u128).unwrap_err(),
            PerpMathError::InsufficientLiquidity
        );
        assert_eq!(
            compute_lp_redeem_assets(total_shares + 1, total_shares, nav, nav as u128).unwrap_err(),
            PerpMathError::Underflow
        );

        // Zero / negative LP NAV blocks BOTH deposits and withdrawals
        assert_eq!(
            compute_lp_mint_shares(100 * USD_SCALE, total_shares, 0).unwrap_err(),
            PerpMathError::InsufficientLiquidity
        );
        assert_eq!(
            compute_lp_mint_shares(100 * USD_SCALE, total_shares, -100).unwrap_err(),
            PerpMathError::InsufficientLiquidity
        );
        assert_eq!(
            compute_lp_redeem_assets(100 * LP_SHARE_SCALE, total_shares, 0, 100 * USD_SCALE)
                .unwrap_err(),
            PerpMathError::InsufficientLiquidity
        );
    }

    #[test]
    fn test_adversarial_lp_pre_liquidation_insurance_arbitrage_defeated() {
        // Scenario: $10,000 LP, $1,500 Insurance, matched Long ($10k, M=$2k) + Short ($10k, M=$2k).
        // Coffee gaps +30% ($10 -> $13): Long MTM = +$3,000, Short MTM = -$3,000 (capped at -$2,000 + $1,000 bad debt).
        let lp_realized = 10_000 * USD_SCALE;
        let ins_fund = 1_500 * USD_SCALE;
        let long_claim =
            compute_capped_trader_net_claim_usd_e8(2_000 * USD_SCALE, 3_000 * USD_SCALE as i128, 0)
                .unwrap();
        let short_claim = compute_capped_trader_net_claim_usd_e8(
            2_000 * USD_SCALE,
            -(3_000 * USD_SCALE as i128),
            0,
        )
        .unwrap();
        let net_claim = long_claim + short_claim; // +$1,000
        let unrealized_bad_debt = 1_000 * USD_SCALE;

        // Without insurance offset, LP MTM drops temporarily to $9,000 (enabling arbitrage!);
        // WITH compute_lp_mtm_equity_with_insurance_offset_e8, LP MTM stays $10,000 before & after liquidation!
        let (nav_before_liq, covered) = compute_lp_mtm_equity_with_insurance_offset_e8(
            lp_realized,
            net_claim,
            unrealized_bad_debt,
            ins_fund,
        )
        .unwrap();
        assert_eq!(covered, 1_000 * USD_SCALE);
        assert_eq!(nav_before_liq, 10_000 * USD_SCALE as i128);

        // After keeper liquidates Short: LP gets $2,000 margin + $1,000 insurance = $13,000 realized;
        // remaining open claim is just Long's +$3,000 -> LP MTM = $13,000 - $3,000 = $10,000 (0 jump!)
        let nav_after_liq = compute_lp_mtm_equity_e8(13_000 * USD_SCALE, long_claim).unwrap();
        assert_eq!(nav_before_liq, nav_after_liq);

        // Meanwhile withdrawable cash excludes Insurance offset before realization (`max(reserved=$5k, claim=+$1k)`)
        let avail_before =
            compute_lp_available_liquidity_usd_e8(lp_realized, 5_000 * USD_SCALE, net_claim);
        assert_eq!(avail_before, 5_000 * USD_SCALE);
    }

    #[test]
    fn test_adversarial_one_sided_max_gain_cap_guarantees_vault_solvency() {
        // $100,000 LP Vault, max utilization 75% => $75,000 pure Long skew OI ($7,500 * $10/kg), 0 Shorts.
        let lp_equity = 100_000 * USD_SCALE;
        let entry_notional = 75_000 * USD_SCALE;
        let util = compute_utilization_bps(entry_notional, lp_equity as i128).unwrap();
        assert_eq!(util, 7_500);

        // Extreme +1000% coffee shock ($10 -> $110/kg): raw Long win = $750,000 (> LP Vault!),
        // but max_pnl_bps = 10_000 caps payout at 100% of reserved notional ($75,000), keeping $25,000 LP solvent!
        let raw_win = compute_unrealized_pnl_usd_e8(
            PositionSide::Long,
            7_500 * SIZE_SCALE,
            10 * PRICE_SCALE,
            110 * PRICE_SCALE,
        )
        .unwrap();
        assert_eq!(raw_win, 750_000 * USD_SCALE as i128);

        let capped_win =
            apply_profit_cap_usd_e8(raw_win, entry_notional, Some(TESTNET_DEFAULT_MAX_PNL_BPS))
                .unwrap();
        assert_eq!(capped_win, 75_000 * USD_SCALE as i128);

        let mtm_lp = compute_lp_mtm_equity_e8(lp_equity, capped_win).unwrap();
        assert_eq!(mtm_lp, 25_000 * USD_SCALE as i128);
    }

    #[test]
    fn test_double_entry_end_to_end_solvency_invariant_preservation() {
        // Track exact physical token balance at rest alongside contract ledger buckets
        let mut physical = 0u128;
        let mut b = ContractCashBuckets::default();
        assert_eq!(b.verify_solvency(physical).unwrap(), 0);

        // 1. Trader A deposits $1,000 rUSD; Trader B deposits $500 rUSD; Insurance funded $400 rUSD
        let dep_a = 1_000 * USD_SCALE;
        let dep_b = 500 * USD_SCALE;
        let ins_fund = 400 * USD_SCALE;
        physical += dep_a + dep_b + ins_fund;
        b.total_trader_free_collateral_e8 += dep_a + dep_b;
        b.insurance_fund_e8 += ins_fund;
        assert_eq!(b.verify_solvency(physical).unwrap(), 0);

        // 2. LP deposits $10,000 rUSD
        let lp_dep = 10_000 * USD_SCALE;
        physical += lp_dep;
        b.lp_vault_realized_equity_e8 += lp_dep;
        assert_eq!(b.verify_solvency(physical).unwrap(), 0);

        // 3. Trader A opens Long ($1,000 notional, IM $200, open fee $1 -> 70% LP / 30% Protocol)
        let im_a = 200 * USD_SCALE;
        let fee_a = compute_trading_fee_usd_e8(1_000 * USD_SCALE, 10).unwrap();
        let lp_cut = fee_a * 70 / 100;
        let proto_cut = fee_a - lp_cut;
        b.total_trader_free_collateral_e8 -= im_a + fee_a;
        b.total_trader_locked_margin_e8 += im_a;
        b.lp_vault_realized_equity_e8 += lp_cut;
        b.protocol_fees_e8 += proto_cut;
        assert_eq!(b.verify_solvency(physical).unwrap(), 0);

        // 4. Trader A closes at +$150 profit & $1.15 close fee
        let win_pnl = 150 * USD_SCALE;
        let close_fee = 115_000_000u128;
        b.total_trader_locked_margin_e8 -= im_a;
        b.lp_vault_realized_equity_e8 = b.lp_vault_realized_equity_e8 - win_pnl + close_fee;
        b.total_trader_free_collateral_e8 += im_a + win_pnl - close_fee;
        assert_eq!(b.verify_solvency(physical).unwrap(), 0);

        // 5. Trader B opens Short ($2,000 notional, $400 IM) and suffers -$550 gap loss ($150 bad debt)
        let im_b = 400 * USD_SCALE;
        b.total_trader_free_collateral_e8 -= im_b;
        b.total_trader_locked_margin_e8 += im_b;
        assert_eq!(b.verify_solvency(physical).unwrap(), 0);

        // Settlement on liquidation: margin ($400) forfeited to LP Vault, plus $150 bad debt from Insurance -> LP Vault
        let cascade = apply_bad_debt_cascade(
            150 * USD_SCALE,
            b.insurance_fund_e8,
            b.lp_vault_realized_equity_e8,
        );
        assert_eq!(cascade.absorbed_by_insurance_e8, 150 * USD_SCALE);
        assert!(!cascade.should_pause_market);

        b.total_trader_locked_margin_e8 -= im_b;
        b.lp_vault_realized_equity_e8 += im_b + cascade.absorbed_by_insurance_e8;
        b.insurance_fund_e8 -= cascade.absorbed_by_insurance_e8;
        assert_eq!(b.verify_solvency(physical).unwrap(), 0);

        // 6. Trader A initiates async withdrawal of $500 rUSD -> InFlight before ft_transfer completes
        let w_amt = 500 * USD_SCALE;
        b.total_trader_free_collateral_e8 -= w_amt;
        b.total_in_flight_withdrawals_e8 += w_amt;
        assert_eq!(b.verify_solvency(physical).unwrap(), 0);

        // 6a. Case failure callback: tokens never left contract -> safely restored to Free Collateral
        b.total_in_flight_withdrawals_e8 -= w_amt;
        b.total_trader_free_collateral_e8 += w_amt;
        assert_eq!(b.verify_solvency(physical).unwrap(), 0);

        // 6b. Retry & Ok callback: tokens transfer out -> physical and in_flight drop together
        b.total_trader_free_collateral_e8 -= w_amt;
        b.total_in_flight_withdrawals_e8 += w_amt;
        physical -= w_amt;
        b.total_in_flight_withdrawals_e8 -= w_amt;
        assert_eq!(b.verify_solvency(physical).unwrap(), 0);
    }
}
