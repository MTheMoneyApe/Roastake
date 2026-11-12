//! Shared types, schemas, and validation utilities for ROASTAKE.
//!
//! Provides canonical data structures exchanged between the NEAR Oracle Adapter contract
//! and the OutLayer WASI Coffee Price Index worker.

use serde::{Deserialize, Serialize};

pub mod fixed_point;
pub use fixed_point::{
    convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8,
    convert_ice_usc_per_lb_e12_to_usd_per_kg_e8, convert_usc_per_lb_e12_to_usd_per_kg_e8,
    parse_decimal_e12, parse_decimal_e12_round_half_up, parse_decimal_e8, round_half_up_div_u128,
    FixedPointError, CEPEA_BAG_WEIGHT_KG, FIXED_POINT_SCALE_E12, FIXED_POINT_SCALE_E12_DECIMALS,
    FIXED_POINT_SCALE_E8, FIXED_POINT_SCALE_E8_DECIMALS, MAX_HIGH_PRECISION_DECIMAL_FRAC_DIGITS,
    SI_GRAMS_PER_POUND_E8,
};

/// Canonical schema version supported by this release.
pub const SCHEMA_VERSION: u32 = 1;

/// Initial demo market symbol for coffee price index.
pub const MARKET_GESHA_DEMO: &str = "GESHA-DEMO/USD";

/// Public historical monthly benchmark market sourced from IMF PCPS (`PCOFFOTM`).
pub const MARKET_ARABICA_MONTHLY_IMF: &str = "ARABICA-MONTHLY-IMF/USD";

/// Deterministic fixture execution mode.
pub const MODE_FIXTURE: &str = "fixture";

/// Historical public benchmark execution mode (no API key, monthly lag).
pub const MODE_PUBLIC_BENCHMARK: &str = "public_benchmark";

/// Live coffee index execution mode backed by real external sources.
pub const MODE_LIVE_INDEX: &str = "live_index";

/// Canonical source tag expected from deterministic fixture executions.
pub const DEFAULT_FIXTURE_SOURCE: &str = "roastake-fixture";

/// Canonical source identifier for IMF PCPS Other Mild Arabica (`PCOFFOTM`).
pub const DEFAULT_IMF_PCPS_SOURCE: &str = "imf-pcps-pcoffotm";

/// Maximum UTF-8 byte length for `WorkerOutput.source` / `PriceObservation.source`.
pub const MAX_WORKER_SOURCE_BYTES: usize = 128;

/// Maximum UTF-8 byte length for `WorkerOutput.data_class` / `PriceObservation.data_class`.
pub const MAX_WORKER_DATA_CLASS_BYTES: usize = 32;

/// Default maximum source spread allowed across participating providers (500 bps = 5%).
pub const DEFAULT_MAX_SOURCE_SPREAD_BPS: u32 = 500;

/// Standard unit of measure for coffee index observations.
pub const UNIT_USD_PER_KG: &str = "USD/kg";

/// Standard fixed-point decimal precision for coffee prices (10^8).
pub const DECIMALS_COFFEE_INDEX: u8 = 8;

/// Data classification tag for synthetic demo observations.
pub const DATA_CLASS_SYNTHETIC: &str = "SYNTHETIC_DEMO";

/// Data classification tag for single-source historical monthly public benchmark observations.
pub const DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY: &str = "PUBLIC_BENCHMARK_MONTHLY";

/// Data classification tag for single-source verified live observations.
pub const DATA_CLASS_LIVE_VERIFIED: &str = "LIVE_VERIFIED";

/// Data classification tag for multi-source aggregated live observations.
pub const DATA_CLASS_LIVE_AGGREGATED: &str = "LIVE_AGGREGATED";

/// Mandatory attribution & disclosure notice for IMF PCPS (`PCOFFOTM`) metadata and docs (241 bytes <= 256).
pub const IMF_PCPS_ATTRIBUTION_NOTICE: &str = "Source: International Monetary Fund (IMF), Primary Commodity Price System (PCPS), PCOFFOTM — ICO Other Mild Arabicas New York cash monthly average. Historical monthly data, not live ICE Coffee C futures, and does not imply IMF endorsement.";

/// Official OutLayer coordinator contract on NEAR Testnet.
pub const DEFAULT_OUTLAYER_TESTNET: &str = "outlayer.testnet";

/// Default FastFS Testnet endpoint for WASM binaries.
pub const DEFAULT_FASTFS_BASE_URL: &str = "https://test.fastfs.io";

