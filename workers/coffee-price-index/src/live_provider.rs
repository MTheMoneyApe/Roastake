//! Real Testnet coffee futures providers and deterministic multi-vendor aggregation.
//!
//! This module deliberately does not contain fixture fallbacks. It accepts only the
//! two explicit ICE coffee futures roots supported by this rollout and returns a
//! LIVE_VERIFIED or LIVE_AGGREGATED WorkerOutput based on the number of valid vendor
//! observations actually used.

use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::transport::{HttpClient, HttpRequest, HttpTransport};
use roastake_shared::{
    calculate_divergence_bps, convert_usc_per_lb_e12_to_usd_per_kg_e8,
    parse_decimal_e12_round_half_up, round_half_up_div_u128, WorkerInput, WorkerOutput,
    DATA_CLASS_LIVE_AGGREGATED, DATA_CLASS_LIVE_VERIFIED, DECIMALS_COFFEE_INDEX,
    MODE_LIVE_INDEX, SCHEMA_VERSION, UNIT_USD_PER_KG,
};
use serde_json::Value;

const COMMODITIES_API_SOURCE: &str = "commodities-api";
const BARCHART_SOURCE: &str = "barchart";
const COMMODITIES_API_HOST: &str = "commodities-api.com";
const BARCHART_HOST: &str = "ondemand.websol.barchart.com";
const MAX_CLOCK_DRIFT_SEC: u64 = 60;
const INVERSE_RATE_NUMERATOR_E24: u128 = 1_000_000_000_000_000_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Quote {
    source: String,
    price_usd_per_kg_e8: u128,
    observed_at_sec: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoffeeKind {
    Arabica,
    Robusta,
}

impl CoffeeKind {
    fn from_market(market: &str) -> Result<Self, String> {
        match market {
            "COFFEE-ARABICA-REALDATA-TESTNET/USD" => Ok(Self::Arabica),
            "COFFEE-ROBUSTA-REALDATA-TESTNET/USD" => Ok(Self::Robusta),
            _ => Err(format!(
                "live_index market '{}' is not an approved Testnet ICE coffee market",
                market
            )),
        }
    }

    fn commodities_symbol(self) -> &'static str {
        match self {
            Self::Arabica => "COFFEE",
            Self::Robusta => "ROBUSTA",
        }
    }

    fn barchart_root(self) -> &'static str {
        match self {
            Self::Arabica => "KC",
            Self::Robusta => "RM",
        }
    }

    fn convert_commodities_inverse_rate(self, rate: &str) -> Result<u128, String> {
        let rate_e12 = parse_decimal_e12_round_half_up(rate)
            .map_err(|e| format!("invalid Commodities-API rate decimal: {e}"))?;
        if rate_e12 == 0 {
            return Err("Commodities-API returned a zero rate".to_string());
        }
        // With base=USD, the API returns units of the commodity bought by one USD.
        // Its inverse is the quoted ICE price. Arabica's quote is US cents/lb;
        // Robusta's quote is USD/metric tonne.
        let quote_e12 = round_half_up_div_u128(INVERSE_RATE_NUMERATOR_E24, rate_e12)
            .map_err(|e| format!("Commodities-API inverse-rate conversion failed: {e}"))?;
        self.convert_exchange_quote_e12(quote_e12)
    }

    fn convert_exchange_quote_e12(self, quote_e12: u128) -> Result<u128, String> {
        match self {
            // ICE Coffee C: US cents per pound -> USD/kg.
            Self::Arabica => convert_usc_per_lb_e12_to_usd_per_kg_e8(quote_e12)
                .map_err(|e| format!("Arabica USc/lb to USD/kg conversion failed: {e}")),
            // ICE Robusta: USD per metric tonne -> USD/kg. E12 -> E8 and /1,000 kg.
            Self::Robusta => round_half_up_div_u128(quote_e12, 10_000_000)
                .map_err(|e| format!("Robusta USD/tonne to USD/kg conversion failed: {e}")),
        }
    }
}

