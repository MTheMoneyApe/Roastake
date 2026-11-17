use near_sdk::{env, near, AccountId, Gas, NearToken, Promise, PromiseOrValue};
use roastake_shared::{
    ExecutionSource, RequestParams, ResourceLimits, SecretsReference, WorkerOutput,
};

#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MockMode {
    #[default]
    Success,
    NullResponse,
    InitialPanic,
    DownstreamPromiseFailure,
    CustomOutput,
}

/// Safe metadata snapshot captured from the canonical 7-argument OutLayer `request_execution` ABI.
///
/// Records routing, limits, stdin payload, presence of `secrets_ref`, response format,
/// payer account, and optional `params` without storing or exposing secret values.
#[near(serializers = [borsh, json])]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RecordedExecutionMetadata {
    pub request_index: u64,
    pub source_kind: String,
    pub wasm_url: Option<String>,
    pub wasm_hash: Option<String>,
    pub build_target: Option<String>,
    pub max_instructions: Option<u64>,
    pub max_memory_mb: Option<u32>,
    pub max_execution_seconds: Option<u64>,
    pub input_data: Option<String>,
    pub secrets_ref_present: bool,
    pub response_format: Option<String>,
    pub payer_account_id: Option<AccountId>,
    pub params_present: bool,
    pub params_json: Option<String>,
}

#[near(contract_state)]
pub struct MockOutlayer {
    pub mode: MockMode,
    pub total_deposits_received: NearToken,
    pub retained_deposit: NearToken,
    pub last_payer_account_id: Option<AccountId>,
    pub request_count: u64,
    pub custom_output_json: Option<String>,
    pub last_execution: Option<RecordedExecutionMetadata>,
}

impl Default for MockOutlayer {
    fn default() -> Self {
        Self {
            mode: MockMode::Success,
            total_deposits_received: NearToken::from_yoctonear(0),
            retained_deposit: NearToken::from_yoctonear(0),
            last_payer_account_id: None,
            request_count: 0,
            custom_output_json: None,
            last_execution: None,
        }
    }
}