/// Default WASI build target for OutLayer workers.
pub const DEFAULT_BUILD_TARGET: &str = "wasm32-wasip2";

/// Default maximum allowable price age in seconds (1 hour).
pub const DEFAULT_MAX_PRICE_AGE_SEC: u64 = 3600;

/// Maximum allowable price age ceiling for `live_index` markets (24 hours).
pub const DEFAULT_LIVE_INDEX_MAX_PRICE_AGE_SEC: u64 = 24 * 60 * 60;

/// Historical monthly benchmark maximum allowable price age in seconds (90 days = 7,776,000 s).
/// Applies strictly to `ARABICA-MONTHLY-IMF/USD`; never weakens live or fixture markets.
pub const HISTORICAL_MONTHLY_MAX_PRICE_AGE_SEC: u64 = 90 * 24 * 60 * 60;

/// Maximum allowable clock drift into the future in seconds (1 minute).
pub const DEFAULT_MAX_CLOCK_DRIFT_SEC: u64 = 60;

/// Default maximum price divergence in basis points (2000 bps = 20%).
pub const DEFAULT_MAX_DIVERGENCE_BPS: u32 = 2000;

/// Demonstration fixture price in 8-decimal fixed-point format ($18.50/kg).
pub const DEFAULT_FIXTURE_PRICE_RAW: u128 = 1_850_000_000;

/// Demonstration fixture price string as expected from the worker.
pub const DEFAULT_FIXTURE_PRICE_STR: &str = "1850000000";

/// Input payload delivered to the Coffee Price Index WASI worker via stdin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerInput {
    pub schema_version: u32,
    pub market: String,
    pub mode: String,
    /// Minimum participating sources required by market policy (used in `live_index` mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_source_count: Option<u32>,
    /// Maximum acceptable upstream quote age in seconds (used in `live_index` mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_upstream_age_sec: Option<u64>,
    /// Maximum allowed dispersion across participating sources in bps (used in `live_index` mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_source_spread_bps: Option<u32>,
}

impl WorkerInput {
    /// Construct a deterministic fixture request preserving legacy wire JSON compatibility.
    pub fn new_fixture(market: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            market: market.into(),
            mode: MODE_FIXTURE.to_string(),
            min_source_count: None,
            max_upstream_age_sec: None,
            max_source_spread_bps: None,
        }
    }

    /// Construct a public historical benchmark execution request (e.g., IMF PCPS monthly series).
    pub fn new_public_benchmark(market: impl Into<String>, max_upstream_age_sec: u64) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            market: market.into(),
            mode: MODE_PUBLIC_BENCHMARK.to_string(),
            min_source_count: Some(1),
            max_upstream_age_sec: Some(max_upstream_age_sec),
            max_source_spread_bps: Some(0),
        }
    }

    /// Construct a live index execution request carrying explicit market policy thresholds.
    pub fn new_live_index(
        market: impl Into<String>,
        min_source_count: u32,
        max_upstream_age_sec: u64,
        max_source_spread_bps: u32,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            market: market.into(),
            mode: MODE_LIVE_INDEX.to_string(),
            min_source_count: Some(min_source_count),
            max_upstream_age_sec: Some(max_upstream_age_sec),
            max_source_spread_bps: Some(max_source_spread_bps),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema_version {}, expected {}",
                self.schema_version, SCHEMA_VERSION
            ));
        }
        if self.market.trim().is_empty() {
            return Err("market identifier cannot be empty".to_string());
        }
        match self.mode.as_str() {
            MODE_FIXTURE => {
                if self.min_source_count.is_some()
                    || self.max_upstream_age_sec.is_some()
                    || self.max_source_spread_bps.is_some()
                {
                    return Err(
                        "fixture mode must not specify live policy threshold parameters"
                            .to_string(),
                    );
                }
                Ok(())
            }
            MODE_PUBLIC_BENCHMARK => {
                if self.market != MARKET_ARABICA_MONTHLY_IMF {
                    return Err(format!(
                        "public_benchmark mode is restricted strictly to '{}', got '{}'",
                        MARKET_ARABICA_MONTHLY_IMF, self.market
                    ));
                }
                if self.min_source_count != Some(1) {
                    return Err("public_benchmark mode requires min_source_count == 1".to_string());
                }
                let max_age = self.max_upstream_age_sec.ok_or_else(|| {
                    "public_benchmark mode requires explicit max_upstream_age_sec".to_string()
                })?;
                if max_age == 0 || max_age > HISTORICAL_MONTHLY_MAX_PRICE_AGE_SEC {
                    return Err(format!(
                        "public_benchmark max_upstream_age_sec ({}) must be in 1..={} seconds",
                        max_age, HISTORICAL_MONTHLY_MAX_PRICE_AGE_SEC
                    ));
                }
                if self.max_source_spread_bps != Some(0) {
                    return Err(
                        "public_benchmark mode requires max_source_spread_bps == 0".to_string()
                    );
                }
                Ok(())
            }
            MODE_LIVE_INDEX => {
                if self.market == MARKET_GESHA_DEMO || self.market == MARKET_ARABICA_MONTHLY_IMF {
                    return Err(format!(
                        "market '{}' is not permitted in live_index mode",
                        self.market
                    ));
                }
                let min_sources = self.min_source_count.ok_or_else(|| {
                    "live_index mode requires explicit min_source_count".to_string()
                })?;
                if min_sources < 1 {
                    return Err("live_index min_source_count must be at least 1".to_string());
                }
                let max_age = self.max_upstream_age_sec.ok_or_else(|| {
                    "live_index mode requires explicit max_upstream_age_sec".to_string()
                })?;
                if max_age == 0 {
                    return Err(
                        "live_index max_upstream_age_sec must be greater than zero".to_string()
                    );
                }
                if self.max_source_spread_bps.is_none() {
                    return Err(
                        "live_index mode requires explicit max_source_spread_bps".to_string()
                    );
                }
                Ok(())
            }
            other => Err(format!(
                "unsupported mode '{}', expected '{}', '{}' or '{}'",
                other, MODE_FIXTURE, MODE_PUBLIC_BENCHMARK, MODE_LIVE_INDEX
            )),
        }
    }
}