/// Fetch live quotes using every configured vendor, validate each quote, then aggregate.
/// Credentials are read only from OutLayer-injected environment variables.
pub fn fetch_live_index_price<T: HttpTransport>(
    client: &HttpClient<T>,
    input: &WorkerInput,
    now_sec: u64,
) -> Result<WorkerOutput, String> {
    if input.mode != MODE_LIVE_INDEX {
        return Err("live provider called for a non-live_index request".to_string());
    }
    let market = input.market.as_str();
    let kind = CoffeeKind::from_market(market)?;
    let min_source_count = input
        .min_source_count
        .ok_or_else(|| "live_index request is missing min_source_count".to_string())?;
    let max_age_sec = input
        .max_upstream_age_sec
        .ok_or_else(|| "live_index request is missing max_upstream_age_sec".to_string())?;
    let max_spread_bps = input
        .max_source_spread_bps
        .ok_or_else(|| "live_index request is missing max_source_spread_bps".to_string())?;

    let mut quotes = Vec::with_capacity(2);
    let mut failures = Vec::new();

    match env::var("COMMODITIES_API_KEY") {
        Ok(key) if !key.trim().is_empty() => match fetch_commodities_api_quote(client, kind, &key) {
            Ok(quote) => match validate_quote_freshness(&quote, now_sec, max_age_sec) {
                Ok(()) => quotes.push(quote),
                Err(error) => failures.push(format!("{COMMODITIES_API_SOURCE}: {error}")),
            },
            Err(error) => failures.push(format!("{COMMODITIES_API_SOURCE}: {error}")),
        },
        _ => failures.push("COMMODITIES_API_KEY is not configured in OutLayer Secrets".to_string()),
    }

    match env::var("BARCHART_API_KEY") {
        Ok(key) if !key.trim().is_empty() => match fetch_barchart_quote(client, kind, &key) {
            Ok(quote) => match validate_quote_freshness(&quote, now_sec, max_age_sec) {
                Ok(()) => quotes.push(quote),
                Err(error) => failures.push(format!("{BARCHART_SOURCE}: {error}")),
            },
            Err(error) => failures.push(format!("{BARCHART_SOURCE}: {error}")),
        },
        _ => failures.push("BARCHART_API_KEY is not configured in OutLayer Secrets".to_string()),
    }

    aggregate_quotes(market, quotes, min_source_count, max_spread_bps, &failures)
}

fn fetch_commodities_api_quote<T: HttpTransport>(
    client: &HttpClient<T>,
    kind: CoffeeKind,
    api_key: &str,
) -> Result<Quote, String> {
    let symbol = kind.commodities_symbol();
    let url = format!(
        "https://{COMMODITIES_API_HOST}/api/latest?access_key={}&base=USD&symbols={symbol}",
        percent_encode_query_component(api_key)
    );
    let request = HttpRequest::get(Some(COMMODITIES_API_SOURCE), url)
        .with_header("Accept", "application/json")
        .with_header("User-Agent", "ROASTAKE-Testnet-Coffee-Oracle/1.0");
    let response = client
        .execute(&request)
        .map_err(|e| format!("latest quote request failed: {e}"))?;
    let payload: Value = response
        .json(Some(COMMODITIES_API_SOURCE))
        .map_err(|e| format!("latest quote response rejected: {e}"))?;

    if payload.get("success").and_then(Value::as_bool) != Some(true) {
        return Err("provider did not report success=true".to_string());
    }
    if payload.get("base").and_then(Value::as_str) != Some("USD") {
        return Err("provider returned an unexpected base currency".to_string());
    }
    let observed_at_sec = payload
        .get("timestamp")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .ok_or_else(|| "provider response is missing a positive Unix timestamp".to_string())?;
    let rate_value = payload
        .get("rates")
        .and_then(|v| v.get(symbol))
        .ok_or_else(|| format!("provider response is missing rates.{symbol}"))?;
    let rate = json_number_as_decimal(rate_value)
        .ok_or_else(|| format!("provider rates.{symbol} is not a decimal number"))?;
    let price_usd_per_kg_e8 = kind.convert_commodities_inverse_rate(&rate)?;

    Ok(Quote {
        source: format!("{COMMODITIES_API_SOURCE}:{symbol}"),
        price_usd_per_kg_e8,
        observed_at_sec,
    })
}

