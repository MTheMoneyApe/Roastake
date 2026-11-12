//! Provider abstraction and coffee price data source implementations.
//!
//! Supported in this phase:
//! - [`FixtureProvider`]: deterministic synthetic demo fixture strictly for `GESHA-DEMO/USD`.
//! - [`ImfPcpsProvider`]: free keyless IMF Primary Commodity Price System (PCPS) historical monthly
//!   Other Mild Arabicas (`PCOFFOTM`) provider strictly for `ARABICA-MONTHLY-IMF/USD`.
//!
//! Unverified live commercial feeds (`IcoProvider`, `IceProvider`, `VerifiedSupplierProvider`)
//! remain explicitly fail-closed stubs until licensing and ingestion agreements are established.

use crate::transport::{HttpClient, HttpRequest, HttpTransport};
use roastake_shared::{
    convert_usc_per_lb_e12_to_usd_per_kg_e8, parse_decimal_e12_round_half_up, WorkerOutput,
    DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY, DATA_CLASS_SYNTHETIC, DECIMALS_COFFEE_INDEX,
    DEFAULT_FIXTURE_PRICE_STR, DEFAULT_IMF_PCPS_SOURCE, MARKET_ARABICA_MONTHLY_IMF,
    MARKET_GESHA_DEMO, SCHEMA_VERSION, UNIT_USD_PER_KG,
};

/// Official keyless IMF PCPS SDMX 3.0 endpoint for monthly Other Mild Arabicas (`PCOFFOTM`) in USc/lb.
pub const IMF_PCPS_PCOFFOTM_URL: &str =
    "https://api.imf.org/external/sdmx/3.0/data/dataflow/IMF.RES/PCPS/+/G001.PCOFFOTM.USD.M?lastNObservations=1";

/// Expected IMF SDMX 3.0 series coordinates for Other Mild Arabicas cash benchmark (`USc/lb`, monthly).
const IMF_EXPECTED_COUNTRY: &str = "G001";
const IMF_EXPECTED_INDICATOR: &str = "PCOFFOTM";
const IMF_EXPECTED_TRANSFORMATION: &str = "USD";
const IMF_EXPECTED_FREQUENCY: &str = "M";

/// Common trait for all coffee price data providers.
pub trait PriceProvider {
    /// Return the canonical provider identifier tag.
    fn name(&self) -> &'static str;

    /// Retrieve or compute a normalized `WorkerOutput` for the specified market.
    fn fetch_price(&self, market: &str) -> Result<WorkerOutput, String>;
}

/// Deterministic synthetic demonstration fixture provider (`GESHA-DEMO/USD` only).
pub struct FixtureProvider;

impl PriceProvider for FixtureProvider {
    fn name(&self) -> &'static str {
        "roastake-fixture"
    }

    fn fetch_price(&self, market: &str) -> Result<WorkerOutput, String> {
        if market != MARKET_GESHA_DEMO {
            return Err(format!(
                "FixtureProvider only supports market '{}', requested '{}'",
                MARKET_GESHA_DEMO, market
            ));
        }

        Ok(WorkerOutput {
            schema_version: SCHEMA_VERSION,
            market: MARKET_GESHA_DEMO.to_string(),
            price: DEFAULT_FIXTURE_PRICE_STR.to_string(),
            decimals: DECIMALS_COFFEE_INDEX,
            unit: UNIT_USD_PER_KG.to_string(),
            source: self.name().to_string(),
            source_count: 1,
            observed_at: 0,
            data_class: DATA_CLASS_SYNTHETIC.to_string(),
        })
    }
}

/// IMF Primary Commodity Price System (PCPS) historical monthly benchmark provider.
///
/// Queries IMF's keyless SDMX 3.0 JSON API for `G001.PCOFFOTM.USD.M` (*Coffee, Other Mild
/// Arabicas, ICO New York ex-dock cash monthly average* in `USc/lb`), deterministically
/// derives the period's final UTC timestamp from `TIME_PERIOD` (`YYYY-Mmm`), and converts the
/// quote to `USD/kg` (`10^8`) using exact fixed-point half-up arithmetic.
pub struct ImfPcpsProvider<'a, T: HttpTransport> {
    client: &'a HttpClient<T>,
}

