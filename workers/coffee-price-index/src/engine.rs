//! Core execution engine for the Coffee Price Index WASI worker.

use crate::provider::{FixtureProvider, PriceProvider};
use roastake_shared::{WorkerInput, WorkerOutput, MODE_FIXTURE};

/// Execute worker logic given a parsed `WorkerInput`.
pub fn process_request(input: WorkerInput) -> Result<WorkerOutput, String> {
    // Validate input schema and required fields
    input.validate()?;

    // Route based on requested mode
    match input.mode.as_str() {
        MODE_FIXTURE => {
            let provider = FixtureProvider;
            let output = provider.fetch_price(&input.market)?;
            // Final validation of output against contract expectations
            output.validate(&input.market)?;
            Ok(output)
        }
        other => Err(format!(
            "unsupported execution mode '{}'. In Phase 1, only 'fixture' is permitted",
            other
        )),
    }
}

/// Execute worker from raw JSON string, returning serialized JSON string response.
pub fn process_raw_json(raw_input: &str) -> Result<String, String> {
    if raw_input.trim().is_empty() {
        return Err("stdin received empty input payload".to_string());
    }

    let input: WorkerInput = serde_json::from_str(raw_input)
        .map_err(|e| format!("failed to parse stdin as WorkerInput JSON: {}", e))?;

    let output = process_request(input)?;

    serde_json::to_string(&output)
        .map_err(|e| format!("failed to serialize WorkerOutput to JSON: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use roastake_shared::{
        DATA_CLASS_SYNTHETIC, DEFAULT_FIXTURE_PRICE_STR, MARKET_GESHA_DEMO, SCHEMA_VERSION,
        UNIT_USD_PER_KG,
    };

    #[test]
    fn test_valid_fixture_request() {
        let input = WorkerInput::new_fixture(MARKET_GESHA_DEMO);
        let output = process_request(input).expect("successful processing");

        assert_eq!(output.schema_version, SCHEMA_VERSION);
        assert_eq!(output.market, MARKET_GESHA_DEMO);
        assert_eq!(output.price, DEFAULT_FIXTURE_PRICE_STR);
        assert_eq!(output.decimals, 8);
        assert_eq!(output.unit, UNIT_USD_PER_KG);
        assert_eq!(output.source, "roastake-fixture");
        assert_eq!(output.source_count, 1);
        assert_eq!(output.observed_at, 0);
        assert_eq!(output.data_class, DATA_CLASS_SYNTHETIC);
    }

    #[test]
    fn test_unknown_market() {
        let input = WorkerInput::new_fixture("ETH-USD");
        let err = process_request(input).unwrap_err();
        assert!(err.contains("FixtureProvider only supports market"));
    }

    #[test]
    fn test_unsupported_mode() {
        let input = WorkerInput {
            schema_version: SCHEMA_VERSION,
            market: MARKET_GESHA_DEMO.to_string(),
            mode: "live-scrape".to_string(),
        };
        let err = process_request(input).unwrap_err();
        assert!(err.contains("unsupported mode"));
    }

    #[test]
    fn test_process_raw_json_exact() {
        let raw = r#"{
  "schema_version": 1,
  "market": "GESHA-DEMO/USD",
  "mode": "fixture"
}"#;
        let out_json = process_raw_json(raw).expect("valid raw json processing");
        let parsed: serde_json::Value = serde_json::from_str(&out_json).unwrap();

        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["market"], "GESHA-DEMO/USD");
        assert_eq!(parsed["price"], "1850000000");
        assert_eq!(parsed["decimals"], 8);
        assert_eq!(parsed["unit"], "USD/kg");
        assert_eq!(parsed["source"], "roastake-fixture");
        assert_eq!(parsed["source_count"], 1);
        assert_eq!(parsed["observed_at"], 0);
        assert_eq!(parsed["data_class"], "SYNTHETIC_DEMO");
    }

    #[test]
    fn test_process_raw_json_invalid_syntax() {
        let err = process_raw_json("not valid json").unwrap_err();
        assert!(err.contains("failed to parse stdin"));
    }
}