fn fetch_barchart_quote<T: HttpTransport>(
    client: &HttpClient<T>,
    kind: CoffeeKind,
    api_key: &str,
) -> Result<Quote, String> {
    let root = kind.barchart_root();
    let symbol = format!("{root}*0");
    let encoded_symbol = format!("{root}%2A0");
    let url = format!(
        "https://{BARCHART_HOST}/getQuote.json?apikey={}&symbols={encoded_symbol}",
        percent_encode_query_component(api_key)
    );
    let request = HttpRequest::get(Some(BARCHART_SOURCE), url)
        .with_header("Accept", "application/json")
        .with_header("User-Agent", "ROASTAKE-Testnet-Coffee-Oracle/1.0");
    let response = client
        .execute(&request)
        .map_err(|e| format!("getQuote request failed: {e}"))?;
    let payload: Value = response
        .json(Some(BARCHART_SOURCE))
        .map_err(|e| format!("getQuote response rejected: {e}"))?;

    if let Some(code) = payload
        .get("status")
        .and_then(|v| v.get("code"))
        .and_then(Value::as_i64)
    {
        if code != 200 {
            return Err(format!("provider status code {code}"));
        }
    }
    let results = payload
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| "provider response is missing results array".to_string())?;
    let expected_root = root;
    let matching: Vec<&Value> = results
        .iter()
        .filter(|item| {
            item.get("symbol")
                .and_then(Value::as_str)
                .map(|actual| {
                    actual == symbol
                        || (actual.starts_with(expected_root) && actual.len() >= 4)
                })
                .unwrap_or(false)
        })
        .collect();
    if matching.len() != 1 {
        return Err(format!(
            "expected exactly one active {root} futures quote, received {}",
            matching.len()
        ));
    }
    let item = matching[0];

    let mode = item
        .get("mode")
        .and_then(Value::as_str)
        .ok_or_else(|| "provider quote is missing mode".to_string())?
        .to_ascii_lowercase();
    if mode != "r" && mode != "i" {
        return Err(format!(
            "provider quote mode '{mode}' is not real-time or delayed intraday data"
        ));
    }

    let raw_price = item
        .get("lastPrice")
        .and_then(json_number_as_decimal)
        .ok_or_else(|| "provider quote is missing numeric lastPrice".to_string())?;
    let quote_e12 = parse_decimal_e12_round_half_up(&raw_price)
        .map_err(|e| format!("invalid Barchart lastPrice decimal: {e}"))?;
    if quote_e12 == 0 {
        return Err("Barchart returned a zero lastPrice".to_string());
    }
    let price_usd_per_kg_e8 = kind.convert_exchange_quote_e12(quote_e12)?;

    let trade_timestamp = item
        .get("tradeTimestamp")
        .and_then(Value::as_str)
        .ok_or_else(|| "provider quote is missing tradeTimestamp".to_string())?;
    let observed_at_sec = parse_barchart_datetime_to_unix(trade_timestamp)?;
    let actual_symbol = item
        .get("symbol")
        .and_then(Value::as_str)
        .unwrap_or(&symbol);

    Ok(Quote {
        source: format!("{BARCHART_SOURCE}:{actual_symbol}:{mode}"),
        price_usd_per_kg_e8,
        observed_at_sec,
    })
}