impl<'a, T: HttpTransport> ImfPcpsProvider<'a, T> {
    /// Wrap a policy-enforcing HTTP client for IMF PCPS requests.
    pub fn new(client: &'a HttpClient<T>) -> Self {
        Self { client }
    }
}

impl<T: HttpTransport> PriceProvider for ImfPcpsProvider<'_, T> {
    fn name(&self) -> &'static str {
        DEFAULT_IMF_PCPS_SOURCE
    }

    fn fetch_price(&self, market: &str) -> Result<WorkerOutput, String> {
        if market != MARKET_ARABICA_MONTHLY_IMF {
            return Err(format!(
                "ImfPcpsProvider only supports market '{}', requested '{}'",
                MARKET_ARABICA_MONTHLY_IMF, market
            ));
        }

        let request = HttpRequest::get(Some(DEFAULT_IMF_PCPS_SOURCE), IMF_PCPS_PCOFFOTM_URL)
            .with_header("Accept", "application/vnd.sdmx.data+json;version=2.0.0")
            .with_header("User-Agent", "Roastake-Oracle-Worker/1.0");

        let response = self
            .client
            .execute(&request)
            .map_err(|e| format!("IMF PCPS HTTP request failed: {e}"))?;

        parse_imf_pcps_pcoffotm_json(&response.body)
    }
}

/// Deterministically convert an IMF monthly SDMX period (`"YYYY-Mmm"`, e.g. `"2026-M09"`)
/// into the month's **final UTC second** (`YYYY-MM-last_dayT23:59:59Z`).
///
/// Convention: Monthly averages become finalized only once the calendar month concludes;
/// using `month_end_23_59_59_utc` equals `(next_month_day1_00_00_00_utc - 1)` in pure integer
/// proleptic Gregorian calendar arithmetic without consulting host clocks.
pub fn parse_imf_monthly_period_final_utc(period: &str) -> Result<u64, String> {
    let bytes = period.as_bytes();
    if bytes.len() != 8 || &bytes[4..6] != b"-M" {
        return Err(format!(
            "invalid IMF monthly TIME_PERIOD '{}': expected strict 'YYYY-Mmm' format",
            period
        ));
    }

    let year_str = &period[0..4];
    let month_str = &period[6..8];
    if !year_str.bytes().all(|b| b.is_ascii_digit())
        || !month_str.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(format!(
            "invalid IMF monthly TIME_PERIOD '{}': non-digit year or month",
            period
        ));
    }

    let year: u64 = year_str
        .parse()
        .map_err(|_| format!("invalid year in TIME_PERIOD '{}'", period))?;
    let month: u64 = month_str
        .parse()
        .map_err(|_| format!("invalid month in TIME_PERIOD '{}'", period))?;

    if year < 1970 {
        return Err(format!(
            "pre-Unix-epoch year {} rejected in IMF TIME_PERIOD '{}'",
            year, period
        ));
    }
    if !(1..=12).contains(&month) {
        return Err(format!(
            "out-of-range month {} in IMF TIME_PERIOD '{}'",
            month, period
        ));
    }

    let (next_year, next_month) = if month == 12 {
        (year + 1, 1u64)
    } else {
        (year, month + 1)
    };

    let mut total_days: u64 = 0;
    for y in 1970..next_year {
        total_days = total_days
            .checked_add(if is_leap_year(y) { 366 } else { 365 })
            .ok_or_else(|| "calendar day calculation overflow".to_string())?;
    }
    for m in 1..next_month {
        total_days = total_days
            .checked_add(days_in_month(next_year, m))
            .ok_or_else(|| "calendar day calculation overflow".to_string())?;
    }

    total_days
        .checked_mul(86_400)
        .and_then(|secs| secs.checked_sub(1))
        .ok_or_else(|| "UTC timestamp calculation overflow".to_string())
}

