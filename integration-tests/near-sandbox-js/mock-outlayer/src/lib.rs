use near_sdk::{env, near, AccountId, Gas, NearToken, Promise, PromiseOrValue};
use roastake_shared::{
    ExecutionSource, RequestParams, ResourceLimits, SecretsReference, WorkerOutput,
};

#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MockMode {
    Success,
    NullResponse,
    InitialPanic,
    DownstreamPromiseFailure,
}

impl Default for MockMode {
    fn default() -> Self {
        MockMode::Success
    }
}

#[near(contract_state)]
pub struct MockOutlayer {
    pub mode: MockMode,
    pub total_deposits_received: NearToken,
    pub retained_deposit: NearToken,
    pub last_payer_account_id: Option<AccountId>,
    pub request_count: u64,
}

impl Default for MockOutlayer {
    fn default() -> Self {
        Self {
            mode: MockMode::Success,
            total_deposits_received: NearToken::from_yoctonear(0),
            retained_deposit: NearToken::from_yoctonear(0),
            last_payer_account_id: None,
            request_count: 0,
        }
    }
}

#[near]
impl MockOutlayer {
    #[init]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_mode(&mut self, mode: MockMode) {
        self.mode = mode;
    }

    pub fn get_mode(&self) -> MockMode {
        self.mode
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
    #[allow(unused_variables)]
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
        self.last_payer_account_id = payer_account_id;

        match self.mode {
            MockMode::InitialPanic => {
                env::panic_str("MockOutlayer: simulated initial execution panic in request_execution");
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
}