fn aggregate_quotes(
    market: &str,
    mut quotes: Vec<Quote>,
    min_source_count: u32,
    max_source_spread_bps: u32,
    failures: &[String],
) -> Result<WorkerOutput, String> {
    if quotes.len() < min_source_count as usize {
        let reasons = if failures.is_empty() {
            "no upstream provider returned a valid quote".to_string()
        } else {
            failures.join("; ")
        };
        return Err(format!(
            "insufficient valid live sources: received {}, require {}; {}",
            quotes.len(),
            min_source_count,
            reasons
        ));
    }
    if quotes.is_empty() {
        return Err("no valid live source quotes are available".to_string());
    }

    quotes.sort_by_key(|q| q.price_usd_per_kg_e8);
    let min_price = quotes.first().expect("non-empty after count check").price_usd_per_kg_e8;
    let max_price = quotes.last().expect("non-empty after count check").price_usd_per_kg_e8;
    let spread_bps = calculate_divergence_bps(min_price, max_price);
    if spread_bps > max_source_spread_bps {
        return Err(format!(
            "live source spread {spread_bps} bps exceeds configured maximum {max_source_spread_bps} bps"
        ));
    }

    let median_price = if quotes.len() % 2 == 1 {
        quotes[quotes.len() / 2].price_usd_per_kg_e8
    } else {
        let lower = quotes[quotes.len() / 2 - 1].price_usd_per_kg_e8;
        let upper = quotes[quotes.len() / 2].price_usd_per_kg_e8;
        let sum = lower
            .checked_add(upper)
            .ok_or_else(|| "aggregated price overflow".to_string())?;
        round_half_up_div_u128(sum, 2)
            .map_err(|e| format!("median price rounding failed: {e}"))?
    };

    let source = quotes
        .iter()
        .map(|q| q.source.as_str())
        .collect::<Vec<_>>()
        .join("+");
    let observed_at_sec = quotes
        .iter()
        .map(|q| q.observed_at_sec)
        .min()
        .ok_or_else(|| "no timestamp among accepted quotes".to_string())?;
    let data_class = if quotes.len() >= 2 {
        DATA_CLASS_LIVE_AGGREGATED
    } else {
        DATA_CLASS_LIVE_VERIFIED
    };

    let output = WorkerOutput {
        schema_version: SCHEMA_VERSION,
        market: market.to_string(),
        price: median_price.to_string(),
        decimals: DECIMALS_COFFEE_INDEX,
        unit: UNIT_USD_PER_KG.to_string(),
        source,
        source_count: quotes.len() as u32,
        observed_at: observed_at_sec,
        data_class: data_class.to_string(),
    };
    output
        .validate(market)
        .map_err(|e| format!("aggregated worker output failed schema validation: {e}"))?;
    Ok(output)
}

fn validate_quote_freshness(
    quote: &Quote,
    now_sec: u64,
    max_upstream_age_sec: u64,
) -> Result<(), String> {
    if quote.observed_at_sec == 0 {
        return Err("upstream timestamp must be positive".to_string());
    }
    if quote.observed_at_sec > now_sec.saturating_add(MAX_CLOCK_DRIFT_SEC) {
        return Err("upstream timestamp is too far in the future".to_string());
    }
    let age = now_sec.saturating_sub(quote.observed_at_sec);
    if age > max_upstream_age_sec {
        return Err(format!(
            "upstream quote is stale: age {age}s exceeds maximum {max_upstream_age_sec}s"
        ));
    }
    if quote.price_usd_per_kg_e8 == 0 {
        return Err("upstream normalized price must be positive".to_string());
    }
    Ok(())
}

fn json_number_as_decimal(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_number().map(ToString::to_string))
}

