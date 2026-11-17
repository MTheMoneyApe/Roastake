//! Core execution engine for the Coffee Price Index WASI worker.

use crate::provider::{FixtureProvider, ImfPcpsProvider, PriceProvider};
#[cfg(feature = "wasi-http")]
use crate::transport::WasiHttpTransport;
use crate::transport::{HttpClient, HttpTransport, NetworkAllowlist};
#[cfg(not(feature = "wasi-http"))]
use crate::transport::{HttpRequest, HttpResponse, TransportError, ValidatedUrl};
use roastake_shared::{WorkerInput, WorkerOutput, MODE_FIXTURE, MODE_PUBLIC_BENCHMARK};

/// Native fallback transport that always fails closed when `wasi-http` is disabled.
#[cfg(not(feature = "wasi-http"))]
struct DisabledHttpTransport;

#[cfg(not(feature = "wasi-http"))]
impl HttpTransport for DisabledHttpTransport {
    fn send_validated(
        &self,
        request: &HttpRequest,
        _validated_url: &ValidatedUrl,
    ) -> Result<HttpResponse, TransportError> {
        Err(TransportError::NotConfigured {
            provider: request.provider_id,
        })
    }
}

/// Execute worker logic given a parsed `WorkerInput` using the target's default transport
/// and embedded `outlayer.manifest` allowlist.
pub fn process_request(input: WorkerInput) -> Result<WorkerOutput, String> {
    let allowlist = NetworkAllowlist::from_embedded_manifest()
        .map_err(|e| format!("manifest allowlist failure: {e}"))?;

    #[cfg(feature = "wasi-http")]
    {
        let transport = WasiHttpTransport::new();
        process_request_with_transport(input, &transport, &allowlist)
    }

    #[cfg(not(feature = "wasi-http"))]
    {
        let transport = DisabledHttpTransport;
        process_request_with_transport(input, &transport, &allowlist)
    }
}

/// Execute worker logic with an explicit [`HttpTransport`] and [`NetworkAllowlist`].
///
/// Enables deterministic host testing via `MockHttpTransport` with zero network I/O
/// and zero fallback to synthetic fixtures if any external fetch or parse fails.
pub fn process_request_with_transport<T: HttpTransport>(
    input: WorkerInput,
    transport: &T,
    allowlist: &NetworkAllowlist,
) -> Result<WorkerOutput, String> {
    input.validate()?;

    let output = match input.mode.as_str() {
        MODE_FIXTURE => {
            let provider = FixtureProvider;
            provider.fetch_price(&input.market)?
        }
        MODE_PUBLIC_BENCHMARK => {
            let client = HttpClient::new(transport, allowlist.clone());
            let provider = ImfPcpsProvider::new(&client);
            provider.fetch_price(&input.market)?
        }
        other => {
            return Err(format!(
                "unsupported execution mode '{}'. Commercial live_index sources remain disabled until authorized",
                other
            ));
        }
    };

    output.validate(&input.market)?;
    Ok(output)
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
    use crate::transport::{MockHttpTransport, TimeoutPhase, TransportError};
    use roastake_shared::{
        DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY, DATA_CLASS_SYNTHETIC, DEFAULT_FIXTURE_PRICE_STR,
        DEFAULT_IMF_PCPS_SOURCE, HISTORICAL_MONTHLY_MAX_PRICE_AGE_SEC, MARKET_ARABICA_MONTHLY_IMF,
        MARKET_GESHA_DEMO, SCHEMA_VERSION, UNIT_USD_PER_KG,
    };

    fn sample_imf_json_bytes() -> Vec<u8> {
        serde_json::json!({
            "meta": {},
            "data": {
                "dataSets": [{
                    "structure": 0,
                    "action": "Replace",
                    "series": {
                        "0:0:0:0": {
                            "attributes": [0],
                            "observations": {
                                "0": ["330.1219047619048", null, 0]
                            }
                        }
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
                        "observation": [
                            {"id": "TIME_PERIOD", "values": [{"value": "2026-M09"}]}
                        ]
                    },
                    "measures": {
                        "observation": [{"id": "OBS_VALUE"}]
                    },
                    "attributes": {
                        "series": [{"id": "SCALE", "values": [{"id": "0"}]}]
                    }
                }]
            }
        })
        .to_string()
        .into_bytes()
    }

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
        let mut input = WorkerInput::new_fixture(MARKET_GESHA_DEMO);
        input.mode = "live-scrape".to_string();
        let err = process_request(input).unwrap_err();
        assert!(err.contains("unsupported mode"));
    }

    #[test]
    fn test_live_index_mode_fails_closed_without_fixture_fallback() {
        let input = WorkerInput::new_live_index("ARABICA-REF/USD", 2, 3600, 500);
        let err = process_request(input).unwrap_err();
        assert!(err.contains("unsupported execution mode 'live_index'"));
    }

    #[test]
    fn test_public_benchmark_mode_imf_pcps_and_no_fixture_fallback() {
        let allowlist = NetworkAllowlist::from_embedded_manifest().unwrap();
        let input = WorkerInput::new_public_benchmark(
            MARKET_ARABICA_MONTHLY_IMF,
            HISTORICAL_MONTHLY_MAX_PRICE_AGE_SEC,
        );

        // 1. Valid IMF SDMX fetch succeeds cleanly
        let mock_ok = MockHttpTransport::new();
        mock_ok.enqueue_ok_json(200, sample_imf_json_bytes());
        let output =
            process_request_with_transport(input.clone(), &mock_ok, &allowlist).expect("IMF ok");
        assert_eq!(output.market, MARKET_ARABICA_MONTHLY_IMF);
        assert_eq!(output.price, "727794219");
        assert_eq!(output.source, DEFAULT_IMF_PCPS_SOURCE);
        assert_eq!(output.source_count, 1);
        assert_eq!(output.observed_at, 1_790_812_799);
        assert_eq!(output.data_class, DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY);

        // 2. When IMF fails (HTTP 503, timeout, or malformed JSON), engine MUST NOT fall back to fixture
        let mock_503 = MockHttpTransport::new();
        mock_503.enqueue_ok_json(503, b"{}".to_vec());
        let err_503 =
            process_request_with_transport(input.clone(), &mock_503, &allowlist).unwrap_err();
        assert!(err_503.contains("503"));
        assert!(!err_503.contains("roastake-fixture"));

        let mock_timeout = MockHttpTransport::new();
        mock_timeout.enqueue_err(TransportError::Timeout {
            provider: Some(DEFAULT_IMF_PCPS_SOURCE),
            phase: TimeoutPhase::FirstByte,
        });
        assert!(process_request_with_transport(input.clone(), &mock_timeout, &allowlist).is_err());

        let mock_bad = MockHttpTransport::new();
        mock_bad.enqueue_ok_json(200, b"{corrupt".to_vec());
        assert!(process_request_with_transport(input.clone(), &mock_bad, &allowlist).is_err());

        // 3. Disallowed host fails closed even if mock would return 200
        let empty_allowlist = NetworkAllowlist::empty();
        let mock_blocked = MockHttpTransport::new();
        mock_blocked.enqueue_ok_json(200, sample_imf_json_bytes());
        let err_blocked =
            process_request_with_transport(input, &mock_blocked, &empty_allowlist).unwrap_err();
        assert!(err_blocked.contains("not in allowlist"));
        assert_eq!(mock_blocked.recorded_calls().len(), 0);
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