#[near]
#[allow(clippy::too_many_arguments)]
impl MockOutlayer {
    #[init]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_mode(&mut self, mode: MockMode) {
        self.mode = mode;
    }

    pub fn set_custom_output(&mut self, output: WorkerOutput) {
        self.custom_output_json = Some(
            serde_json::to_string(&output)
                .unwrap_or_else(|e| env::panic_str(&format!("ERR_MOCK_SERIALIZE: {e}"))),
        );
        self.mode = MockMode::CustomOutput;
    }

    pub fn clear_custom_output(&mut self) {
        self.custom_output_json = None;
    }

    pub fn get_mode(&self) -> MockMode {
        self.mode
    }

    pub fn get_custom_output(&self) -> Option<WorkerOutput> {
        self.custom_output_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<WorkerOutput>(s).ok())
    }

    pub fn get_last_execution(&self) -> Option<RecordedExecutionMetadata> {
        self.last_execution.clone()
    }

    pub fn get_stats(&self) -> (u64, NearToken, NearToken, Option<AccountId>) {
        (
            self.request_count,
            self.total_deposits_received,
            self.retained_deposit,
            self.last_payer_account_id.clone(),
        )
    }

    #[payable]
    #[allow(clippy::too_many_arguments)]
    pub fn request_execution(
        &mut self,
        source: ExecutionSource,
        resource_limits: Option<ResourceLimits>,
        input_data: Option<String>,
        secrets_ref: Option<SecretsReference>,
        response_format: Option<String>,
        payer_account_id: Option<AccountId>,
        params: Option<RequestParams>,
    ) -> PromiseOrValue<Option<WorkerOutput>> {
        let deposit = env::attached_deposit();
        self.request_count += 1;
        self.total_deposits_received = self.total_deposits_received.saturating_add(deposit);
        self.last_payer_account_id = payer_account_id.clone();

        let (source_kind, wasm_url, wasm_hash, build_target) = match &source {
            ExecutionSource::WasmUrl { wasm_url } => (
                "WasmUrl".to_string(),
                Some(wasm_url.url.clone()),
                Some(wasm_url.hash.clone()),
                Some(wasm_url.build_target.clone()),
            ),
            ExecutionSource::Project { .. } => ("Project".to_string(), None, None, None),
            ExecutionSource::GitHub { .. } => ("GitHub".to_string(), None, None, None),
        };

        let (max_instructions, max_memory_mb, max_execution_seconds) = match &resource_limits {
            Some(rl) => (
                rl.max_instructions,
                rl.max_memory_mb,
                rl.max_execution_seconds,
            ),
            None => (None, None, None),
        };

        let params_present = params.is_some();
        let params_json = params.as_ref().and_then(|p| serde_json::to_string(p).ok());

        // Record metadata strictly from the 7-argument ABI without exposing secret contents.
        self.last_execution = Some(RecordedExecutionMetadata {
            request_index: self.request_count,
            source_kind,
            wasm_url,
            wasm_hash,
            build_target,
            max_instructions,
            max_memory_mb,
            max_execution_seconds,
            input_data,
            secrets_ref_present: secrets_ref.is_some(),
            response_format,
            payer_account_id,
            params_present,
            params_json,
        });

        match self.mode {
            MockMode::InitialPanic => {
                env::panic_str(
                    "MockOutlayer: simulated initial execution panic in request_execution",
                );
            }
            MockMode::Success => {
                let now_sec = env::block_timestamp_ms() / 1000;
                let mut output = WorkerOutput::fixture_gesha_demo();
                output.observed_at = now_sec;
                PromiseOrValue::Value(Some(output))
            }
            MockMode::NullResponse => {
                // Returns JSON null, matching OutLayer worker failure
                PromiseOrValue::Value(None)
            }
            MockMode::CustomOutput => {
                let raw = self.custom_output_json.as_deref().unwrap_or_else(|| {
                    env::panic_str(
                        "MockOutlayer: CustomOutput mode requires set_custom_output(...) first",
                    )
                });
                let configured: WorkerOutput = serde_json::from_str(raw).unwrap_or_else(|e| {
                    env::panic_str(&format!("ERR_MOCK_DESERIALIZE_CUSTOM_OUTPUT: {e}"))
                });
                PromiseOrValue::Value(Some(configured))
            }
            MockMode::DownstreamPromiseFailure => {
                // Retain the attached deposit in this contract
                self.retained_deposit = self.retained_deposit.saturating_add(deposit);
                // Return a dependent promise that fails downstream in Sandbox:
                // Calling a nonexistent method on current account will panic with MethodNotFound
                let promise = Promise::new(env::current_account_id()).function_call(
                    "nonexistent_downstream_target_method_xyz".to_string(),
                    vec![],
                    NearToken::from_yoctonear(0),
                    Gas::from_tgas(5),
                );
                PromiseOrValue::Promise(promise)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::testing_env;
    use roastake_shared::{
        DATA_CLASS_PUBLIC_BENCHMARK_MONTHLY, DECIMALS_COFFEE_INDEX, DEFAULT_IMF_PCPS_SOURCE,
        MARKET_ARABICA_MONTHLY_IMF, SCHEMA_VERSION, UNIT_USD_PER_KG,
    };
    use serde_json::json;

    #[test]
    fn test_mock_modes_and_stats() {
        let mut mock = MockOutlayer::new();
        assert_eq!(mock.get_mode(), MockMode::Success);

        mock.set_mode(MockMode::NullResponse);
        assert_eq!(mock.get_mode(), MockMode::NullResponse);

        let (count, total, retained, last_payer) = mock.get_stats();
        assert_eq!(count, 0);
        assert_eq!(total, NearToken::from_yoctonear(0));
        assert_eq!(retained, NearToken::from_yoctonear(0));
        assert_eq!(last_payer, None);
    }

    #[test]
    fn test_canonical_json_deserialization_of_args() {
        // Construct canonical 7-argument JSON payload matching OutLayer ABI
        let raw_json = json!({
            "source": {
                "WasmUrl": {
                    "url": "https://test.fastfs.io/roastake/worker.wasm",
                    "hash": "abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6",
                    "build_target": "wasm32-wasip2"
                }
            },
            "resource_limits": {
                "max_instructions": 1_000_000_000,
                "max_memory_mb": 128,
                "max_execution_seconds": 60
            },
            "input_data": r#"{"schema_version":1,"market":"GESHA-DEMO/USD","mode":"fixture"}"#,
            "secrets_ref": null,
            "response_format": "Json",
            "payer_account_id": "alice.testnet",
            "params": null
        });

        // Test that serde_json deserializes these keys into respective types without error
        let _source: ExecutionSource = serde_json::from_value(raw_json["source"].clone()).unwrap();
        let limits: Option<ResourceLimits> =
            serde_json::from_value(raw_json["resource_limits"].clone()).unwrap();
        let _input_data: Option<String> =
            serde_json::from_value(raw_json["input_data"].clone()).unwrap();
        let _secrets_ref: Option<SecretsReference> =
            serde_json::from_value(raw_json["secrets_ref"].clone()).unwrap();
        let response_format: Option<String> =
            serde_json::from_value(raw_json["response_format"].clone()).unwrap();
        let payer_account_id: Option<AccountId> =
            serde_json::from_value(raw_json["payer_account_id"].clone()).unwrap();
        let _params: Option<RequestParams> =
            serde_json::from_value(raw_json["params"].clone()).unwrap();

        assert_eq!(payer_account_id.unwrap().as_str(), "alice.testnet");
        assert_eq!(response_format.unwrap(), "Json");
        assert!(limits.is_some());
    }

    #[test]
    fn test_mock_request_execution_success() {
        let context = VMContextBuilder::new()
            .attached_deposit(NearToken::from_yoctonear(50_000_000_000_000_000_000_000))
            .block_timestamp(1_700_000_000_000_000_000)
            .build();
        testing_env!(context);

        let mut mock = MockOutlayer::new();
        let source = ExecutionSource::wasm_url(
            "https://test.fastfs.io/roastake/worker.wasm",
            "abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6",
            "wasm32-wasip2",
        );

        let res = mock.request_execution(
            source,
            Some(ResourceLimits::default()),
            Some("{}".to_string()),
            None,
            Some("Json".to_string()),
            Some("payer.testnet".parse().unwrap()),
            None,
        );

        match res {
            PromiseOrValue::Value(Some(output)) => {
                assert_eq!(output.market, "GESHA-DEMO/USD");
                assert_eq!(output.price, "1850000000");
            }
            _ => panic!("Expected Value(Some(WorkerOutput))"),
        }

        let (count, total, retained, last_payer) = mock.get_stats();
        assert_eq!(count, 1);
        assert_eq!(
            total,
            NearToken::from_yoctonear(50_000_000_000_000_000_000_000)
        );
        assert_eq!(retained, NearToken::from_yoctonear(0));
        assert_eq!(last_payer.unwrap().as_str(), "payer.testnet");
    }

    #[test]
    fn test_canonical_7_arg_abi_with_secrets_ref_and_live_input() {
        let context = VMContextBuilder::new()
            .attached_deposit(NearToken::from_yoctonear(50_000_000_000_000_000_000_000))
            .block_timestamp(1_700_000_000_000_000_000)
            .build();
        testing_env!(context);

        let raw_json = json!({
            "source": {
                "WasmUrl": {
                    "url": "https://test.fastfs.io/roastake/worker.wasm",
                    "hash": "abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6",
                    "build_target": "wasm32-wasip2"
                }
            },
            "resource_limits": {
                "max_instructions": 1_000_000_000,
                "max_memory_mb": 128,
                "max_execution_seconds": 60
            },
            "input_data": r#"{"schema_version":1,"market":"ARABICA-REF/USD","mode":"live_index","min_source_count":2,"max_upstream_age_sec":3600,"max_source_spread_bps":500}"#,
            "secrets_ref": {
                "profile": "arabica-prod",
                "account_id": "roastake-secrets.testnet"
            },
            "response_format": "Json",
            "payer_account_id": "alice.testnet",
            "params": null
        });

        let keys: Vec<&str> = raw_json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys.len(), 7);

        let source: ExecutionSource = serde_json::from_value(raw_json["source"].clone()).unwrap();
        let limits: Option<ResourceLimits> =
            serde_json::from_value(raw_json["resource_limits"].clone()).unwrap();
        let input_data: Option<String> =
            serde_json::from_value(raw_json["input_data"].clone()).unwrap();
        let secrets_ref: Option<SecretsReference> =
            serde_json::from_value(raw_json["secrets_ref"].clone()).unwrap();
        let response_format: Option<String> =
            serde_json::from_value(raw_json["response_format"].clone()).unwrap();
        let payer_account_id: Option<AccountId> =
            serde_json::from_value(raw_json["payer_account_id"].clone()).unwrap();
        let params: Option<RequestParams> =
            serde_json::from_value(raw_json["params"].clone()).unwrap();

        let sr = secrets_ref.clone().expect("non-null secrets_ref");
        assert_eq!(sr.profile, "arabica-prod");
        assert_eq!(sr.account_id, "roastake-secrets.testnet");

        let mut mock = MockOutlayer::new();
        let _res = mock.request_execution(
            source,
            limits,
            input_data,
            secrets_ref,
            response_format,
            payer_account_id,
            params,
        );
        assert_eq!(mock.get_stats().0, 1);

        let meta = mock.get_last_execution().expect("recorded metadata");
        assert!(meta.secrets_ref_present);
        assert_eq!(meta.payer_account_id.unwrap().as_str(), "alice.testnet");
        assert_eq!(meta.response_format.unwrap(), "Json");
        assert!(!meta.params_present);
    }

    #[test]
    fn test_mock_custom_output_and_abi_metadata_capture() {
        let context = VMContextBuilder::new()
            .attached_deposit(NearToken::from_yoctonear(50_000_000_000_000_000_000_000))
            .block_timestamp(1_794_340_800_000_000_000)
            .build();
        testing_env!(context);

        let mut mock = MockOutlayer::new();
        let imf_output = WorkerOutput {
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
        mock.set_custom_output(imf_output.clone());
        assert_eq!(mock.get_mode(), MockMode::CustomOutput);

        let imf_input = r#"{"schema_version":1,"market":"ARABICA-MONTHLY-IMF/USD","mode":"public_benchmark","min_source_count":1,"max_upstream_age_sec":7776000,"max_source_spread_bps":0}"#;
        let res = mock.request_execution(
            ExecutionSource::wasm_url(
                "https://test.fastfs.io/roastake/workers/roastake_coffee_price_index.wasm",
                "855bc554762aa9b367ac2b3c62a219c250410f3715256e822f8c24e23df100f1",
                "wasm32-wasip2",
            ),
            Some(ResourceLimits::default()),
            Some(imf_input.to_string()),
            None,
            Some("Json".to_string()),
            Some("bob.testnet".parse().unwrap()),
            None,
        );

        match res {
            PromiseOrValue::Value(Some(returned)) => assert_eq!(returned, imf_output),
            _ => panic!("expected CustomOutput payload"),
        }

        let meta = mock.get_last_execution().expect("recorded metadata");
        assert_eq!(meta.request_index, 1);
        assert_eq!(meta.source_kind, "WasmUrl");
        assert_eq!(
            meta.wasm_url.as_deref(),
            Some("https://test.fastfs.io/roastake/workers/roastake_coffee_price_index.wasm")
        );
        assert_eq!(
            meta.wasm_hash.as_deref(),
            Some("855bc554762aa9b367ac2b3c62a219c250410f3715256e822f8c24e23df100f1")
        );
        assert_eq!(meta.build_target.as_deref(), Some("wasm32-wasip2"));
        assert_eq!(meta.max_instructions, Some(1_000_000_000));
        assert_eq!(meta.max_memory_mb, Some(128));
        assert_eq!(meta.max_execution_seconds, Some(60));
        assert_eq!(meta.input_data.as_deref(), Some(imf_input));
        assert!(!meta.secrets_ref_present);
        assert_eq!(meta.response_format.as_deref(), Some("Json"));
        assert_eq!(
            meta.payer_account_id.as_ref().map(|a| a.as_str()),
            Some("bob.testnet")
        );
        assert!(!meta.params_present);
        assert_eq!(meta.params_json, None);
    }
}