fn percent_encode_query_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Parse Barchart's documented YYYY-MM-DD HH:MM:SS with explicit timezone / ISO timestamp
/// into Unix seconds. The exchange trade time, not Barchart's server time, is used for freshness.
fn parse_barchart_datetime_to_unix(raw: &str) -> Result<u64, String> {
    let normalized = raw.trim().replace(' ', "T");
    let t_pos = normalized
        .find('T')
        .ok_or_else(|| "invalid Barchart tradeTimestamp: missing date/time separator".to_string())?;
    let date = &normalized[..t_pos];
    let time_and_zone = &normalized[t_pos + 1..];

    let zone_pos = time_and_zone
        .char_indices()
        .find(|(i, c)| *i >= 8 && (*c == '+' || *c == '-'))
        .map(|(i, _)| i);
    let (time_raw, zone_raw) = if let Some(i) = zone_pos {
        (&time_and_zone[..i], &time_and_zone[i..])
    } else if let Some(time) = time_and_zone.strip_suffix('Z') {
        (time, "Z")
    } else {
        return Err("Barchart tradeTimestamp must include an explicit UTC offset".to_string());
    };
    let time_raw = time_raw.split('.').next().unwrap_or(time_raw);

    let mut date_parts = date.split('-');
    let year: u64 = parse_component(date_parts.next(), "year")?;
    let month: u64 = parse_component(date_parts.next(), "month")?;
    let day: u64 = parse_component(date_parts.next(), "day")?;
    if date_parts.next().is_some() || year < 1970 || !(1..=12).contains(&month) {
        return Err("Barchart tradeTimestamp contains an invalid calendar date".to_string());
    }

    let mut time_parts = time_raw.split(':');
    let hour: u64 = parse_component(time_parts.next(), "hour")?;
    let minute: u64 = parse_component(time_parts.next(), "minute")?;
    let second: u64 = parse_component(time_parts.next(), "second")?;
    if time_parts.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return Err("Barchart tradeTimestamp contains an invalid time".to_string());
    }
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => return Err("Barchart tradeTimestamp contains an invalid month".to_string()),
    };
    if day == 0 || day > days_in_month {
        return Err("Barchart tradeTimestamp contains an invalid day".to_string());
    }

    let mut days_since_epoch = 0u64;
    for y in 1970..year {
        days_since_epoch = days_since_epoch
            .checked_add(if is_leap_year(y) { 366 } else { 365 })
            .ok_or_else(|| "Barchart timestamp overflow".to_string())?;
    }
    for m in 1..month {
        days_since_epoch += match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if is_leap_year(year) => 29,
            2 => 28,
            _ => 0,
        };
    }
    days_since_epoch += day - 1;

    let local_seconds = days_since_epoch
        .checked_mul(86_400)
        .and_then(|v| v.checked_add(hour * 3_600 + minute * 60 + second))
        .ok_or_else(|| "Barchart timestamp overflow".to_string())?;

    let offset_seconds: i64 = if zone_raw == "Z" {
        0
    } else {
        if zone_raw.len() != 6 || &zone_raw[3..4] != ":" {
            return Err("Barchart tradeTimestamp has an invalid UTC offset".to_string());
        }
        let sign = if zone_raw.starts_with('-') { -1i64 } else if zone_raw.starts_with('+') {
            1i64
        } else {
            return Err("Barchart tradeTimestamp has an invalid UTC offset sign".to_string());
        };
        let offset_hour: i64 = parse_component(Some(&zone_raw[1..3]), "offset hour")? as i64;
        let offset_minute: i64 = parse_component(Some(&zone_raw[4..6]), "offset minute")? as i64;
        if offset_hour > 23 || offset_minute > 59 {
            return Err("Barchart tradeTimestamp has an out-of-range UTC offset".to_string());
        }
        sign * (offset_hour * 3_600 + offset_minute * 60)
    };

    let utc_seconds = (local_seconds as i128) - (offset_seconds as i128);
    if utc_seconds < 0 || utc_seconds > u64::MAX as i128 {
        return Err("Barchart tradeTimestamp is outside supported Unix range".to_string());
    }
    Ok(utc_seconds as u64)
}

fn parse_component(value: Option<&str>, field: &str) -> Result<u64, String> {
    let raw = value.ok_or_else(|| format!("missing {field} in Barchart timestamp"))?;
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid {field} in Barchart timestamp"));
    }
    raw.parse::<u64>()
        .map_err(|_| format!("invalid {field} in Barchart timestamp"))
}

fn is_leap_year(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn current_unix_time_sec() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| "worker system clock is before Unix epoch".to_string())
}