/// Output payload emitted by the Coffee Price Index WASI worker via stdout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerOutput {
    pub schema_version: u32,
    pub market: String,
    pub price: String,
    pub decimals: u8,
    pub unit: String,
    pub source: String,
    pub source_count: u32,
    pub observed_at: u64,
    pub data_class: String,
}

impl WorkerOutput {
    pub fn fixture_gesha_demo() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            market: MARKET_GESHA_DEMO.to_string(),
            price: DEFAULT_FIXTURE_PRICE_STR.to_string(),
            decimals: DECIMALS_COFFEE_INDEX,
            unit: UNIT_USD_PER_KG.to_string(),
            source: DEFAULT_FIXTURE_SOURCE.to_string(),
            source_count: 1,
            observed_at: 0,
            data_class: DATA_CLASS_SYNTHETIC.to_string(),
        }
    }

    pub fn parse_price(&self) -> Result<u128, String> {
        self.price.parse::<u128>().map_err(|e| {
            format!(
                "failed to parse price '{}' as u128 integer: {}",
                self.price, e
            )
        })
    }

    pub fn validate(&self, expected_market: &str) -> Result<u128, String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema_version {}, expected {}",
                self.schema_version, SCHEMA_VERSION
            ));
        }
        if self.market != expected_market {
            return Err(format!(
                "market mismatch: expected '{}', got '{}'",
                expected_market, self.market
            ));
        }
        if self.unit != UNIT_USD_PER_KG {
            return Err(format!(
                "unit mismatch: expected '{}', got '{}'",
                UNIT_USD_PER_KG, self.unit
            ));
        }
        if self.decimals != DECIMALS_COFFEE_INDEX {
            return Err(format!(
                "decimals mismatch: expected {}, got {}",
                DECIMALS_COFFEE_INDEX, self.decimals
            ));
        }
        if self.source.trim().is_empty() {
            return Err("source cannot be empty".to_string());
        }
        if self.source.len() > MAX_WORKER_SOURCE_BYTES {
            return Err(format!(
                "source UTF-8 byte length ({}) exceeds maximum ({} bytes)",
                self.source.len(),
                MAX_WORKER_SOURCE_BYTES
            ));
        }
        if self.source_count == 0 {
            return Err("source_count must be at least 1".to_string());
        }
        if self.data_class.len() > MAX_WORKER_DATA_CLASS_BYTES {
            return Err(format!(
                "data_class UTF-8 byte length ({}) exceeds maximum ({} bytes)",
                self.data_class.len(),
                MAX_WORKER_DATA_CLASS_BYTES
            ));
        }
        match self.data_class.as_str() {
            DATA_CLASS_SYNTHETIC => {
                if self.market != MARKET_GESHA_DEMO {
                    return Err(format!(
                        "'{}' is restricted strictly to '{}'",
                        DATA_CLASS_SYNTHETIC, MARKET_GESHA_DEMO
                    ));
                }
                if self.source != DEFAULT_FIXTURE_SOURCE {
                    return Err(format!(
                        "synthetic output requires source '{}', got '{}'",
                        DEFAULT_FIXTURE_SOURCE, self.source
                    ));
                }
                if self.source_count != 1 {
                    return Err(format!(
                        "synthetic output requires source_count == 1, got {}",
                        self.source_count
                    ));
                }
            }
            DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY => {
                if self.market != MARKET_ARABICA_MONTHLY_IMF {
                    return Err(format!(
                        "'{}' is restricted strictly to '{}'",
                        DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY, MARKET_ARABICA_MONTHLY_IMF
                    ));
                }
                if self.source != DEFAULT_IMF_PCPS_SOURCE {
                    return Err(format!(
                        "public benchmark output requires source '{}', got '{}'",
                        DEFAULT_IMF_PCPS_SOURCE, self.source
                    ));
                }
                if self.source_count != 1 {
                    return Err(format!(
                        "public benchmark output requires source_count == 1, got {}",
                        self.source_count
                    ));
                }
                if self.observed_at == 0 {
                    return Err(
                        "public benchmark output requires positive observed_at timestamp"
                            .to_string(),
                    );
                }
            }
            DATA_CLASS_LIVE_VERIFIED => {
                if self.market == MARKET_GESHA_DEMO || self.market == MARKET_ARABICA_MONTHLY_IMF {
                    return Err(format!(
                        "'{}' cannot be classified as '{}'",
                        self.market, DATA_CLASS_LIVE_VERIFIED
                    ));
                }
                if self.observed_at == 0 {
                    return Err(
                        "LIVE_VERIFIED output requires positive observed_at timestamp".to_string(),
                    );
                }
            }
            DATA_CLASS_LIVE_AGGREGATED => {
                if self.market == MARKET_GESHA_DEMO || self.market == MARKET_ARABICA_MONTHLY_IMF {
                    return Err(format!(
                        "'{}' cannot be classified as '{}'",
                        self.market, DATA_CLASS_LIVE_AGGREGATED
                    ));
                }
                if self.source_count < 2 {
                    return Err(format!(
                        "LIVE_AGGREGATED output requires at least 2 sources, got {}",
                        self.source_count
                    ));
                }
                if self.observed_at == 0 {
                    return Err(
                        "LIVE_AGGREGATED output requires positive observed_at timestamp"
                            .to_string(),
                    );
                }
            }
            other => {
                return Err(format!("unsupported data_class '{}'", other));
            }
        }
        let price_u128 = self.parse_price()?;
        if price_u128 == 0 {
            return Err("price must be greater than zero".to_string());
        }
        Ok(price_u128)
    }
}

