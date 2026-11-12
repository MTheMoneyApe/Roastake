// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

//! Oracle integration, XCC `ext_oracle::get_price` adapter, and isolated `TESTNET_SANDBOX_FEED` validation.

use near_sdk::ext_contract;
use roastake_perps_core::{evaluate_oracle_for_perps, OracleHealthOutcome, PRICE_SCALE};
use roastake_shared::{
    DATA_CLASS_LIVE_AGGREGATED, DATA_CLASS_LIVE_VERIFIED, DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY,
    DATA_CLASS_SYNTHETIC, MARKET_ARABICA_MONTHLY_IMF, MARKET_GESHA_DEMO, MODE_LIVE_INDEX,
    UNIT_USD_PER_KG,
};

use crate::types::{
    ExternalPriceObservation, OracleSourceConfig, PerpsMarketState, StoredMarketMode,
    DATA_CLASS_TESTNET_SANDBOX, MODE_TESTNET_SANDBOX,
};

#[ext_contract(ext_oracle)]
pub trait OracleAdapterView {
    fn get_price(&self, market: String) -> Option<ExternalPriceObservation>;
}

/// Canonical check ensuring historical IMF benchmarks and synthetic GESHA demos can NEVER drive execution.
pub fn assert_market_symbol_not_prohibited_oracle(symbol: &str) -> Result<(), String> {
    if symbol == MARKET_ARABICA_MONTHLY_IMF || symbol == MARKET_GESHA_DEMO {
        return Err(format!(
            "ERR_ORACLE_CLASS_REJECTED: oracle market '{symbol}' is strictly prohibited for Perpetuals execution"
        ));
    }
    Ok(())
}

/// Infer strict `(data_class, worker_mode)` pair from an external observation, failing closed on banned/unknown classes.
pub fn validate_external_observation_metadata(
    obs: &ExternalPriceObservation,
    expected_oracle_market: &str,
) -> Result<&'static str, String> {
    assert_market_symbol_not_prohibited_oracle(expected_oracle_market)?;
    assert_market_symbol_not_prohibited_oracle(&obs.market)?;

    if obs.market != expected_oracle_market {
        return Err(format!(
            "ERR_ORACLE_MARKET_MISMATCH: expected '{expected_oracle_market}', got '{}'",
            obs.market
        ));
    }
    if obs.decimals != 8 {
        return Err(format!(
            "ERR_ORACLE_DECIMALS_MISMATCH: expected 8 decimals, got {}",
            obs.decimals
        ));
    }
    if obs.unit != UNIT_USD_PER_KG {
        return Err(format!(
            "ERR_ORACLE_UNIT_MISMATCH: expected '{UNIT_USD_PER_KG}', got '{}'",
            obs.unit
        ));
    }
    if obs.data_class == DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY
        || obs.data_class == DATA_CLASS_SYNTHETIC
    {
        return Err(format!(
            "ERR_ORACLE_CLASS_REJECTED: banned data_class '{}'",
            obs.data_class
        ));
    }
    match obs.data_class.as_str() {
        DATA_CLASS_LIVE_VERIFIED | DATA_CLASS_LIVE_AGGREGATED => Ok(MODE_LIVE_INDEX),
        DATA_CLASS_TESTNET_SANDBOX => Ok(MODE_TESTNET_SANDBOX),
        other => Err(format!(
            "ERR_ORACLE_CLASS_REJECTED: unsupported external data_class '{other}'"
        )),
    }
}

/// Validate execution spread & sanity bounds on sandbox price config.
pub fn validate_sandbox_price_bounds(min_price_e8: u128, max_price_e8: u128) -> Result<(), String> {
    if min_price_e8 == 0 || min_price_e8 >= max_price_e8 {
        return Err(
            "ERR_INVALID_PRICE_BOUNDS: min_price_e8 must be > 0 and < max_price_e8".to_string(),
        );
    }
    if max_price_e8 > 1_000_000 * PRICE_SCALE {
        return Err(
            "ERR_INVALID_PRICE_BOUNDS: max_price_e8 exceeds sane safety ceiling".to_string(),
        );
    }
    Ok(())
}

/// Outcome of evaluating a candidate price tick against `roastake_perps_core::evaluate_oracle_for_perps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluatedPriceResult {
    Valid { price_e8: u128, age_sec: u64 },
    TriggerReduceOnly { code: String },
    Rejected { code: String },
}