fn is_leap_year(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn days_in_month(year: u64, month: u64) -> u64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => unreachable!("month already validated in 1..=12"),
    }
}

/// Defensively parse an IMF PCPS SDMX 3.0 JSON payload (`IMF.RES:PCPS` / `G001.PCOFFOTM.USD.M`)
/// and normalize `USc/lb` -> `USD/kg` (`10^8`) with zero floating-point operations.
pub fn parse_imf_pcps_pcoffotm_json(body: &[u8]) -> Result<WorkerOutput, String> {
    if body.is_empty() {
        return Err("IMF PCPS response body is empty".to_string());
    }

    let root: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| "malformed JSON in IMF PCPS response body".to_string())?;

    let data = root
        .get("data")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "IMF PCPS response missing 'data' object".to_string())?;

    let datasets = data
        .get("dataSets")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "IMF PCPS response missing 'data.dataSets' array".to_string())?;
    if datasets.len() != 1 {
        return Err(format!(
            "IMF PCPS response expected exactly 1 dataset, got {}",
            datasets.len()
        ));
    }
    let dataset = datasets[0]
        .as_object()
        .ok_or_else(|| "IMF PCPS dataset[0] must be an object".to_string())?;

    let structure_idx = dataset
        .get("structure")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "IMF PCPS dataset[0] missing integer 'structure' index".to_string())?
        as usize;

    let structures = data
        .get("structures")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "IMF PCPS response missing 'data.structures' array".to_string())?;
    let structure = structures
        .get(structure_idx)
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            format!(
                "IMF PCPS structure index {} missing from 'data.structures'",
                structure_idx
            )
        })?;

    validate_obs_measure_if_present(structure)?;

    let series_map = dataset
        .get("series")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "IMF PCPS dataset missing 'series' object".to_string())?;
    if series_map.len() != 1 {
        return Err(format!(
            "IMF PCPS response expected exactly 1 series entry, got {}",
            series_map.len()
        ));
    }
    let (series_key, series_val) = series_map
        .iter()
        .next()
        .ok_or_else(|| "IMF PCPS series map is empty".to_string())?;
    let series_obj = series_val
        .as_object()
        .ok_or_else(|| "IMF PCPS series entry must be an object".to_string())?;

    let dimensions = structure
        .get("dimensions")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "IMF PCPS structure missing 'dimensions' object".to_string())?;

    verify_series_identity(dimensions, series_key)?;
    verify_series_scale_if_present(structure, series_obj)?;

    let obs_dim_values = extract_time_period_values(dimensions)?;
    let observations = series_obj
        .get("observations")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "IMF PCPS series entry missing 'observations' object".to_string())?;
    if observations.len() != 1 {
        return Err(format!(
            "IMF PCPS single-observation query expected 1 observation, got {}",
            observations.len()
        ));
    }

    let (obs_idx_key, obs_payload) = observations
        .iter()
        .next()
        .ok_or_else(|| "IMF PCPS observations map is empty".to_string())?;
    let obs_idx: usize = obs_idx_key
        .parse()
        .map_err(|_| format!("invalid IMF PCPS observation index key '{}'", obs_idx_key))?;

    let period_str = obs_dim_values
        .get(obs_idx)
        .and_then(|v| v.get("value"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            format!(
                "missing TIME_PERIOD value for observation index {}",
                obs_idx
            )
        })?;
    let observed_at = parse_imf_monthly_period_final_utc(period_str)?;

    let obs_array = obs_payload
        .as_array()
        .ok_or_else(|| "IMF PCPS observation value entry must be an array".to_string())?;
    let raw_price_str = obs_array
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| "IMF PCPS observation value[0] missing or non-string".to_string())?;

    let usc_per_lb_e12 = parse_decimal_e12_round_half_up(raw_price_str)
        .map_err(|e| format!("invalid IMF PCPS decimal price value: {e}"))?;
    if usc_per_lb_e12 == 0 {
        return Err("IMF PCPS observation price must be strictly positive".to_string());
    }

    let usd_per_kg_e8 = convert_usc_per_lb_e12_to_usd_per_kg_e8(usc_per_lb_e12)
        .map_err(|e| format!("IMF PCPS USD/kg conversion overflow: {e}"))?;
    if usd_per_kg_e8 == 0 {
        return Err("IMF PCPS normalized USD/kg price rounded to zero".to_string());
    }

    Ok(WorkerOutput {
        schema_version: SCHEMA_VERSION,
        market: MARKET_ARABICA_MONTHLY_IMF.to_string(),
        price: usd_per_kg_e8.to_string(),
        decimals: DECIMALS_COFFEE_INDEX,
        unit: UNIT_USD_PER_KG.to_string(),
        source: DEFAULT_IMF_PCPS_SOURCE.to_string(),
        source_count: 1,
        observed_at,
        data_class: DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY.to_string(),
    })
}