/// OutLayer ExecutionSource specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExecutionSource {
    WasmUrl {
        #[serde(rename = "WasmUrl")]
        wasm_url: WasmUrlSpec,
    },
    Project {
        #[serde(rename = "Project")]
        project: ProjectSpec,
    },
    GitHub {
        #[serde(rename = "GitHub")]
        github: GitHubSpec,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WasmUrlSpec {
    pub url: String,
    pub hash: String,
    pub build_target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSpec {
    pub project_id: String,
    pub version_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubSpec {
    pub repo: String,
    pub commit: String,
    pub build_target: String,
}

impl ExecutionSource {
    pub fn wasm_url(
        url: impl Into<String>,
        hash: impl Into<String>,
        build_target: impl Into<String>,
    ) -> Self {
        Self::WasmUrl {
            wasm_url: WasmUrlSpec {
                url: url.into(),
                hash: hash.into(),
                build_target: build_target.into(),
            },
        }
    }
}

/// Resource limits enforced inside OutLayer's TEE worker.
///
/// Fields are optional to match OutLayer's upstream `ResourceLimits` type.
/// Omitting a field lets OutLayer apply its own defaults; in practice ROASTAKE
/// always supplies explicit values via `Default::default()`.
///
/// Verified against OutLayer contract source and deployed `outlayer.testnet`:
/// - `max_instructions`: u64 (Default: 1_000_000_000, Max: 500_000_000_000)
/// - `max_memory_mb`: u32 (Default: 128 MB, Max: 4096 MB)
/// - `max_execution_seconds`: u64 (Default: 60s, Max: 180s)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_instructions: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_memory_mb: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_execution_seconds: Option<u64>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_instructions: Some(1_000_000_000),
            max_memory_mb: Some(128),
            max_execution_seconds: Some(60),
        }
    }
}