/// Validate a raw candidate price observation and update `market.mode` / `last_valid_price_e8` metadata.
pub fn evaluate_candidate_price_against_policy(
    market: &mut PerpsMarketState,
    price_e8: u128,
    observed_at_sec: u64,
    data_class: &str,
    worker_mode: &str,
    now_sec: u64,
) -> EvaluatedPriceResult {
    if market.mode == StoredMarketMode::Emergency {
        return EvaluatedPriceResult::Rejected {
            code: "ERR_MARKET_EMERGENCY_FROZEN".to_string(),
        };
    }

    let policy = market.oracle_policy.to_shared();
    match evaluate_oracle_for_perps(
        price_e8,
        observed_at_sec,
        data_class,
        worker_mode,
        market.last_valid_price_e8,
        now_sec,
        &policy,
    ) {
        Ok(OracleHealthOutcome::ValidForExecution {
            price_e8: valid_price,
            age_sec,
            tier: _,
        }) => {
            market.last_valid_price_e8 = Some(valid_price);
            market.last_valid_observed_sec = Some(observed_at_sec);
            EvaluatedPriceResult::Valid {
                price_e8: valid_price,
                age_sec,
            }
        }
        Ok(OracleHealthOutcome::StaleReduceOnly { age_sec }) => {
            if market.mode == StoredMarketMode::Normal {
                market.mode = StoredMarketMode::ReduceOnly;
            }
            EvaluatedPriceResult::TriggerReduceOnly {
                code: format!("ERR_ORACLE_STALE_REDUCE_ONLY: age {age_sec}s exceeds limit"),
            }
        }
        Ok(OracleHealthOutcome::EmergencyEligible { age_sec }) => {
            if market.mode == StoredMarketMode::Normal {
                market.mode = StoredMarketMode::ReduceOnly;
            }
            EvaluatedPriceResult::TriggerReduceOnly {
                code: format!(
                    "ERR_ORACLE_EMERGENCY_ELIGIBLE: outage {age_sec}s reached emergency threshold"
                ),
            }
        }
        Ok(OracleHealthOutcome::JumpCircuitBreaker {
            previous_price_e8,
            observed_price_e8,
            jump_bps,
        }) => {
            if market.mode == StoredMarketMode::Normal {
                market.mode = StoredMarketMode::ReduceOnly;
            }
            EvaluatedPriceResult::TriggerReduceOnly {
                code: format!(
                    "ERR_PRICE_JUMP_CIRCUIT_BREAKER: prev={previous_price_e8}, obs={observed_price_e8}, jump={jump_bps}bps"
                ),
            }
        }
        Err(e) => EvaluatedPriceResult::Rejected {
            code: format!("ERR_ORACLE_VALIDATION_FAILED: {e:?}"),
        },
    }
}

/// Evaluate local `TESTNET_SANDBOX_FEED` observation for synchronous Testnet sandbox execution.
pub fn evaluate_sandbox_feed_for_execution(
    market: &mut PerpsMarketState,
    now_sec: u64,
) -> Result<u128, String> {
    match &market.oracle_source {
        OracleSourceConfig::TestnetSandboxOperator { .. } => {}
        OracleSourceConfig::ExternalXcc { .. } => {
            return Err("ERR_REQUIRES_ASYNC_XCC_ORACLE".to_string());
        }
    }
    let obs = market
        .sandbox_observation
        .clone()
        .ok_or_else(|| "ERR_ORACLE_PRICE_MISSING".to_string())?;

    match evaluate_candidate_price_against_policy(
        market,
        obs.price_e8,
        obs.observed_at_sec,
        &obs.data_class,
        &obs.worker_mode,
        now_sec,
    ) {
        EvaluatedPriceResult::Valid { price_e8, .. } => Ok(price_e8),
        EvaluatedPriceResult::TriggerReduceOnly { code }
        | EvaluatedPriceResult::Rejected { code } => Err(code),
    }
}

/// Extract the reference timestamp anchor used to compute outage age when checking Emergency eligibility.
pub fn emergency_reference_observed_sec(market: &PerpsMarketState) -> Option<u64> {
    if let Some(ts) = market.last_valid_observed_sec {
        return Some(ts);
    }
    market
        .sandbox_observation
        .as_ref()
        .map(|obs| obs.observed_at_sec)
}