fn validate_obs_measure_if_present(
    structure: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    if let Some(obs_measures) = structure
        .get("measures")
        .and_then(|m| m.get("observation"))
        .and_then(|v| v.as_array())
    {
        let first_id = obs_measures
            .first()
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str());
        if first_id != Some("OBS_VALUE") {
            return Err(format!(
                "IMF PCPS unexpected primary observation measure {:?}",
                first_id
            ));
        }
    }
    Ok(())
}

fn verify_series_identity(
    dimensions: &serde_json::Map<String, serde_json::Value>,
    series_key: &str,
) -> Result<(), String> {
    let series_dims = dimensions
        .get("series")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "IMF PCPS structure missing 'dimensions.series' array".to_string())?;

    let key_parts: Vec<&str> = series_key.split(':').collect();
    if key_parts.len() != series_dims.len() || series_dims.len() < 4 {
        return Err(format!(
            "IMF PCPS series key '{}' dimensionality mismatch (expected {} dimensions)",
            series_key,
            series_dims.len()
        ));
    }

    let mut matched_country = false;
    let mut matched_indicator = false;
    let mut matched_transformation = false;
    let mut matched_frequency = false;

    for (dim_obj, part_str) in series_dims.iter().zip(key_parts.iter()) {
        let dim_id = dim_obj
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "IMF PCPS series dimension missing 'id'".to_string())?;
        let val_idx: usize = part_str
            .parse()
            .map_err(|_| format!("invalid IMF PCPS dimension index '{}'", part_str))?;
        let val_id = dim_obj
            .get("values")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.get(val_idx))
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                format!(
                    "IMF PCPS dimension '{}' missing value at index {}",
                    dim_id, val_idx
                )
            })?;

        match dim_id {
            "COUNTRY" => {
                if val_id != IMF_EXPECTED_COUNTRY {
                    return Err(format!(
                        "unexpected IMF PCPS COUNTRY '{}', expected '{}'",
                        val_id, IMF_EXPECTED_COUNTRY
                    ));
                }
                matched_country = true;
            }
            "INDICATOR" => {
                if val_id != IMF_EXPECTED_INDICATOR {
                    return Err(format!(
                        "unexpected IMF PCPS INDICATOR '{}', expected '{}'",
                        val_id, IMF_EXPECTED_INDICATOR
                    ));
                }
                matched_indicator = true;
            }
            "DATA_TRANSFORMATION" => {
                if val_id != IMF_EXPECTED_TRANSFORMATION {
                    return Err(format!(
                        "unexpected IMF PCPS DATA_TRANSFORMATION '{}', expected '{}'",
                        val_id, IMF_EXPECTED_TRANSFORMATION
                    ));
                }
                matched_transformation = true;
            }
            "FREQUENCY" => {
                if val_id != IMF_EXPECTED_FREQUENCY {
                    return Err(format!(
                        "unexpected IMF PCPS FREQUENCY '{}', expected '{}'",
                        val_id, IMF_EXPECTED_FREQUENCY
                    ));
                }
                matched_frequency = true;
            }
            _ => {}
        }
    }

    if !matched_country || !matched_indicator || !matched_transformation || !matched_frequency {
        return Err("IMF PCPS series dimensions missing required coordinates".to_string());
    }
    Ok(())
}