/// Entry point for the worker engine; use the WASI system clock for quote freshness.
pub fn fetch_live_index_price_now<T: HttpTransport>(
    client: &HttpClient<T>,
    input: &WorkerInput,
) -> Result<WorkerOutput, String> {
    let now_sec = current_unix_time_sec()?;
    fetch_live_index_price(client, input, now_sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commodities_api_arabica_inverse_rate_converts_usc_lb_to_usd_kg() {
        let price = CoffeeKind::Arabica
            .convert_commodities_inverse_rate("0.0034825004353126")
            .unwrap();
        assert!((632_000_000..=634_000_000).contains(&price));
    }

    #[test]
    fn commodities_api_robusta_inverse_rate_converts_usd_tonne_to_usd_kg() {
        let price = CoffeeKind::Robusta
            .convert_commodities_inverse_rate("0.00031104199066874")
            .unwrap();
        assert!((321_000_000..=322_000_000).contains(&price));
    }

    #[test]
    fn barchart_exchange_quotes_use_ice_units() {
        let arabica = CoffeeKind::Arabica
            .convert_exchange_quote_e12(279_400_000_000_000)
            .unwrap();
        let robusta = CoffeeKind::Robusta
            .convert_exchange_quote_e12(3_215_000_000_000_000)
            .unwrap();
        assert!((615_000_000..=617_000_000).contains(&arabica));
        assert_eq!(robusta, 321_500_000);
    }

    #[test]
    fn barchart_timestamp_respects_timezone_offset() {
        let local = parse_barchart_datetime_to_unix("2026-11-13 17:29:00-05:00").unwrap();
        let utc = parse_barchart_datetime_to_unix("2026-11-13T22:29:00Z").unwrap();
        assert_eq!(local, utc);
    }

    #[test]
    fn timestamp_parser_rejects_missing_timezone() {
        assert!(parse_barchart_datetime_to_unix("2026-11-13 17:29:00").is_err());
    }

    #[test]
    fn aggregation_uses_midpoint_oldest_timestamp_and_source_count() {
        let output = aggregate_quotes(
            "COFFEE-ARABICA-REALDATA-TESTNET/USD",
            vec![
                Quote {
                    source: "commodities-api:COFFEE".to_string(),
                    price_usd_per_kg_e8: 630_000_000,
                    observed_at_sec: 1_000,
                },
                Quote {
                    source: "barchart:KCZ26:i".to_string(),
                    price_usd_per_kg_e8: 632_000_000,
                    observed_at_sec: 1_100,
                },
            ],
            2,
            500,
            &[],
        )
        .unwrap();
        assert_eq!(output.price, "631000000");
        assert_eq!(output.source_count, 2);
        assert_eq!(output.observed_at, 1_000);
        assert_eq!(output.data_class, DATA_CLASS_LIVE_AGGREGATED);
    }

    #[test]
    fn aggregation_rejects_source_spread_above_policy() {
        let error = aggregate_quotes(
            "COFFEE-ARABICA-REALDATA-TESTNET/USD",
            vec![
                Quote {
                    source: "a".to_string(),
                    price_usd_per_kg_e8: 600_000_000,
                    observed_at_sec: 1_000,
                },
                Quote {
                    source: "b".to_string(),
                    price_usd_per_kg_e8: 640_000_000,
                    observed_at_sec: 1_000,
                },
            ],
            2,
            500,
            &[],
        )
        .unwrap_err();
        assert!(error.contains("spread"));
    }

    #[test]
    fn aggregation_never_silently_downgrades_a_two_source_requirement() {
        let error = aggregate_quotes(
            "COFFEE-ARABICA-REALDATA-TESTNET/USD",
            vec![Quote {
                source: "commodities-api:COFFEE".to_string(),
                price_usd_per_kg_e8: 630_000_000,
                observed_at_sec: 1_000,
            }],
            2,
            500,
            &["barchart: API key missing".to_string()],
        )
        .unwrap_err();
        assert!(error.contains("received 1, require 2"));
    }

    #[test]
    fn freshness_rejects_stale_and_future_quotes() {
        let quote = Quote {
            source: "test".to_string(),
            price_usd_per_kg_e8: 1,
            observed_at_sec: 900,
        };
        assert!(validate_quote_freshness(&quote, 1_000, 99).is_err());
        let future = Quote {
            observed_at_sec: 1_100,
            ..quote
        };
        assert!(validate_quote_freshness(&future, 1_000, 300).is_err());
    }
}