/// Advanced execution control parameters for OutLayer (`params`).
///
/// Verified against OutLayer contract source (`RequestParams` struct).
/// Fields not supplied default to `false` / `None` via serde defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RequestParams {
    /// Force a rebuild of the WASM worker even if a cached version exists.
    #[serde(default, skip_serializing_if = "is_false")]
    pub force_rebuild: bool,
    /// Persist the WASM output artifact on FastFS after execution.
    #[serde(default, skip_serializing_if = "is_false")]
    pub store_on_fastfs: bool,
    /// Run the compilation step only without executing the WASM.
    #[serde(default, skip_serializing_if = "is_false")]
    pub compile_only: bool,
    /// Project UUID (set internally by OutLayer; callers should leave as None).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_uuid: Option<String>,
    /// USD balance attached for Project-source executions (U128 as decimal string).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attached_usd: Option<String>,
    /// Use the OutLayer-bound identity for the execution context.
    #[serde(default, skip_serializing_if = "is_false")]
    pub use_bound_identity: bool,
}

fn is_false(v: &bool) -> bool {
    !v
}

/// Reference to encrypted credentials stored on-chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretsReference {
    pub profile: String,
    pub account_id: String,
}

/// Optional OutLayer wrapper structure when returning execution outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionResponseWrapper {
    pub status: Option<String>,
    pub output: Option<ExecutionOutputPayload>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionOutputPayload {
    pub data: Option<serde_json::Value>,
    pub stdout: Option<String>,
}

/// Extracts a canonical `WorkerOutput` from raw bytes returned by OutLayer.
///
/// Handles:
/// 1. Direct JSON serialization of `WorkerOutput`.
/// 2. JSON string literal wrapping the JSON object.
/// 3. OutLayer `ExecutionResponseWrapper` containing `output.data` or `output.stdout`.
/// 4. Arbitrary JSON object with matching fields.
pub fn extract_worker_output(data: &[u8]) -> Result<WorkerOutput, String> {
    if data.is_empty() {
        return Err("empty execution response data".to_string());
    }

    // 1. Direct parse attempt
    if let Ok(output) = serde_json::from_slice::<WorkerOutput>(data) {
        return Ok(output);
    }

    // Parse as general serde_json::Value
    let value: serde_json::Value = serde_json::from_slice(data)
        .map_err(|e| format!("failed to parse raw response as JSON: {}", e))?;

    // 2. If it's a string, try parsing string contents as JSON
    if let Some(s) = value.as_str() {
        return serde_json::from_str::<WorkerOutput>(s).map_err(|e| {
            format!(
                "failed to parse JSON string inner content as WorkerOutput: {}",
                e
            )
        });
    }

    // 3. If it has wrapper format with status / error
    if let Some(status) = value.get("status").and_then(|v| v.as_str()) {
        if status.eq_ignore_ascii_case("failed") || status.eq_ignore_ascii_case("error") {
            let err_msg = value
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("execution failed in TEE worker");
            return Err(format!("OutLayer reported error: {}", err_msg));
        }
    }

    // Check output.data or output.stdout
    if let Some(output_obj) = value.get("output") {
        if let Some(data_field) = output_obj.get("data") {
            if let Some(s) = data_field.as_str() {
                if let Ok(out) = serde_json::from_str::<WorkerOutput>(s) {
                    return Ok(out);
                }
            } else if let Ok(out) = serde_json::from_value::<WorkerOutput>(data_field.clone()) {
                return Ok(out);
            }
        }
        if let Some(stdout_str) = output_obj.get("stdout").and_then(|v| v.as_str()) {
            if let Ok(out) = serde_json::from_str::<WorkerOutput>(stdout_str) {
                return Ok(out);
            }
        }
    }

    // 4. Try parsing value directly as WorkerOutput
    serde_json::from_value::<WorkerOutput>(value)
        .map_err(|e| format!("failed to map JSON value to WorkerOutput: {}", e))
}

/// Calculate price divergence in basis points (1 bps = 0.01%).
pub fn calculate_divergence_bps(last_price: u128, new_price: u128) -> u32 {
    if last_price == 0 {
        return 0;
    }
    let diff = new_price.abs_diff(last_price);
    // diff * 10000 / last_price
    let bps = (diff.saturating_mul(10_000)) / last_price;
    bps.min(u32::MAX as u128) as u32
}