fn verify_series_scale_if_present(
    structure: &serde_json::Map<String, serde_json::Value>,
    series_obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let Some(series_attrs_meta) = structure
        .get("attributes")
        .and_then(|a| a.get("series"))
        .and_then(|v| v.as_array())
    else {
        return Ok(());
    };

    for (attr_pos, attr_meta) in series_attrs_meta.iter().enumerate() {
        if attr_meta.get("id").and_then(|v| v.as_str()) == Some("SCALE") {
            let Some(attr_indices) = series_obj.get("attributes").and_then(|v| v.as_array()) else {
                continue;
            };
            let Some(val_idx_opt) = attr_indices.get(attr_pos).and_then(|v| v.as_u64()) else {
                continue;
            };
            let scale_id = attr_meta
                .get("values")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.get(val_idx_opt as usize))
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_str());
            if scale_id != Some("0") {
                return Err(format!(
                    "unexpected IMF PCPS SCALE attribute {:?}, expected unscaled '0'",
                    scale_id
                ));
            }
        }
    }
    Ok(())
}

fn extract_time_period_values(
    dimensions: &serde_json::Map<String, serde_json::Value>,
) -> Result<&Vec<serde_json::Value>, String> {
    let obs_dims = dimensions
        .get("observation")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "IMF PCPS structure missing 'dimensions.observation' array".to_string())?;

    let tp_dim = obs_dims
        .iter()
        .find(|d| d.get("id").and_then(|v| v.as_str()) == Some("TIME_PERIOD"))
        .ok_or_else(|| "IMF PCPS dimensions.observation missing 'TIME_PERIOD'".to_string())?;

    tp_dim
        .get("values")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "IMF PCPS TIME_PERIOD dimension missing 'values' array".to_string())
}

/// Extension point: International Coffee Organization (ICO) composite indicator.
/// NOT implemented in Phase 1 (fails closed).
#[allow(dead_code)]
pub struct IcoProvider;

impl PriceProvider for IcoProvider {
    fn name(&self) -> &'static str {
        "ico-composite"
    }

    fn fetch_price(&self, _market: &str) -> Result<WorkerOutput, String> {
        Err(
            "ICO data feed is an unverified external source; access terms, network allowlist, \
             and normalization methodology must be verified before enabling on Testnet."
                .to_string(),
        )
    }
}

/// Extension point: Intercontinental Exchange (ICE) Coffee 'C' futures benchmark.
/// NOT implemented in Phase 1 (fails closed).
#[allow(dead_code)]
pub struct IceProvider;

impl PriceProvider for IceProvider {
    fn name(&self) -> &'static str {
        "ice-coffee-c"
    }

    fn fetch_price(&self, _market: &str) -> Result<WorkerOutput, String> {
        Err(
            "ICE Coffee 'C' futures feed is an unverified external source; commercial license, \
             contract month rolls, and delivery differentials must be established first."
                .to_string(),
        )
    }
}

/// Extension point: Verified physical supplier direct trade quotations.
/// NOT implemented in Phase 1 (fails closed).
#[allow(dead_code)]
pub struct VerifiedSupplierProvider;

