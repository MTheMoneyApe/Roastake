#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::outlayer::calculate_execution_cost;
    use crate::types::{
        ClaimStatus, InsuranceCompensationStatus, PriceObservation, ReconcileOutcome, RefundStatus,
        MAX_WORKER_DATA_CLASS_BYTES, MAX_WORKER_SOURCE_BYTES,
    };
    use crate::{
        RoastakeOracleAdapter, DISPUTE_RESOLUTION_DELAY_BLOCKS,
        HARD_MAX_ACTIVE_INSURANCE_COMPENSATIONS, HARD_MAX_AUTHORIZED_REQUESTERS, HARD_MAX_MARKETS,
        HARD_MAX_OPEN_REQUESTS, OPEN_REQUEST_STORAGE_BUDGET_BYTES, STATE_GROWTH_MARGIN_BYTES,
        STATE_STORAGE_MARGIN_BYTES, STUCK_CLAIM_MIN_AGE_BLOCKS,
    };
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::{testing_env, AccountId, NearToken, PromiseResult, VMContext};
    use roastake_shared::{
        calculate_divergence_bps, format_display_price, ExecutionSource, RequestParams,
        ResourceLimits, WorkerOutput, DATA_CLASS_SYNTHETIC, DECIMALS_COFFEE_INDEX,
        DEFAULT_FIXTURE_PRICE_RAW, MARKET_GESHA_DEMO, SCHEMA_VERSION, UNIT_USD_PER_KG,
    };

    fn alice() -> AccountId {
        "alice.testnet".parse().unwrap()
    }

    fn bob() -> AccountId {
        "bob.testnet".parse().unwrap()
    }

    fn oracle_contract() -> AccountId {
        "oracle.testnet".parse().unwrap()
    }

    fn outlayer_contract() -> AccountId {
        "outlayer.testnet".parse().unwrap()
    }

    fn set_test_env_with_promises(context: VMContext, promise_results: Vec<PromiseResult>) {
        testing_env!(
            context,
            near_sdk::test_vm_config(),
            near_sdk::RuntimeFeesConfig::test(),
            Default::default(),
            promise_results
        );
    }

    const TEST_MIN_OPERATING_RESERVE: NearToken = NearToken::from_millinear(10);
    const TEST_EVIDENCE_SHA256: &str =
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn setup_contract() -> RoastakeOracleAdapter {
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .signer_account_id(alice())
            .account_balance(NearToken::from_near(100))
            .block_timestamp(1_700_000_000_000_000_000); // 1.7B seconds
        testing_env!(context.build());

        RoastakeOracleAdapter::init(
            alice(),
            Some(outlayer_contract()),
            Some("https://test.fastfs.io/roastake/worker.wasm".to_string()),
            Some("abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6".to_string()),
            TEST_MIN_OPERATING_RESERVE,
            None,
        )
    }

    fn designate_and_promote(contract: &mut RoastakeOracleAdapter, nonce: u64, amount: NearToken) {
        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .signer_account_id(alice())
            .account_balance(NearToken::from_near(100));
        testing_env!(admin_ctx.build());
        contract.admin_designate_refund_reserve(amount, TEST_EVIDENCE_SHA256.to_string());
        contract.promote_unreconciled_refund(nonce);
    }

    // 1. Test Valid Oracle Result Flow (removes pending request and stores price)
    #[test]
    fn test_valid_oracle_result_flow() {
        let mut contract = setup_contract();

        // 1. Request price update as authorized admin
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .signer_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50))
            .block_timestamp(1_700_000_000_000_000_000);
        testing_env!(context.build());

        let _promise = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
        assert_eq!(contract.get_next_nonce(), 2);

        let pending = contract
            .get_pending_request(1)
            .expect("pending request stored");
        assert_eq!(pending.nonce, 1);
        assert_eq!(pending.market, MARKET_GESHA_DEMO);
        assert_eq!(pending.requester, alice());

        // 2. Simulate OutLayer callback with valid worker output
        let worker_output = WorkerOutput::fixture_gesha_demo();
        let payload_bytes = serde_json::to_vec(&worker_output).unwrap();

        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract()) // Private self-callback
            .signer_account_id(alice())
            .block_timestamp(1_700_000_010_000_000_000); // +10s

        set_test_env_with_promises(
            cb_context.build(),
            vec![PromiseResult::Successful(payload_bytes)],
        );

        let observation = contract
            .on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string())
            .expect("observation stored");

        assert_eq!(observation.price.0, DEFAULT_FIXTURE_PRICE_RAW);
        assert_eq!(observation.decimals, DECIMALS_COFFEE_INDEX);
        assert_eq!(observation.unit, UNIT_USD_PER_KG);
        assert_eq!(observation.source, "roastake-fixture");
        assert_eq!(observation.source_count, 1);
        assert_eq!(observation.data_class, DATA_CLASS_SYNTHETIC);
        assert_eq!(observation.nonce, 1);

        // Verify state persistence and pending request cleanup
        let stored = contract
            .get_price(MARKET_GESHA_DEMO.to_string())
            .expect("price stored in state");
        assert_eq!(stored.price.0, DEFAULT_FIXTURE_PRICE_RAW);
        assert!(
            contract.get_pending_request(1).is_none(),
            "pending request safely cleaned up on success"
        );
    }

    // 2. Test Unauthorized Request (must panic)
    #[test]
    #[should_panic(expected = "Unauthorized requester")]
    fn test_unauthorized_request() {
        let mut contract = setup_contract();

        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(bob()) // Not authorized
            .signer_account_id(bob())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(context.build());

        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
    }

    // 3. Test Unauthorized Callback (must panic)
    #[test]
    #[should_panic(expected = "Method is private")]
    fn test_unauthorized_callback() {
        let mut contract = setup_contract();

        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(bob()) // Not contract itself!
            .signer_account_id(bob());
        testing_env!(context.build());

        contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
    }

    // 4. Test Duplicate Nonce (must panic to reject replay)
    #[test]
    #[should_panic(expected = "ERR_UNKNOWN_OR_DUPLICATE_NONCE")]
    fn test_duplicate_nonce() {
        let mut contract = setup_contract();

        // Initiate request #1
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // Callback #1 succeeds and removes pending request
        let payload = serde_json::to_vec(&WorkerOutput::fixture_gesha_demo()).unwrap();
        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(
            cb_context.build(),
            vec![PromiseResult::Successful(payload.clone())],
        );
        contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());

        // Replay of same nonce #1 must panic and prevent second update
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Successful(payload)]);
        contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
    }

    // 5. Test Unknown Request Nonce (must panic)
    #[test]
    #[should_panic(expected = "ERR_UNKNOWN_OR_DUPLICATE_NONCE")]
    fn test_unknown_request() {
        let mut contract = setup_contract();

        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Successful(vec![])]);

        // Unknown nonce 999
        contract.on_outlayer_execution(999, MARKET_GESHA_DEMO.to_string());
    }

    // 6. Test Wrong Market (rejected safely, pending request cleaned up, no stuck state)
    #[test]
    fn test_wrong_market() {
        let mut contract = setup_contract();

        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let payload = serde_json::to_vec(&WorkerOutput::fixture_gesha_demo()).unwrap();
        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Successful(payload)]);

        // Callback with mismatched market terminates normally returning None
        let res = contract.on_outlayer_execution(1, "BOURBON-DEMO/USD".to_string());
        assert!(res.is_none(), "mismatched market returns None");
        assert!(
            contract.get_pending_request(1).is_none(),
            "pending request safely cleaned up, not stuck"
        );
        assert!(
            contract.get_price(MARKET_GESHA_DEMO.to_string()).is_none(),
            "no price stored for mismatched market"
        );
    }

    // 7. Test Invalid JSON (rejected safely, pending request cleaned up)
    #[test]
    fn test_invalid_json() {
        let mut contract = setup_contract();

        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let corrupt_payload = b"not valid json {{{".to_vec();
        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(
            cb_context.build(),
            vec![PromiseResult::Successful(corrupt_payload)],
        );

        let res = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert!(res.is_none(), "invalid JSON returns None without panic");
        assert!(
            contract.get_pending_request(1).is_none(),
            "pending request cleaned up, not stuck"
        );
        assert!(contract.get_price(MARKET_GESHA_DEMO.to_string()).is_none());
    }

    // 8. Test Malformed Price (rejected safely, pending request cleaned up)
    #[test]
    fn test_malformed_price_string() {
        let mut contract = setup_contract();

        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut bad_output = WorkerOutput::fixture_gesha_demo();
        bad_output.price = "not_a_number".to_string();
        let payload = serde_json::to_vec(&bad_output).unwrap();

        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Successful(payload)]);

        let res = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert!(res.is_none(), "malformed price returns None without panic");
        assert!(
            contract.get_pending_request(1).is_none(),
            "pending request cleaned up, not stuck"
        );
        assert!(contract.get_price(MARKET_GESHA_DEMO.to_string()).is_none());
    }

    // 9. Test Stale Timestamp (rejected safely, pending request cleaned up)
    #[test]
    fn test_stale_timestamp() {
        let mut contract = setup_contract();

        let now_sec = 1_700_000_000u64;
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50))
            .block_timestamp(now_sec * 1_000_000_000);
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // Max age is 3600s. Observed at (now - 5000s) is stale.
        let mut stale_output = WorkerOutput::fixture_gesha_demo();
        stale_output.observed_at = now_sec - 5000;
        stale_output.data_class = "LIVE_VERIFIED".to_string();
        let payload = serde_json::to_vec(&stale_output).unwrap();

        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .block_timestamp(now_sec * 1_000_000_000);
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Successful(payload)]);

        let res = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert!(res.is_none(), "stale timestamp returns None");
        assert!(
            contract.get_pending_request(1).is_none(),
            "pending request cleaned up, not stuck"
        );
        assert!(contract.get_price(MARKET_GESHA_DEMO.to_string()).is_none());
    }

    // 10. Test Future Timestamp (rejected safely, pending request cleaned up)
    #[test]
    fn test_future_timestamp() {
        let mut contract = setup_contract();

        let now_sec = 1_700_000_000u64;
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50))
            .block_timestamp(now_sec * 1_000_000_000);
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // Future timestamp (+500s exceeds 60s max drift)
        let mut future_output = WorkerOutput::fixture_gesha_demo();
        future_output.observed_at = now_sec + 500;
        future_output.data_class = "LIVE_VERIFIED".to_string();
        let payload = serde_json::to_vec(&future_output).unwrap();

        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .block_timestamp(now_sec * 1_000_000_000);
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Successful(payload)]);

        let res = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert!(res.is_none(), "future timestamp returns None");
        assert!(
            contract.get_pending_request(1).is_none(),
            "pending request cleaned up, not stuck"
        );
        assert!(contract.get_price(MARKET_GESHA_DEMO.to_string()).is_none());
    }

    // 11. Test Excessive Price Movement (rejected safely, preserves previous valid price)
    #[test]
    fn test_excessive_price_movement() {
        let mut contract = setup_contract();

        // Store first valid price: $18.50 (1_850_000_000)
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let payload1 = serde_json::to_vec(&WorkerOutput::fixture_gesha_demo()).unwrap();
        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(
            cb_context.build(),
            vec![PromiseResult::Successful(payload1)],
        );
        let first_obs = contract
            .on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string())
            .expect("first price stored");
        assert_eq!(first_obs.price.0, DEFAULT_FIXTURE_PRICE_RAW);

        // Initiate second request
        context.attached_deposit(NearToken::from_millinear(50));
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // Attempt price of $30.00 (3_000_000_000): +62% change vs 20% max divergence
        let mut diverged_output = WorkerOutput::fixture_gesha_demo();
        diverged_output.price = "3000000000".to_string();
        let payload2 = serde_json::to_vec(&diverged_output).unwrap();

        set_test_env_with_promises(
            cb_context.build(),
            vec![PromiseResult::Successful(payload2)],
        );
        let res = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());
        assert!(res.is_none(), "divergent price returns None");
        assert!(
            contract.get_pending_request(2).is_none(),
            "pending request #2 cleaned up"
        );

        // Verify original price is 100% preserved
        let preserved = contract
            .get_price(MARKET_GESHA_DEMO.to_string())
            .expect("original price intact");
        assert_eq!(preserved.price.0, DEFAULT_FIXTURE_PRICE_RAW);
        assert_eq!(preserved.nonce, 1);
    }

    // 12. Test Execution Failure Preserves Last Valid Price
    #[test]
    fn test_execution_failure_preserves_last_price() {
        let mut contract = setup_contract();

        // Store first valid price
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let payload = serde_json::to_vec(&WorkerOutput::fixture_gesha_demo()).unwrap();
        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Successful(payload)]);
        contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());

        let initial_price = contract.get_price(MARKET_GESHA_DEMO.to_string()).unwrap();

        // Second execution request
        context.attached_deposit(NearToken::from_millinear(50));
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // Callback receives Failed promise
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Failed]);

        let res = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());
        assert!(res.is_none(), "failed callback returns None");
        assert!(
            contract.get_pending_request(2).is_none(),
            "failed pending request cleaned up"
        );

        // Verify last valid price was NOT overwritten or cleared
        let preserved_price = contract.get_price(MARKET_GESHA_DEMO.to_string()).unwrap();
        assert_eq!(preserved_price.price.0, initial_price.price.0);
        assert_eq!(preserved_price.nonce, 1);
    }

    // 13. Test Oversized Payload Handled Safely
    #[test]
    fn test_oversized_payload_handled_safely() {
        let mut contract = setup_contract();

        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // Payload > 16 KB (16 * 1024 = 16384 bytes)
        let huge_payload = vec![b'x'; 20_000];
        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(
            cb_context.build(),
            vec![PromiseResult::Successful(huge_payload)],
        );

        let res = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert!(res.is_none(), "oversized payload returns None");
        assert!(
            contract.get_pending_request(1).is_none(),
            "pending request cleaned up on oversized payload"
        );
    }

    // 14. Test Out-of-Range Price Rejected Safely
    #[test]
    fn test_out_of_range_price_rejected() {
        let mut contract = setup_contract();

        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // Price $1000/kg (100_000_000_000) exceeds max_price $500/kg (50_000_000_000)
        let mut high_output = WorkerOutput::fixture_gesha_demo();
        high_output.price = "100000000000".to_string();
        let payload = serde_json::to_vec(&high_output).unwrap();

        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Successful(payload)]);

        let res = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert!(res.is_none(), "out-of-bounds price returns None");
        assert!(contract.get_pending_request(1).is_none());
        assert!(contract.get_price(MARKET_GESHA_DEMO.to_string()).is_none());
    }

    // 15. Test Wrong Unit or Decimals Rejected Safely
    #[test]
    fn test_wrong_unit_or_decimals_rejected() {
        let mut contract = setup_contract();

        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // Wrong unit ("EUR/kg" instead of "USD/kg")
        let mut bad_unit_output = WorkerOutput::fixture_gesha_demo();
        bad_unit_output.unit = "EUR/kg".to_string();
        let payload = serde_json::to_vec(&bad_unit_output).unwrap();

        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Successful(payload)]);

        let res = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert!(res.is_none(), "wrong unit returns None");
        assert!(contract.get_pending_request(1).is_none());
    }

    // 16. Test Correct Fixed-Point Conversion and Formatting
    #[test]
    fn test_fixed_point_conversion_and_formatting() {
        // Standard Gesha fixture ($18.50/kg)
        assert_eq!(format_display_price(1_850_000_000, 8), "18.50000000");

        // Sub-dollar coffee grade (e.g. $0.75/kg)
        assert_eq!(format_display_price(75_000_000, 8), "0.75000000");

        // Premium auction lot (e.g. $250.00/kg)
        assert_eq!(format_display_price(25_000_000_000, 8), "250.00000000");

        // Verify basis points divergence calculation
        let base = 1_850_000_000u128;
        let up_5pct = 1_942_500_000u128;
        assert_eq!(calculate_divergence_bps(base, up_5pct), 500); // 5% = 500 bps
    }

    // 17. Test Correct Storage of Source Metadata
    #[test]
    fn test_storage_of_source_metadata() {
        let mut contract = setup_contract();

        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50))
            .block_timestamp(1_700_000_000_000_000_000);
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let payload = serde_json::to_vec(&WorkerOutput::fixture_gesha_demo()).unwrap();
        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .block_timestamp(1_700_000_025_000_000_000); // 1_700_000_025 sec
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Successful(payload)]);

        let obs = contract
            .on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string())
            .unwrap();

        assert_eq!(obs.market, "GESHA-DEMO/USD");
        assert_eq!(obs.price.0, 1_850_000_000);
        assert_eq!(obs.decimals, 8);
        assert_eq!(obs.unit, "USD/kg");
        assert_eq!(obs.source, "roastake-fixture");
        assert_eq!(obs.source_count, 1);
        assert_eq!(obs.observed_at_sec, 0);
        assert_eq!(obs.stored_at_sec, 1_700_000_025);
        assert_eq!(obs.nonce, 1);
        assert_eq!(obs.data_class, "SYNTHETIC_DEMO");
    }

    // 18. Test Mainnet Endpoint Rejection
    #[test]
    fn test_mainnet_endpoint_rejection() {
        // 1. Mainnet account rejection on init
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice());
        testing_env!(context.build());

        let res1 = std::panic::catch_unwind(|| {
            RoastakeOracleAdapter::init(
                "admin.near".parse().unwrap(), // Mainnet account!
                Some(outlayer_contract()),
                None,
                None,
                TEST_MIN_OPERATING_RESERVE,
                None,
            );
        });
        assert!(res1.is_err(), "mainnet admin must be rejected");

        let res2 = std::panic::catch_unwind(|| {
            RoastakeOracleAdapter::init(
                alice(),
                Some("outlayer.near".parse().unwrap()), // Mainnet outlayer!
                None,
                None,
                TEST_MIN_OPERATING_RESERVE,
                None,
            );
        });
        assert!(res2.is_err(), "mainnet outlayer contract must be rejected");

        // 2. Mainnet admin change rejection
        let mut contract = setup_contract();
        let res3 = std::panic::catch_unwind(move || {
            contract.set_admin("new-admin.near".parse().unwrap());
        });
        assert!(res3.is_err(), "mainnet admin update must be rejected");

        // 3. Mainnet outlayer update rejection
        let mut contract2 = setup_contract();
        let res4 = std::panic::catch_unwind(move || {
            contract2.set_outlayer_contract("outlayer.near".parse().unwrap());
        });
        assert!(res4.is_err(), "mainnet outlayer update must be rejected");

        // 4. Mainnet requester addition rejection
        let mut contract3 = setup_contract();
        let res5 = std::panic::catch_unwind(move || {
            contract3.add_requester("trader.near".parse().unwrap());
        });
        assert!(res5.is_err(), "mainnet requester must be rejected");
    }

    // 19. Test OutLayer request_execution JSON Argument Serialization (Canonical 7-Param ABI)
    #[test]
    fn test_outlayer_request_execution_serialization() {
        let source = ExecutionSource::wasm_url(
            "https://test.fastfs.io/roastake/worker.wasm",
            "abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6",
            "wasm32-wasip2",
        );
        let limits = ResourceLimits::default();
        let input_json = r#"{"schema_version":1,"market":"GESHA-DEMO/USD","mode":"fixture"}"#;
        let payer: AccountId = "alice.testnet".parse().unwrap();

        // Exact 7-argument OutLayer ABI
        let request_payload = serde_json::json!({
            "source": source,
            "resource_limits": limits,
            "input_data": input_json,
            "secrets_ref": null,
            "response_format": "Json",
            "payer_account_id": payer,
            "params": null,
        });

        let json_str = serde_json::to_string(&request_payload).unwrap();

        // Verify all 7 fields match OutLayer canonical ABI
        assert!(json_str.contains("\"source\":{\"WasmUrl\":{"));
        assert!(json_str.contains("\"resource_limits\":{"));
        assert!(json_str.contains("\"max_instructions\":1000000000"));
        assert!(json_str.contains("\"max_execution_seconds\":60"));
        assert!(json_str.contains("\"input_data\":"));
        assert!(json_str.contains("\"secrets_ref\":null"));
        assert!(json_str.contains("\"response_format\":\"Json\""));
        assert!(json_str.contains("\"payer_account_id\":\"alice.testnet\""));
        assert!(json_str.contains("\"params\":null"));
        // Confirm callback_receiver_id is NOT in the 7-param OutLayer ABI
        assert!(!json_str.contains("callback_receiver_id"));

        // Verify non-null RequestParams serialization
        let active_params = RequestParams {
            force_rebuild: true,
            store_on_fastfs: false,
            compile_only: false,
            project_uuid: None,
            attached_usd: Some("2500000".to_string()),
            use_bound_identity: true,
        };
        let params_json = serde_json::to_string(&active_params).unwrap();
        assert!(params_json.contains("\"attached_usd\":\"2500000\""));
        assert!(params_json.contains("\"use_bound_identity\":true"));
        assert!(params_json.contains("\"force_rebuild\":true"));
    }

    // 20. Test Pricing Formula Matches Live RPC Queries to the exact yoctoNEAR
    #[test]
    fn test_pricing_formula_verified_against_live_rpc() {
        let default_limits = ResourceLimits::default();

        // Benchmark 1: 1B instructions, 60s, 0 bytes
        // Expected: 7,000,100,000,000,000,000,000 yoctoNEAR
        let cost1 = calculate_execution_cost(&default_limits, 0);
        assert_eq!(cost1.as_yoctonear(), 7_000_100_000_000_000_000_000);

        // Benchmark 2: 1B instructions, 60s, 100 bytes (under 32KB threshold)
        // Expected: 7,000,100,000,000,000,000,000 yoctoNEAR
        let cost2 = calculate_execution_cost(&default_limits, 100);
        assert_eq!(cost2.as_yoctonear(), 7_000_100_000_000_000_000_000);

        // Benchmark 3: 1B instructions, 60s, exactly 32,768 bytes (free threshold boundary)
        // Expected: 7,000,100,000,000,000,000,000 yoctoNEAR
        let cost3 = calculate_execution_cost(&default_limits, 32_768);
        assert_eq!(cost3.as_yoctonear(), 7_000_100_000_000_000_000_000);

        // Benchmark 4: 1B instructions, 60s, 32,769 bytes (1 byte over threshold: +10^19 yocto)
        // Expected: 7,010,100,000,000,000,000,000 yoctoNEAR
        let cost4 = calculate_execution_cost(&default_limits, 32_769);
        assert_eq!(cost4.as_yoctonear(), 7_010_100_000_000_000_000_000);

        // Benchmark 5: 1B instructions, 60s, 40,000 bytes (7,232 bytes over: +7.232 * 10^22 yocto)
        // Expected: 79,320,100,000,000,000,000,000 yoctoNEAR
        let cost5 = calculate_execution_cost(&default_limits, 40_000);
        assert_eq!(cost5.as_yoctonear(), 79_320_100_000_000_000_000_000);

        // Benchmark 6: 5B instructions, 60s, 0 bytes (+4 * 10^17 yocto)
        // Expected: 7,000,500,000,000,000,000,000 yoctoNEAR
        let limits_5b = ResourceLimits {
            max_instructions: Some(5_000_000_000),
            max_memory_mb: Some(128),
            max_execution_seconds: Some(60),
        };
        let cost6 = calculate_execution_cost(&limits_5b, 0);
        assert_eq!(cost6.as_yoctonear(), 7_000_500_000_000_000_000_000);

        // Benchmark 7: 1B instructions, 120s, 0 bytes (+60s * 10^20 = +6 * 10^21 yocto)
        // Expected: 13,000,100,000,000,000,000,000 yoctoNEAR
        let limits_120s = ResourceLimits {
            max_instructions: Some(1_000_000_000),
            max_memory_mb: Some(128),
            max_execution_seconds: Some(120),
        };
        let cost7 = calculate_execution_cost(&limits_120s, 0);
        assert_eq!(cost7.as_yoctonear(), 13_000_100_000_000_000_000_000);
    }

    // 21. Test OutLayer Worker Failure Returning "null" Handled Safely
    #[test]
    fn test_outlayer_null_response_handled_safely() {
        let mut contract = setup_contract();

        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // OutLayer yield-resume resolves with b"null" when worker fails
        let null_payload = b"null".to_vec();
        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(
            cb_context.build(),
            vec![PromiseResult::Successful(null_payload)],
        );

        let res = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert!(res.is_none(), "null response returns None");
        assert!(
            contract.get_pending_request(1).is_none(),
            "pending request cleaned up on null response"
        );
        assert!(contract.get_price(MARKET_GESHA_DEMO.to_string()).is_none());
    }

    // 22. Test Cross-Contract Failure (PromiseError::Failed): records UnreconciledFailure,
    // promotes against designated reserve, and verifies complete lifecycle with claim initiation,
    // in-flight state, callback finalization, and terminal claim removal.
    #[test]
    fn test_cross_contract_failure_records_refund_credit() {
        let mut contract = setup_contract();

        let deposit = NearToken::from_millinear(50);
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // Confirm pending request is tracked
        assert!(contract.get_pending_request(1).is_some());

        // Simulate NEAR PromiseResult::Failed
        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Failed]);

        let res = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert!(res.is_none(), "failed call returns None");

        // 1. Pending request is cleaned up (removed from state, cannot be replayed)
        assert!(
            contract.get_pending_request(1).is_none(),
            "pending request cleaned up after cross-contract failure"
        );

        // 2. Previous price is preserved (not modified)
        assert!(
            contract.get_price(MARKET_GESHA_DEMO.to_string()).is_none(),
            "no spurious price written on failure"
        );

        // 2b. UnreconciledFailure is recorded with 0 payable liability initially
        let unreconciled = contract
            .get_unreconciled_failure(1)
            .expect("unreconciled failure recorded");
        assert_eq!(unreconciled.requester, alice());
        assert_eq!(
            unreconciled.deposit_at_risk.as_yoctonear(),
            deposit.as_yoctonear()
        );
        assert!(contract.get_pending_refund(alice()).is_none());
        assert_eq!(contract.get_total_refund_liability().0, 0);

        // 2c. Designate returned funds as refund reserve and promote to backed RefundCredit
        designate_and_promote(&mut contract, 1, deposit);
        assert!(contract.get_unreconciled_failure(1).is_none());

        // 3. A RefundCredit is recorded for alice (the original requester) with Available status
        let credit = contract
            .get_pending_refund(alice())
            .expect("refund credit recorded for alice");
        assert_eq!(credit.requester, alice(), "credit is attributed to alice");
        assert_eq!(
            credit.available_amount.as_yoctonear(),
            deposit.as_yoctonear(),
            "available credit amount equals attached deposit"
        );
        assert_eq!(
            credit.in_flight_amount.as_yoctonear(),
            0,
            "no in-flight amount initially"
        );
        assert_eq!(credit.status, RefundStatus::Available);
        assert_eq!(credit.from_nonce, 1, "credit references correct nonce");

        // 4. Aggregate liability is updated to match deposit exactly
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear(),
            "total refund liability matches deposit"
        );

        // 5. Alice initiates claim_refund()
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100));
        testing_env!(cb_context.build());
        let _claim_promise = contract.claim_refund();

        // 6. In-Flight State: Available is 0, InFlight is deposit, Liability is STILL deposit!
        let in_flight_credit = contract
            .get_pending_refund(alice())
            .expect("credit remains recorded while in flight");
        assert_eq!(
            in_flight_credit.available_amount.as_yoctonear(),
            0,
            "available amount is 0 while in flight"
        );
        assert_eq!(
            in_flight_credit.in_flight_amount.as_yoctonear(),
            deposit.as_yoctonear(),
            "in-flight amount tracks active transfer"
        );
        assert_eq!(in_flight_credit.status, RefundStatus::InFlight);
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear(),
            "liability is NOT prematurely decreased before transfer confirmation"
        );

        let claim_id = in_flight_credit
            .active_claim_id
            .expect("active claim id recorded");
        assert_eq!(
            contract.get_claim(claim_id).unwrap().status,
            ClaimStatus::InFlight
        );

        // 7. Successful Transfer Callback: on_refund_transfer with Successful
        let mut final_cb = VMContextBuilder::new();
        final_cb
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(final_cb.build(), vec![PromiseResult::Successful(vec![])]);

        let cb_res = contract.on_refund_transfer(alice(), deposit, claim_id);
        assert!(cb_res, "successful transfer callback returns true");
        assert!(
            contract.get_claim(claim_id).is_none(),
            "terminal settled claim record removed from storage"
        );

        // 8. After successful confirmation, credit record is completely removed and liability zeroed
        assert!(
            contract.get_pending_refund(alice()).is_none(),
            "credit consumed after confirmed transfer"
        );
        assert_eq!(
            contract.get_total_refund_liability().0,
            0,
            "liability zeroed after confirmed transfer"
        );
    }

    // Note on refund routing transparency:
    // The test framework's `get_created_receipts()` can inspect mock receipt actions.
    // However `Promise::refund_to()` requires the `deterministic-account-ids` feature
    // (gated in near-sdk 5.30.1, not enabled). Consequently `MockAction::SetRefundTo` is
    // never emitted. The credit-based fallback is verified through state inspection above,
    // which is the only mechanism available without that feature flag.

    // 23. Test claim_refund by unauthorized account panics
    #[test]
    #[should_panic(expected = "No pending refund credit")]
    fn test_claim_refund_no_credit_panics() {
        let mut contract = setup_contract();

        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(bob());
        testing_env!(context.build());

        // Bob has no credit, should panic
        let _ = contract.claim_refund();
    }

    // Helper to generate a WorkerOutput serialized JSON padded with trailing JSON whitespace to an exact byte length
    fn make_worker_output_with_len(target_len: usize) -> Vec<u8> {
        let output = WorkerOutput {
            schema_version: SCHEMA_VERSION,
            market: MARKET_GESHA_DEMO.to_string(),
            price: DEFAULT_FIXTURE_PRICE_RAW.to_string(),
            decimals: DECIMALS_COFFEE_INDEX,
            unit: UNIT_USD_PER_KG.to_string(),
            source: "roastake-fixture".to_string(),
            source_count: 1,
            observed_at: 1_700_000_000,
            data_class: DATA_CLASS_SYNTHETIC.to_string(),
        };
        let mut full_json = serde_json::to_vec(&output).unwrap();
        let base_len = full_json.len();
        assert!(
            base_len <= target_len,
            "base JSON length ({}) exceeds target length ({})",
            base_len,
            target_len
        );
        let pad_len = target_len - base_len;
        full_json.extend(std::iter::repeat_n(b' ', pad_len));
        assert_eq!(full_json.len(), target_len, "exact byte length mismatch");
        full_json
    }

    // 24. Test TooLong response at exactly 1,025 bytes: request cleaned up, price preserved, no credit recorded
    #[test]
    fn test_too_long_response_handled_safely() {
        let mut contract = setup_contract();

        // Store a first valid price so we can verify it's preserved
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let payload = serde_json::to_vec(&WorkerOutput::fixture_gesha_demo()).unwrap();
        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(payload)]);
        let first = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert!(first.is_some(), "first price stored");

        // Initiate a second request
        ctx.attached_deposit(NearToken::from_millinear(50));
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // Simulate a payload exactly at the 1,024-byte boundary + 1 (exactly 1,025 bytes)
        // The SDK's promise_result_checked returns PromiseError::TooLong when len > max_len.
        let oversized = make_worker_output_with_len(1025);
        assert_eq!(oversized.len(), 1025, "must be exactly 1,025 bytes");
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(oversized)]);

        let res = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());
        assert!(
            res.is_none(),
            "too-long response (1,025 bytes) returns None"
        );

        // Pending request #2 is removed
        assert!(
            contract.get_pending_request(2).is_none(),
            "pending request #2 cleaned up"
        );

        // Previous valid price is preserved, unaltered
        let preserved = contract
            .get_price(MARKET_GESHA_DEMO.to_string())
            .expect("first price preserved");
        assert_eq!(preserved.nonce, 1, "original price (nonce=1) preserved");

        // TooLong is not a cross-contract failure — no refund credit is recorded
        assert!(
            contract.get_pending_refund(alice()).is_none(),
            "no refund credit for TooLong (OutLayer accepted and ran the execution)"
        );
    }

    // 25. Test valid response at the exact 1,024-byte boundary succeeds and stores observation
    #[test]
    fn test_valid_response_at_boundary_accepted() {
        let mut contract = setup_contract();

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // Construct a payload that is EXACTLY 1,024 bytes (the direct yield/resume maximum)
        let payload = make_worker_output_with_len(crate::MAX_OUTLAYER_RESPONSE_BYTES);
        assert_eq!(
            payload.len(),
            1024,
            "must be exactly 1,024 bytes at boundary"
        );

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .block_timestamp(1_700_000_010 * 1_000_000_000);
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(payload)]);

        let result = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert!(
            result.is_some(),
            "valid fixture at exactly 1,024 bytes is accepted"
        );
        let obs = result.unwrap();
        assert_eq!(obs.price.0, DEFAULT_FIXTURE_PRICE_RAW);
        assert!(
            contract.get_pending_request(1).is_none(),
            "pending cleaned up"
        );
        assert!(
            contract.get_price(MARKET_GESHA_DEMO.to_string()).is_some(),
            "price observation stored"
        );
    }

    // 26. Test Failed Refund Transfer: transitions to ReturnPending, reconciles return, and allows retry
    #[test]
    fn test_failed_refund_transfer_restores_entitlement() {
        let mut contract = setup_contract();

        let deposit = NearToken::from_millinear(50);
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // OutLayer call fails -> UnreconciledFailure -> promote to RefundCredit
        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit);

        // Alice claims refund -> status InFlight
        cb_context.predecessor_account_id(alice());
        testing_env!(cb_context.build());
        let _ = contract.claim_refund();

        let inflight = contract.get_pending_refund(alice()).expect("in flight");
        assert_eq!(inflight.status, RefundStatus::InFlight);
        let claim_id_1 = inflight.active_claim_id.expect("active claim id 1");
        assert_eq!(inflight.available_amount.as_yoctonear(), 0);
        assert_eq!(
            inflight.in_flight_amount.as_yoctonear(),
            deposit.as_yoctonear()
        );
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear()
        );

        // NEAR Transfer FAILS! (PromiseResult::Failed) -> transitions to ReturnPending
        let mut transfer_cb = VMContextBuilder::new();
        transfer_cb
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(transfer_cb.build(), vec![PromiseResult::Failed]);

        let success = contract.on_refund_transfer(alice(), deposit, claim_id_1);
        assert!(!success, "callback returns false on failure");
        assert_eq!(
            contract.get_claim(claim_id_1).unwrap().status,
            ClaimStatus::ReturnPending,
            "claim record marked ReturnPending until system refund is verified"
        );

        let pending_ret = contract
            .get_pending_refund(alice())
            .expect("return pending credit");
        assert_eq!(pending_ret.status, RefundStatus::ReturnPending);
        assert_eq!(pending_ret.available_amount.as_yoctonear(), 0);
        assert_eq!(pending_ret.in_flight_amount.as_yoctonear(), 0);
        assert_eq!(
            pending_ret.return_pending_amount.as_yoctonear(),
            deposit.as_yoctonear()
        );
        assert_eq!(pending_ret.active_claim_id, Some(claim_id_1));
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear()
        );

        // Reconcile returned transfer after system refund settles
        testing_env!(cb_context.build());
        contract.reconcile_returned_refund(claim_id_1);
        assert!(
            contract.get_claim(claim_id_1).is_none(),
            "terminal claim_id_1 removed from claims map after reconciliation"
        );

        // Verify entitlement is fully RESTORED to Available
        let restored = contract
            .get_pending_refund(alice())
            .expect("restored credit");
        assert_eq!(restored.status, RefundStatus::Available);
        assert_eq!(
            restored.available_amount.as_yoctonear(),
            deposit.as_yoctonear()
        );
        assert_eq!(restored.in_flight_amount.as_yoctonear(), 0);
        assert_eq!(restored.return_pending_amount.as_yoctonear(), 0);
        assert_eq!(restored.active_claim_id, None);
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear()
        );

        // Alice can claim again (second attempt succeeds)
        testing_env!(cb_context.build());
        let _ = contract.claim_refund();
        let claim_id_2 = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .expect("active claim id 2");
        assert_ne!(claim_id_1, claim_id_2, "new claim id allocated for retry");

        // Second transfer succeeds
        set_test_env_with_promises(transfer_cb.build(), vec![PromiseResult::Successful(vec![])]);
        let second_success = contract.on_refund_transfer(alice(), deposit, claim_id_2);
        assert!(second_success, "second callback succeeds");
        assert!(
            contract.get_claim(claim_id_2).is_none(),
            "terminal claim_id_2 removed from claims storage upon settlement"
        );

        assert!(contract.get_pending_refund(alice()).is_none());
        assert_eq!(contract.get_total_refund_liability().0, 0);
    }

    // 27. Test Duplicate Claim Attempt While Transfer is In-Flight Panics
    #[test]
    #[should_panic(
        expected = "A refund claim is already in-flight for this account; wait for transfer confirmation"
    )]
    fn test_duplicate_claim_attempt_while_in_flight_panics() {
        let mut contract = setup_contract();

        let deposit = NearToken::from_millinear(50);
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit);

        // First claim succeeds and puts transfer in flight
        cb_context.predecessor_account_id(alice());
        testing_env!(cb_context.build());
        let _ = contract.claim_refund();

        // Second claim immediately while in-flight MUST panic
        let _ = contract.claim_refund();
    }

    // 28. Test Multiple Failed Requests Accumulate Safely for Same Requester
    #[test]
    fn test_multiple_failures_accumulate_safely_for_same_requester() {
        let mut contract = setup_contract();

        let deposit1 = NearToken::from_millinear(40);
        let deposit2 = NearToken::from_millinear(30);

        // Request 1 fails
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit1);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit1);

        // Request 2 fails
        ctx.attached_deposit(deposit2);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 2, deposit2);

        // Total accumulated: 70 mNEAR
        let total_deposit = deposit1.as_yoctonear() + deposit2.as_yoctonear();
        let credit = contract
            .get_pending_refund(alice())
            .expect("accumulated credit");
        assert_eq!(credit.available_amount.as_yoctonear(), total_deposit);
        assert_eq!(credit.from_nonce, 2);
        assert_eq!(contract.get_total_refund_liability().0, total_deposit);

        // Alice claims the accumulated 70 mNEAR in one operation
        ctx.predecessor_account_id(alice());
        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        let claim_id = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .expect("active claim id");

        // Callback succeeds
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);
        let ok = contract.on_refund_transfer(
            alice(),
            NearToken::from_yoctonear(total_deposit),
            claim_id,
        );
        assert!(ok);
        assert!(contract.get_claim(claim_id).is_none());

        assert!(contract.get_pending_refund(alice()).is_none());
        assert_eq!(contract.get_total_refund_liability().0, 0);
    }

    // 29. Test Multiple Requesters with Independent Credits and Settlements
    #[test]
    fn test_multiple_requesters_independent_credits_and_settlement() {
        let mut contract = setup_contract();
        contract.add_requester(bob());

        let dep_alice = NearToken::from_millinear(40);
        let dep_bob = NearToken::from_millinear(60);

        // Alice request fails
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(dep_alice);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, dep_alice);

        // Bob request fails
        ctx.predecessor_account_id(bob()).attached_deposit(dep_bob);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 2, dep_bob);

        let total = dep_alice.as_yoctonear() + dep_bob.as_yoctonear();
        assert_eq!(contract.get_total_refund_liability().0, total);

        // Alice claims her refund
        ctx.predecessor_account_id(alice());
        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        let claim_id_alice = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .expect("alice claim id");

        // Bob's credit is untouched
        let bob_credit = contract
            .get_pending_refund(bob())
            .expect("bob credit intact");
        assert_eq!(
            bob_credit.available_amount.as_yoctonear(),
            dep_bob.as_yoctonear()
        );
        assert_eq!(bob_credit.status, RefundStatus::Available);

        // Alice transfer succeeds
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);
        let _ = contract.on_refund_transfer(alice(), dep_alice, claim_id_alice);
        assert!(contract.get_claim(claim_id_alice).is_none());

        assert!(contract.get_pending_refund(alice()).is_none());
        assert_eq!(
            contract.get_total_refund_liability().0,
            dep_bob.as_yoctonear()
        );

        // Bob claims his refund
        ctx.predecessor_account_id(bob());
        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        let claim_id_bob_1 = contract
            .get_pending_refund(bob())
            .unwrap()
            .active_claim_id
            .expect("bob claim id 1");

        // Bob transfer fails -> ReturnPending -> reconcile
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_refund_transfer(bob(), dep_bob, claim_id_bob_1);
        testing_env!(ctx.build());
        contract.reconcile_returned_refund(claim_id_bob_1);
        assert!(contract.get_claim(claim_id_bob_1).is_none());

        // Bob restored
        let bob_restored = contract.get_pending_refund(bob()).expect("bob restored");
        assert_eq!(
            bob_restored.available_amount.as_yoctonear(),
            dep_bob.as_yoctonear()
        );
        assert_eq!(
            contract.get_total_refund_liability().0,
            dep_bob.as_yoctonear()
        );

        // Bob retries and succeeds
        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        let claim_id_bob_2 = contract
            .get_pending_refund(bob())
            .unwrap()
            .active_claim_id
            .expect("bob claim id 2");

        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);
        let _ = contract.on_refund_transfer(bob(), dep_bob, claim_id_bob_2);
        assert!(contract.get_claim(claim_id_bob_2).is_none());

        assert!(contract.get_pending_refund(bob()).is_none());
        assert_eq!(contract.get_total_refund_liability().0, 0);
    }

    // 30. Test Aggregate Liability Exactness Across Interleaved Transitions
    #[test]
    fn test_aggregate_liability_exactness_across_all_transitions() {
        let mut contract = setup_contract();

        let d1 = NearToken::from_millinear(50);
        let d2 = NearToken::from_millinear(20);

        // Step 1: Initial state
        assert_eq!(contract.get_total_refund_liability().0, 0);

        // Step 2: Failure 1 (+50 mNEAR)
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(d1);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, d1);

        assert_eq!(contract.get_total_refund_liability().0, d1.as_yoctonear());

        // Step 3: Alice claims 50 mNEAR (InFlight)
        ctx.predecessor_account_id(alice());
        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        let claim_id_1 = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .expect("active claim id 1");

        // Liability must STILL be 50 mNEAR!
        assert_eq!(contract.get_total_refund_liability().0, d1.as_yoctonear());

        // Step 4: Mid-flight failure! Request 2 fails while Request 1 transfer is in flight (+20 mNEAR)
        ctx.attached_deposit(d2);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 2, d2);

        // Total liability must be 50 (in flight) + 20 (available) = 70 mNEAR!
        let total_70 = d1.as_yoctonear() + d2.as_yoctonear();
        assert_eq!(contract.get_total_refund_liability().0, total_70);

        let c = contract.get_pending_refund(alice()).expect("credit");
        assert_eq!(c.in_flight_amount.as_yoctonear(), d1.as_yoctonear());
        assert_eq!(c.available_amount.as_yoctonear(), d2.as_yoctonear());
        assert_eq!(c.total_entitlement().as_yoctonear(), total_70);

        // Step 5: Transfer 1 succeeds (-50 mNEAR)
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);
        let _ = contract.on_refund_transfer(alice(), d1, claim_id_1);
        assert!(contract.get_claim(claim_id_1).is_none());

        // Liability must now be 20 mNEAR, and record remains with status Available
        assert_eq!(contract.get_total_refund_liability().0, d2.as_yoctonear());
        let c2 = contract
            .get_pending_refund(alice())
            .expect("remaining credit");
        assert_eq!(c2.in_flight_amount.as_yoctonear(), 0);
        assert_eq!(c2.available_amount.as_yoctonear(), d2.as_yoctonear());
        assert_eq!(c2.status, RefundStatus::Available);

        // Step 6: Claim remaining 20 mNEAR
        ctx.predecessor_account_id(alice());
        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        let claim_id_2 = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .expect("active claim id 2");

        assert_eq!(contract.get_total_refund_liability().0, d2.as_yoctonear());

        // Step 7: Transfer 2 succeeds (-20 mNEAR)
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);
        let _ = contract.on_refund_transfer(alice(), d2, claim_id_2);
        assert!(contract.get_claim(claim_id_2).is_none());

        // Final state: 0 liability, record removed
        assert_eq!(contract.get_total_refund_liability().0, 0);
        assert!(contract.get_pending_refund(alice()).is_none());
    }

    // 31. Test Insufficient Spendable Contract Balance Prevents Claim
    #[test]
    #[should_panic(expected = "SOLVENCY_INVARIANT_VIOLATION")]
    fn test_insufficient_contract_balance_prevents_claim() {
        let mut contract = setup_contract();

        let deposit = NearToken::from_millinear(50);
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit);

        // Context with 0 account balance (less than storage requirement, liquid balance = 0)
        let mut starved_context = VMContextBuilder::new();
        starved_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_yoctonear(0));
        testing_env!(starved_context.build());

        // Attempting to claim without liquid balance MUST panic and protect contract solvency
        let _ = contract.claim_refund();
    }

    // 32. Test Refund Callback Unauthorized Caller Panics
    #[test]
    #[should_panic(expected = "Method is private: callback must be invoked by contract itself")]
    fn test_refund_callback_unauthorized_caller_panics() {
        let mut contract = setup_contract();

        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(bob()); // Bob attempts to invoke private callback
        testing_env!(context.build());

        let _ = contract.on_refund_transfer(alice(), NearToken::from_millinear(50), 1);
    }

    // 33. Test Repeated Callback Replay Attempt Panics
    #[test]
    #[should_panic(expected = "missing refund claim record for claim_id")]
    fn test_repeated_callback_replay_attempt_panics() {
        let mut contract = setup_contract();

        let deposit = NearToken::from_millinear(50);
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit);

        cb_context.predecessor_account_id(alice());
        testing_env!(cb_context.build());
        let _ = contract.claim_refund();
        let claim_id = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .expect("active claim id");

        // First callback succeeds and deletes terminal claim record
        cb_context.predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Successful(vec![])]);
        let ok = contract.on_refund_transfer(alice(), deposit, claim_id);
        assert!(ok);
        assert!(contract.get_claim(claim_id).is_none());

        // Replay of the exact same callback MUST panic (terminal claim was removed)
        let _ = contract.on_refund_transfer(alice(), deposit, claim_id);
    }

    // 34. Test Callback Replay with Excess Amount Panics
    #[test]
    #[should_panic(expected = "claim amount mismatch")]
    fn test_callback_replay_with_excess_amount_panics() {
        let mut contract = setup_contract();

        let deposit = NearToken::from_millinear(20);
        let mut context = VMContextBuilder::new();
        context
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(context.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_context = VMContextBuilder::new();
        cb_context
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit);

        cb_context.predecessor_account_id(alice());
        testing_env!(cb_context.build());
        let _ = contract.claim_refund();
        let claim_id = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .expect("active claim id");

        // Callback with 50 mNEAR when only 20 mNEAR was in-flight MUST panic
        cb_context.predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_context.build(), vec![PromiseResult::Successful(vec![])]);
        let _ = contract.on_refund_transfer(alice(), NearToken::from_millinear(50), claim_id);
    }

    // 35. Test Failed OutLayer Call Preserves Valid Price
    #[test]
    fn test_failed_outlayer_call_preserves_valid_price() {
        let mut contract = setup_contract();

        // Store first valid observation (nonce 1)
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50));
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let payload = serde_json::to_vec(&WorkerOutput::fixture_gesha_demo()).unwrap();
        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(payload)]);
        let obs1 = contract
            .on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string())
            .expect("price stored");

        // Request 2 fails with PromiseResult::Failed
        ctx.attached_deposit(NearToken::from_millinear(50));
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let res2 = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());
        assert!(res2.is_none());

        // Stored price MUST be completely unchanged
        let current_price = contract
            .get_price(MARKET_GESHA_DEMO.to_string())
            .expect("price preserved");
        assert_eq!(current_price.nonce, 1);
        assert_eq!(current_price.price.0, obs1.price.0);
        assert_eq!(current_price.observed_at_sec, obs1.observed_at_sec);
    }

    // =========================================================================
    // SECTION 5: DEDICATED GLOBAL SOLVENCY & RECOVERY HARDENING TESTS (1-15)
    // =========================================================================

    // 36 (Solvency 1): Multiple users with aggregate liabilities greater than one user's individual refund
    #[test]
    fn test_solvency_1_multiple_users_aggregate_liabilities_exceed_single_refund() {
        let mut contract = setup_contract();
        contract.add_requester(bob());

        let dep_alice = NearToken::from_millinear(30);
        let dep_bob = NearToken::from_millinear(40);

        // Alice request fails
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(dep_alice);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, dep_alice);

        // Bob request fails
        ctx.predecessor_account_id(bob()).attached_deposit(dep_bob);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 2, dep_bob);

        let total_liability = contract.get_total_refund_liability().0;
        let expected_total = dep_alice.as_yoctonear() + dep_bob.as_yoctonear();
        assert_eq!(total_liability, expected_total);
        assert!(total_liability > dep_alice.as_yoctonear());
        assert!(total_liability > dep_bob.as_yoctonear());
    }

    // 37 (Solvency 2): Under-collateralized contract rejects a claimant even when that claimant's individual amount is affordable
    #[test]
    #[should_panic(expected = "SOLVENCY_INVARIANT_VIOLATION")]
    fn test_solvency_2_undercollateralized_contract_rejects_claim_when_single_amount_affordable() {
        let mut contract = setup_contract();
        contract.add_requester(bob());

        let dep_alice = NearToken::from_millinear(30);
        let dep_bob = NearToken::from_millinear(40);

        // Fail Alice request (30 mNEAR)
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(dep_alice);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, dep_alice);

        // Fail Bob request (40 mNEAR) -> total liability = 70 mNEAR, min_operating = 100 mNEAR -> required = 170 mNEAR
        ctx.predecessor_account_id(bob()).attached_deposit(dep_bob);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 2, dep_bob);

        // Set spendable balance to 50 mNEAR (covers min_operating 10 + Alice 30 = 40, but < 80 total required)
        let mut starved_ctx = VMContextBuilder::new();
        starved_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_millinear(50))
            .storage_usage(0);
        testing_env!(starved_ctx.build());

        // Alice claims: MUST fail closed to protect Bob and operating reserve from insolvency
        let _ = contract.claim_refund();
    }

    // 38 (Solvency 3): A fully collateralized contract permits sequential claims
    #[test]
    fn test_solvency_3_fully_collateralized_permits_sequential_claims() {
        let mut contract = setup_contract();
        contract.add_requester(bob());

        let dep_alice = NearToken::from_millinear(30);
        let dep_bob = NearToken::from_millinear(40);

        // Fail Alice & Bob
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(dep_alice);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, dep_alice);

        ctx.predecessor_account_id(bob()).attached_deposit(dep_bob);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 2, dep_bob);

        // Contract has 100 NEAR spendable balance (> 170 mNEAR required backing)
        let mut claim_ctx = VMContextBuilder::new();
        claim_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100));
        testing_env!(claim_ctx.build());

        // Alice claims
        let _ = contract.claim_refund();
        let claim_alice = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .unwrap();

        // Settle Alice
        cb_ctx.account_balance(NearToken::from_near(100));
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);
        assert!(contract.on_refund_transfer(alice(), dep_alice, claim_alice));
        assert!(contract.get_claim(claim_alice).is_none());
        assert_eq!(
            contract.get_total_refund_liability().0,
            dep_bob.as_yoctonear()
        );

        // Bob claims
        claim_ctx.predecessor_account_id(bob());
        testing_env!(claim_ctx.build());
        let _ = contract.claim_refund();
        let claim_bob = contract
            .get_pending_refund(bob())
            .unwrap()
            .active_claim_id
            .unwrap();

        // Settle Bob
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);
        assert!(contract.on_refund_transfer(bob(), dep_bob, claim_bob));
        assert!(contract.get_claim(claim_bob).is_none());

        // Both settled, total liability zeroed
        assert_eq!(contract.get_total_refund_liability().0, 0);
        assert!(contract.get_pending_refund(alice()).is_none());
        assert!(contract.get_pending_refund(bob()).is_none());
    }

    // 39 (Solvency 4): A failed claim transfer preserves liability in ReturnPending and restores after reconciliation
    #[test]
    fn test_solvency_4_failed_claim_transfer_restores_exact_liability() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit);

        let initial_liability = contract.get_total_refund_liability().0;
        assert_eq!(initial_liability, deposit.as_yoctonear());

        ctx.predecessor_account_id(alice());
        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        let claim_id = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .unwrap();

        // InFlight liability is STILL initial_liability
        assert_eq!(contract.get_total_refund_liability().0, initial_liability);

        // Transfer fails -> ReturnPending
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let success = contract.on_refund_transfer(alice(), deposit, claim_id);
        assert!(!success);
        assert_eq!(contract.get_total_refund_liability().0, initial_liability);
        let ret_pending = contract.get_pending_refund(alice()).unwrap();
        assert_eq!(ret_pending.status, RefundStatus::ReturnPending);
        assert_eq!(
            ret_pending.return_pending_amount.as_yoctonear(),
            initial_liability
        );

        // Reconcile returned refund
        testing_env!(ctx.build());
        contract.reconcile_returned_refund(claim_id);
        assert!(contract.get_claim(claim_id).is_none());

        // Liability restored to Available exactly, no drift
        assert_eq!(contract.get_total_refund_liability().0, initial_liability);
        let restored = contract.get_pending_refund(alice()).unwrap();
        assert_eq!(restored.available_amount.as_yoctonear(), initial_liability);
        assert_eq!(restored.in_flight_amount.as_yoctonear(), 0);
        assert_eq!(restored.return_pending_amount.as_yoctonear(), 0);
        assert_eq!(restored.status, RefundStatus::Available);
    }

    // 40 (Solvency 5): A successful transfer reduces liability exactly once
    #[test]
    fn test_solvency_5_successful_transfer_reduces_liability_exactly_once() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit);

        ctx.predecessor_account_id(alice());
        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        let claim_id = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .unwrap();

        // Transfer succeeds
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);
        let ok = contract.on_refund_transfer(alice(), deposit, claim_id);
        assert!(ok);
        assert!(contract.get_claim(claim_id).is_none());

        // Reduced by exactly deposit (now 0)
        assert_eq!(contract.get_total_refund_liability().0, 0);
    }

    // 41 (Solvency 6): InFlight credits remain included in aggregate liabilities
    #[test]
    fn test_solvency_6_inflight_credits_remain_included_in_aggregate_liabilities() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit);

        // Status Available
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear()
        );

        // Move to InFlight
        ctx.predecessor_account_id(alice());
        testing_env!(ctx.build());
        let _ = contract.claim_refund();

        // InFlight MUST still be included in total_refund_liability!
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear()
        );
        let credit = contract.get_pending_refund(alice()).unwrap();
        assert_eq!(credit.status, RefundStatus::InFlight);
        assert_eq!(
            credit.in_flight_amount.as_yoctonear(),
            deposit.as_yoctonear()
        );
        assert_eq!(credit.available_amount.as_yoctonear(), 0);
    }

    // 42 (Solvency 7): Two claims cannot collectively exceed available funds
    #[test]
    #[should_panic(expected = "SOLVENCY_INVARIANT_VIOLATION")]
    fn test_solvency_7_two_claims_cannot_collectively_exceed_available_funds() {
        let mut contract = setup_contract();
        contract.add_requester(bob());

        let dep_alice = NearToken::from_millinear(40);
        let dep_bob = NearToken::from_millinear(40);

        // Fail Alice and Bob (total liability = 80 mNEAR, min_operating = 100 mNEAR -> required = 180 mNEAR)
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(dep_alice);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, dep_alice);

        ctx.predecessor_account_id(bob()).attached_deposit(dep_bob);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 2, dep_bob);

        // Balance is 60 mNEAR (less than 90 mNEAR required backing = 10 min_operating + 80 liability)
        let mut starved_ctx = VMContextBuilder::new();
        starved_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(bob())
            .account_balance(NearToken::from_millinear(60))
            .storage_usage(0);
        testing_env!(starved_ctx.build());

        // Bob cannot claim because collective required backing exceeds spendable balance
        let _ = contract.claim_refund();
    }

    // 43 (Solvency 8): New refund credits cannot silently overflow the aggregate liability
    #[test]
    #[should_panic(expected = "total_refund_liability overflow")]
    fn test_solvency_8_new_refund_credits_cannot_silently_overflow() {
        let mut contract = setup_contract();
        // Artificially saturate total_refund_liability
        contract.total_refund_liability = u128::MAX;

        // Recording any additional credit MUST panic with checked arithmetic error
        contract.record_refund_credit(&alice(), NearToken::from_yoctonear(1), 1);
    }

    // 44 (Solvency 9): Storage requirements are correctly incorporated into spendable balance
    #[test]
    fn test_solvency_9_storage_requirements_correctly_incorporated() {
        let mut ctx = VMContextBuilder::new();
        // 10 NEAR total liquid balance
        // 100_000 bytes storage usage -> at default 10^19 yocto/byte = 1 NEAR cost
        ctx.account_balance(NearToken::from_near(10))
            .storage_usage(100_000);
        testing_env!(ctx.build());

        let spendable = crate::compute_spendable_balance();
        // Spendable must be exactly 10 - 1 = 9 NEAR
        assert_eq!(spendable, NearToken::from_near(9).as_yoctonear());
    }

    // 45 (Solvency 10): Nonzero locked balance is handled according to verified NEAR runtime semantics
    #[test]
    fn test_solvency_10_nonzero_locked_balance_does_not_reduce_spendable() {
        let mut ctx = VMContextBuilder::new();
        // 10 NEAR liquid balance, 100_000 bytes storage (1 NEAR cost)
        // 50 NEAR validator locked stake!
        ctx.account_balance(NearToken::from_near(10))
            .storage_usage(100_000)
            .account_locked_balance(NearToken::from_near(50));
        testing_env!(ctx.build());

        let spendable = crate::compute_spendable_balance();
        // Verified NEAR runtime semantics: env::account_balance() is liquid/unlocked balance only.
        // env::account_locked_balance() is validator stake, not usable for storage and not included in account_balance().
        // Therefore, spendable remains 9 NEAR!
        assert_eq!(spendable, NearToken::from_near(9).as_yoctonear());
    }

    // 46 (Solvency 11): Storage underfunding fails closed
    #[test]
    #[should_panic(expected = "SOLVENCY_INVARIANT_VIOLATION")]
    fn test_solvency_11_storage_underfunding_fails_closed() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit);

        // Account balance is 1 NEAR, but storage usage is 200_000 bytes (requires 2 NEAR)
        // Spendable = 1 NEAR.saturating_sub(2 NEAR) = 0 yocto!
        let mut underfunded_ctx = VMContextBuilder::new();
        underfunded_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(1))
            .storage_usage(200_000);
        testing_env!(underfunded_ctx.build());

        // Claim MUST fail closed
        let _ = contract.claim_refund();
    }

    // 47 (Solvency 12): Ordinary outgoing operations cannot spend refund-reserved funds
    #[test]
    #[should_panic(expected = "Insufficient deposit")]
    fn test_solvency_12_ordinary_operations_cannot_spend_refund_reserved_funds() {
        let mut contract = setup_contract();

        // Attempting to request execution with 0 attached deposit
        // The contract requires caller to fund the execution deposit 100% upfront.
        // It cannot drain contract balance or refund liabilities.
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_yoctonear(0));
        testing_env!(ctx.build());

        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
    }

    // 48 (Solvency 13): Multiple accounts remain isolated
    #[test]
    fn test_solvency_13_multiple_accounts_remain_isolated() {
        let mut contract = setup_contract();
        contract.add_requester(bob());

        let dep_alice = NearToken::from_millinear(25);
        let dep_bob = NearToken::from_millinear(35);

        // Fail Alice and Bob
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(dep_alice);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, dep_alice);

        ctx.predecessor_account_id(bob()).attached_deposit(dep_bob);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 2, dep_bob);

        // Alice claims
        ctx.predecessor_account_id(alice());
        testing_env!(ctx.build());
        let _ = contract.claim_refund();

        // Verify Bob's credit is strictly unaffected
        let bob_credit = contract.get_pending_refund(bob()).unwrap();
        assert_eq!(
            bob_credit.available_amount.as_yoctonear(),
            dep_bob.as_yoctonear()
        );
        assert_eq!(bob_credit.in_flight_amount.as_yoctonear(), 0);
        assert_eq!(bob_credit.status, RefundStatus::Available);
        assert_eq!(bob_credit.active_claim_id, None);
    }

    // 49 (Solvency 14a): Nonexistent claim ID is rejected
    #[test]
    #[should_panic(expected = "missing refund claim record for claim_id 9999")]
    fn test_solvency_14a_nonexistent_claim_id_rejected() {
        let mut contract = setup_contract();

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);

        let _ = contract.on_refund_transfer(alice(), NearToken::from_millinear(50), 9999);
    }

    // 50 (Solvency 14b): Mismatched claimant is rejected
    #[test]
    #[should_panic(expected = "CRITICAL_ACCOUNTING_ERROR: claimant mismatch")]
    fn test_solvency_14b_mismatched_claimant_rejected() {
        let mut contract = setup_contract();
        contract.add_requester(bob());
        let deposit = NearToken::from_millinear(50);

        // Fail Alice request
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit);

        // Alice claims -> claim #1 created for Alice
        ctx.predecessor_account_id(alice());
        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        let claim_id = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .unwrap();

        // Callback attempts to attribute Alice's claim to Bob -> MUST panic with claimant mismatch
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);
        let _ = contract.on_refund_transfer(bob(), deposit, claim_id);
    }

    // 51 (Solvency 15a): Manual reconciliation requires admin authorization
    #[test]
    #[should_panic(expected = "Only admin can call this method")]
    fn test_solvency_15a_unauthorized_reconciliation_panics() {
        let mut contract = setup_contract();

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(bob()); // Bob is not admin
        testing_env!(ctx.build());

        contract.admin_reconcile_stuck_claim(1, ReconcileOutcome::ConfirmedSuccess);
    }

    // 52 (Solvency 15b): Manual reconciliation rejects non-existent claim
    #[test]
    #[should_panic(expected = "ERR_CLAIM_NOT_FOUND")]
    fn test_solvency_15b_reconcile_nonexistent_claim_panics() {
        let mut contract = setup_contract();

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice()); // alice is admin
        testing_env!(ctx.build());

        contract.admin_reconcile_stuck_claim(9999, ReconcileOutcome::ConfirmedSuccess);
    }

    // 53 (Solvency 15c): Hardened manual reconciliation: InFlight -> Disputed (ConfirmedFailure), and Disputed -> Settled (ConfirmedSuccess after delay)
    #[test]
    fn test_solvency_15c_manual_reconciliation_lifecycle() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);

        // 1. Setup stuck InFlight claim #1 for Alice at block 10
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit)
            .block_height(10);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .block_height(12);
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit);

        // Alice claims -> InFlight claim #1 at block 15
        ctx.predecessor_account_id(alice()).block_height(15);
        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        let claim_id_1 = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .unwrap();

        // 2. After STUCK_CLAIM_MIN_AGE_BLOCKS, admin marks Disputed via ConfirmedFailure (NON-PAYABLE!)
        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .block_height(15 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(admin_ctx.build());

        contract.admin_reconcile_stuck_claim(claim_id_1, ReconcileOutcome::ConfirmedFailure);

        let disputed_claim = contract
            .get_claim(claim_id_1)
            .expect("disputed claim retained");
        assert_eq!(disputed_claim.status, ClaimStatus::Disputed);
        assert_eq!(
            disputed_claim.disputed_at_block,
            Some(15 + STUCK_CLAIM_MIN_AGE_BLOCKS)
        );
        let disputed_credit = contract.get_pending_refund(alice()).unwrap();
        assert_eq!(disputed_credit.status, RefundStatus::Disputed);
        assert_eq!(disputed_credit.available_amount.as_yoctonear(), 0);
        assert_eq!(
            disputed_credit.disputed_amount.as_yoctonear(),
            deposit.as_yoctonear()
        );
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear()
        );

        // 3. After DISPUTE_RESOLUTION_DELAY_BLOCKS, admin proposes ConfirmedSuccess attestation (non-destructive)
        admin_ctx.block_height(15 + STUCK_CLAIM_MIN_AGE_BLOCKS + DISPUTE_RESOLUTION_DELAY_BLOCKS);
        testing_env!(admin_ctx.build());
        contract.admin_reconcile_stuck_claim(claim_id_1, ReconcileOutcome::ConfirmedSuccess);

        // Claim record and liability remain preserved until claimant acknowledges receipt
        let proposed_claim = contract
            .get_claim(claim_id_1)
            .expect("claim preserved after admin proposal");
        assert_eq!(proposed_claim.status, ClaimStatus::Disputed);
        assert!(proposed_claim.admin_settlement_proposed);
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear()
        );

        // 4. Claimant acknowledges receipt -> terminal claim record removed, liability zeroed, no second payout
        ctx.predecessor_account_id(alice())
            .block_height(15 + STUCK_CLAIM_MIN_AGE_BLOCKS + DISPUTE_RESOLUTION_DELAY_BLOCKS + 1);
        testing_env!(ctx.build());
        contract.acknowledge_disputed_transfer_received(claim_id_1);

        assert!(contract.get_claim(claim_id_1).is_none());
        assert_eq!(contract.get_total_refund_liability().0, 0);
        assert!(contract.get_pending_refund(alice()).is_none());
    }

    // 54 (Solvency 15d): Manual reconciliation idempotent check - cannot reconcile settled/removed claim
    #[test]
    #[should_panic(expected = "ERR_CLAIM_NOT_FOUND")]
    fn test_solvency_15d_repeated_reconciliation_panics() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit)
            .block_height(10);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .block_height(12);
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit);

        ctx.predecessor_account_id(alice()).block_height(15);
        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        let claim_id = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .unwrap();

        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .block_height(15 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(admin_ctx.build());

        // Step 1: mark Disputed via ConfirmedFailure
        contract.admin_reconcile_stuck_claim(claim_id, ReconcileOutcome::ConfirmedFailure);

        // Step 2: propose ConfirmedSuccess after appeal delay, then claimant acknowledges
        admin_ctx.block_height(15 + STUCK_CLAIM_MIN_AGE_BLOCKS + DISPUTE_RESOLUTION_DELAY_BLOCKS);
        testing_env!(admin_ctx.build());
        contract.admin_reconcile_stuck_claim(claim_id, ReconcileOutcome::ConfirmedSuccess);
        contract.acknowledge_disputed_transfer_received(claim_id);

        // Reconciliation on closed claim MUST panic because terminal claim was removed
        contract.admin_reconcile_stuck_claim(claim_id, ReconcileOutcome::ConfirmedSuccess);
    }

    // =========================================================================
    // SECTION 6: MISSION 03C.3.6 UNRECONCILED FAILURES, RESERVE & LIFECYCLE TESTS
    // =========================================================================

    // 55. Init rejects zero minimum_operating_reserve
    #[test]
    #[should_panic(expected = "minimum_operating_reserve must be strictly greater than zero")]
    fn test_m0336_init_rejects_zero_minimum_operating_reserve() {
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(10));
        testing_env!(ctx.build());

        let _ = RoastakeOracleAdapter::init(
            alice(),
            Some(outlayer_contract()),
            Some("https://test.fastfs.io/roastake/worker.wasm".to_string()),
            Some("abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6".to_string()),
            NearToken::from_yoctonear(0),
            None,
        );
    }

    // 56. Init rejects underfunded contract balance below minimum_operating_reserve
    #[test]
    #[should_panic(expected = "SOLVENCY_INVARIANT_VIOLATION")]
    fn test_m0336_init_rejects_underfunded_operating_reserve() {
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_millinear(50));
        testing_env!(ctx.build());

        let _ = RoastakeOracleAdapter::init(
            alice(),
            Some(outlayer_contract()),
            Some("https://test.fastfs.io/roastake/worker.wasm".to_string()),
            Some("abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6".to_string()),
            NearToken::from_millinear(100),
            None,
        );
    }

    // 57. Downstream OutLayer failure (Test D) cannot promote without reserve backing
    #[test]
    #[should_panic(expected = "ERR_INSUFFICIENT_REFUND_RESERVE")]
    fn test_m0336_unreconciled_failure_cannot_promote_without_reserve() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());

        // Verify UnreconciledFailure exists, 0 liability, 0 reserve
        assert!(contract.get_unreconciled_failure(1).is_some());
        assert_eq!(contract.get_total_refund_liability().0, 0);
        assert_eq!(contract.get_refund_reserve_balance().0, 0);

        // Attempting to promote without reserve MUST panic
        testing_env!(ctx.build());
        contract.promote_unreconciled_refund(1);
    }

    // 58. Admin cannot designate reserve when spendable balance has no unallocated funds
    #[test]
    #[should_panic(expected = "SOLVENCY_INVARIANT_VIOLATION")]
    fn test_m0336_admin_designate_reserve_fails_when_no_unallocated_funds() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);

        // Contract has exactly 10 mNEAR spendable (= TEST_MIN_OPERATING_RESERVE)
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_millinear(10))
            .storage_usage(0);
        testing_env!(ctx.build());

        // Attempting to designate 50 mNEAR reserve when spendable == 10 mNEAR == min_operating_reserve MUST fail
        contract.admin_designate_refund_reserve(
            deposit,
            "a37bd40d21a7d85e0de6e6bfb369bebe41313222dd938b6d6a5361d1d99883e4".to_string(),
        );
    }

    // 59. Concurrent unresolved failures (Test G): reserve funded for ONE cannot promote BOTH
    #[test]
    #[should_panic(expected = "ERR_INSUFFICIENT_REFUND_RESERVE")]
    fn test_m0336_concurrent_unreconciled_failures_single_reserve_cannot_promote_both() {
        let mut contract = setup_contract();
        contract.add_requester(bob());
        let deposit = NearToken::from_millinear(50);

        // Request 1 (Alice) & Request 2 (Bob)
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        ctx.predecessor_account_id(bob()).attached_deposit(deposit);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        // Both fail
        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());

        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());

        // Fund reserve for ONLY ONE failure (50 mNEAR)
        let mut fund_ctx = VMContextBuilder::new();
        fund_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit)
            .account_balance(NearToken::from_near(10));
        testing_env!(fund_ctx.build());
        contract.fund_refund_reserve();
        assert_eq!(
            contract.get_refund_reserve_balance().0,
            deposit.as_yoctonear()
        );

        // First promotion (nonce 1) succeeds and consumes the 50 mNEAR reserve
        contract.promote_unreconciled_refund(1);
        let promoted = contract
            .get_pending_refund(alice())
            .expect("promoted credit");
        assert_eq!(promoted.available_amount, deposit);
        assert_eq!(contract.get_refund_reserve_balance().0, 0);
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear()
        );
        assert!(contract.verify_solvency_invariant());

        // Second promotion (nonce 2) MUST panic because reserve is now 0
        contract.promote_unreconciled_refund(2);
    }

    // 60. External resolution closes UnreconciledFailure without creating local refund liability
    #[test]
    fn test_m0336_admin_resolve_unreconciled_external_closes_without_liability() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());

        assert!(contract.get_unreconciled_failure(1).is_some());

        // Admin proposes external resolution (e.g., OutLayer refunded requester directly)
        testing_env!(ctx.build());
        contract.admin_resolve_unreconciled_external(
            1,
            "74ab074a27ccddd9b6f400f3f8453c1039419acba556ba77dfb288759b3dbd0f".to_string(),
        );

        let proposed_failure = contract
            .get_unreconciled_failure(1)
            .expect("unreconciled failure preserved until requester acknowledges");
        assert!(proposed_failure.external_resolution_proposed);

        // Requester acknowledges external resolution -> record removed
        contract.acknowledge_unreconciled_resolved_external(1);

        assert!(contract.get_unreconciled_failure(1).is_none());
        assert!(contract.get_pending_refund(alice()).is_none());
        assert_eq!(contract.get_total_refund_liability().0, 0);
        assert!(contract.verify_solvency_invariant());
    }

    // 61. Reconcile returned refund fails when system refund has not yet restored backing balance
    #[test]
    #[should_panic(expected = "SOLVENCY_INVARIANT_VIOLATION")]
    fn test_m0336_reconcile_returned_refund_fails_before_system_refund_settles() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 1, deposit);

        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        let claim_id = contract
            .get_pending_refund(alice())
            .unwrap()
            .active_claim_id
            .unwrap();

        // Transfer fails -> ReturnPending
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        assert!(!contract.on_refund_transfer(alice(), deposit, claim_id));

        // Simulate delayed system-refund receipt: spendable balance is only 10 mNEAR (min_operating_reserve),
        // missing the 50 mNEAR transfer return
        let mut unreturned_ctx = VMContextBuilder::new();
        unreturned_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_millinear(10))
            .storage_usage(0);
        testing_env!(unreturned_ctx.build());

        // Reconciling before the 50 mNEAR system refund arrives MUST fail closed
        contract.reconcile_returned_refund(claim_id);
    }

    // 62. Duplicate promotion or external resolution after promotion panics
    #[test]
    #[should_panic(expected = "ERR_UNRECONCILED_NOT_FOUND")]
    fn test_m0336_duplicate_promotion_panics() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(deposit);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract());
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());

        // Fund enough reserve for two promotions, but promote the same nonce twice
        designate_and_promote(&mut contract, 1, deposit);
        contract.admin_designate_refund_reserve(
            deposit,
            "a37bd40d21a7d85e0de6e6bfb369bebe41313222dd938b6d6a5361d1d99883e4".to_string(),
        );

        // Second promotion of nonce 1 MUST panic because UnreconciledFailure was removed
        contract.promote_unreconciled_refund(1);
    }

    // 63. Increase minimum operating reserve enforces monotonicity and solvency invariant
    #[test]
    fn test_m0336_increase_minimum_operating_reserve() {
        let mut contract = setup_contract();
        assert_eq!(
            contract.get_minimum_operating_reserve().0,
            TEST_MIN_OPERATING_RESERVE.as_yoctonear()
        );

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(10));
        testing_env!(ctx.build());

        contract.increase_minimum_operating_reserve(NearToken::from_millinear(250));
        assert_eq!(
            contract.get_minimum_operating_reserve().0,
            NearToken::from_millinear(250).as_yoctonear()
        );
        assert!(contract.verify_solvency_invariant());
    }

    // =========================================================================
    // SECTION 7: MISSION 03C.3.7.2 HARDENED RECONCILIATION, INSURANCE & STORAGE TESTS
    // =========================================================================

    fn setup_in_flight_claim(
        contract: &mut RoastakeOracleAdapter,
        user: AccountId,
        nonce: u64,
        amount: NearToken,
        claim_block: u64,
    ) -> u64 {
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(user.clone())
            .attached_deposit(amount)
            .account_balance(NearToken::from_near(100))
            .block_height(claim_block);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .account_balance(NearToken::from_near(100))
            .block_height(claim_block);
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(nonce, MARKET_GESHA_DEMO.to_string());

        designate_and_promote(contract, nonce, amount);

        ctx.predecessor_account_id(user.clone())
            .attached_deposit(NearToken::from_yoctonear(0))
            .block_height(claim_block);
        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        contract
            .get_pending_refund(user)
            .unwrap()
            .active_claim_id
            .unwrap()
    }

    // 64. ConfirmedFailure NEVER restores Available even with 100 NEAR unallocated surplus
    #[test]
    fn test_m03372_confirmed_failure_never_restores_available_even_with_surplus() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);
        let claim_id = setup_in_flight_claim(&mut contract, alice(), 1, deposit, 10);

        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(500))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(admin_ctx.build());

        // Even with 500 NEAR surplus, ConfirmedFailure moves InFlight -> Disputed (non-payable)
        contract.admin_reconcile_stuck_claim(claim_id, ReconcileOutcome::ConfirmedFailure);

        let claim = contract.get_claim(claim_id).unwrap();
        assert_eq!(claim.status, ClaimStatus::Disputed);

        let credit = contract.get_pending_refund(alice()).unwrap();
        assert_eq!(credit.status, RefundStatus::Disputed);
        assert_eq!(credit.available_amount.as_yoctonear(), 0);
        assert_eq!(credit.in_flight_amount.as_yoctonear(), 0);
        assert_eq!(credit.return_pending_amount.as_yoctonear(), 0);
        assert_eq!(
            credit.disputed_amount.as_yoctonear(),
            deposit.as_yoctonear()
        );

        // Calling claim_refund while Disputed MUST panic
        let claim_again = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = contract.claim_refund();
        }));
        assert!(claim_again.is_err());

        // Calling reconcile_returned_refund on Disputed claim MUST panic
        let ret_attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.reconcile_returned_refund(claim_id);
        }));
        assert!(ret_attempt.is_err());

        // Pooled surplus did not back it into ReturnPending or Available
        assert_eq!(contract.get_backed_refund_liability().0, 0);
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear()
        );
    }

    // 65. ConfirmedFailure before STUCK_CLAIM_MIN_AGE_BLOCKS panics
    #[test]
    #[should_panic(expected = "ERR_CLAIM_TOO_RECENT")]
    fn test_m03372_confirmed_failure_before_min_age_blocks_panics() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);
        let claim_id = setup_in_flight_claim(&mut contract, alice(), 1, deposit, 10);

        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS - 1);
        testing_env!(admin_ctx.build());

        contract.admin_reconcile_stuck_claim(claim_id, ReconcileOutcome::ConfirmedFailure);
    }

    // 66. ConfirmedSuccess cannot bypass Disputed status or DISPUTE_RESOLUTION_DELAY_BLOCKS
    #[test]
    fn test_m03372_confirmed_success_requires_disputed_and_appeal_delay() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);
        let claim_id = setup_in_flight_claim(&mut contract, alice(), 1, deposit, 10);

        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(admin_ctx.build());

        // Cannot call ConfirmedSuccess directly on InFlight claim
        let direct_success = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.admin_reconcile_stuck_claim(claim_id, ReconcileOutcome::ConfirmedSuccess);
        }));
        assert!(direct_success.is_err());

        // Move InFlight -> Disputed
        contract.mark_claim_disputed(claim_id);

        // Cannot finalize ConfirmedSuccess before DISPUTE_RESOLUTION_DELAY_BLOCKS elapses
        admin_ctx
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS + DISPUTE_RESOLUTION_DELAY_BLOCKS - 1);
        testing_env!(admin_ctx.build());
        let early_success = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.admin_reconcile_stuck_claim(claim_id, ReconcileOutcome::ConfirmedSuccess);
        }));
        assert!(early_success.is_err());

        // Once delay elapses, ConfirmedSuccess proposes settlement without unilaterally deleting the claim
        admin_ctx.block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS + DISPUTE_RESOLUTION_DELAY_BLOCKS);
        testing_env!(admin_ctx.build());
        contract.admin_reconcile_stuck_claim(claim_id, ReconcileOutcome::ConfirmedSuccess);
        let proposed = contract
            .get_claim(claim_id)
            .expect("Disputed claim preserved until claimant acknowledges");
        assert!(proposed.admin_settlement_proposed);
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear()
        );

        // Claimant acknowledges -> claim deleted and liability zeroed
        contract.acknowledge_disputed_transfer_received(claim_id);
        assert!(contract.get_claim(claim_id).is_none());
        assert_eq!(contract.get_total_refund_liability().0, 0);
    }

    // 67. Late objective on_refund_transfer callback resolves a Disputed claim cleanly
    #[test]
    fn test_m03372_late_objective_callback_resolves_disputed_claim() {
        let mut contract = setup_contract();
        contract.add_requester(bob());
        let deposit = NearToken::from_millinear(50);

        // Claim 1 (Alice): disputed, then late callback objectively reports Ok (paid)
        let claim1 = setup_in_flight_claim(&mut contract, alice(), 1, deposit, 10);
        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(admin_ctx.build());
        contract.mark_claim_disputed(claim1);

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .account_balance(NearToken::from_near(100));
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);
        assert!(contract.on_refund_transfer(alice(), deposit, claim1));
        assert!(contract.get_claim(claim1).is_none());
        assert!(contract.get_pending_refund(alice()).is_none());
        assert_eq!(contract.get_total_refund_liability().0, 0);

        // Claim 2 (Bob): disputed, then late callback objectively reports Failed -> ReturnPending -> Available
        let claim2 = setup_in_flight_claim(&mut contract, bob(), 2, deposit, 20);
        admin_ctx.block_height(20 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(admin_ctx.build());
        contract.mark_claim_disputed(claim2);

        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        assert!(!contract.on_refund_transfer(bob(), deposit, claim2));
        assert_eq!(
            contract.get_claim(claim2).unwrap().status,
            ClaimStatus::ReturnPending
        );

        admin_ctx.predecessor_account_id(bob());
        testing_env!(admin_ctx.build());
        contract.reconcile_returned_refund(claim2);
        assert!(contract.get_claim(claim2).is_none());
        let restored = contract.get_pending_refund(bob()).unwrap();
        assert_eq!(restored.status, RefundStatus::Available);
        assert_eq!(restored.available_amount, deposit);
    }

    // 68. Claimant self-acknowledgement clears disputed claim without payout; non-claimant rejected
    #[test]
    fn test_m03372_claimant_acknowledgement_clears_disputed_without_payout() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);
        let claim_id = setup_in_flight_claim(&mut contract, alice(), 1, deposit, 10);

        let mut bob_ctx = VMContextBuilder::new();
        bob_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(bob())
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(bob_ctx.build());
        let unauth = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.acknowledge_disputed_transfer_received(claim_id);
        }));
        assert!(unauth.is_err());

        let mut alice_ctx = VMContextBuilder::new();
        alice_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(alice_ctx.build());
        contract.mark_claim_disputed(claim_id);
        contract.acknowledge_disputed_transfer_received(claim_id);
        assert!(contract.get_claim(claim_id).is_none());
        assert!(contract.get_pending_refund(alice()).is_none());
        assert_eq!(contract.get_total_refund_liability().0, 0);
    }

    // 69. Insurance disabled by default & strictly isolated from refund_reserve_balance / surplus
    #[test]
    fn test_m03372_insurance_disabled_by_default_and_isolated_from_refund_reserve() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);
        assert!(!contract.is_insurance_enabled());

        let claim_id = setup_in_flight_claim(&mut contract, alice(), 1, deposit, 10);
        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(admin_ctx.build());
        contract.mark_claim_disputed(claim_id);

        // Fund refund_reserve_balance (NOT insurance_reserve_balance!)
        admin_ctx.attached_deposit(deposit);
        testing_env!(admin_ctx.build());
        contract.fund_refund_reserve();

        admin_ctx
            .attached_deposit(NearToken::from_yoctonear(0))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS + DISPUTE_RESOLUTION_DELAY_BLOCKS);
        testing_env!(admin_ctx.build());

        // Fails because insurance is disabled
        let disabled_attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = contract.compensate_disputed_claim(claim_id, TEST_EVIDENCE_SHA256.to_string());
        }));
        assert!(disabled_attempt.is_err());

        // Enable insurance, but refund_reserve_balance cannot fund insurance compensation
        contract.set_insurance_enabled(true);
        let no_ins_reserve = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = contract.compensate_disputed_claim(claim_id, TEST_EVIDENCE_SHA256.to_string());
        }));
        assert!(no_ins_reserve.is_err());
    }

    // 70. Insurance compensation consumes insurance_reserve_balance exclusively & handles failed return
    #[test]
    fn test_m03372_insurance_compensation_separate_accounting_and_failed_return() {
        let mut contract = setup_contract();
        contract.add_requester(bob());
        let deposit = NearToken::from_millinear(50);

        let claim_alice = setup_in_flight_claim(&mut contract, alice(), 1, deposit, 10);
        let claim_bob = setup_in_flight_claim(&mut contract, bob(), 2, deposit, 10);

        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(admin_ctx.build());
        contract.mark_claim_disputed(claim_alice);
        contract.mark_claim_disputed(claim_bob);

        contract.set_insurance_enabled(true);
        // Fund ONLY 50 mNEAR (enough for ONE claim, not both)
        admin_ctx.attached_deposit(deposit);
        testing_env!(admin_ctx.build());
        contract.fund_insurance_reserve();

        admin_ctx
            .attached_deposit(NearToken::from_yoctonear(0))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS + DISPUTE_RESOLUTION_DELAY_BLOCKS);
        testing_env!(admin_ctx.build());

        // First compensation succeeds and drains insurance_reserve_balance to 0
        let _ = contract.compensate_disputed_claim(claim_alice, TEST_EVIDENCE_SHA256.to_string());
        assert_eq!(contract.get_insurance_reserve_balance().0, 0);
        assert!(contract.get_claim(claim_alice).is_none());
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear()
        ); // Only Bob Disputed remains

        // Second concurrent compensation for Bob MUST fail closed
        let second_attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = contract.compensate_disputed_claim(claim_bob, TEST_EVIDENCE_SHA256.to_string());
        }));
        assert!(second_attempt.is_err());

        // Simulate transfer failure on compensation_id = 1 -> ReturnPending -> reconcile_returned_insurance
        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .account_balance(NearToken::from_near(100));
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        assert!(!contract.on_insurance_transfer(1, alice(), deposit));
        assert_eq!(
            contract.get_insurance_compensation(1).unwrap().status,
            InsuranceCompensationStatus::ReturnPending
        );

        testing_env!(admin_ctx.build());
        contract.reconcile_returned_insurance(1);
        assert!(contract.get_insurance_compensation(1).is_none());
        assert_eq!(
            contract.get_insurance_reserve_balance().0,
            deposit.as_yoctonear()
        );
    }

    // 71. Open request cap and storage budget capacity enforce strict bounds
    #[test]
    fn test_m03372_open_request_cap_and_storage_budget_exhaustion() {
        let mut init_ctx = VMContextBuilder::new();
        init_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(10));
        testing_env!(init_ctx.build());

        let mut contract = RoastakeOracleAdapter::init(
            alice(),
            Some(outlayer_contract()),
            Some("https://test.fastfs.io/roastake/worker.wasm".to_string()),
            Some("abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6".to_string()),
            NearToken::from_millinear(10),
            Some(2),
        );

        let deposit = NearToken::from_millinear(50);
        let mut req_ctx = VMContextBuilder::new();
        req_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(10))
            .attached_deposit(deposit);
        testing_env!(req_ctx.build());

        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
        assert_eq!(contract.get_open_request_counts(), (2, 0, 2));

        // 3rd request MUST be rejected due to max_open_requests = 2
        let cap_hit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
        }));
        assert!(cap_hit.is_err());

        // Even after nonce 1 turns into UnreconciledFailure, total open (pending 1 + unreconciled 1 = 2) still hits cap
        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .account_balance(NearToken::from_near(10));
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert_eq!(contract.get_open_request_counts(), (1, 1, 2));

        testing_env!(req_ctx.build());
        let cap_hit_unreconciled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
        }));
        assert!(cap_hit_unreconciled.is_err());

        // Proposing external resolution preserves the slot until requester acknowledges
        contract.admin_resolve_unreconciled_external(1, TEST_EVIDENCE_SHA256.to_string());
        assert_eq!(contract.get_open_request_counts(), (1, 1, 2));
        contract.acknowledge_unreconciled_resolved_external(1);
        assert_eq!(contract.get_open_request_counts(), (1, 0, 2));

        // Now verify storage headroom check: pre-call balance = 12 mNEAR (plus 50 mNEAR attached = 62 mNEAR);
        // spendable excluding deposit = 12 mNEAR < min_op (10 mNEAR) + 512 byte storage margin (5.12 mNEAR) = 15.12 mNEAR
        req_ctx
            .account_balance(NearToken::from_millinear(12))
            .storage_usage(0);
        testing_env!(req_ctx.build());
        let low_storage = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
        }));
        assert!(low_storage.is_err());
    }

    // 72. InFlight claim blocks admin_designate_refund_reserve from usurping returning system refund
    #[test]
    #[should_panic(expected = "SOLVENCY_INVARIANT_VIOLATION")]
    fn test_m03372_in_flight_claim_blocks_admin_designate_race() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);
        let _claim_id = setup_in_flight_claim(&mut contract, alice(), 1, deposit, 10);

        // Suppose spendable temporarily equals min_op (10) + returned deposit (50) = 60 mNEAR
        // before on_refund_transfer executes. Admin MUST NOT grab those 50 mNEAR into refund_reserve_balance!
        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_millinear(60))
            .storage_usage(0);
        testing_env!(admin_ctx.build());

        contract.admin_designate_refund_reserve(deposit, TEST_EVIDENCE_SHA256.to_string());
    }

    // 73. Proof that serialized M03C.3.6 state cannot deserialize under hardened Borsh layout
    #[test]
    fn test_m03372_legacy_borsh_state_layout_cannot_deserialize_as_current() {
        #[near_sdk::near(serializers = [borsh])]
        struct LegacyM0336Adapter {
            admin: AccountId,
            outlayer_contract: AccountId,
            authorized_requesters: near_sdk::store::LookupSet<AccountId>,
            markets: near_sdk::store::IterableMap<String, crate::types::MarketConfig>,
            prices: near_sdk::store::IterableMap<String, crate::types::PriceObservation>,
            pending_requests: near_sdk::store::LookupMap<u64, crate::types::PendingRequest>,
            unreconciled_failures:
                near_sdk::store::LookupMap<u64, crate::types::UnreconciledFailure>,
            next_nonce: u64,
            pending_refunds: near_sdk::store::LookupMap<AccountId, crate::types::RefundCredit>,
            total_refund_liability: u128,
            refund_reserve_balance: u128,
            minimum_operating_reserve: u128,
            next_claim_id: u64,
            claims: near_sdk::store::LookupMap<u64, crate::types::RefundClaim>,
        }

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice());
        testing_env!(ctx.build());

        let legacy = LegacyM0336Adapter {
            admin: alice(),
            outlayer_contract: outlayer_contract(),
            authorized_requesters: near_sdk::store::LookupSet::new(b"r"),
            markets: near_sdk::store::IterableMap::new(b"m"),
            prices: near_sdk::store::IterableMap::new(b"p"),
            pending_requests: near_sdk::store::LookupMap::new(b"q"),
            unreconciled_failures: near_sdk::store::LookupMap::new(b"u"),
            next_nonce: 1,
            pending_refunds: near_sdk::store::LookupMap::new(b"f"),
            total_refund_liability: 0,
            refund_reserve_balance: 0,
            minimum_operating_reserve: 10_000_000_000_000_000_000_000,
            next_claim_id: 1,
            claims: near_sdk::store::LookupMap::new(b"c"),
        };

        let legacy_bytes = near_sdk::borsh::to_vec(&legacy).unwrap();
        let decode_attempt = near_sdk::borsh::from_slice::<RoastakeOracleAdapter>(&legacy_bytes);
        assert!(
            decode_attempt.is_err(),
            "Legacy M03C.3.6 Borsh state must fail deserialization under M03C.3.7.2 layout"
        );
    }

    // =========================================================================
    // SECTION 8: MISSION 03C.3.7.4 CONSOLIDATED INSURANCE SOLVENCY, CLAIM
    //            LIVENESS, STORAGE SAFETY & ADMIN PROPOSAL TESTS
    // =========================================================================

    // 74 (F-02): Insurance InFlight liability is tracked and blocks admin_designate_refund_reserve race
    #[test]
    fn test_m03374_f02_insurance_inflight_liability_blocks_reserve_designation() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);

        let claim_id = setup_in_flight_claim(&mut contract, alice(), 1, deposit, 10);
        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(admin_ctx.build());
        contract.mark_claim_disputed(claim_id);

        contract.set_insurance_enabled(true);
        admin_ctx.attached_deposit(deposit);
        testing_env!(admin_ctx.build());
        contract.fund_insurance_reserve();

        admin_ctx
            .attached_deposit(NearToken::from_yoctonear(0))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS + DISPUTE_RESOLUTION_DELAY_BLOCKS);
        testing_env!(admin_ctx.build());

        let _ = contract.compensate_disputed_claim(claim_id, TEST_EVIDENCE_SHA256.to_string());
        assert_eq!(contract.get_insurance_reserve_balance().0, 0);
        assert_eq!(
            contract.get_insurance_in_flight_liability().0,
            deposit.as_yoctonear()
        );
        assert_eq!(contract.get_insurance_unresolved_liability().0, 0);
        let snap = contract.get_accounting_snapshot();
        assert_eq!(snap.insurance_in_flight_liability.0, deposit.as_yoctonear());
        assert_eq!(snap.active_insurance_compensation_count, 1);

        // Suppose the insurance transfer fails at the recipient and 50 mNEAR returns to contract
        // before on_insurance_transfer executes, making spendable = min_op (10) + 50 = 60 mNEAR.
        // Admin MUST NOT be able to designate those 50 mNEAR into refund_reserve_balance!
        let mut tight_ctx = VMContextBuilder::new();
        tight_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_millinear(60))
            .storage_usage(0);
        testing_env!(tight_ctx.build());

        let usurp_attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.admin_designate_refund_reserve(deposit, TEST_EVIDENCE_SHA256.to_string());
        }));
        assert!(
            usurp_attempt.is_err(),
            "In-flight insurance liability must block admin_designate_refund_reserve race"
        );
    }

    // 75 (F-02): Missing insurance callback recovery via Disputed status, recipient ack, and late callback
    #[test]
    fn test_m03374_f02_missing_insurance_callback_disputed_and_recovery_paths() {
        let mut contract = setup_contract();
        contract.add_requester(bob());
        let deposit = NearToken::from_millinear(50);

        let claim1 = setup_in_flight_claim(&mut contract, alice(), 1, deposit, 10);
        let claim2 = setup_in_flight_claim(&mut contract, bob(), 2, deposit, 10);

        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(admin_ctx.build());
        contract.mark_claim_disputed(claim1);
        contract.mark_claim_disputed(claim2);

        contract.set_insurance_enabled(true);
        admin_ctx.attached_deposit(NearToken::from_millinear(100));
        testing_env!(admin_ctx.build());
        contract.fund_insurance_reserve();

        let comp_block = 10 + STUCK_CLAIM_MIN_AGE_BLOCKS + DISPUTE_RESOLUTION_DELAY_BLOCKS;
        admin_ctx
            .attached_deposit(NearToken::from_yoctonear(0))
            .block_height(comp_block);
        testing_env!(admin_ctx.build());

        let _ = contract.compensate_disputed_claim(claim1, TEST_EVIDENCE_SHA256.to_string());
        let _ = contract.compensate_disputed_claim(claim2, TEST_EVIDENCE_SHA256.to_string());
        assert_eq!(
            contract.get_insurance_in_flight_liability().0,
            deposit.as_yoctonear() * 2
        );

        // Cannot mark insurance compensation disputed before STUCK_CLAIM_MIN_AGE_BLOCKS
        admin_ctx.block_height(comp_block + STUCK_CLAIM_MIN_AGE_BLOCKS - 1);
        testing_env!(admin_ctx.build());
        let too_early = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.mark_insurance_compensation_disputed(1);
        }));
        assert!(too_early.is_err());

        // After STUCK_CLAIM_MIN_AGE_BLOCKS, mark both compensation 1 and 2 as Disputed
        admin_ctx.block_height(comp_block + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(admin_ctx.build());
        contract.mark_insurance_compensation_disputed(1);
        contract.mark_insurance_compensation_disputed(2);
        assert_eq!(contract.get_insurance_in_flight_liability().0, 0);
        assert_eq!(
            contract.get_insurance_unresolved_liability().0,
            deposit.as_yoctonear() * 2
        );
        assert_eq!(
            contract.get_insurance_compensation(1).unwrap().status,
            InsuranceCompensationStatus::Disputed
        );

        // Non-recipient (Bob) cannot acknowledge Alice's insurance compensation #1
        let mut bob_ctx = VMContextBuilder::new();
        bob_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(bob())
            .account_balance(NearToken::from_near(100));
        testing_env!(bob_ctx.build());
        let wrong_recipient = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.acknowledge_insurance_compensation_received(1);
        }));
        assert!(wrong_recipient.is_err());

        // Path A: Recipient (Alice) acknowledges receipt of compensation #1 -> deleted, liability reduced
        testing_env!(admin_ctx.build());
        contract.acknowledge_insurance_compensation_received(1);
        assert!(contract.get_insurance_compensation(1).is_none());
        assert_eq!(
            contract.get_insurance_unresolved_liability().0,
            deposit.as_yoctonear()
        );
        assert_eq!(
            contract
                .get_accounting_snapshot()
                .active_insurance_compensation_count,
            1
        );

        // Path B: Late objective callback on_insurance_transfer(Failed) arrives for Disputed compensation #2
        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .account_balance(NearToken::from_near(100));
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        assert!(!contract.on_insurance_transfer(2, bob(), deposit));
        assert_eq!(contract.get_insurance_unresolved_liability().0, 0);
        assert_eq!(
            contract
                .get_accounting_snapshot()
                .insurance_return_pending_liability
                .0,
            deposit.as_yoctonear()
        );

        // Reconcile returned insurance #2 back to insurance_reserve_balance and delete terminal record
        testing_env!(admin_ctx.build());
        contract.reconcile_returned_insurance(2);
        assert!(contract.get_insurance_compensation(2).is_none());
        assert_eq!(
            contract.get_insurance_reserve_balance().0,
            deposit.as_yoctonear()
        );
        assert_eq!(
            contract
                .get_accounting_snapshot()
                .active_insurance_compensation_count,
            0
        );
    }

    // 76 (F-02): Active insurance compensation cap enforced
    #[test]
    fn test_m03374_f02_max_active_insurance_compensations_cap() {
        let mut contract = setup_contract();
        contract.add_requester(bob());
        let deposit = NearToken::from_millinear(50);

        let claim1 = setup_in_flight_claim(&mut contract, alice(), 1, deposit, 10);
        let claim2 = setup_in_flight_claim(&mut contract, bob(), 2, deposit, 10);

        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(admin_ctx.build());
        contract.mark_claim_disputed(claim1);
        contract.mark_claim_disputed(claim2);

        contract.set_insurance_enabled(true);
        contract.set_max_active_insurance_compensations(1);
        admin_ctx.attached_deposit(NearToken::from_millinear(100));
        testing_env!(admin_ctx.build());
        contract.fund_insurance_reserve();

        admin_ctx
            .attached_deposit(NearToken::from_yoctonear(0))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS + DISPUTE_RESOLUTION_DELAY_BLOCKS);
        testing_env!(admin_ctx.build());

        let _ = contract.compensate_disputed_claim(claim1, TEST_EVIDENCE_SHA256.to_string());
        let cap_exceeded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = contract.compensate_disputed_claim(claim2, TEST_EVIDENCE_SHA256.to_string());
        }));
        assert!(cap_exceeded.is_err());

        // Rejects setting above HARD_MAX_ACTIVE_INSURANCE_COMPENSATIONS
        let over_hard_cap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.set_max_active_insurance_compensations(
                HARD_MAX_ACTIVE_INSURANCE_COMPENSATIONS + 1,
            );
        }));
        assert!(over_hard_cap.is_err());
    }

    // 77 (F-01): Admin cannot unilaterally extinguish a Disputed claim via ConfirmedSuccess or settlement proposal
    #[test]
    fn test_m03374_f01_admin_cannot_unilaterally_extinguish_disputed_claim() {
        let mut contract = setup_contract();
        contract.add_requester(bob());
        let deposit = NearToken::from_millinear(50);

        let claim_id = setup_in_flight_claim(&mut contract, bob(), 1, deposit, 10);
        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(admin_ctx.build());
        contract.mark_claim_disputed(claim_id);

        admin_ctx.block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS + DISPUTE_RESOLUTION_DELAY_BLOCKS);
        testing_env!(admin_ctx.build());

        // Admin proposes settlement with evidence hash
        contract.admin_propose_disputed_settlement(claim_id, TEST_EVIDENCE_SHA256.to_string());

        // Claim and liability are STILL intact!
        let claim = contract
            .get_claim(claim_id)
            .expect("Disputed claim must not be deleted by admin");
        assert_eq!(claim.status, ClaimStatus::Disputed);
        assert!(claim.admin_settlement_proposed);
        assert_eq!(
            claim.settlement_evidence_sha256.as_deref(),
            Some(TEST_EVIDENCE_SHA256)
        );
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit.as_yoctonear()
        );
        assert_eq!(
            contract.get_accounting_snapshot().disputed_liability.0,
            deposit.as_yoctonear()
        );

        // Duplicate proposal panics
        let dup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.admin_reconcile_stuck_claim(claim_id, ReconcileOutcome::ConfirmedSuccess);
        }));
        assert!(dup.is_err());

        // Admin (alice) cannot acknowledge on Bob's behalf
        let admin_ack = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.acknowledge_disputed_transfer_received(claim_id);
        }));
        assert!(admin_ack.is_err());

        // Claimant (Bob) acknowledges -> claim closed and liability zeroed
        let mut bob_ctx = VMContextBuilder::new();
        bob_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(bob())
            .account_balance(NearToken::from_near(100));
        testing_env!(bob_ctx.build());
        contract.acknowledge_disputed_transfer_received(claim_id);
        assert!(contract.get_claim(claim_id).is_none());
        assert_eq!(contract.get_total_refund_liability().0, 0);
    }

    // 78 (F-01): Admin cannot unilaterally delete an UnreconciledFailure without requester acknowledgement
    #[test]
    fn test_m03374_f01_admin_cannot_unilaterally_delete_unreconciled_failure() {
        let mut contract = setup_contract();
        contract.add_requester(bob());
        let deposit = NearToken::from_millinear(50);

        let mut bob_ctx = VMContextBuilder::new();
        bob_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(bob())
            .account_balance(NearToken::from_near(100))
            .attached_deposit(deposit)
            .block_height(20);
        testing_env!(bob_ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .account_balance(NearToken::from_near(100))
            .block_height(22);
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());

        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100))
            .block_height(25);
        testing_env!(admin_ctx.build());
        contract.admin_resolve_unreconciled_external(1, TEST_EVIDENCE_SHA256.to_string());

        // Record is STILL present and open slot is still reserved
        let rec = contract
            .get_unreconciled_failure(1)
            .expect("UnreconciledFailure must not be deleted by admin alone");
        assert!(rec.external_resolution_proposed);
        assert_eq!(
            rec.resolution_evidence_sha256.as_deref(),
            Some(TEST_EVIDENCE_SHA256)
        );
        assert_eq!(rec.resolution_proposed_at_block, Some(25));

        // Admin (alice) cannot acknowledge on Bob's behalf
        let wrong_caller = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.acknowledge_unreconciled_resolved_external(1);
        }));
        assert!(wrong_caller.is_err());

        // Bob acknowledges -> record deleted
        bob_ctx.attached_deposit(NearToken::from_yoctonear(0));
        testing_env!(bob_ctx.build());
        contract.acknowledge_unreconciled_resolved_external(1);
        assert!(contract.get_unreconciled_failure(1).is_none());
    }

    // 79 (F-01 / Section 4): Two-step admin transfer (propose_admin + accept_admin + cancel_admin_proposal)
    #[test]
    fn test_m03374_f01_two_step_admin_transfer_and_set_admin_disabled() {
        let mut contract = setup_contract();

        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100));
        testing_env!(admin_ctx.build());

        // Single-step set_admin is disabled even for valid testnet accounts
        let single_step = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.set_admin(bob());
        }));
        assert!(single_step.is_err());

        // Propose and cancel
        contract.propose_admin(bob());
        assert_eq!(contract.get_pending_admin(), Some(bob()));
        contract.cancel_admin_proposal();
        assert_eq!(contract.get_pending_admin(), None);

        // Propose Bob again
        contract.propose_admin(bob());
        assert_eq!(contract.get_admin(), alice());

        // Alice cannot accept on Bob's behalf
        let wrong_accept = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.accept_admin();
        }));
        assert!(wrong_accept.is_err());

        // Bob accepts -> becomes admin and authorized requester
        let mut bob_ctx = VMContextBuilder::new();
        bob_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(bob())
            .account_balance(NearToken::from_near(100));
        testing_env!(bob_ctx.build());
        contract.accept_admin();
        assert_eq!(contract.get_admin(), bob());
        assert_eq!(contract.get_pending_admin(), None);
        assert!(contract.is_requester(bob()));
    }

    // 80 (F-03 / Section 5): Disputed historical claim never blocks claiming a new independent Available refund
    #[test]
    fn test_m03374_f03_disputed_claim_does_not_block_new_available_refund() {
        let mut contract = setup_contract();
        let deposit1 = NearToken::from_millinear(50);
        let deposit2 = NearToken::from_millinear(30);

        // 1. Alice gets a 50 mNEAR stuck InFlight claim #1 and marks it Disputed
        let claim1 = setup_in_flight_claim(&mut contract, alice(), 1, deposit1, 10);
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(ctx.build());
        contract.mark_claim_disputed(claim1);

        let credit_after_dispute = contract.get_pending_refund(alice()).unwrap();
        assert_eq!(credit_after_dispute.disputed_amount, deposit1);
        assert_eq!(credit_after_dispute.available_amount.as_yoctonear(), 0);
        assert_eq!(credit_after_dispute.active_claim_id, None);

        // 2. Alice submits a second request (nonce = 2) with 30 mNEAR, which fails and is promoted
        ctx.attached_deposit(deposit2).block_height(100);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .account_balance(NearToken::from_near(100))
            .block_height(102);
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());
        designate_and_promote(&mut contract, 2, deposit2);

        let credit_with_new = contract.get_pending_refund(alice()).unwrap();
        assert_eq!(credit_with_new.available_amount, deposit2);
        assert_eq!(credit_with_new.disputed_amount, deposit1);
        assert_eq!(credit_with_new.status, RefundStatus::Available);

        // 3. Alice calls claim_refund() -> MUST succeed and claim ONLY the 30 mNEAR available_amount!
        ctx.attached_deposit(NearToken::from_yoctonear(0))
            .block_height(105);
        testing_env!(ctx.build());
        let _ = contract.claim_refund();

        let credit_in_flight = contract.get_pending_refund(alice()).unwrap();
        let claim2 = credit_in_flight.active_claim_id.expect("new claim_id 2");
        assert_ne!(claim1, claim2);
        assert_eq!(credit_in_flight.available_amount.as_yoctonear(), 0);
        assert_eq!(credit_in_flight.in_flight_amount, deposit2);
        assert_eq!(credit_in_flight.disputed_amount, deposit1);
        assert_eq!(contract.get_claim(claim2).unwrap().amount, deposit2);
        assert_eq!(
            contract.get_claim(claim1).unwrap().status,
            ClaimStatus::Disputed
        );

        // 4. Claim #2 (30 mNEAR) settles via on_refund_transfer(Ok) -> Disputed claim #1 (50 mNEAR) remains intact
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);
        assert!(contract.on_refund_transfer(alice(), deposit2, claim2));
        assert!(contract.get_claim(claim2).is_none());
        let credit_after_claim2 = contract.get_pending_refund(alice()).unwrap();
        assert_eq!(credit_after_claim2.in_flight_amount.as_yoctonear(), 0);
        assert_eq!(credit_after_claim2.disputed_amount, deposit1);
        assert_eq!(credit_after_claim2.status, RefundStatus::Disputed);
        assert_eq!(
            contract.get_total_refund_liability().0,
            deposit1.as_yoctonear()
        );

        // 5. Later, a late callback for Disputed claim #1 (50 mNEAR) arrives and settles cleanly
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);
        assert!(contract.on_refund_transfer(alice(), deposit1, claim1));
        assert!(contract.get_claim(claim1).is_none());
        assert!(contract.get_pending_refund(alice()).is_none());
        assert_eq!(contract.get_total_refund_liability().0, 0);
    }

    // 81 (F-04 / Section 6): flush_storage updates storage_usage before solvency check under tight balance
    #[test]
    fn test_m03374_f04_flush_storage_prevents_stale_storage_usage_under_tight_balance() {
        let mut contract = setup_contract();
        let deposit = NearToken::from_millinear(50);

        // Create an UnreconciledFailure for nonce 1
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100))
            .attached_deposit(deposit)
            .block_height(10);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .account_balance(NearToken::from_near(100))
            .block_height(12);
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());

        // Fund 50 mNEAR into refund_reserve_balance
        ctx.attached_deposit(deposit);
        testing_env!(ctx.build());
        contract.fund_refund_reserve();

        // Measure current post-flush storage_usage
        let pre_promote_storage_bytes = near_sdk::env::storage_usage();
        let pre_promote_locked = u128::from(pre_promote_storage_bytes)
            * near_sdk::env::storage_byte_cost().as_yoctonear();
        let exact_required_before_new_credit =
            pre_promote_locked + TEST_MIN_OPERATING_RESERVE.as_yoctonear() + deposit.as_yoctonear();

        // Set account_balance to exact_required_before_new_credit (0 yoctoNEAR of storage headroom for new RefundCredit).
        // Without flush_storage(), promote_unreconciled_refund would insert a new RefundCredit into pending_refunds
        // without env::storage_usage() reflecting the trie write prior to assert_solvency_invariant().
        // With flush_storage() + assert_solvency_invariant(), it MUST fail closed!
        let mut tight_ctx = VMContextBuilder::new();
        tight_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_yoctonear(exact_required_before_new_credit))
            .storage_usage(pre_promote_storage_bytes);
        testing_env!(tight_ctx.build());

        let tight_promote = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.promote_unreconciled_refund(1);
        }));
        assert!(
            tight_promote.is_err(),
            "Tight-balance promotion without storage margin must fail closed"
        );
    }

    // 82 (F-07 / Section 7): fund_operating_reserve increases minimum_operating_reserve and prevents reclassification
    #[test]
    fn test_m03374_f07_fund_operating_reserve_segregated_from_refund_reserve_designation() {
        let mut contract = setup_contract();
        let top_up = NearToken::from_millinear(25);

        // Start with exact balance = minimum_operating_reserve (10 mNEAR) + top_up (25 mNEAR) = 35 mNEAR
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_millinear(35))
            .attached_deposit(top_up)
            .storage_usage(0);
        testing_env!(ctx.build());

        contract.fund_operating_reserve();
        assert_eq!(
            contract.get_minimum_operating_reserve().0,
            (TEST_MIN_OPERATING_RESERVE.as_yoctonear() + top_up.as_yoctonear())
        );

        // Attempting to reclassify the 25 mNEAR operating reserve top-up via admin_designate_refund_reserve MUST panic
        ctx.attached_deposit(NearToken::from_yoctonear(0));
        testing_env!(ctx.build());
        let reclassify = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.admin_designate_refund_reserve(top_up, TEST_EVIDENCE_SHA256.to_string());
        }));
        assert!(
            reclassify.is_err(),
            "Operating reserve top-up must not be reclassifiable into refund_reserve_balance"
        );
    }

    // 83 (Section 8): Open request slots retained across full refund lifecycle & hard caps enforced
    #[test]
    fn test_m03374_section8_open_slots_retained_across_refund_lifecycle_and_hard_caps() {
        let mut contract = setup_contract();
        contract.set_max_open_requests(1);
        let deposit = NearToken::from_millinear(50);

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100))
            .attached_deposit(deposit)
            .block_height(10);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
        assert_eq!(contract.get_accounting_snapshot().open_request_slots, 1);

        // Fails into UnreconciledFailure -> slot still 1
        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .account_balance(NearToken::from_near(100))
            .block_height(12);
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert_eq!(contract.get_accounting_snapshot().open_request_slots, 1);

        // Promoted to Available RefundCredit -> slot STILL 1 (cannot bypass max_open_requests = 1)
        designate_and_promote(&mut contract, 1, deposit);
        assert_eq!(contract.get_accounting_snapshot().open_request_slots, 1);

        ctx.attached_deposit(deposit).block_height(15);
        testing_env!(ctx.build());
        let blocked_while_available =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
            }));
        assert!(
            blocked_while_available.is_err(),
            "Unsettled RefundCredit must occupy an open request slot"
        );

        // Claimed -> InFlight -> slot STILL 1
        ctx.attached_deposit(NearToken::from_yoctonear(0));
        testing_env!(ctx.build());
        let _ = contract.claim_refund();
        assert_eq!(contract.get_accounting_snapshot().open_request_slots, 1);

        // Settled via on_refund_transfer(Ok) -> slot drops to 0
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Successful(vec![])]);
        assert!(contract.on_refund_transfer(alice(), deposit, 1));
        assert_eq!(contract.get_accounting_snapshot().open_request_slots, 0);

        // Hard cap checks
        testing_env!(ctx.build());
        let over_open_cap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.set_max_open_requests(HARD_MAX_OPEN_REQUESTS + 1);
        }));
        assert!(over_open_cap.is_err());
        assert!(HARD_MAX_MARKETS >= 32);
        assert!(HARD_MAX_AUTHORIZED_REQUESTERS >= 100);
    }

    // 84 (Section 8 / Section 12): Proof that serialized M03C.3.7.2 state cannot deserialize under M03C.3.7.4 layout
    #[test]
    fn test_m03374_legacy_m03372_borsh_state_cannot_deserialize_as_current() {
        #[near_sdk::near(serializers = [borsh])]
        struct LegacyM03372Adapter {
            admin: AccountId,
            outlayer_contract: AccountId,
            authorized_requesters: near_sdk::store::LookupSet<AccountId>,
            markets: near_sdk::store::IterableMap<String, crate::types::MarketConfig>,
            prices: near_sdk::store::IterableMap<String, crate::types::PriceObservation>,
            pending_requests: near_sdk::store::LookupMap<u64, crate::types::PendingRequest>,
            unreconciled_failures:
                near_sdk::store::LookupMap<u64, crate::types::UnreconciledFailure>,
            next_nonce: u64,
            pending_refunds: near_sdk::store::LookupMap<AccountId, crate::types::RefundCredit>,
            total_refund_liability: u128,
            total_in_flight_liability: u128,
            total_disputed_liability: u128,
            refund_reserve_balance: u128,
            insurance_reserve_balance: u128,
            insurance_return_pending_balance: u128,
            insurance_enabled: bool,
            minimum_operating_reserve: u128,
            next_claim_id: u64,
            claims: near_sdk::store::LookupMap<u64, crate::types::RefundClaim>,
            next_compensation_id: u64,
            insurance_compensations:
                near_sdk::store::LookupMap<u64, crate::types::InsuranceCompensation>,
            pending_request_count: u32,
            unreconciled_failure_count: u32,
            max_open_requests: u32,
        }

        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice());
        testing_env!(ctx.build());

        let legacy = LegacyM03372Adapter {
            admin: alice(),
            outlayer_contract: outlayer_contract(),
            authorized_requesters: near_sdk::store::LookupSet::new(b"r"),
            markets: near_sdk::store::IterableMap::new(b"m"),
            prices: near_sdk::store::IterableMap::new(b"p"),
            pending_requests: near_sdk::store::LookupMap::new(b"q"),
            unreconciled_failures: near_sdk::store::LookupMap::new(b"u"),
            next_nonce: 1,
            pending_refunds: near_sdk::store::LookupMap::new(b"f"),
            total_refund_liability: 0,
            total_in_flight_liability: 0,
            total_disputed_liability: 0,
            refund_reserve_balance: 0,
            insurance_reserve_balance: 0,
            insurance_return_pending_balance: 0,
            insurance_enabled: false,
            minimum_operating_reserve: 10_000_000_000_000_000_000_000,
            next_claim_id: 1,
            claims: near_sdk::store::LookupMap::new(b"c"),
            next_compensation_id: 1,
            insurance_compensations: near_sdk::store::LookupMap::new(b"i"),
            pending_request_count: 0,
            unreconciled_failure_count: 0,
            max_open_requests: 50,
        };

        let legacy_bytes = near_sdk::borsh::to_vec(&legacy).unwrap();
        let decode_attempt = near_sdk::borsh::from_slice::<RoastakeOracleAdapter>(&legacy_bytes);
        assert!(
            decode_attempt.is_err(),
            "Legacy M03C.3.7.2 Borsh state must fail deserialization under M03C.3.7.4 layout"
        );
    }

    // 85 (L-02 / Section 3): External resolution proposal blocks promote_unreconciled_refund until rejected or cancelled
    #[test]
    fn test_m03376_l02_external_resolution_blocks_promotion_until_rejected_or_cancelled() {
        let mut contract = setup_contract();
        contract.add_requester(bob());
        let deposit = NearToken::from_millinear(50);

        // Create two UnreconciledFailures (nonce 1 and nonce 2) for Bob
        let mut bob_ctx = VMContextBuilder::new();
        bob_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(bob())
            .attached_deposit(deposit)
            .account_balance(NearToken::from_near(100))
            .block_height(10);
        testing_env!(bob_ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .account_balance(NearToken::from_near(100))
            .block_height(12);
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        let _ = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());

        // Fund reserve for both nonces
        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(100))
            .account_balance(NearToken::from_near(100))
            .block_height(20);
        testing_env!(admin_ctx.build());
        contract.fund_refund_reserve();

        // Admin proposes external resolution for nonce 1
        admin_ctx.attached_deposit(NearToken::from_yoctonear(0));
        testing_env!(admin_ctx.build());
        contract.admin_resolve_unreconciled_external(1, TEST_EVIDENCE_SHA256.to_string());

        // Promoting nonce 1 while external_resolution_proposed == true MUST fail closed
        testing_env!(bob_ctx.build());
        let blocked_promote = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.promote_unreconciled_refund(1);
        }));
        assert!(
            blocked_promote.is_err(),
            "promote_unreconciled_refund must reject active external resolution proposal"
        );

        // Non-requester (alice) cannot reject Bob's proposal
        testing_env!(admin_ctx.build());
        let bad_reject = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.reject_unreconciled_external_resolution(1);
        }));
        assert!(bad_reject.is_err());

        // Non-admin (bob) cannot call admin_cancel_unreconciled_external_resolution
        testing_env!(bob_ctx.build());
        let bad_cancel = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.admin_cancel_unreconciled_external_resolution(1);
        }));
        assert!(bad_cancel.is_err());

        // Requester Bob rejects proposal -> cleared safely without deleting failure or creating refund
        contract.reject_unreconciled_external_resolution(1);
        let cleared = contract.get_unreconciled_failure(1).unwrap();
        assert!(!cleared.external_resolution_proposed);
        assert_eq!(cleared.resolution_evidence_sha256, None);
        assert_eq!(cleared.resolution_proposed_at_block, None);
        assert!(contract.get_pending_refund(bob()).is_none());

        // Duplicate reject fails closed
        let dup_reject = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.reject_unreconciled_external_resolution(1);
        }));
        assert!(dup_reject.is_err());

        // Now backed promotion for nonce 1 succeeds
        contract.promote_unreconciled_refund(1);
        assert_eq!(
            contract.get_pending_refund(bob()).unwrap().available_amount,
            deposit
        );

        // Nonce 2: admin proposes external resolution, then admin cancels -> preserves record and allows promotion
        testing_env!(admin_ctx.build());
        contract.admin_resolve_unreconciled_external(2, TEST_EVIDENCE_SHA256.to_string());
        contract.admin_cancel_unreconciled_external_resolution(2);
        let cleared2 = contract.get_unreconciled_failure(2).unwrap();
        assert!(!cleared2.external_resolution_proposed);
        contract.promote_unreconciled_refund(2);
        assert_eq!(
            contract.get_pending_refund(bob()).unwrap().available_amount,
            NearToken::from_millinear(100)
        );
    }

    // 86 (L-03 Part A): UTF-8 byte length bounds on WorkerOutput.source / data_class & worst-case PriceObservation footprint
    #[test]
    fn test_m03376_l03_worker_source_and_data_class_utf8_byte_bounds() {
        // Check worst-case serialized Borsh footprint of PriceObservation inside IterableMap
        let worst_obs = PriceObservation {
            market: "M".repeat(crate::types::MAX_MARKET_SYMBOL_BYTES),
            price: near_sdk::json_types::U128(12_345_000_000),
            decimals: DECIMALS_COFFEE_INDEX,
            unit: "U".repeat(crate::types::MAX_MARKET_UNIT_BYTES),
            source: "S".repeat(MAX_WORKER_SOURCE_BYTES),
            source_count: 10,
            observed_at_sec: 1_700_000_000,
            stored_at_sec: 1_700_000_010,
            nonce: 99,
            data_class: "D".repeat(MAX_WORKER_DATA_CLASS_BYTES),
        };
        let obs_borsh = near_sdk::borsh::to_vec(&worst_obs).unwrap().len() as u64;
        // IterableMap storage per new entry:
        //   keys Vector entry: 40 (trie) + 6 (key) + 4 (str len) + 64 (symbol) = 114 B
        //   values LookupMap entry: 40 (trie) + 34 (key "p"+sha256) + obs_borsh + 4 (u32 key_index)
        let worst_iterable_map_bytes = 114 + 40 + 34 + obs_borsh + 4;
        assert!(
            worst_iterable_map_bytes <= OPEN_REQUEST_STORAGE_BUDGET_BYTES,
            "worst-case IterableMap PriceObservation ({} bytes) must fit in OPEN_REQUEST_STORAGE_BUDGET_BYTES ({})",
            worst_iterable_map_bytes,
            OPEN_REQUEST_STORAGE_BUDGET_BYTES
        );

        let mut contract = setup_contract();
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .attached_deposit(NearToken::from_millinear(50))
            .account_balance(NearToken::from_near(100))
            .block_timestamp(1_700_000_000_000_000_000);

        // 1. Exact MAX_WORKER_SOURCE_BYTES (128 bytes) accepted
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
        let valid_max_source = WorkerOutput {
            schema_version: SCHEMA_VERSION,
            market: MARKET_GESHA_DEMO.to_string(),
            price: DEFAULT_FIXTURE_PRICE_RAW.to_string(),
            decimals: DECIMALS_COFFEE_INDEX,
            unit: UNIT_USD_PER_KG.to_string(),
            source: "a".repeat(MAX_WORKER_SOURCE_BYTES),
            source_count: 1,
            observed_at: 1_700_000_000,
            data_class: DATA_CLASS_SYNTHETIC.to_string(),
        };
        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .account_balance(NearToken::from_near(100))
            .block_timestamp(1_700_000_010_000_000_000);
        set_test_env_with_promises(
            cb_ctx.build(),
            vec![PromiseResult::Successful(
                serde_json::to_vec(&valid_max_source).unwrap(),
            )],
        );
        let stored = contract.on_outlayer_execution(1, MARKET_GESHA_DEMO.to_string());
        assert!(stored.is_some(), "source at exact 128 bytes must succeed");

        // 2. Just over MAX_WORKER_SOURCE_BYTES (129 ASCII bytes) rejected without corrupting state
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
        let over_ascii = WorkerOutput {
            source: "b".repeat(MAX_WORKER_SOURCE_BYTES + 1),
            ..valid_max_source.clone()
        };
        set_test_env_with_promises(
            cb_ctx.build(),
            vec![PromiseResult::Successful(
                serde_json::to_vec(&over_ascii).unwrap(),
            )],
        );
        assert!(
            contract
                .on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string())
                .is_none(),
            "129-byte source must be rejected"
        );
        assert!(contract.get_pending_request(2).is_none());
        assert_eq!(
            contract
                .get_price(MARKET_GESHA_DEMO.to_string())
                .unwrap()
                .nonce,
            1
        );

        // 3. Multi-byte UTF-8 trap: 33 chars ('🦀' = 4 UTF-8 bytes each = 132 bytes > 128 even though 33 chars < 128)
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
        let over_multibyte = WorkerOutput {
            source: "🦀".repeat(33),
            ..valid_max_source.clone()
        };
        assert_eq!(over_multibyte.source.chars().count(), 33);
        assert_eq!(over_multibyte.source.len(), 132);
        set_test_env_with_promises(
            cb_ctx.build(),
            vec![PromiseResult::Successful(
                serde_json::to_vec(&over_multibyte).unwrap(),
            )],
        );
        assert!(
            contract
                .on_outlayer_execution(3, MARKET_GESHA_DEMO.to_string())
                .is_none(),
            "multi-byte UTF-8 source > 128 bytes must be rejected even if char count <= 128"
        );
        assert!(contract.get_pending_request(3).is_none());
        assert_eq!(
            contract
                .get_price(MARKET_GESHA_DEMO.to_string())
                .unwrap()
                .nonce,
            1
        );

        // 4. Oversized data_class (> MAX_WORKER_DATA_CLASS_BYTES = 32 bytes) rejected cleanly
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());
        let over_data_class = WorkerOutput {
            data_class: "D".repeat(MAX_WORKER_DATA_CLASS_BYTES + 1),
            ..valid_max_source
        };
        set_test_env_with_promises(
            cb_ctx.build(),
            vec![PromiseResult::Successful(
                serde_json::to_vec(&over_data_class).unwrap(),
            )],
        );
        assert!(
            contract
                .on_outlayer_execution(4, MARKET_GESHA_DEMO.to_string())
                .is_none(),
            "data_class > 32 bytes must be rejected"
        );
        assert!(contract.get_pending_request(4).is_none());
        assert_eq!(
            contract
                .get_price(MARKET_GESHA_DEMO.to_string())
                .unwrap()
                .nonce,
            1
        );
    }

    // 87 (L-03 Part B): Dedicated STATE storage margin on init & post-return AccountId growth setters
    #[test]
    fn test_m03376_l03_tight_balance_state_growth_margin_on_init_and_admin_setters() {
        let storage_cost = near_sdk::env::storage_byte_cost().as_yoctonear();

        // 1. init() with spendable == minimum_operating_reserve + (STATE_STORAGE_MARGIN_BYTES - 1) MUST fail
        let mut init_tight = VMContextBuilder::new();
        init_tight
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .storage_usage(0)
            .account_balance(NearToken::from_yoctonear(
                TEST_MIN_OPERATING_RESERVE.as_yoctonear()
                    + u128::from(STATE_STORAGE_MARGIN_BYTES - 1) * storage_cost,
            ));
        testing_env!(init_tight.build());
        let under_state_margin = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = RoastakeOracleAdapter::init(
                alice(),
                Some(outlayer_contract()),
                Some("https://test.fastfs.io/roastake/worker.wasm".to_string()),
                Some(
                    "abac707a27b4633857a9c2703a2cae67e18d5369e7d41daed946ad8945ffe2a6".to_string(),
                ),
                TEST_MIN_OPERATING_RESERVE,
                None,
            );
        }));
        assert!(
            under_state_margin.is_err(),
            "init must reject contract balance lacking STATE_STORAGE_MARGIN_BYTES headroom"
        );

        // 2. Now test post-init STATE expansion under tight balance in propose_admin & set_outlayer_contract
        let mut contract = setup_contract();
        let storage_now = near_sdk::env::storage_usage();
        let locked_now = u128::from(storage_now) * storage_cost;
        let long_64_admin: AccountId =
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.testnet"
                .parse()
                .unwrap();
        assert_eq!(long_64_admin.as_str().len(), 64);

        // Leave only 50 bytes (< STATE_GROWTH_MARGIN_BYTES = 128 and < +68 byte growth of Some(64B AccountId))
        let insufficient_headroom_bal =
            locked_now + TEST_MIN_OPERATING_RESERVE.as_yoctonear() + 50 * storage_cost;
        let mut setter_ctx = VMContextBuilder::new();
        setter_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .storage_usage(storage_now)
            .account_balance(NearToken::from_yoctonear(insufficient_headroom_bal));
        testing_env!(setter_ctx.build());

        let tight_propose = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.propose_admin(long_64_admin.clone());
        }));
        assert!(
            tight_propose.is_err(),
            "propose_admin must fail closed when headroom < STATE_GROWTH_MARGIN_BYTES"
        );

        let tight_outlayer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract.set_outlayer_contract(long_64_admin.clone());
        }));
        assert!(
            tight_outlayer.is_err(),
            "set_outlayer_contract must fail closed when headroom < STATE_GROWTH_MARGIN_BYTES"
        );

        // With exact STATE_GROWTH_MARGIN_BYTES (128 bytes), both max-length 64B updates succeed
        // and leave actual post-serialization STATE growth strictly above minimum_operating_reserve
        let sufficient_headroom_bal = locked_now
            + TEST_MIN_OPERATING_RESERVE.as_yoctonear()
            + u128::from(STATE_GROWTH_MARGIN_BYTES) * storage_cost;
        setter_ctx.account_balance(NearToken::from_yoctonear(sufficient_headroom_bal));
        testing_env!(setter_ctx.build());

        let before_state_len = near_sdk::borsh::to_vec(&contract).unwrap().len() as u64;
        contract.propose_admin(long_64_admin.clone());
        contract.set_outlayer_contract(long_64_admin);
        let after_state_len = near_sdk::borsh::to_vec(&contract).unwrap().len() as u64;
        let actual_delta = after_state_len - before_state_len;
        assert!(
            actual_delta <= STATE_GROWTH_MARGIN_BYTES,
            "actual STATE growth ({} bytes) must stay within STATE_GROWTH_MARGIN_BYTES ({} bytes)",
            actual_delta,
            STATE_GROWTH_MARGIN_BYTES
        );
    }

    // 88 (M-01 / Section 1): Preserve fail-closed floor while verifying fund_refund_reserve mitigation unblocks unrelated requester
    #[test]
    fn test_m03376_m01_disputed_claim_preserves_fail_closed_floor_while_funded_reserve_unblocks_unrelated_requester(
    ) {
        let mut contract = setup_contract();
        contract.add_requester(bob());
        let deposit = NearToken::from_millinear(50);

        // 1. Alice's claim #1 becomes Disputed (total_disputed_liability = 50 mNEAR)
        let claim1 = setup_in_flight_claim(&mut contract, alice(), 1, deposit, 10);
        let mut ctx = VMContextBuilder::new();
        ctx.current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(ctx.build());
        contract.mark_claim_disputed(claim1);

        // 2. Unrelated requester Bob suffers a failed request (nonce 2)
        ctx.predecessor_account_id(bob())
            .attached_deposit(deposit)
            .block_height(20);
        testing_env!(ctx.build());
        let _ = contract.request_price_update(MARKET_GESHA_DEMO.to_string());

        let mut cb_ctx = VMContextBuilder::new();
        cb_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(oracle_contract())
            .account_balance(NearToken::from_near(100))
            .block_height(22);
        set_test_env_with_promises(cb_ctx.build(), vec![PromiseResult::Failed]);
        let _ = contract.on_outlayer_execution(2, MARKET_GESHA_DEMO.to_string());

        // 3. Verify M-01 fail-closed invariant on an independent contract instance where spendable ==
        // minimum_operating_reserve + 50 mNEAR (ambiguous between Alice's Scenario 2 return vs Bob's InitialPanic).
        let mut contract_blocked = setup_contract();
        let blocked_claim = setup_in_flight_claim(&mut contract_blocked, alice(), 1, deposit, 10);
        let mut blocked_ctx = VMContextBuilder::new();
        blocked_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .account_balance(NearToken::from_near(100))
            .block_height(10 + STUCK_CLAIM_MIN_AGE_BLOCKS);
        testing_env!(blocked_ctx.build());
        contract_blocked.mark_claim_disputed(blocked_claim);

        let blocked_storage = near_sdk::env::storage_usage();
        let blocked_locked =
            u128::from(blocked_storage) * near_sdk::env::storage_byte_cost().as_yoctonear();
        blocked_ctx
            .storage_usage(blocked_storage)
            .account_balance(NearToken::from_yoctonear(
                blocked_locked + TEST_MIN_OPERATING_RESERVE.as_yoctonear() + deposit.as_yoctonear(),
            ));
        testing_env!(blocked_ctx.build());
        let blocked_designate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            contract_blocked
                .admin_designate_refund_reserve(deposit, TEST_EVIDENCE_SHA256.to_string());
        }));
        assert!(
            blocked_designate.is_err(),
            "M-01 fail-closed invariant: admin_designate_refund_reserve must remain blocked while disputed liability encumbers unallocated surplus"
        );

        // 4. Concrete operational mitigation on `contract`: fund_refund_reserve() adds fresh backed 50 mNEAR
        // directly to refund_reserve_balance and promotes/pays Bob without touching Alice's Disputed claim #1.
        let cur_storage = near_sdk::env::storage_usage();
        let cur_locked =
            u128::from(cur_storage) * near_sdk::env::storage_byte_cost().as_yoctonear();
        let funded_total =
            cur_locked + TEST_MIN_OPERATING_RESERVE.as_yoctonear() + 2 * deposit.as_yoctonear();

        let mut admin_ctx = VMContextBuilder::new();
        admin_ctx
            .current_account_id(oracle_contract())
            .predecessor_account_id(alice())
            .storage_usage(cur_storage)
            .account_balance(NearToken::from_yoctonear(funded_total))
            .attached_deposit(deposit);
        testing_env!(admin_ctx.build());
        contract.fund_refund_reserve();

        admin_ctx
            .storage_usage(near_sdk::env::storage_usage())
            .attached_deposit(NearToken::from_yoctonear(0));
        testing_env!(admin_ctx.build());
        contract.promote_unreconciled_refund(2);

        let bob_claim_ctx = {
            let post_promote_storage = near_sdk::env::storage_usage();
            let post_promote_locked = u128::from(post_promote_storage)
                * near_sdk::env::storage_byte_cost().as_yoctonear();
            let mut c = VMContextBuilder::new();
            c.current_account_id(oracle_contract())
                .predecessor_account_id(bob())
                .storage_usage(post_promote_storage)
                .account_balance(NearToken::from_yoctonear(
                    post_promote_locked
                        + TEST_MIN_OPERATING_RESERVE.as_yoctonear()
                        + 2 * deposit.as_yoctonear(),
                ));
            c
        };
        testing_env!(bob_claim_ctx.build());
        let _ = contract.claim_refund();

        // Alice's Disputed claim #1 remains completely intact while Bob's claim #2 proceeds independently
        assert_eq!(
            contract.get_claim(claim1).unwrap().status,
            ClaimStatus::Disputed
        );
        let bob_credit = contract.get_pending_refund(bob()).unwrap();
        assert_eq!(bob_credit.in_flight_amount, deposit);
        assert_eq!(bob_credit.status, RefundStatus::InFlight);
    }
}