/// Validate that an account identifier conforms strictly to NEAR Testnet rules.
/// Rejects any account ending in `.near` (Mainnet) or referencing Mainnet hosts.
pub fn validate_testnet_account_id(account_id: &str) -> Result<(), String> {
    let lower = account_id.to_lowercase();
    if lower.ends_with(".near") {
        return Err(format!(
            "mainnet account '{}' rejected: only NEAR Testnet accounts (*.testnet) are permitted",
            account_id
        ));
    }
    if lower.contains("mainnet") {
        return Err(format!(
            "mainnet reference in account '{}' rejected: testnet policy violation",
            account_id
        ));
    }
    if !lower.ends_with(".testnet")
        && !lower
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "invalid account format '{}': must end with .testnet or be a valid test identifier",
            account_id
        ));
    }
    Ok(())
}

/// Format 8-decimal fixed-point price into human-readable string (e.g. 1850000000 -> "18.50000000").
pub fn format_display_price(price: u128, decimals: u8) -> String {
    let factor = 10u128.pow(decimals as u32);
    let whole = price / factor;
    let frac = price % factor;
    format!("{}.{:0width$}", whole, frac, width = decimals as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_output_validation() {
        let output = WorkerOutput::fixture_gesha_demo();
        let price = output.validate(MARKET_GESHA_DEMO).expect("valid fixture");
        assert_eq!(price, DEFAULT_FIXTURE_PRICE_RAW);
    }

    #[test]
    fn test_worker_output_wrong_market() {
        let output = WorkerOutput::fixture_gesha_demo();
        let err = output.validate("ETH/USD").unwrap_err();
        assert!(err.contains("market mismatch"));
    }

    #[test]
    fn test_extract_worker_output_direct() {
        let output = WorkerOutput::fixture_gesha_demo();
        let json = serde_json::to_vec(&output).unwrap();
        let extracted = extract_worker_output(&json).unwrap();
        assert_eq!(extracted, output);
    }

    #[test]
    fn test_extract_worker_output_wrapped() {
        let output = WorkerOutput::fixture_gesha_demo();
        let output_json = serde_json::to_string(&output).unwrap();
        let wrapper = serde_json::json!({
            "status": "Success",
            "output": {
                "data": output_json,
                "stdout": ""
            }
        });
        let json = serde_json::to_vec(&wrapper).unwrap();
        let extracted = extract_worker_output(&json).unwrap();
        assert_eq!(extracted, output);
    }

    #[test]
    fn test_calculate_divergence_bps() {
        let last = 1_000_000_000u128; // $10.00
        let new_price = 1_100_000_000u128; // $11.00 (+10%)
        let bps = calculate_divergence_bps(last, new_price);
        assert_eq!(bps, 1000); // 10% = 1000 bps

        let dropped = 900_000_000u128; // $9.00 (-10%)
        let bps_drop = calculate_divergence_bps(last, dropped);
        assert_eq!(bps_drop, 1000);
    }

    #[test]
    fn test_validate_testnet_account() {
        assert!(validate_testnet_account_id("oracle.testnet").is_ok());
        assert!(validate_testnet_account_id("outlayer.testnet").is_ok());
        assert!(validate_testnet_account_id("roastake-admin.testnet").is_ok());
        assert!(validate_testnet_account_id("local-test").is_ok());

        assert!(validate_testnet_account_id("outlayer.near").is_err());
        assert!(validate_testnet_account_id("alice.near").is_err());
        assert!(validate_testnet_account_id("mainnet-oracle.testnet").is_err());
    }

    #[test]
    fn test_format_display_price() {
        assert_eq!(format_display_price(1_850_000_000, 8), "18.50000000");
        assert_eq!(format_display_price(50_000_000, 8), "0.50000000");
    }

    #[test]
    fn test_outlayer_wasm_url_serialization() {
        let source = ExecutionSource::wasm_url(
            "https://test.fastfs.io/roastake/worker.wasm",
            "abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6",
            "wasm32-wasip2",
        );
        let serialized = serde_json::to_string(&source).unwrap();
        assert!(serialized.contains("\"WasmUrl\""));
        assert!(serialized.contains("\"url\":\"https://test.fastfs.io/roastake/worker.wasm\""));
        assert!(serialized.contains(
            "\"hash\":\"abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6\""
        ));
        assert!(serialized.contains("\"build_target\":\"wasm32-wasip2\""));

        // Verify roundtrip deserialization
        let deserialized: ExecutionSource = serde_json::from_str(&serialized).unwrap();
        assert_eq!(source, deserialized);
    }

    #[test]
    fn test_resource_limits_serialization() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.max_instructions, Some(1_000_000_000));
        assert_eq!(limits.max_memory_mb, Some(128));
        assert_eq!(limits.max_execution_seconds, Some(60));

        let json_val = serde_json::to_value(&limits).unwrap();
        assert_eq!(json_val["max_instructions"], 1_000_000_000u64);
        assert_eq!(json_val["max_memory_mb"], 128);
        assert_eq!(json_val["max_execution_seconds"], 60);

        // Verify partial/empty limits serialize with omitted fields
        let custom = ResourceLimits {
            max_instructions: Some(5_000_000_000),
            max_memory_mb: None,
            max_execution_seconds: None,
        };
        let custom_json = serde_json::to_string(&custom).unwrap();
        assert!(custom_json.contains("\"max_instructions\":5000000000"));
        assert!(!custom_json.contains("max_memory_mb"));
        assert!(!custom_json.contains("max_execution_seconds"));
    }

    #[test]
    fn test_request_params_serialization() {
        let default_params = RequestParams::default();
        let default_json = serde_json::to_string(&default_params).unwrap();
        // With skip_serializing_if, defaults serialize as empty object
        assert_eq!(default_json, "{}");

        let active_params = RequestParams {
            force_rebuild: true,
            store_on_fastfs: false,
            compile_only: false,
            project_uuid: None,
            attached_usd: Some("5000000".to_string()),
            use_bound_identity: true,
        };
        let active_json = serde_json::to_string(&active_params).unwrap();
        assert!(active_json.contains("\"force_rebuild\":true"));
        assert!(active_json.contains("\"attached_usd\":\"5000000\""));
        assert!(active_json.contains("\"use_bound_identity\":true"));
        assert!(!active_json.contains("store_on_fastfs"));
    }

    #[test]
    fn test_worker_input_fixture_backwards_compatibility() {
        let input = WorkerInput::new_fixture(MARKET_GESHA_DEMO);
        assert!(input.validate().is_ok());

        let json = serde_json::to_string(&input).unwrap();
        assert_eq!(
            json,
            r#"{"schema_version":1,"market":"GESHA-DEMO/USD","mode":"fixture"}"#
        );

        let legacy_raw = r#"{"schema_version":1,"market":"GESHA-DEMO/USD","mode":"fixture"}"#;
        let parsed: WorkerInput = serde_json::from_str(legacy_raw).unwrap();
        assert_eq!(parsed, input);
    }

    #[test]
    fn test_worker_input_live_index_and_secrets_ref_serialization() {
        let input = WorkerInput::new_live_index("ARABICA-REF/USD", 2, 3600, 500);
        assert!(input.validate().is_ok());
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"mode\":\"live_index\""));
        assert!(json.contains("\"min_source_count\":2"));
        assert!(json.contains("\"max_upstream_age_sec\":3600"));
        assert!(json.contains("\"max_source_spread_bps\":500"));

        let mut bad_fixture = WorkerInput::new_fixture(MARKET_GESHA_DEMO);
        bad_fixture.min_source_count = Some(1);
        assert!(bad_fixture.validate().is_err());

        let mut bad_live = input.clone();
        bad_live.min_source_count = Some(0);
        assert!(bad_live.validate().is_err());

        let mut missing_spread = input;
        missing_spread.max_source_spread_bps = None;
        assert!(missing_spread.validate().is_err());

        let secrets_ref = SecretsReference {
            profile: "prod".to_string(),
            account_id: "roastake-secrets.testnet".to_string(),
        };
        let sr_json = serde_json::to_string(&secrets_ref).unwrap();
        assert_eq!(
            sr_json,
            r#"{"profile":"prod","account_id":"roastake-secrets.testnet"}"#
        );
    }

    #[test]
    fn test_public_benchmark_monthly_class_bounds_and_validation() {
        assert!(
            DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY.len() <= MAX_WORKER_DATA_CLASS_BYTES,
            "DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY ({} bytes) must fit in {} bytes",
            DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY.len(),
            MAX_WORKER_DATA_CLASS_BYTES
        );
        assert!(DEFAULT_IMF_PCPS_SOURCE.len() <= MAX_WORKER_SOURCE_BYTES);
        assert!(IMF_PCPS_ATTRIBUTION_NOTICE.len() <= 256);

        let input = WorkerInput::new_public_benchmark(
            MARKET_ARABICA_MONTHLY_IMF,
            HISTORICAL_MONTHLY_MAX_PRICE_AGE_SEC,
        );
        assert!(input.validate().is_ok());

        let mut wrong_mkt_in = input.clone();
        wrong_mkt_in.market = "ARABICA-REF/USD".to_string();
        assert!(wrong_mkt_in.validate().is_err());

        let mut excessive_age_in = input.clone();
        excessive_age_in.max_upstream_age_sec = Some(HISTORICAL_MONTHLY_MAX_PRICE_AGE_SEC + 1);
        assert!(excessive_age_in.validate().is_err());

        // Reject ARABICA-MONTHLY-IMF/USD disguised under live_index
        let bad_live_in = WorkerInput::new_live_index(MARKET_ARABICA_MONTHLY_IMF, 1, 3600, 0);
        assert!(bad_live_in.validate().is_err());

        let valid_out = WorkerOutput {
            schema_version: SCHEMA_VERSION,
            market: MARKET_ARABICA_MONTHLY_IMF.to_string(),
            price: "727794219".to_string(),
            decimals: DECIMALS_COFFEE_INDEX,
            unit: UNIT_USD_PER_KG.to_string(),
            source: DEFAULT_IMF_PCPS_SOURCE.to_string(),
            source_count: 1,
            observed_at: 1_790_812_799,
            data_class: DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY.to_string(),
        };
        assert_eq!(
            valid_out.validate(MARKET_ARABICA_MONTHLY_IMF).unwrap(),
            727_794_219
        );

        // Wrong market rejected
        let wrong_market_out = WorkerOutput {
            market: "ARABICA-REF/USD".to_string(),
            ..valid_out.clone()
        };
        assert!(wrong_market_out.validate("ARABICA-REF/USD").is_err());

        // Wrong source rejected
        let wrong_src_out = WorkerOutput {
            source: "fred-csv".to_string(),
            ..valid_out.clone()
        };
        assert!(wrong_src_out.validate(MARKET_ARABICA_MONTHLY_IMF).is_err());

        // Zero observed_at rejected
        let zero_ts_out = WorkerOutput {
            observed_at: 0,
            ..valid_out.clone()
        };
        assert!(zero_ts_out.validate(MARKET_ARABICA_MONTHLY_IMF).is_err());

        // source_count != 1 rejected
        let bad_cnt_out = WorkerOutput {
            source_count: 2,
            ..valid_out.clone()
        };
        assert!(bad_cnt_out.validate(MARKET_ARABICA_MONTHLY_IMF).is_err());

        // Never allow IMF benchmark to masquerade as LIVE_VERIFIED or LIVE_AGGREGATED
        let as_live_verified = WorkerOutput {
            data_class: DATA_CLASS_LIVE_VERIFIED.to_string(),
            ..valid_out.clone()
        };
        assert!(as_live_verified
            .validate(MARKET_ARABICA_MONTHLY_IMF)
            .is_err());

        let as_live_agg = WorkerOutput {
            source_count: 2,
            data_class: DATA_CLASS_LIVE_AGGREGATED.to_string(),
            ..valid_out.clone()
        };
        assert!(as_live_agg.validate(MARKET_ARABICA_MONTHLY_IMF).is_err());

        // Keep SYNTHETIC_DEMO restricted strictly to GESHA-DEMO/USD
        let synth_on_imf = WorkerOutput {
            source: DEFAULT_FIXTURE_SOURCE.to_string(),
            data_class: DATA_CLASS_SYNTHETIC.to_string(),
            ..valid_out.clone()
        };
        assert!(synth_on_imf.validate(MARKET_ARABICA_MONTHLY_IMF).is_err());

        // Unknown & oversized data_class rejected fail-closed
        let unknown_cls = WorkerOutput {
            data_class: "OFFICIAL_LIVE".to_string(),
            ..valid_out.clone()
        };
        assert!(unknown_cls.validate(MARKET_ARABICA_MONTHLY_IMF).is_err());

        let oversized_cls = WorkerOutput {
            data_class: "X".repeat(MAX_WORKER_DATA_CLASS_BYTES + 1),
            ..valid_out
        };
        assert!(oversized_cls.validate(MARKET_ARABICA_MONTHLY_IMF).is_err());
    }
}