impl PriceProvider for VerifiedSupplierProvider {
    fn name(&self) -> &'static str {
        "verified-supplier"
    }

    fn fetch_price(&self, _market: &str) -> Result<WorkerOutput, String> {
        Err(
            "Verified supplier ingestion requires on-chain supplier registration, \
             cryptographic warehouse receipts, and physical quality audit verification."
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{MockHttpTransport, NetworkAllowlist, TimeoutPhase, TransportError};

    fn valid_imf_sample_payload(period: &str, value_str: &str) -> Vec<u8> {
        serde_json::json!({
            "meta": {},
            "data": {
                "dataSets": [{
                    "structure": 0,
                    "action": "Replace",
                    "series": {
                        "0:0:0:0": {
                            "attributes": [0, null, null],
                            "observations": {
                                "0": [value_str, null, 0]
                            }
                        }
                    }
                }],
                "structures": [{
                    "dataSets": [0],
                    "dimensions": {
                        "series": [
                            {"id": "COUNTRY", "keyPosition": 0, "roles": [], "values": [{"id": "G001"}]},
                            {"id": "INDICATOR", "keyPosition": 1, "roles": [], "values": [{"id": "PCOFFOTM"}]},
                            {"id": "DATA_TRANSFORMATION", "keyPosition": 2, "roles": [], "values": [{"id": "USD"}]},
                            {"id": "FREQUENCY", "keyPosition": 3, "roles": [], "values": [{"id": "M"}]}
                        ],
                        "observation": [
                            {"id": "TIME_PERIOD", "keyPosition": 4, "values": [{"value": period}]}
                        ]
                    },
                    "measures": {
                        "observation": [{"id": "OBS_VALUE", "roles": []}]
                    },
                    "attributes": {
                        "series": [
                            {"id": "SCALE", "values": [{"id": "0"}]}
                        ]
                    }
                }]
            }
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn test_imf_pcps_valid_sdmx_payload_and_http_provider() {
        let mock = MockHttpTransport::new();
        mock.enqueue_ok_json(
            200,
            valid_imf_sample_payload("2026-M09", "330.1219047619048"),
        );

        let allowlist = NetworkAllowlist::from_embedded_manifest().unwrap();
        let client = HttpClient::new(&mock, allowlist);
        let provider = ImfPcpsProvider::new(&client);

        let output = provider
            .fetch_price(MARKET_ARABICA_MONTHLY_IMF)
            .expect("valid IMF SDMX payload must succeed");

        assert_eq!(output.schema_version, SCHEMA_VERSION);
        assert_eq!(output.market, MARKET_ARABICA_MONTHLY_IMF);
        assert_eq!(output.price, "727794219"); // $7.27794219/kg
        assert_eq!(output.decimals, 8);
        assert_eq!(output.unit, UNIT_USD_PER_KG);
        assert_eq!(output.source, DEFAULT_IMF_PCPS_SOURCE);
        assert_eq!(output.source_count, 1);
        assert_eq!(output.observed_at, 1_790_812_799); // 2026-09-30T23:59:59Z
        assert_eq!(output.data_class, DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY);

        output
            .validate(MARKET_ARABICA_MONTHLY_IMF)
            .expect("worker output must satisfy shared validation");

        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].host, "api.imf.org");
    }

    #[test]
    fn test_imf_monthly_period_utc_timestamps_including_leap_years() {
        // 2026-09-30T23:59:59Z
        assert_eq!(
            parse_imf_monthly_period_final_utc("2026-M09").unwrap(),
            1_790_812_799
        );
        // 2024 leap year Feb -> 2024-02-29T23:59:59Z
        assert_eq!(
            parse_imf_monthly_period_final_utc("2024-M02").unwrap(),
            1_709_251_199
        );
        // 2025 non-leap year Feb -> 2025-02-28T23:59:59Z
        assert_eq!(
            parse_imf_monthly_period_final_utc("2025-M02").unwrap(),
            1_740_787_199
        );
        // Century 2000 divisible by 400 (leap year) -> 2000-02-29T23:59:59Z
        assert_eq!(
            parse_imf_monthly_period_final_utc("2000-M02").unwrap(),
            951_868_799
        );
        // Century 2100 divisible by 100 but not 400 (non-leap year) -> 2100-02-28T23:59:59Z
        assert_eq!(
            parse_imf_monthly_period_final_utc("2100-M02").unwrap(),
            4_107_542_399
        );
        // Year-boundary December rollover -> 2026-12-31T23:59:59Z
        assert_eq!(
            parse_imf_monthly_period_final_utc("2026-M12").unwrap(),
            1_798_761_599
        );
        // Unix epoch month 1 -> 1970-01-31T23:59:59Z
        assert_eq!(
            parse_imf_monthly_period_final_utc("1970-M01").unwrap(),
            2_678_399
        );

        for bad_period in [
            "",
            "2026-09",
            "2026-m09",
            "2026-Q03",
            "2026-M00",
            "2026-M13",
            "1969-M12",
            "2026-M9",
            "2026-M09 ",
        ] {
            assert!(
                parse_imf_monthly_period_final_utc(bad_period).is_err(),
                "period '{bad_period}' must be rejected"
            );
        }
    }

    #[test]
    fn test_imf_pcps_parser_rejects_empty_missing_malformed_and_invalid_values() {
        // 1. Empty payload / malformed JSON
        assert!(parse_imf_pcps_pcoffotm_json(b"").is_err());
        assert!(parse_imf_pcps_pcoffotm_json(b"{not json").is_err());
        assert!(parse_imf_pcps_pcoffotm_json(b"null").is_err());

        // 2. Empty observations or empty series
        let empty_obs = serde_json::json!({
            "data": {
                "dataSets": [{
                    "structure": 0,
                    "series": {
                        "0:0:0:0": { "observations": {} }
                    }
                }],
                "structures": [{
                    "dimensions": {
                        "series": [
                            {"id": "COUNTRY", "values": [{"id": "G001"}]},
                            {"id": "INDICATOR", "values": [{"id": "PCOFFOTM"}]},
                            {"id": "DATA_TRANSFORMATION", "values": [{"id": "USD"}]},
                            {"id": "FREQUENCY", "values": [{"id": "M"}]}
                        ],
                        "observation": [{"id": "TIME_PERIOD", "values": [{"value": "2026-M09"}]}]
                    }
                }]
            }
        })
        .to_string();
        assert!(parse_imf_pcps_pcoffotm_json(empty_obs.as_bytes()).is_err());

        let empty_series = serde_json::json!({
            "data": {
                "dataSets": [{"structure": 0, "series": {}}],
                "structures": [{"dimensions": {"series": [], "observation": []}}]
            }
        })
        .to_string();
        assert!(parse_imf_pcps_pcoffotm_json(empty_series.as_bytes()).is_err());

        // 3. Missing series/dimensions
        let missing_dims = serde_json::json!({
            "data": {
                "dataSets": [{"structure": 0, "series": {"0:0:0:0": {"observations": {"0": ["330.12"]}}}}],
                "structures": [{}]
            }
        })
        .to_string();
        assert!(parse_imf_pcps_pcoffotm_json(missing_dims.as_bytes()).is_err());

        // 4. Wrong indicator (e.g., PCOFFROB) or wrong transformation (INDEX) rejected
        let wrong_indicator = serde_json::json!({
            "data": {
                "dataSets": [{
                    "structure": 0,
                    "series": {"0:0:0:0": {"observations": {"0": ["330.12"]}}}
                }],
                "structures": [{
                    "dimensions": {
                        "series": [
                            {"id": "COUNTRY", "values": [{"id": "G001"}]},
                            {"id": "INDICATOR", "values": [{"id": "PCOFFROB"}]},
                            {"id": "DATA_TRANSFORMATION", "values": [{"id": "USD"}]},
                            {"id": "FREQUENCY", "values": [{"id": "M"}]}
                        ],
                        "observation": [{"id": "TIME_PERIOD", "values": [{"value": "2026-M09"}]}]
                    }
                }]
            }
        })
        .to_string();
        assert!(parse_imf_pcps_pcoffotm_json(wrong_indicator.as_bytes()).is_err());

        // 5. Non-zero SCALE attribute rejected fail-closed
        let bad_scale = serde_json::json!({
            "data": {
                "dataSets": [{
                    "structure": 0,
                    "series": {"0:0:0:0": {"attributes": [0], "observations": {"0": ["330.12"]}}}
                }],
                "structures": [{
                    "dimensions": {
                        "series": [
                            {"id": "COUNTRY", "values": [{"id": "G001"}]},
                            {"id": "INDICATOR", "values": [{"id": "PCOFFOTM"}]},
                            {"id": "DATA_TRANSFORMATION", "values": [{"id": "USD"}]},
                            {"id": "FREQUENCY", "values": [{"id": "M"}]}
                        ],
                        "observation": [{"id": "TIME_PERIOD", "values": [{"value": "2026-M09"}]}]
                    },
                    "attributes": {
                        "series": [{"id": "SCALE", "values": [{"id": "3"}]}]
                    }
                }]
            }
        })
        .to_string();
        assert!(parse_imf_pcps_pcoffotm_json(bad_scale.as_bytes()).is_err());

        // 6. Invalid values: zero, negative, scientific notation, >24 decimals, NaN, null
        let frac_25 = format!("330.{}", "1".repeat(25));
        for bad_val in ["0", "0.00", "-330.12", "3.3e2", "NaN", "", &frac_25] {
            let payload = valid_imf_sample_payload("2026-M09", bad_val);
            assert!(
                parse_imf_pcps_pcoffotm_json(&payload).is_err(),
                "value '{bad_val}' must be rejected"
            );
        }

        // Null observation value rejected
        let null_obs = serde_json::json!({
            "data": {
                "dataSets": [{
                    "structure": 0,
                    "series": {"0:0:0:0": {"observations": {"0": [null]}}}
                }],
                "structures": [{
                    "dimensions": {
                        "series": [
                            {"id": "COUNTRY", "values": [{"id": "G001"}]},
                            {"id": "INDICATOR", "values": [{"id": "PCOFFOTM"}]},
                            {"id": "DATA_TRANSFORMATION", "values": [{"id": "USD"}]},
                            {"id": "FREQUENCY", "values": [{"id": "M"}]}
                        ],
                        "observation": [{"id": "TIME_PERIOD", "values": [{"value": "2026-M09"}]}]
                    }
                }]
            }
        })
        .to_string();
        assert!(parse_imf_pcps_pcoffotm_json(null_obs.as_bytes()).is_err());
    }

    #[test]
    fn test_imf_pcps_provider_http_errors_timeouts_and_size_limits() {
        let allowlist = NetworkAllowlist::from_embedded_manifest().unwrap();

        // HTTP status errors
        for status in [400u16, 404, 429, 500, 503] {
            let mock = MockHttpTransport::new();
            mock.enqueue_ok_json(status, b"{}".to_vec());
            let client = HttpClient::new(&mock, allowlist.clone());
            let provider = ImfPcpsProvider::new(&client);
            let err = provider
                .fetch_price(MARKET_ARABICA_MONTHLY_IMF)
                .unwrap_err();
            assert!(err.contains(&status.to_string()));
        }

        // Timeout propagation without retries
        for phase in [
            TimeoutPhase::Connect,
            TimeoutPhase::FirstByte,
            TimeoutPhase::BetweenBytes,
        ] {
            let mock = MockHttpTransport::new();
            mock.enqueue_err(TransportError::Timeout {
                provider: Some(DEFAULT_IMF_PCPS_SOURCE),
                phase,
            });
            let client = HttpClient::new(&mock, allowlist.clone());
            let provider = ImfPcpsProvider::new(&client);
            assert!(provider.fetch_price(MARKET_ARABICA_MONTHLY_IMF).is_err());
            assert_eq!(mock.recorded_calls().len(), 1);
        }

        // Oversized response rejected (> MAX_HTTP_RESPONSE_BYTES)
        let mock_big = MockHttpTransport::new();
        mock_big.enqueue_ok_json(
            200,
            vec![b'x'; crate::transport::MAX_HTTP_RESPONSE_BYTES + 1],
        );
        let client_big = HttpClient::new(&mock_big, allowlist.clone());
        let provider_big = ImfPcpsProvider::new(&client_big);
        assert!(provider_big
            .fetch_price(MARKET_ARABICA_MONTHLY_IMF)
            .unwrap_err()
            .contains("exceeded max"));

        // Wrong market rejected without sending HTTP request
        let mock_unused = MockHttpTransport::new();
        let client_unused = HttpClient::new(&mock_unused, allowlist);
        let provider_unused = ImfPcpsProvider::new(&client_unused);
        assert!(provider_unused.fetch_price(MARKET_GESHA_DEMO).is_err());
        assert_eq!(mock_unused.recorded_calls().len(), 0);
    }
}
