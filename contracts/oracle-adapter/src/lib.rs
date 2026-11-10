//! ROASTAKE Oracle Adapter Smart Contract (NEAR Testnet).
//!
//! Requests verifiable off-chain execution through OutLayer and receives/validates
//! coffee index observations for commodity markets.

pub mod outlayer;
pub mod types;

#[cfg(test)]
mod tests;

use near_sdk::json_types::U128;
use near_sdk::store::{IterableMap, LookupMap, LookupSet};
use near_sdk::{
    env, near, require, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError,
};
use outlayer::{calculate_execution_cost, ext_outlayer};
use roastake_shared::{
    calculate_divergence_bps, extract_worker_output, validate_testnet_account_id, ExecutionSource,
    ResourceLimits, WorkerInput, WorkerOutput, DATA_CLASS_SYNTHETIC, DEFAULT_MAX_CLOCK_DRIFT_SEC,
    DEFAULT_OUTLAYER_TESTNET, MARKET_GESHA_DEMO, SCHEMA_VERSION,
};
use types::{
    AccountingSnapshot, ClaimStatus, InsuranceCompensation, InsuranceCompensationStatus,
    MarketConfig, PendingRequest, PriceObservation, ReconcileOutcome, RefundClaim, RefundCredit,
    RefundStatus, UnreconciledFailure, MAX_WORKER_DATA_CLASS_BYTES, MAX_WORKER_SOURCE_BYTES,
};

/// Minimum age (in blocks) of an `InFlight` claim before it may be marked `Disputed`.
///
/// This is NOT evidence of anything. It only prevents disputes from being opened while the
/// confirmation callback is still expected to execute in the normal case. A late callback still
/// resolves a `Disputed` claim objectively.
pub const STUCK_CLAIM_MIN_AGE_BLOCKS: u64 = 100;

/// Minimum number of blocks a claim must remain `Disputed` before a governed paid-attestation
/// or an insurance compensation may be executed (acts as a governance delay / appeal window).
pub const DISPUTE_RESOLUTION_DELAY_BLOCKS: u64 = 1_000;

/// Default cap on simultaneously open requests (`pending_requests` + `unreconciled_failures` + unresolved refund credits/claims).
pub const DEFAULT_MAX_OPEN_REQUESTS: u32 = 100;

/// Hard protocol ceiling on `max_open_requests` that even the administrator cannot exceed.
pub const HARD_MAX_OPEN_REQUESTS: u32 = 500;

/// Default cap on simultaneously active insurance compensation records.
pub const DEFAULT_MAX_ACTIVE_INSURANCE_COMPENSATIONS: u32 = 32;

/// Hard ceiling on simultaneously active insurance compensation records.
pub const HARD_MAX_ACTIVE_INSURANCE_COMPENSATIONS: u32 = 100;

/// Hard ceiling on registered markets in `self.markets` (and corresponding `self.prices` entries).
pub const HARD_MAX_MARKETS: u32 = 32;

/// Hard ceiling on authorized requester accounts in `self.authorized_requesters`.
pub const HARD_MAX_AUTHORIZED_REQUESTERS: u32 = 100;

/// Conservative storage budget reserved per open request or new persistent record.
/// Measured Sandbox footprints are documented in the integration-test README (~195 bytes/entry for
/// `UnreconciledFailure`, ~251 bytes/entry for `RefundCredit`, and <= 493 bytes worst-case for a
/// new `PriceObservation` with `MAX_WORKER_SOURCE_BYTES = 128`); this 512-byte budget covers every
/// per-request state transition.
pub const OPEN_REQUEST_STORAGE_BUDGET_BYTES: u64 = 512;

/// Conservative storage budget required before inserting new claim, insurance, market, or requester records.
pub const RECORD_STORAGE_MARGIN_BYTES: u64 = 512;

/// Dedicated storage margin (512 bytes) covering the maximum serialized NEAR trie footprint of the
/// root contract `STATE` record (`45` bytes trie key overhead + `258` bytes fixed fields + up to
/// `205` bytes for three maximum-length 64-byte `AccountId`s = `508` bytes worst-case, rounded to
/// `512` bytes). Enforced during `init()` before the initial `STATE` key is persisted on return.
pub const STATE_STORAGE_MARGIN_BYTES: u64 = 512;

/// Dedicated incremental `STATE` growth margin (128 bytes) enforced when post-init admin setters
/// mutate variable-length fields inside `STATE` (`pending_admin`: `None -> Some(64-byte AccountId)`
/// = `+68` bytes max; `outlayer_contract`: `9 -> 64 bytes` = `+55` bytes max; combined <= `123` bytes,
/// rounded to `128` bytes). Ensures post-method `STATE` re-serialization on return can never
/// encroach on operating, refund, or insurance reserves.
pub const STATE_GROWTH_MARGIN_BYTES: u64 = 128;

/// Default gas allocated for calling OutLayer's `request_execution` (100 TGas).
pub const OUTLAYER_CALL_GAS: Gas = Gas::from_tgas(100);

/// Default gas allocated for the contract callback `on_outlayer_execution` (50 TGas).
pub const CALLBACK_GAS: Gas = Gas::from_tgas(50);

/// Gas allocated for the private refund transfer callback `on_refund_transfer` (20 TGas).
pub const REFUND_CALLBACK_GAS: Gas = Gas::from_tgas(20);

/// Maximum payload size accepted via OutLayer's direct yield/resume path (1,024 bytes).
///
/// OutLayer delivers responses through NEAR Protocol's yield/resume mechanism.
/// For the **direct path** (`ResponseFormat::Json` with small outputs), the resume payload is
/// limited to 1,024 bytes. Payloads larger than 1,024 bytes must use OutLayer's two-stage
/// output-storage mechanism (`submit_execution_output_and_resolve`), which is not currently
/// implemented in ROASTAKE Mission 01 (see `docs/OUTLAYER-TESTNET.md`).
///
/// The GESHA-DEMO worker output is approximately 300 bytes, well within this limit.
/// Any payload exceeding 1,024 bytes is rejected via [`PromiseError::TooLong`], logged,
/// and the previous valid price is preserved.
pub const MAX_OUTLAYER_RESPONSE_BYTES: usize = 1_024;

/// ROASTAKE Oracle Adapter contract state.
#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct RoastakeOracleAdapter {
    /// Authorized administrator account (recommended: >= 2-of-3 multisig contract)
    pub admin: AccountId,
    /// Pending administrator account awaiting explicit `accept_admin` confirmation
    pub pending_admin: Option<AccountId>,
    /// Configured OutLayer coordinator contract on Testnet
    pub outlayer_contract: AccountId,
    /// Set of accounts authorized to trigger price refresh requests
    pub authorized_requesters: LookupSet<AccountId>,
    /// Number of accounts currently stored in `authorized_requesters`
    pub authorized_requester_count: u32,
    /// Registered coffee market configurations
    pub markets: IterableMap<String, MarketConfig>,
    /// Number of registered markets in `markets`
    pub market_count: u32,
    /// Latest verified price observations by market
    pub prices: IterableMap<String, PriceObservation>,
    /// Pending in-flight execution requests indexed by nonce
    pub pending_requests: LookupMap<u64, PendingRequest>,
    /// Ambiguous failed executions awaiting explicit reserve promotion or requester-acknowledged external resolution
    pub unreconciled_failures: LookupMap<u64, UnreconciledFailure>,
    /// Monotonic request nonce counter
    pub next_nonce: u64,
    /// Pending backed refund credits indexed by requester account
    pub pending_refunds: LookupMap<AccountId, RefundCredit>,
    /// Number of active accounts in `pending_refunds`
    pub active_refund_credit_count: u32,
    /// Sum of all outstanding refund entitlements (available + in-flight + return-pending + disputed) in yoctoNEAR
    pub total_refund_liability: u128,
    /// Portion of `total_refund_liability` whose transfer has been dispatched and not yet observed
    pub total_in_flight_liability: u128,
    /// Portion of `total_refund_liability` whose transfer outcome is unobservable (non-payable)
    pub total_disputed_liability: u128,
    /// Explicitly funded or designated reserve available to back unresolved refund promotions (in yoctoNEAR)
    pub refund_reserve_balance: u128,
    /// Protected minimum operating reserve in yoctoNEAR (strictly segregated; increases with `fund_operating_reserve`)
    pub minimum_operating_reserve: u128,
    /// Monotonically increasing refund payout claim identifier counter
    pub next_claim_id: u64,
    /// Active refund payout claims indexed by unique claim ID (terminal claims are removed)
    pub claims: LookupMap<u64, RefundClaim>,
    /// Number of active records in `claims`
    pub active_claim_count: u32,
    /// Whether discretionary insurance compensation of `Disputed` claims is enabled (default: false)
    pub insurance_enabled: bool,
    /// Explicitly funded insurance reserve, separate from `refund_reserve_balance` (in yoctoNEAR)
    pub insurance_reserve_balance: u128,
    /// Sum of scheduled insurance compensation transfers currently `InFlight` (in yoctoNEAR)
    pub total_in_flight_insurance_liability: u128,
    /// Sum of failed insurance compensation transfers awaiting system-refund backing confirmation (in yoctoNEAR)
    pub insurance_return_pending_balance: u128,
    /// Sum of ambiguous insurance compensation transfers marked `Disputed` after callback timeout (in yoctoNEAR)
    pub total_unresolved_insurance_liability: u128,
    /// Monotonic identifier for insurance compensations (terminal records removed on settlement/return)
    pub next_compensation_id: u64,
    /// Active insurance compensation records indexed by compensation ID (removed on terminal settlement)
    pub insurance_compensations: LookupMap<u64, InsuranceCompensation>,
    /// Number of active records in `insurance_compensations`
    pub active_insurance_compensation_count: u32,
    /// Maximum simultaneously active records in `insurance_compensations`
    pub max_active_insurance_compensations: u32,
    /// Maximum simultaneously open request slots
    pub max_open_requests: u32,
    /// Number of `pending_requests` entries
    pub pending_request_count: u32,
    /// Number of `unreconciled_failures` entries
    pub unreconciled_failure_count: u32,
}

#[near]
impl RoastakeOracleAdapter {
    /// Initialize the Roastake Oracle Adapter contract on NEAR Testnet.
    ///
    /// Requires an explicit positive `minimum_operating_reserve` to protect operating liquidity
    /// and storage/gas margins. Never silently defaults the reserve to zero.
    #[init]
    pub fn init(
        admin: AccountId,
        outlayer_contract: Option<AccountId>,
        initial_wasm_url: Option<String>,
        initial_wasm_sha256: Option<String>,
        minimum_operating_reserve: NearToken,
        max_open_requests: Option<u32>,
    ) -> Self {
        require!(!env::state_exists(), "Contract is already initialized");

        let min_reserve_yocto = minimum_operating_reserve.as_yoctonear();
        require!(
            min_reserve_yocto > 0,
            "minimum_operating_reserve must be strictly greater than zero"
        );
        let max_open = max_open_requests.unwrap_or(DEFAULT_MAX_OPEN_REQUESTS);
        require!(
            max_open > 0 && max_open <= HARD_MAX_OPEN_REQUESTS,
            format!(
                "ERR_INVALID_MAX_OPEN_REQUESTS: max_open_requests ({}) must be in 1..={}",
                max_open, HARD_MAX_OPEN_REQUESTS
            )
        );

        // Enforce Testnet policy on all configured accounts
        validate_testnet_account_id(admin.as_str())
            .unwrap_or_else(|e| env::panic_str(&format!("admin validation failed: {e}")));

        let outlayer = outlayer_contract.unwrap_or_else(|| {
            DEFAULT_OUTLAYER_TESTNET
                .parse::<AccountId>()
                .expect("valid default outlayer account")
        });

        validate_testnet_account_id(outlayer.as_str()).unwrap_or_else(|e| {
            env::panic_str(&format!("outlayer contract validation failed: {e}"))
        });

        let mut requesters = LookupSet::new(b"r");
        requesters.insert(admin.clone());

        let mut markets = IterableMap::new(b"m");

        // Seed initial GESHA-DEMO/USD market configuration
        let wasm_url = initial_wasm_url.unwrap_or_else(|| {
            "https://test.fastfs.io/roastake/workers/roastake_coffee_price_index.wasm".to_string()
        });
        let wasm_sha256 = initial_wasm_sha256.unwrap_or_else(|| {
            // Placeholder 64-char hex, updated via admin or deployment pipeline
            "0000000000000000000000000000000000000000000000000000000000000000".to_string()
        });

        let default_config = MarketConfig::gesha_demo_default(wasm_url, wasm_sha256);
        default_config.validate().unwrap_or_else(|e| {
            env::panic_str(&format!(
                "initial market configuration validation failed: {e}"
            ))
        });
        markets.insert(MARKET_GESHA_DEMO.to_string(), default_config);

        let mut instance = Self {
            admin,
            pending_admin: None,
            outlayer_contract: outlayer,
            authorized_requesters: requesters,
            authorized_requester_count: 1,
            markets,
            market_count: 1,
            prices: IterableMap::new(b"p"),
            pending_requests: LookupMap::new(b"q"),
            unreconciled_failures: LookupMap::new(b"u"),
            next_nonce: 1,
            pending_refunds: LookupMap::new(b"f"),
            active_refund_credit_count: 0,
            total_refund_liability: 0,
            total_in_flight_liability: 0,
            total_disputed_liability: 0,
            refund_reserve_balance: 0,
            minimum_operating_reserve: min_reserve_yocto,
            next_claim_id: 1,
            claims: LookupMap::new(b"c"),
            active_claim_count: 0,
            insurance_enabled: false,
            insurance_reserve_balance: 0,
            total_in_flight_insurance_liability: 0,
            insurance_return_pending_balance: 0,
            total_unresolved_insurance_liability: 0,
            next_compensation_id: 1,
            insurance_compensations: LookupMap::new(b"i"),
            active_insurance_compensation_count: 0,
            max_active_insurance_compensations: DEFAULT_MAX_ACTIVE_INSURANCE_COMPENSATIONS,
            max_open_requests: max_open,
            pending_request_count: 0,
            unreconciled_failure_count: 0,
        };

        instance.flush_storage();
        instance.assert_solvency_invariant();
        instance.assert_state_storage_margin();
        instance
    }

    // -------------------------------------------------------------------------
    // Execution Requests & OutLayer Integration
    // -------------------------------------------------------------------------

    /// Request a price index update through OutLayer Testnet.
    ///
    /// Requires an attached deposit to fund verifiable execution inside the TEE worker.
    /// Preserves `payer_account_id = Some(caller)` so OutLayer sends normal execution
    /// settlement refunds directly to the requester.
    #[payable]
    pub fn request_price_update(&mut self, market: String) -> Promise {
        let caller = env::predecessor_account_id();

        // 1. Authorization check
        require!(
            caller == self.admin || self.authorized_requesters.contains(&caller),
            "Unauthorized requester: caller is not in authorized list"
        );

        // 2. Market check
        let market_cfg =
            self.markets.get(&market).cloned().unwrap_or_else(|| {
                env::panic_str(&format!("Market '{}' is not registered", market))
            });

        require!(market_cfg.active, "Market is inactive");

        // 3. Cost & Deposit check
        let resource_limits = ResourceLimits::default();
        let input = WorkerInput::new_fixture(&market);
        let input_json = serde_json::to_string(&input)
            .unwrap_or_else(|e| env::panic_str(&format!("Failed to serialize WorkerInput: {e}")));

        let estimated_cost = calculate_execution_cost(&resource_limits, input_json.len() as u64);
        let deposit = env::attached_deposit();

        require!(
            deposit >= estimated_cost,
            format!(
                "Insufficient deposit: attached {}, estimated minimum required {}",
                deposit, estimated_cost
            )
        );

        // 4. Open request cap & safe storage capacity enforcement
        // Do not release a request slot while associated user rights or unresolved records still require accounting.
        let open_requests = self.compute_open_request_slots();
        require!(
            open_requests < self.max_open_requests,
            format!(
                "ERR_OPEN_REQUEST_LIMIT_REACHED: open requests ({}) >= max_open_requests ({}); resolve pending/unreconciled/refund records first",
                open_requests, self.max_open_requests
            )
        );

        let spendable_excluding_deposit =
            compute_spendable_balance().saturating_sub(deposit.as_yoctonear());
        let per_request_storage_reserve = (OPEN_REQUEST_STORAGE_BUDGET_BYTES as u128)
            .checked_mul(env::storage_byte_cost().as_yoctonear())
            .expect("ERR_STORAGE_BUDGET_OVERFLOW: storage reserve overflow");
        let min_required_with_storage = self
            .compute_required_backing()
            .checked_add(per_request_storage_reserve)
            .expect("ERR_SOLVENCY_OVERFLOW: storage margin overflow");
        require!(
            spendable_excluding_deposit >= min_required_with_storage,
            format!(
                "ERR_INSUFFICIENT_STORAGE_RESERVE: unallocated spendable balance excluding attached deposit ({} yocto) < required backing plus {} byte storage margin ({} yocto)",
                spendable_excluding_deposit, OPEN_REQUEST_STORAGE_BUDGET_BYTES, min_required_with_storage
            )
        );

        // 5. Nonce assignment and tracking
        let nonce = self.next_nonce;
        self.next_nonce = self
            .next_nonce
            .checked_add(1)
            .expect("ERR_NONCE_OVERFLOW: next_nonce overflow");

        let now_sec = env::block_timestamp_ms() / 1000;
        let pending = PendingRequest {
            nonce,
            requester: caller.clone(),
            market: market.clone(),
            requested_at_sec: now_sec,
            expected_wasm_hash: market_cfg.wasm_sha256.clone(),
            deposit_attached: deposit,
        };
        self.pending_requests.insert(nonce, pending);
        self.pending_request_count = self
            .pending_request_count
            .checked_add(1)
            .expect("ERR_PENDING_COUNT_OVERFLOW: pending_request_count overflow");
        self.flush_storage();

        // 6. Construct OutLayer request
        let source = ExecutionSource::wasm_url(
            market_cfg.wasm_url,
            market_cfg.wasm_sha256,
            market_cfg.build_target,
        );

        // 7. Cross-contract call to outlayer.testnet + chained private self-callback.
        ext_outlayer::ext(self.outlayer_contract.clone())
            .with_attached_deposit(deposit)
            .with_static_gas(OUTLAYER_CALL_GAS)
            .request_execution(
                source,
                Some(resource_limits),
                Some(input_json),
                None, // secrets_ref
                Some("Json".to_string()),
                Some(caller), // payer_account_id: OutLayer's execution-settlement refund destination
                None,         // params
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(CALLBACK_GAS)
                    .on_outlayer_execution(nonce, market),
            )
    }

    /// Private self-callback processing the result from OutLayer.
    ///
    /// ## Promise Error & Refund Solvency Handling
    ///
    /// - `Ok(Some(output))`: Stores verified price observation, clears pending request, creates no credit.
    /// - `Ok(b"null")`: Normal WASI worker failure inside OutLayer TEE. OutLayer handles its own
    ///   payer settlement refund directly to `payer_account_id`. Clears pending request, creates no local credit.
    /// - `Err(PromiseError::Failed)`: Ambiguous cross-contract or yield failure. NEVER immediately
    ///   credits `pending_refunds` because OutLayer may have already accepted and retained the deposit
    ///   (or already refunded `payer_account_id`). Records an `UnreconciledFailure` with zero payable
    ///   liability and emits `execution_failed_unreconciled`.
    /// - `Err(PromiseError::TooLong(len))`: OutLayer accepted and completed the execution, returning a
    ///   payload > 1,024 bytes. OutLayer settles its own payer refund. Clears pending request, creates no credit.
    /// - Other `PromiseError` variants: Treated conservatively as `UnreconciledFailure` without creating
    ///   payable liability.
    #[private]
    pub fn on_outlayer_execution(
        &mut self,
        nonce: u64,
        market: String,
    ) -> Option<PriceObservation> {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "Method is private: callback must be invoked by contract itself"
        );

        // 1. Retrieve and remove pending request (replay protection)
        let pending = match self.pending_requests.remove(&nonce) {
            Some(p) => p,
            None => {
                env::panic_str(&format!("ERR_UNKNOWN_OR_DUPLICATE_NONCE: {}", nonce));
            }
        };
        self.pending_request_count = self.pending_request_count.saturating_sub(1);

        // 2. Validate market matches original request
        if pending.market != market {
            self.flush_storage();
            self.assert_solvency_invariant();
            env::log_str(&format!(
                "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"execution_failed\",\"nonce\":{},\"market\":\"{}\",\"reason\":\"ERR_MARKET_MISMATCH: expected {}, got {}\"}}",
                nonce, market, pending.market, market
            ));
            return None;
        }

        // 3. Fetch market configuration
        let market_cfg = match self.markets.get(&market) {
            Some(cfg) => cfg.clone(),
            None => {
                self.flush_storage();
                self.assert_solvency_invariant();
                env::log_str(&format!(
                    "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"execution_failed\",\"nonce\":{},\"market\":\"{}\",\"reason\":\"ERR_MARKET_NOT_FOUND: {}\"}}",
                    nonce, market, market
                ));
                return None;
            }
        };

        let now_sec = env::block_timestamp_ms() / 1000;

        // 4. Inspect promise result from OutLayer.
        let promise_data = match env::promise_result_checked(0, MAX_OUTLAYER_RESPONSE_BYTES) {
            Ok(data) => {
                if data == b"null" {
                    self.flush_storage();
                    self.assert_solvency_invariant();
                    env::log_str(&format!(
                        "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"execution_failed\",\"nonce\":{},\"market\":\"{}\",\"reason\":\"outlayer worker execution failed (null response — TEE worker returned non-zero exit or exceeded resource limits)\"}}",
                        nonce, market
                    ));
                    return None;
                }
                data
            }
            Err(PromiseError::Failed) => {
                let amount = pending.deposit_attached;
                let amount_yocto = amount.as_yoctonear();
                if amount_yocto > 0 {
                    let unreconciled = UnreconciledFailure {
                        nonce,
                        requester: pending.requester.clone(),
                        market: market.clone(),
                        deposit_at_risk: amount,
                        failed_at_sec: now_sec,
                        reason: "PromiseError::Failed".to_string(),
                        outlayer_request_id: None,
                        external_resolution_proposed: false,
                        resolution_evidence_sha256: None,
                        resolution_proposed_at_block: None,
                    };
                    self.unreconciled_failures.insert(nonce, unreconciled);
                    self.unreconciled_failure_count =
                        self.unreconciled_failure_count.checked_add(1).expect(
                            "ERR_UNRECONCILED_COUNT_OVERFLOW: unreconciled_failure_count overflow",
                        );
                }
                self.flush_storage();
                self.assert_solvency_invariant();
                env::log_str(&format!(
                    "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"execution_failed_unreconciled\",\"nonce\":{},\"requester\":\"{}\",\"market\":\"{}\",\"amount\":\"{}\",\"reason\":\"PromiseError::Failed — recorded as UnreconciledFailure pending backing verification\"}}",
                    nonce, pending.requester, market, amount_yocto
                ));
                return None;
            }
            Err(PromiseError::TooLong(len)) => {
                self.flush_storage();
                self.assert_solvency_invariant();
                env::log_str(&format!(
                    "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"execution_failed\",\"nonce\":{},\"market\":\"{}\",\"reason\":\"OutLayer response exceeded direct yield/resume limit: {} bytes > {} bytes max (two-stage large-output retrieval is deferred to a future mission)\"}}",
                    nonce, market, len, MAX_OUTLAYER_RESPONSE_BYTES
                ));
                return None;
            }
            Err(_) => {
                let amount = pending.deposit_attached;
                let amount_yocto = amount.as_yoctonear();
                if amount_yocto > 0 {
                    let unreconciled = UnreconciledFailure {
                        nonce,
                        requester: pending.requester.clone(),
                        market: market.clone(),
                        deposit_at_risk: amount,
                        failed_at_sec: now_sec,
                        reason: "unexpected PromiseError variant".to_string(),
                        outlayer_request_id: None,
                        external_resolution_proposed: false,
                        resolution_evidence_sha256: None,
                        resolution_proposed_at_block: None,
                    };
                    self.unreconciled_failures.insert(nonce, unreconciled);
                    self.unreconciled_failure_count =
                        self.unreconciled_failure_count.checked_add(1).expect(
                            "ERR_UNRECONCILED_COUNT_OVERFLOW: unreconciled_failure_count overflow",
                        );
                }
                self.flush_storage();
                self.assert_solvency_invariant();
                env::log_str(&format!(
                    "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"execution_failed_unreconciled\",\"nonce\":{},\"requester\":\"{}\",\"market\":\"{}\",\"amount\":\"{}\",\"reason\":\"unexpected PromiseError variant — recorded as UnreconciledFailure\"}}",
                    nonce, pending.requester, market, amount_yocto
                ));
                return None;
            }
        };

        // 5. Complete typed validation without panics
        let existing_price = self.prices.get(&market);

        let worker_output = match validate_worker_output(
            &promise_data,
            &market,
            &market_cfg,
            existing_price,
            now_sec,
        ) {
            Ok(output) => output,
            Err(err_msg) => {
                self.flush_storage();
                self.assert_solvency_invariant();
                env::log_str(&format!(
                    "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"execution_failed\",\"nonce\":{},\"market\":\"{}\",\"reason\":\"{}\"}}",
                    nonce, market, err_msg
                ));
                return None;
            }
        };

        let price_u128 = worker_output
            .parse_price()
            .expect("price already validated in validate_worker_output");

        // 6. Store validated price observation and enforce post-flush solvency invariant
        let observation = PriceObservation {
            market: market.clone(),
            price: U128(price_u128),
            decimals: worker_output.decimals,
            unit: worker_output.unit,
            source: worker_output.source,
            source_count: worker_output.source_count,
            observed_at_sec: worker_output.observed_at,
            stored_at_sec: now_sec,
            nonce,
            data_class: worker_output.data_class,
        };

        self.prices.insert(market.clone(), observation.clone());
        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"price_stored\",\"market\":\"{}\",\"price\":\"{}\",\"nonce\":{}}}",
            market, price_u128, nonce
        ));

        Some(observation)
    }

    // -------------------------------------------------------------------------
    // Refund Reserve Funding, Promotion & External Reconciliation
    // -------------------------------------------------------------------------

    /// Explicitly fund the dedicated refund-backing reserve by attaching NEAR.
    ///
    /// Designated reserve funds cannot be withdrawn by users unless an `UnreconciledFailure`
    /// is explicitly promoted against the reserve.
    #[payable]
    pub fn fund_refund_reserve(&mut self) {
        let deposit = env::attached_deposit().as_yoctonear();
        require!(
            deposit > 0,
            "ERR_ZERO_RESERVE_DEPOSIT: attached deposit must be strictly positive"
        );

        let funder = env::predecessor_account_id();
        self.refund_reserve_balance = self
            .refund_reserve_balance
            .checked_add(deposit)
            .expect("ERR_RESERVE_OVERFLOW: refund_reserve_balance overflow");

        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"refund_reserve_funded\",\"funder\":\"{}\",\"amount\":\"{}\",\"new_reserve_balance\":\"{}\"}}",
            funder, deposit, self.refund_reserve_balance
        ));
    }

    /// Explicitly fund the protected minimum operating reserve by attaching NEAR.
    ///
    /// Contributed operating funds are permanently added to `minimum_operating_reserve` (included
    /// once in `compute_required_backing()`) so they are strictly segregated and can never be
    /// reclassified as unrestricted surplus or designated into `refund_reserve_balance`.
    #[payable]
    pub fn fund_operating_reserve(&mut self) {
        let deposit = env::attached_deposit().as_yoctonear();
        require!(
            deposit > 0,
            "ERR_ZERO_OPERATING_DEPOSIT: attached deposit must be strictly positive"
        );
        self.minimum_operating_reserve = self
            .minimum_operating_reserve
            .checked_add(deposit)
            .expect("ERR_OPERATING_RESERVE_OVERFLOW: minimum_operating_reserve overflow");

        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"operating_reserve_funded\",\"funder\":\"{}\",\"amount\":\"{}\",\"new_minimum_operating_reserve\":\"{}\",\"spendable_balance\":\"{}\"}}",
            env::predecessor_account_id(), deposit, self.minimum_operating_reserve, compute_spendable_balance()
        ));
    }

    /// Administratively adjust the open request capacity limit.
    ///
    /// Enforces `1 <= max_open_requests <= HARD_MAX_OPEN_REQUESTS` and verifies that unallocated
    /// spendable balance covers the storage budget for all remaining open request slots.
    pub fn set_max_open_requests(&mut self, max_open_requests: u32) {
        self.assert_admin();
        require!(
            max_open_requests > 0 && max_open_requests <= HARD_MAX_OPEN_REQUESTS,
            format!(
                "ERR_INVALID_MAX_OPEN_REQUESTS: max_open_requests ({}) must be in 1..={}",
                max_open_requests, HARD_MAX_OPEN_REQUESTS
            )
        );
        self.flush_storage();
        let open_slots = self.compute_open_request_slots();
        let remaining_slots = max_open_requests.saturating_sub(open_slots) as u128;
        let per_slot_budget = (OPEN_REQUEST_STORAGE_BUDGET_BYTES as u128)
            .checked_mul(env::storage_byte_cost().as_yoctonear())
            .expect("ERR_STORAGE_BUDGET_OVERFLOW: per-slot storage budget overflow");
        let required_storage_backing = remaining_slots
            .checked_mul(per_slot_budget)
            .expect("ERR_STORAGE_BUDGET_OVERFLOW: total slot storage budget overflow");
        let required_total = self
            .compute_required_backing()
            .checked_add(required_storage_backing)
            .expect("ERR_SOLVENCY_OVERFLOW: max_open_requests backing check overflow");
        let spendable = compute_spendable_balance();
        require!(
            spendable >= required_total,
            format!(
                "ERR_INSUFFICIENT_STORAGE_RESERVE_FOR_MAX_OPEN_REQUESTS: spendable ({} yocto) < required_backing + {} slot storage budget ({} yocto)",
                spendable, remaining_slots, required_total
            )
        );

        let previous = self.max_open_requests;
        self.max_open_requests = max_open_requests;
        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"max_open_requests_updated\",\"previous\":{},\"new_max\":{}}}",
            previous, max_open_requests
        ));
    }

    /// Administratively designate already-held unallocated contract balance as refund-backing reserve.
    ///
    /// Used after an administrator verifies off-chain (e.g., via NEAR receipt inspection) that a
    /// NEAR system-refund receipt physically returned a failed execution's deposit to this contract.
    ///
    /// Also checks `spendable_balance >= compute_encumbered_backing_floor()` so an admin cannot
    /// allocate a returning system refund from an `InFlight` or `Disputed` refund claim or insurance
    /// compensation before its callback settles.
    ///
    /// NOTE: `evidence_sha256` is an administrative audit attestation, not an on-chain cryptographic
    /// proof of receipt inclusion.
    pub fn admin_designate_refund_reserve(&mut self, amount: NearToken, evidence_sha256: String) {
        self.assert_admin();

        let amount_yocto = amount.as_yoctonear();
        require!(
            amount_yocto > 0,
            "ERR_ZERO_DESIGNATION: designated reserve amount must be strictly positive"
        );
        Self::assert_valid_sha256_hex(&evidence_sha256, "evidence_sha256");

        self.refund_reserve_balance = self
            .refund_reserve_balance
            .checked_add(amount_yocto)
            .expect("ERR_RESERVE_OVERFLOW: refund_reserve_balance overflow");

        self.flush_storage();
        self.assert_solvency_invariant();
        let spendable = compute_spendable_balance();
        let encumbered_floor = self.compute_encumbered_backing_floor();
        require!(
            spendable >= encumbered_floor,
            format!(
                "SOLVENCY_INVARIANT_VIOLATION: spendable ({}) < encumbered_backing_floor ({}); wait for InFlight/Disputed claims and insurance transfers to settle before designating surplus",
                spendable, encumbered_floor
            )
        );

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"refund_reserve_designated\",\"admin\":\"{}\",\"amount\":\"{}\",\"new_reserve_balance\":\"{}\",\"evidence_sha256\":\"{}\"}}",
            self.admin, amount_yocto, self.refund_reserve_balance, evidence_sha256
        ));
    }

    /// Administratively increase the protected minimum operating reserve.
    ///
    /// The minimum operating reserve can only be increased, never lowered below its current value.
    pub fn increase_minimum_operating_reserve(&mut self, new_minimum: NearToken) {
        self.assert_admin();

        let new_min_yocto = new_minimum.as_yoctonear();
        require!(
            new_min_yocto > self.minimum_operating_reserve,
            format!(
                "ERR_INVALID_OPERATING_RESERVE: new_minimum ({}) must be strictly greater than current minimum_operating_reserve ({})",
                new_min_yocto, self.minimum_operating_reserve
            )
        );

        let previous = self.minimum_operating_reserve;
        self.minimum_operating_reserve = new_min_yocto;

        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"minimum_operating_reserve_increased\",\"previous\":\"{}\",\"new_minimum\":\"{}\"}}",
            previous, new_min_yocto
        ));
    }

    /// Promote an `UnreconciledFailure` into a payable `RefundCredit` backed 1:1 by `refund_reserve_balance`.
    ///
    /// Permissionless / requester-accessible: the recipient and amount are strictly bound to the
    /// stored `UnreconciledFailure` record and cannot be chosen by the caller.
    pub fn promote_unreconciled_refund(&mut self, nonce: u64) {
        let failure = match self.unreconciled_failures.get(&nonce) {
            Some(f) => f.clone(),
            None => env::panic_str(&format!(
                "ERR_UNRECONCILED_NOT_FOUND: no unresolved failure for nonce {}",
                nonce
            )),
        };

        require!(
            !failure.external_resolution_proposed,
            format!(
                "ERR_EXTERNAL_RESOLUTION_PROPOSED: external resolution is proposed for nonce {}; requester must acknowledge external receipt or reject/cancel the proposal before promoting",
                nonce
            )
        );

        let amount = failure.deposit_at_risk;
        let amount_yocto = amount.as_yoctonear();
        require!(
            amount_yocto > 0,
            "ERR_ZERO_UNRECONCILED_AMOUNT: deposit_at_risk is zero"
        );

        require!(
            self.refund_reserve_balance >= amount_yocto,
            format!(
                "ERR_INSUFFICIENT_REFUND_RESERVE: refund_reserve_balance ({} yocto) < deposit_at_risk ({} yocto) for nonce {}",
                self.refund_reserve_balance, amount_yocto, nonce
            )
        );

        // Verify formal solvency invariant prior to transition
        self.flush_storage();
        self.assert_solvency_invariant();

        // Atomically consume reserve, remove unresolved failure, and credit user liability
        self.refund_reserve_balance = self
            .refund_reserve_balance
            .checked_sub(amount_yocto)
            .expect("ERR_RESERVE_UNDERFLOW: refund_reserve_balance underflow");

        self.unreconciled_failures.remove(&nonce);
        self.unreconciled_failure_count = self.unreconciled_failure_count.saturating_sub(1);
        self.record_refund_credit(&failure.requester, amount, nonce);

        // Flush all modified LookupMap entries before asserting post-transition solvency invariant
        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"refund_credit_backed\",\"nonce\":{},\"requester\":\"{}\",\"amount\":\"{}\",\"remaining_reserve\":\"{}\",\"total_refund_liability\":\"{}\"}}",
            nonce, failure.requester, amount_yocto, self.refund_reserve_balance, self.total_refund_liability
        ));
    }

    /// Administratively propose external resolution for an `UnreconciledFailure` after verifying
    /// off-chain that OutLayer settled or refunded the requester directly.
    ///
    /// IMPORTANT (F-01 Hardening): This method NEVER unilaterally deletes the user's record.
    /// It records an external resolution proposal on the `UnreconciledFailure`; the record and its
    /// request-capacity slot remain preserved until the original requester explicitly accepts on-chain
    /// via `acknowledge_unreconciled_resolved_external(nonce)`.
    pub fn admin_resolve_unreconciled_external(&mut self, nonce: u64, evidence_sha256: String) {
        self.assert_admin();
        Self::assert_valid_sha256_hex(&evidence_sha256, "evidence_sha256");

        let now_block = env::block_height();
        let (requester, amount_yocto) = {
            let failure = match self.unreconciled_failures.get_mut(&nonce) {
                Some(f) => f,
                None => env::panic_str(&format!(
                    "ERR_UNRECONCILED_NOT_FOUND: no unresolved failure for nonce {}",
                    nonce
                )),
            };
            require!(
                !failure.external_resolution_proposed,
                format!(
                    "ERR_EXTERNAL_RESOLUTION_ALREADY_PROPOSED: external resolution already proposed for nonce {}",
                    nonce
                )
            );
            failure.external_resolution_proposed = true;
            failure.resolution_evidence_sha256 = Some(evidence_sha256.clone());
            failure.resolution_proposed_at_block = Some(now_block);
            (
                failure.requester.clone(),
                failure.deposit_at_risk.as_yoctonear(),
            )
        };

        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"unreconciled_failure_resolution_proposed\",\"nonce\":{},\"requester\":\"{}\",\"amount\":\"{}\",\"proposed_at_block\":{},\"evidence_sha256\":\"{}\"}}",
            nonce, requester, amount_yocto, now_block, evidence_sha256
        ));
    }

    /// Requester acceptance/acknowledgement that an `UnreconciledFailure` was resolved externally
    /// (e.g., direct OutLayer refund to `payer_account_id`).
    ///
    /// Callable only by `failure.requester`. Removes the `UnreconciledFailure` record without
    /// creating local refund liability.
    pub fn acknowledge_unreconciled_resolved_external(&mut self, nonce: u64) {
        let caller = env::predecessor_account_id();
        let failure = match self.unreconciled_failures.get(&nonce) {
            Some(f) => f.clone(),
            None => env::panic_str(&format!(
                "ERR_UNRECONCILED_NOT_FOUND: no unresolved failure for nonce {}",
                nonce
            )),
        };
        require!(
            caller == failure.requester,
            "ERR_ONLY_REQUESTER_CAN_ACKNOWLEDGE: only the original requester can accept external resolution"
        );

        self.unreconciled_failures.remove(&nonce);
        self.unreconciled_failure_count = self.unreconciled_failure_count.saturating_sub(1);
        self.flush_storage();
        self.assert_solvency_invariant();

        let evidence = failure
            .resolution_evidence_sha256
            .unwrap_or_else(|| "self_acknowledged".to_string());
        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"unreconciled_failure_resolved_external\",\"nonce\":{},\"requester\":\"{}\",\"amount\":\"{}\",\"evidence_sha256\":\"{}\"}}",
            nonce,
            failure.requester,
            failure.deposit_at_risk.as_yoctonear(),
            evidence
        ));
    }

    /// Requester rejection/dispute of an active administrative external-resolution proposal (`L-02`).
    ///
    /// Callable only by `failure.requester` when `failure.external_resolution_proposed == true`.
    /// Clears the proposal metadata while preserving the `UnreconciledFailure` record and original
    /// refund entitlement so backed recovery via `promote_unreconciled_refund` may proceed.
    pub fn reject_unreconciled_external_resolution(&mut self, nonce: u64) {
        let caller = env::predecessor_account_id();
        let (requester, amount_yocto) = {
            let failure = match self.unreconciled_failures.get_mut(&nonce) {
                Some(f) => f,
                None => env::panic_str(&format!(
                    "ERR_UNRECONCILED_NOT_FOUND: no unresolved failure for nonce {}",
                    nonce
                )),
            };
            require!(
                caller == failure.requester,
                "ERR_ONLY_REQUESTER_CAN_REJECT: only the original requester can reject an external resolution proposal"
            );
            require!(
                failure.external_resolution_proposed,
                format!(
                    "ERR_NO_EXTERNAL_RESOLUTION_PROPOSED: no active external resolution proposal for nonce {}",
                    nonce
                )
            );
            failure.external_resolution_proposed = false;
            failure.resolution_evidence_sha256 = None;
            failure.resolution_proposed_at_block = None;
            (
                failure.requester.clone(),
                failure.deposit_at_risk.as_yoctonear(),
            )
        };

        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"unreconciled_failure_resolution_rejected\",\"nonce\":{},\"requester\":\"{}\",\"amount\":\"{}\"}}",
            nonce, requester, amount_yocto
        ));
    }

    /// Administrative cancellation of an active external-resolution proposal (`L-02`).
    ///
    /// Preserves the `UnreconciledFailure` record and requester rights; does NOT delete the record
    /// and does NOT automatically create a refund credit.
    pub fn admin_cancel_unreconciled_external_resolution(&mut self, nonce: u64) {
        self.assert_admin();
        let (requester, amount_yocto) = {
            let failure = match self.unreconciled_failures.get_mut(&nonce) {
                Some(f) => f,
                None => env::panic_str(&format!(
                    "ERR_UNRECONCILED_NOT_FOUND: no unresolved failure for nonce {}",
                    nonce
                )),
            };
            require!(
                failure.external_resolution_proposed,
                format!(
                    "ERR_NO_EXTERNAL_RESOLUTION_PROPOSED: no active external resolution proposal for nonce {}",
                    nonce
                )
            );
            failure.external_resolution_proposed = false;
            failure.resolution_evidence_sha256 = None;
            failure.resolution_proposed_at_block = None;
            (
                failure.requester.clone(),
                failure.deposit_at_risk.as_yoctonear(),
            )
        };

        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"unreconciled_failure_resolution_cancelled\",\"nonce\":{},\"admin\":\"{}\",\"requester\":\"{}\",\"amount\":\"{}\"}}",
            nonce, self.admin, requester, amount_yocto
        ));
    }

    // -------------------------------------------------------------------------
    // Refund Claim & Return Reconciliation Lifecycle
    // -------------------------------------------------------------------------

    /// Claim a pending backed deposit refund credit.
    ///
    /// Enforces the formal global solvency invariant before and after flushing the new
    /// `RefundClaim` record to storage.
    ///
    /// Historical `Disputed` claims are tracked independently by `claim_id` in `self.claims` and
    /// `credit.disputed_amount` (non-payable) and do NOT block claiming new independent
    /// `available_amount` credits for the same account (F-03 remediation).
    pub fn claim_refund(&mut self) -> Promise {
        let caller = env::predecessor_account_id();

        // Global Solvency Invariant check before mutating state
        self.flush_storage();
        self.assert_solvency_invariant();

        let (claim_amount, claim_yocto, originating_nonce) = {
            let credit = self
                .pending_refunds
                .get(&caller)
                .unwrap_or_else(|| env::panic_str("No pending refund credit for this account"));

            require!(
                credit.in_flight_amount.as_yoctonear() == 0,
                "A refund claim is already in-flight for this account; wait for transfer confirmation"
            );
            require!(
                credit.return_pending_amount.as_yoctonear() == 0,
                "A failed refund transfer return is pending reconciliation for this account; call reconcile_returned_refund first"
            );
            require!(
                credit.active_claim_id.is_none(),
                "An active in-flight or return-pending claim already exists for this account"
            );

            let amount = credit.available_amount;
            let yocto = amount.as_yoctonear();
            require!(yocto > 0, "No available refund credit to claim");
            (amount, yocto, credit.from_nonce)
        };

        let claim_id = self.next_claim_id;
        self.next_claim_id = self
            .next_claim_id
            .checked_add(1)
            .expect("ERR_CLAIM_ID_OVERFLOW: next_claim_id overflow");

        let now_sec = env::block_timestamp_ms() / 1000;
        let now_block = env::block_height();
        let claim_record = RefundClaim {
            claim_id,
            claimant: caller.clone(),
            amount: claim_amount,
            originating_nonce,
            initiated_at_sec: now_sec,
            initiated_at_block: now_block,
            disputed_at_block: None,
            status: ClaimStatus::InFlight,
            admin_settlement_proposed: false,
            settlement_proposed_at_block: None,
            settlement_evidence_sha256: None,
        };
        self.claims.insert(claim_id, claim_record);
        self.active_claim_count = self
            .active_claim_count
            .checked_add(1)
            .expect("ERR_CLAIM_COUNT_OVERFLOW: active_claim_count overflow");

        // Atomic state transition: Available -> InFlight (preserving any independent disputed_amount)
        {
            let credit = self
                .pending_refunds
                .get_mut(&caller)
                .expect("credit verified above");
            credit.in_flight_amount = claim_amount;
            credit.available_amount = NearToken::from_yoctonear(0);
            credit.status = RefundStatus::InFlight;
            credit.active_claim_id = Some(claim_id);
        }

        self.total_in_flight_liability = self
            .total_in_flight_liability
            .checked_add(claim_yocto)
            .expect("ERR_OVERFLOW: total_in_flight_liability overflow");

        // Flush `claims` and `pending_refunds` to the storage trie BEFORE checking post-flush solvency
        // and dispatching the outbound Transfer promise (F-04 remediation).
        self.flush_storage();
        self.assert_solvency_invariant_with_outbound_transfer(claim_yocto);

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"refund_claim_started\",\"claim_id\":{},\"claimant\":\"{}\",\"amount\":\"{}\",\"originating_nonce\":{},\"initiated_at_block\":{}}}",
            claim_id, caller, claim_yocto, originating_nonce, now_block
        ));

        Promise::new(caller.clone()).transfer(claim_amount).then(
            Self::ext(env::current_account_id())
                .with_static_gas(REFUND_CALLBACK_GAS)
                .on_refund_transfer(caller, claim_amount, claim_id),
        )
    }

    /// Private self-callback finalizing a refund transfer or transitioning a failed transfer
    /// to `ReturnPending`.
    ///
    /// Accepts claims in `InFlight` OR `Disputed` (so a late objective NEAR callback still overrides
    /// an ambiguous dispute without interfering with any newer claim for the same user).
    /// Deleted claims are never recreated.
    #[private]
    pub fn on_refund_transfer(
        &mut self,
        account_id: AccountId,
        amount: NearToken,
        claim_id: u64,
    ) -> bool {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "Method is private: callback must be invoked by contract itself"
        );

        let amount_yocto = amount.as_yoctonear();

        // 1. Claim verification and replay prevention
        let was_disputed = {
            let claim = match self.claims.get(&claim_id) {
                Some(c) => c,
                None => {
                    env::panic_str(&format!(
                        "CRITICAL_ACCOUNTING_ERROR: missing refund claim record for claim_id {}",
                        claim_id
                    ));
                }
            };

            require!(
                claim.claimant == account_id,
                format!(
                    "CRITICAL_ACCOUNTING_ERROR: claimant mismatch (claim {} belongs to {}, callback specified {})",
                    claim_id, claim.claimant, account_id
                )
            );
            require!(
                claim.amount.as_yoctonear() == amount_yocto,
                format!(
                    "CRITICAL_ACCOUNTING_ERROR: claim amount mismatch for claim_id {}",
                    claim_id
                )
            );
            require!(
                claim.status == ClaimStatus::InFlight || claim.status == ClaimStatus::Disputed,
                format!(
                    "CRITICAL_ACCOUNTING_ERROR: claim {} is not InFlight or Disputed (status: {:?})",
                    claim_id, claim.status
                )
            );
            claim.status == ClaimStatus::Disputed
        };

        // 2. Promise result check (objective runtime outcome of NEAR Transfer receipt)
        let transfer_succeeded = match env::promise_result_checked(0, 64) {
            Ok(_) => true,
            Err(PromiseError::Failed) => false,
            Err(PromiseError::TooLong(_)) => false,
            #[allow(unreachable_patterns)]
            Err(_) => false,
        };

        let should_remove_credit = {
            let credit = match self.pending_refunds.get_mut(&account_id) {
                Some(c) => c,
                None => {
                    env::panic_str(&format!(
                        "CRITICAL_ACCOUNTING_ERROR: missing refund credit record for account {}",
                        account_id
                    ));
                }
            };

            if was_disputed {
                require!(
                    credit.disputed_amount.as_yoctonear() >= amount_yocto
                        && self.total_disputed_liability >= amount_yocto,
                    "CRITICAL_ACCOUNTING_ERROR: disputed_amount mismatch"
                );
                let new_disputed = credit
                    .disputed_amount
                    .as_yoctonear()
                    .checked_sub(amount_yocto)
                    .expect("ERR_UNDERFLOW: disputed_amount underflow");
                credit.disputed_amount = NearToken::from_yoctonear(new_disputed);
                self.total_disputed_liability = self
                    .total_disputed_liability
                    .checked_sub(amount_yocto)
                    .expect("ERR_UNDERFLOW: total_disputed_liability underflow");
                if credit.active_claim_id == Some(claim_id) {
                    credit.active_claim_id = None;
                }
            } else {
                require!(
                    credit.active_claim_id == Some(claim_id),
                    format!(
                        "CRITICAL_ACCOUNTING_ERROR: credit active_claim_id ({:?}) mismatch for InFlight claim_id {}",
                        credit.active_claim_id, claim_id
                    )
                );
                require!(
                    credit.in_flight_amount.as_yoctonear() >= amount_yocto
                        && self.total_in_flight_liability >= amount_yocto,
                    format!(
                        "CRITICAL_ACCOUNTING_ERROR: in_flight_amount ({}) less than callback amount ({})",
                        credit.in_flight_amount.as_yoctonear(),
                        amount_yocto
                    )
                );
                let new_in_flight = credit
                    .in_flight_amount
                    .as_yoctonear()
                    .checked_sub(amount_yocto)
                    .expect("ERR_UNDERFLOW: in_flight_amount underflow");
                credit.in_flight_amount = NearToken::from_yoctonear(new_in_flight);
                self.total_in_flight_liability = self
                    .total_in_flight_liability
                    .checked_sub(amount_yocto)
                    .expect("ERR_UNDERFLOW: total_in_flight_liability underflow");
            }

            if transfer_succeeded {
                if credit.active_claim_id == Some(claim_id) {
                    credit.active_claim_id = None;
                }

                self.total_refund_liability = self
                    .total_refund_liability
                    .checked_sub(amount_yocto)
                    .expect("ERR_UNDERFLOW: total_refund_liability underflow");

                Self::refresh_credit_status(credit)
            } else {
                let new_return_pending = credit
                    .return_pending_amount
                    .as_yoctonear()
                    .checked_add(amount_yocto)
                    .expect("ERR_OVERFLOW: return_pending_amount overflow");
                credit.return_pending_amount = NearToken::from_yoctonear(new_return_pending);
                if credit.active_claim_id.is_none() || credit.active_claim_id == Some(claim_id) {
                    credit.active_claim_id = Some(claim_id);
                    credit.status = RefundStatus::ReturnPending;
                }
                false
            }
        };

        if transfer_succeeded {
            if should_remove_credit {
                self.pending_refunds.remove(&account_id);
                self.active_refund_credit_count = self.active_refund_credit_count.saturating_sub(1);
            }

            // Remove terminal settled claim record from storage
            self.claims.remove(&claim_id);
            self.active_claim_count = self.active_claim_count.saturating_sub(1);
            self.flush_storage();
            self.assert_solvency_invariant();

            env::log_str(&format!(
                "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"refund_claimed\",\"claim_id\":{},\"claimant\":\"{}\",\"amount\":\"{}\"}}",
                claim_id, account_id, amount_yocto
            ));

            true
        } else {
            // Objective proof from NEAR runtime that Transfer failed -> ReturnPending.
            // Note: The asynchronous NEAR system-refund receipt restoring `amount_yocto` may settle in
            // a separate receipt; `reconcile_returned_refund` enforces `self.assert_solvency_invariant()`
            // after storage flush before restoring `ReturnPending` -> `Available`.
            if let Some(claim) = self.claims.get_mut(&claim_id) {
                claim.status = ClaimStatus::ReturnPending;
                claim.disputed_at_block = None;
                claim.admin_settlement_proposed = false;
                claim.settlement_proposed_at_block = None;
                claim.settlement_evidence_sha256 = None;
            }
            self.flush_storage();

            env::log_str(&format!(
                "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"refund_transfer_return_pending\",\"claim_id\":{},\"claimant\":\"{}\",\"amount\":\"{}\",\"reason\":\"NEAR transfer failed; entitlement held in ReturnPending until system-refund backing is verified via reconcile_returned_refund\"}}",
                claim_id, account_id, amount_yocto
            ));

            false
        }
    }

    /// Reconcile a `ReturnPending` refund claim after the NEAR system-refund receipt has
    /// returned the failed transfer's funds to this contract.
    ///
    /// NOTE: Only `on_refund_transfer` observing `Err(...)` can create `ReturnPending` claims.
    pub fn reconcile_returned_refund(&mut self, claim_id: u64) {
        let (claimant, amount_yocto) = {
            let claim = match self.claims.get(&claim_id) {
                Some(c) => c,
                None => env::panic_str(&format!("ERR_CLAIM_NOT_FOUND: claim_id {}", claim_id)),
            };
            require!(
                claim.status == ClaimStatus::ReturnPending,
                format!(
                    "ERR_CLAIM_NOT_RETURN_PENDING: claim {} has status {:?}; expected ReturnPending",
                    claim_id, claim.status
                )
            );
            (claim.claimant.clone(), claim.amount.as_yoctonear())
        };

        // Require that the returned funds are physically present under the global solvency invariant.
        self.flush_storage();
        self.assert_solvency_invariant();

        {
            let credit = match self.pending_refunds.get_mut(&claimant) {
                Some(c) => c,
                None => env::panic_str(&format!(
                    "CRITICAL_ACCOUNTING_ERROR: missing refund credit record for claimant {}",
                    claimant
                )),
            };

            if let Some(active_id) = credit.active_claim_id {
                require!(
                    active_id == claim_id,
                    format!(
                        "ERR_CLAIM_ID_MISMATCH: credit active_claim_id ({:?}) does not match claim_id ({})",
                        credit.active_claim_id, claim_id
                    )
                );
            }
            require!(
                credit.return_pending_amount.as_yoctonear() >= amount_yocto,
                "ERR_INSUFFICIENT_RETURN_PENDING_AMOUNT"
            );

            let new_return_pending = credit
                .return_pending_amount
                .as_yoctonear()
                .checked_sub(amount_yocto)
                .expect("ERR_UNDERFLOW: return_pending_amount underflow");
            credit.return_pending_amount = NearToken::from_yoctonear(new_return_pending);

            let new_available = credit
                .available_amount
                .as_yoctonear()
                .checked_add(amount_yocto)
                .expect("ERR_OVERFLOW: available_amount restore overflow");
            credit.available_amount = NearToken::from_yoctonear(new_available);
            if credit.active_claim_id == Some(claim_id) {
                credit.active_claim_id = None;
            }
            let _ = Self::refresh_credit_status(credit);
        }

        // Remove terminal failed claim record from storage after restoring entitlement
        self.claims.remove(&claim_id);
        self.active_claim_count = self.active_claim_count.saturating_sub(1);
        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"refund_return_reconciled\",\"claim_id\":{},\"claimant\":\"{}\",\"amount\":\"{}\"}}",
            claim_id, claimant, amount_yocto
        ));
    }

    /// Move a stuck `InFlight` claim to `Disputed` (callable by admin OR the claimant themselves)
    /// once `STUCK_CLAIM_MIN_AGE_BLOCKS` blocks have elapsed.
    ///
    /// Elapsed blocks are NOT proof that the transfer failed; `Disputed` is strictly NON-PAYABLE,
    /// and clearing `active_claim_id` ensures unrelated future credits remain claimable.
    pub fn mark_claim_disputed(&mut self, claim_id: u64) {
        let claimant = match self.claims.get(&claim_id) {
            Some(c) => c.claimant.clone(),
            None => env::panic_str(&format!("ERR_CLAIM_NOT_FOUND: claim_id {}", claim_id)),
        };
        let caller = env::predecessor_account_id();
        require!(
            caller == self.admin || caller == claimant,
            "ERR_UNAUTHORIZED: only admin or claimant may mark a stuck claim disputed"
        );
        self.dispute_inflight_claim(claim_id);
    }

    /// Claimant self-acknowledgement that an `InFlight` or `Disputed` transfer was received.
    ///
    /// This is the canonical non-destructive path to close a disputed claim (with or without a
    /// prior administrative settlement proposal). Closes the liability and removes the terminal
    /// claim record with zero payout.
    pub fn acknowledge_disputed_transfer_received(&mut self, claim_id: u64) {
        let caller = env::predecessor_account_id();
        let (claimant, amount_yocto, was_disputed) = {
            let claim = match self.claims.get(&claim_id) {
                Some(c) => c,
                None => env::panic_str(&format!("ERR_CLAIM_NOT_FOUND: claim_id {}", claim_id)),
            };
            require!(
                caller == claim.claimant,
                "ERR_UNAUTHORIZED: only claimant can acknowledge receipt of transfer"
            );
            require!(
                claim.status == ClaimStatus::InFlight || claim.status == ClaimStatus::Disputed,
                format!(
                    "ERR_INVALID_CLAIM_STATUS: claim {} has status {:?}",
                    claim_id, claim.status
                )
            );
            (
                claim.claimant.clone(),
                claim.amount.as_yoctonear(),
                claim.status == ClaimStatus::Disputed,
            )
        };

        self.close_settled_stuck_claim(claim_id, &claimant, amount_yocto, was_disputed);

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"refund_claim_acknowledged_settled\",\"claim_id\":{},\"claimant\":\"{}\",\"amount\":\"{}\"}}",
            claim_id, claimant, amount_yocto
        ));
    }

    /// Hardened administrative reconciliation for stuck refund claims whose callback did not settle.
    ///
    /// - `ReconcileOutcome::ConfirmedFailure`: Allowed only on `InFlight` claims at least
    ///   `STUCK_CLAIM_MIN_AGE_BLOCKS` old. Moves the claim to `Disputed` (NON-PAYABLE). It NEVER
    ///   restores the entitlement to `Available`, NEVER enters `ReturnPending`, and does NOT delete
    ///   the claim.
    /// - `ReconcileOutcome::ConfirmedSuccess`: Allowed only on an already `Disputed` claim after
    ///   the `DISPUTE_RESOLUTION_DELAY_BLOCKS` window has elapsed.
    ///   IMPORTANT (F-01 Hardening): An administrator supplying `ConfirmedSuccess` NEVER deletes
    ///   the user's `Disputed` claim or reduces `total_disputed_liability`. It only records a
    ///   settlement proposal (`admin_settlement_proposed = true`); the claim record and liability
    ///   remain preserved until the claimant accepts via `acknowledge_disputed_transfer_received(claim_id)`
    ///   (or a late objective callback arrives).
    pub fn admin_reconcile_stuck_claim(&mut self, claim_id: u64, outcome: ReconcileOutcome) {
        self.assert_admin();

        match outcome {
            ReconcileOutcome::ConfirmedFailure => {
                self.dispute_inflight_claim(claim_id);
            }
            ReconcileOutcome::ConfirmedSuccess => {
                self.propose_disputed_claim_settlement_internal(claim_id, None);
            }
        }
    }

    /// Administratively propose external/paid settlement for a `Disputed` claim with a SHA-256
    /// off-chain evidence digest after `DISPUTE_RESOLUTION_DELAY_BLOCKS`.
    ///
    /// Preserves the `RefundClaim` and `total_disputed_liability` until the claimant accepts via
    /// `acknowledge_disputed_transfer_received(claim_id)`.
    pub fn admin_propose_disputed_settlement(&mut self, claim_id: u64, evidence_sha256: String) {
        self.assert_admin();
        Self::assert_valid_sha256_hex(&evidence_sha256, "evidence_sha256");
        self.propose_disputed_claim_settlement_internal(claim_id, Some(evidence_sha256));
    }

    // -------------------------------------------------------------------------
    // Separate Discretionary Insurance Mechanism (Disabled by Default)
    // -------------------------------------------------------------------------

    /// Enable or disable discretionary insurance compensation for `Disputed` claims.
    pub fn set_insurance_enabled(&mut self, enabled: bool) {
        self.assert_admin();
        self.insurance_enabled = enabled;
        self.flush_storage();
        self.assert_solvency_invariant();
        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"insurance_policy_updated\",\"enabled\":{}}}",
            enabled
        ));
    }

    /// Administratively adjust the maximum simultaneously active insurance compensation records.
    pub fn set_max_active_insurance_compensations(&mut self, max_active: u32) {
        self.assert_admin();
        require!(
            max_active > 0 && max_active <= HARD_MAX_ACTIVE_INSURANCE_COMPENSATIONS,
            format!(
                "ERR_INVALID_MAX_INSURANCE_COMPENSATIONS: max_active ({}) must be in 1..={}",
                max_active, HARD_MAX_ACTIVE_INSURANCE_COMPENSATIONS
            )
        );
        self.flush_storage();
        let remaining = max_active.saturating_sub(self.active_insurance_compensation_count) as u128;
        let per_record_budget = (RECORD_STORAGE_MARGIN_BYTES as u128)
            .checked_mul(env::storage_byte_cost().as_yoctonear())
            .expect("ERR_STORAGE_BUDGET_OVERFLOW: insurance storage budget overflow");
        let required_total = self
            .compute_required_backing()
            .checked_add(
                remaining
                    .checked_mul(per_record_budget)
                    .expect("ERR_STORAGE_BUDGET_OVERFLOW"),
            )
            .expect("ERR_SOLVENCY_OVERFLOW");
        let spendable = compute_spendable_balance();
        require!(
            spendable >= required_total,
            format!(
                "ERR_INSUFFICIENT_STORAGE_RESERVE_FOR_INSURANCE_CAP: spendable ({} yocto) < required_backing + {} insurance slot budget ({} yocto)",
                spendable, remaining, required_total
            )
        );

        let previous = self.max_active_insurance_compensations;
        self.max_active_insurance_compensations = max_active;
        self.flush_storage();
        self.assert_solvency_invariant();
        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"max_active_insurance_compensations_updated\",\"previous\":{},\"new_max\":{}}}",
            previous, max_active
        ));
    }

    /// Explicitly fund the dedicated insurance reserve pool (separate from refund reserve & operating reserve).
    #[payable]
    pub fn fund_insurance_reserve(&mut self) {
        let deposit = env::attached_deposit().as_yoctonear();
        require!(
            deposit > 0,
            "ERR_ZERO_INSURANCE_DEPOSIT: attached deposit must be strictly positive"
        );
        self.insurance_reserve_balance = self
            .insurance_reserve_balance
            .checked_add(deposit)
            .expect("ERR_INSURANCE_OVERFLOW: insurance_reserve_balance overflow");

        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"insurance_reserve_funded\",\"funder\":\"{}\",\"amount\":\"{}\",\"new_insurance_reserve\":\"{}\"}}",
            env::predecessor_account_id(), deposit, self.insurance_reserve_balance
        ));
    }

    /// Issue an explicit, separately funded discretionary insurance compensation for a `Disputed` claim
    /// whose original NEAR transfer outcome could not be verified on-chain.
    ///
    /// ## F-02 & Section 3 Hardening
    /// - Enforces `active_insurance_compensation_count < max_active_insurance_compensations`.
    /// - Decreases `insurance_reserve_balance` by `amount_yocto` and increases
    ///   `total_in_flight_insurance_liability` by `amount_yocto`.
    /// - Flushes `insurance_compensations`, `claims`, and `pending_refunds` before verifying
    ///   `assert_solvency_invariant_with_outbound_transfer(amount_yocto)`.
    pub fn compensate_disputed_claim(&mut self, claim_id: u64, evidence_sha256: String) -> Promise {
        self.assert_admin();
        require!(
            self.insurance_enabled,
            "ERR_INSURANCE_DISABLED: discretionary insurance compensation is disabled"
        );
        Self::assert_valid_sha256_hex(&evidence_sha256, "evidence_sha256");

        require!(
            self.active_insurance_compensation_count < self.max_active_insurance_compensations,
            format!(
                "ERR_INSURANCE_COMPENSATION_LIMIT_REACHED: active insurance compensations ({}) >= max_active_insurance_compensations ({})",
                self.active_insurance_compensation_count, self.max_active_insurance_compensations
            )
        );

        self.flush_storage();
        self.assert_solvency_invariant();
        self.assert_storage_margin_for_new_record();

        let (claimant, amount, originating_nonce, disputed_at) = {
            let claim = match self.claims.get(&claim_id) {
                Some(c) => c,
                None => env::panic_str(&format!("ERR_CLAIM_NOT_FOUND: claim_id {}", claim_id)),
            };
            require!(
                claim.status == ClaimStatus::Disputed,
                format!(
                    "ERR_CLAIM_NOT_DISPUTED: claim {} is {:?}; only Disputed claims are eligible for insurance compensation",
                    claim_id, claim.status
                )
            );
            let disputed_block = claim
                .disputed_at_block
                .expect("CRITICAL_ACCOUNTING_ERROR: Disputed claim missing disputed_at_block");
            (
                claim.claimant.clone(),
                claim.amount,
                claim.originating_nonce,
                disputed_block,
            )
        };

        let now_block = env::block_height();
        let unlock_block = disputed_at.saturating_add(DISPUTE_RESOLUTION_DELAY_BLOCKS);
        require!(
            now_block >= unlock_block,
            format!(
                "ERR_DISPUTE_TIMELOCK_ACTIVE: insurance compensation requires {} more blocks (now {}, unlock {})",
                unlock_block - now_block, now_block, unlock_block
            )
        );

        let amount_yocto = amount.as_yoctonear();
        require!(
            self.insurance_reserve_balance >= amount_yocto,
            format!(
                "ERR_INSUFFICIENT_INSURANCE_RESERVE: insurance_reserve_balance ({}) < compensation amount ({})",
                self.insurance_reserve_balance, amount_yocto
            )
        );

        let compensation_id = self.next_compensation_id;
        self.next_compensation_id = self
            .next_compensation_id
            .checked_add(1)
            .expect("ERR_COMPENSATION_ID_OVERFLOW: next_compensation_id overflow");

        // Move committed amount from insurance_reserve_balance into total_in_flight_insurance_liability
        self.insurance_reserve_balance = self
            .insurance_reserve_balance
            .checked_sub(amount_yocto)
            .expect("ERR_INSURANCE_UNDERFLOW: insurance_reserve_balance underflow");
        self.total_in_flight_insurance_liability = self
            .total_in_flight_insurance_liability
            .checked_add(amount_yocto)
            .expect("ERR_INSURANCE_OVERFLOW: total_in_flight_insurance_liability overflow");

        self.close_settled_stuck_claim(claim_id, &claimant, amount_yocto, true);

        let comp = InsuranceCompensation {
            compensation_id,
            originating_claim_id: claim_id,
            originating_nonce,
            recipient: claimant.clone(),
            amount,
            initiated_at_sec: env::block_timestamp_ms() / 1000,
            initiated_at_block: now_block,
            disputed_at_block: None,
            evidence_sha256: evidence_sha256.clone(),
            status: InsuranceCompensationStatus::InFlight,
        };
        self.insurance_compensations.insert(compensation_id, comp);
        self.active_insurance_compensation_count = self
            .active_insurance_compensation_count
            .checked_add(1)
            .expect("ERR_INSURANCE_COUNT_OVERFLOW: active_insurance_compensation_count overflow");

        // Flush all modified collections before asserting post-flush solvency with outbound transfer
        self.flush_storage();
        self.assert_solvency_invariant_with_outbound_transfer(amount_yocto);

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"insurance_compensation_started\",\"compensation_id\":{},\"claim_id\":{},\"recipient\":\"{}\",\"amount\":\"{}\",\"remaining_insurance_reserve\":\"{}\",\"in_flight_insurance_liability\":\"{}\",\"evidence_sha256\":\"{}\",\"warning\":\"discretionary insurance compensation; original transfer outcome unverified\"}}",
            compensation_id, claim_id, claimant, amount_yocto, self.insurance_reserve_balance, self.total_in_flight_insurance_liability, evidence_sha256
        ));

        Promise::new(claimant.clone()).transfer(amount).then(
            Self::ext(env::current_account_id())
                .with_static_gas(REFUND_CALLBACK_GAS)
                .on_insurance_transfer(compensation_id, claimant, amount),
        )
    }

    /// Private self-callback finalizing an insurance compensation transfer.
    ///
    /// Accepts `InFlight` OR `Disputed` (so a late objective callback resolves a compensation
    /// even if it was previously marked `Disputed` after a callback timeout).
    #[private]
    pub fn on_insurance_transfer(
        &mut self,
        compensation_id: u64,
        recipient: AccountId,
        amount: NearToken,
    ) -> bool {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "Method is private: callback must be invoked by contract itself"
        );

        let (claim_id, was_disputed) = {
            let comp = match self.insurance_compensations.get(&compensation_id) {
                Some(c) => c,
                None => env::panic_str(&format!(
                    "CRITICAL_ACCOUNTING_ERROR: missing insurance compensation record {}",
                    compensation_id
                )),
            };
            require!(
                comp.recipient == recipient,
                "CRITICAL_ACCOUNTING_ERROR: recipient mismatch"
            );
            require!(
                comp.amount == amount,
                "CRITICAL_ACCOUNTING_ERROR: amount mismatch"
            );
            require!(
                comp.status == InsuranceCompensationStatus::InFlight
                    || comp.status == InsuranceCompensationStatus::Disputed,
                format!(
                    "CRITICAL_ACCOUNTING_ERROR: compensation {} not InFlight or Disputed (status: {:?})",
                    compensation_id, comp.status
                )
            );
            (
                comp.originating_claim_id,
                comp.status == InsuranceCompensationStatus::Disputed,
            )
        };

        let succeeded = matches!(env::promise_result_checked(0, 64), Ok(_));
        let amount_yocto = amount.as_yoctonear();

        if was_disputed {
            self.total_unresolved_insurance_liability = self
                .total_unresolved_insurance_liability
                .checked_sub(amount_yocto)
                .expect("ERR_UNDERFLOW: total_unresolved_insurance_liability underflow");
        } else {
            self.total_in_flight_insurance_liability = self
                .total_in_flight_insurance_liability
                .checked_sub(amount_yocto)
                .expect("ERR_UNDERFLOW: total_in_flight_insurance_liability underflow");
        }

        if succeeded {
            self.insurance_compensations.remove(&compensation_id);
            self.active_insurance_compensation_count =
                self.active_insurance_compensation_count.saturating_sub(1);
            self.flush_storage();
            self.assert_solvency_invariant();
            env::log_str(&format!(
                "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"insurance_compensation_paid\",\"compensation_id\":{},\"claim_id\":{},\"recipient\":\"{}\",\"amount\":\"{}\"}}",
                compensation_id, claim_id, recipient, amount_yocto
            ));
            true
        } else {
            // Note: System-refund receipt for the failed outbound insurance transfer arrives asynchronously;
            // `reconcile_returned_insurance` enforces `self.assert_solvency_invariant()` after storage flush
            // before returning funds from `insurance_return_pending_balance` to `insurance_reserve_balance`.
            if let Some(comp) = self.insurance_compensations.get_mut(&compensation_id) {
                comp.status = InsuranceCompensationStatus::ReturnPending;
                comp.disputed_at_block = None;
            }
            self.insurance_return_pending_balance = self
                .insurance_return_pending_balance
                .checked_add(amount_yocto)
                .expect("ERR_OVERFLOW: insurance_return_pending_balance overflow");
            self.flush_storage();
            env::log_str(&format!(
                "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"insurance_compensation_return_pending\",\"compensation_id\":{},\"recipient\":\"{}\",\"amount\":\"{}\"}}",
                compensation_id, recipient, amount_yocto
            ));
            false
        }
    }

    /// Reconcile a failed insurance payout back into `insurance_reserve_balance` once the system refund arrives.
    pub fn reconcile_returned_insurance(&mut self, compensation_id: u64) {
        let comp = match self.insurance_compensations.get(&compensation_id) {
            Some(c) => c.clone(),
            None => env::panic_str(&format!(
                "ERR_COMPENSATION_NOT_FOUND: compensation_id {}",
                compensation_id
            )),
        };
        require!(
            comp.status == InsuranceCompensationStatus::ReturnPending,
            "ERR_COMPENSATION_NOT_RETURN_PENDING"
        );

        self.flush_storage();
        self.assert_solvency_invariant();

        let amount_yocto = comp.amount.as_yoctonear();
        self.insurance_return_pending_balance = self
            .insurance_return_pending_balance
            .checked_sub(amount_yocto)
            .expect("ERR_UNDERFLOW: insurance_return_pending_balance underflow");
        self.insurance_reserve_balance = self
            .insurance_reserve_balance
            .checked_add(amount_yocto)
            .expect("ERR_OVERFLOW: insurance_reserve_balance overflow");
        self.insurance_compensations.remove(&compensation_id);
        self.active_insurance_compensation_count =
            self.active_insurance_compensation_count.saturating_sub(1);

        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"insurance_return_reconciled\",\"compensation_id\":{},\"amount\":\"{}\",\"new_insurance_reserve\":\"{}\"}}",
            compensation_id, amount_yocto, self.insurance_reserve_balance
        ));
    }

    /// Mark an `InFlight` insurance compensation as `Disputed` (unresolved) if its callback has not
    /// completed after `STUCK_CLAIM_MIN_AGE_BLOCKS` blocks.
    ///
    /// Does NOT return funds to `insurance_reserve_balance` and does NOT issue a second payment.
    /// Moves the commitment from `total_in_flight_insurance_liability` to `total_unresolved_insurance_liability`.
    pub fn mark_insurance_compensation_disputed(&mut self, compensation_id: u64) {
        let now_block = env::block_height();
        let caller = env::predecessor_account_id();

        let (recipient, amount_yocto) = {
            let comp = match self.insurance_compensations.get_mut(&compensation_id) {
                Some(c) => c,
                None => env::panic_str(&format!(
                    "ERR_COMPENSATION_NOT_FOUND: compensation_id {}",
                    compensation_id
                )),
            };
            require!(
                caller == self.admin || caller == comp.recipient,
                "ERR_UNAUTHORIZED: only admin or recipient may mark an insurance compensation disputed"
            );
            require!(
                comp.status == InsuranceCompensationStatus::InFlight,
                format!(
                    "ERR_COMPENSATION_NOT_IN_FLIGHT: compensation {} is {:?}",
                    compensation_id, comp.status
                )
            );
            let min_dispute_block = comp
                .initiated_at_block
                .saturating_add(STUCK_CLAIM_MIN_AGE_BLOCKS);
            require!(
                now_block >= min_dispute_block,
                format!(
                    "ERR_COMPENSATION_TOO_RECENT: allow {} more blocks for callback before disputing (now {}, unlock {})",
                    min_dispute_block - now_block, now_block, min_dispute_block
                )
            );

            comp.status = InsuranceCompensationStatus::Disputed;
            comp.disputed_at_block = Some(now_block);
            (comp.recipient.clone(), comp.amount.as_yoctonear())
        };

        self.total_in_flight_insurance_liability = self
            .total_in_flight_insurance_liability
            .checked_sub(amount_yocto)
            .expect("ERR_UNDERFLOW: total_in_flight_insurance_liability underflow");
        self.total_unresolved_insurance_liability = self
            .total_unresolved_insurance_liability
            .checked_add(amount_yocto)
            .expect("ERR_OVERFLOW: total_unresolved_insurance_liability overflow");

        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"insurance_compensation_disputed\",\"compensation_id\":{},\"recipient\":\"{}\",\"amount\":\"{}\",\"disputed_at_block\":{},\"note\":\"Insurance callback missing; held unresolved without reserve restoration or second payout\"}}",
            compensation_id, recipient, amount_yocto, now_block
        ));
    }

    /// Recipient acknowledgement that an `InFlight` or `Disputed` insurance compensation transfer
    /// was received. Closes the unresolved/in-flight insurance liability and removes the terminal
    /// compensation record with zero additional payout.
    pub fn acknowledge_insurance_compensation_received(&mut self, compensation_id: u64) {
        let caller = env::predecessor_account_id();
        let comp = match self.insurance_compensations.get(&compensation_id) {
            Some(c) => c.clone(),
            None => env::panic_str(&format!(
                "ERR_COMPENSATION_NOT_FOUND: compensation_id {}",
                compensation_id
            )),
        };
        require!(
            caller == comp.recipient,
            "ERR_UNAUTHORIZED: only the compensation recipient can acknowledge receipt"
        );
        require!(
            comp.status == InsuranceCompensationStatus::InFlight
                || comp.status == InsuranceCompensationStatus::Disputed,
            format!(
                "ERR_INVALID_COMPENSATION_STATUS: compensation {} has status {:?}",
                compensation_id, comp.status
            )
        );

        let amount_yocto = comp.amount.as_yoctonear();
        if comp.status == InsuranceCompensationStatus::Disputed {
            self.total_unresolved_insurance_liability = self
                .total_unresolved_insurance_liability
                .checked_sub(amount_yocto)
                .expect("ERR_UNDERFLOW: total_unresolved_insurance_liability underflow");
        } else {
            self.total_in_flight_insurance_liability = self
                .total_in_flight_insurance_liability
                .checked_sub(amount_yocto)
                .expect("ERR_UNDERFLOW: total_in_flight_insurance_liability underflow");
        }

        self.insurance_compensations.remove(&compensation_id);
        self.active_insurance_compensation_count =
            self.active_insurance_compensation_count.saturating_sub(1);
        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"insurance_compensation_acknowledged\",\"compensation_id\":{},\"claim_id\":{},\"recipient\":\"{}\",\"amount\":\"{}\"}}",
            compensation_id, comp.originating_claim_id, comp.recipient, amount_yocto
        ));
    }
}

/// Internal helpers for refund credit accounting, storage flushing, and formal solvency invariant enforcement.
impl RoastakeOracleAdapter {
    /// Explicitly flush all 7 cached `near_sdk::store` collections (out of 8 persistent collections
    /// total on `RoastakeOracleAdapter`) to the NEAR storage trie so subsequent calls to
    /// `env::storage_usage()` reflect the exact persisted storage footprint.
    /// (The 8th collection, `authorized_requesters: LookupSet`, is uncached in `near-sdk` and writes
    /// directly to storage on `insert`/`remove`.)
    pub(crate) fn flush_storage(&mut self) {
        self.markets.flush();
        self.prices.flush();
        self.pending_requests.flush();
        self.unreconciled_failures.flush();
        self.pending_refunds.flush();
        self.claims.flush();
        self.insurance_compensations.flush();
    }

    /// Compute the total number of open request capacity slots currently occupied.
    /// Does not release a slot while associated user refund credits or claims remain unresolved.
    pub(crate) fn compute_open_request_slots(&self) -> u32 {
        let unresolved_refund_slots = self.active_refund_credit_count.max(self.active_claim_count);
        self.pending_request_count
            .checked_add(self.unreconciled_failure_count)
            .and_then(|v| v.checked_add(unresolved_refund_slots))
            .expect("ERR_OPEN_COUNT_OVERFLOW: open request slot calculation overflow")
    }

    pub(crate) fn assert_storage_headroom_bytes(&self, margin_bytes: u64, context: &str) {
        let spendable = compute_spendable_balance();
        let margin_yocto = (margin_bytes as u128)
            .checked_mul(env::storage_byte_cost().as_yoctonear())
            .expect("ERR_STORAGE_BUDGET_OVERFLOW: storage margin overflow");
        let required_with_margin = self
            .compute_required_backing()
            .checked_add(margin_yocto)
            .expect("ERR_SOLVENCY_OVERFLOW: storage margin check overflow");
        require!(
            spendable >= required_with_margin,
            format!(
                "ERR_INSUFFICIENT_STORAGE_RESERVE: spendable balance ({} yocto) < required backing plus {} byte {} ({} yocto)",
                spendable, margin_bytes, context, required_with_margin
            )
        );
    }

    pub(crate) fn assert_storage_margin_for_new_record(&self) {
        self.assert_storage_headroom_bytes(RECORD_STORAGE_MARGIN_BYTES, "record storage margin");
    }

    /// Enforce dedicated `STATE` storage headroom for `init()`, covering the initial Borsh
    /// serialization of root `RoastakeOracleAdapter` state written when `init()` returns.
    pub(crate) fn assert_state_storage_margin(&self) {
        let serialized_len = near_sdk::borsh::to_vec(self)
            .expect("ERR_STATE_SERIALIZE: failed to serialize contract state")
            .len() as u64;
        // NEAR trie stores a 5-byte "STATE" key with ~40 bytes per-entry overhead.
        let estimated_footprint = 45u64.saturating_add(serialized_len);
        let required_margin = STATE_STORAGE_MARGIN_BYTES.max(estimated_footprint);
        self.assert_storage_headroom_bytes(required_margin, "STATE storage margin");
    }

    /// Enforce incremental `STATE` growth margin before/after setters that mutate variable-length
    /// root `STATE` fields (`pending_admin`, `outlayer_contract`) serialized after method return.
    pub(crate) fn assert_state_growth_margin(&self, exact_growth_bytes: u64) {
        let required_margin = STATE_GROWTH_MARGIN_BYTES.max(exact_growth_bytes);
        self.assert_storage_headroom_bytes(required_margin, "STATE growth margin");
    }

    fn refresh_credit_status(credit: &mut RefundCredit) -> bool {
        let avail = credit.available_amount.as_yoctonear();
        let inflight = credit.in_flight_amount.as_yoctonear();
        let ret = credit.return_pending_amount.as_yoctonear();
        let disp = credit.disputed_amount.as_yoctonear();
        if avail == 0 && inflight == 0 && ret == 0 && disp == 0 {
            return true;
        }
        if inflight > 0 {
            credit.status = RefundStatus::InFlight;
        } else if ret > 0 {
            credit.status = RefundStatus::ReturnPending;
        } else if avail > 0 {
            credit.status = RefundStatus::Available;
        } else {
            credit.status = RefundStatus::Disputed;
        }
        false
    }

    fn assert_valid_sha256_hex(digest: &str, field_name: &str) {
        require!(
            digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit()),
            format!(
                "ERR_INVALID_EVIDENCE_HASH: {} must be a valid 64-character hexadecimal SHA-256 digest",
                field_name
            )
        );
    }

    fn propose_disputed_claim_settlement_internal(
        &mut self,
        claim_id: u64,
        evidence_sha256: Option<String>,
    ) {
        let now_block = env::block_height();
        let (claimant, amount_yocto) = {
            let claim = match self.claims.get_mut(&claim_id) {
                Some(c) => c,
                None => env::panic_str(&format!("ERR_CLAIM_NOT_FOUND: claim_id {}", claim_id)),
            };
            require!(
                claim.status == ClaimStatus::Disputed,
                format!(
                    "ERR_CLAIM_NOT_DISPUTED: claim {} has status {:?}; ConfirmedSuccess requires Disputed status and appeal delay first",
                    claim_id, claim.status
                )
            );
            require!(
                !claim.admin_settlement_proposed,
                format!(
                    "ERR_SETTLEMENT_ALREADY_PROPOSED: settlement already proposed for claim {}",
                    claim_id
                )
            );
            let disputed_block = claim
                .disputed_at_block
                .expect("CRITICAL_ACCOUNTING_ERROR: Disputed claim missing disputed_at_block");
            let unlock_block = disputed_block.saturating_add(DISPUTE_RESOLUTION_DELAY_BLOCKS);
            require!(
                now_block >= unlock_block,
                format!(
                    "ERR_DISPUTE_TIMELOCK_ACTIVE: ConfirmedSuccess settlement proposal requires {} more blocks (now {}, unlock {})",
                    unlock_block - now_block, now_block, unlock_block
                )
            );

            claim.admin_settlement_proposed = true;
            claim.settlement_proposed_at_block = Some(now_block);
            claim.settlement_evidence_sha256 = evidence_sha256.clone();
            (claim.claimant.clone(), claim.amount.as_yoctonear())
        };

        self.flush_storage();
        self.assert_solvency_invariant();

        let evidence_str = evidence_sha256.unwrap_or_else(|| "none".to_string());
        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"disputed_claim_settlement_proposed\",\"claim_id\":{},\"claimant\":\"{}\",\"amount\":\"{}\",\"proposed_at_block\":{},\"evidence_sha256\":\"{}\",\"note\":\"Admin settlement proposal recorded; entitlement preserved until claimant accepts via acknowledge_disputed_transfer_received\"}}",
            claim_id, claimant, amount_yocto, now_block, evidence_str
        ));
    }

    fn dispute_inflight_claim(&mut self, claim_id: u64) {
        let now_block = env::block_height();
        let (claimant, amount_yocto) = {
            let claim = match self.claims.get_mut(&claim_id) {
                Some(c) => c,
                None => env::panic_str(&format!("ERR_CLAIM_NOT_FOUND: claim_id {}", claim_id)),
            };

            require!(
                claim.status == ClaimStatus::InFlight,
                format!(
                    "ERR_CLAIM_NOT_IN_FLIGHT: claim {} is currently {:?}; only InFlight claims can be marked Disputed",
                    claim_id, claim.status
                )
            );
            let min_dispute_block = claim
                .initiated_at_block
                .saturating_add(STUCK_CLAIM_MIN_AGE_BLOCKS);
            require!(
                now_block >= min_dispute_block,
                format!(
                    "ERR_CLAIM_TOO_RECENT: allow {} more blocks for callback before disputing (now {}, unlock {})",
                    min_dispute_block - now_block, now_block, min_dispute_block
                )
            );

            claim.status = ClaimStatus::Disputed;
            claim.disputed_at_block = Some(now_block);
            (claim.claimant.clone(), claim.amount.as_yoctonear())
        };

        {
            let credit = match self.pending_refunds.get_mut(&claimant) {
                Some(c) => c,
                None => env::panic_str(&format!(
                    "ERR_CREDIT_NOT_FOUND: no credit for claimant {}",
                    claimant
                )),
            };

            require!(
                credit.active_claim_id == Some(claim_id),
                format!(
                    "ERR_CLAIM_ID_MISMATCH: credit active_claim_id ({:?}) does not match claim_id ({})",
                    credit.active_claim_id, claim_id
                )
            );
            require!(
                credit.in_flight_amount.as_yoctonear() >= amount_yocto,
                "ERR_INSUFFICIENT_IN_FLIGHT_AMOUNT"
            );

            let new_in_flight = credit
                .in_flight_amount
                .as_yoctonear()
                .checked_sub(amount_yocto)
                .expect("ERR_UNDERFLOW: in_flight_amount underflow");
            credit.in_flight_amount = NearToken::from_yoctonear(new_in_flight);

            let new_disputed = credit
                .disputed_amount
                .as_yoctonear()
                .checked_add(amount_yocto)
                .expect("ERR_OVERFLOW: disputed_amount overflow");
            credit.disputed_amount = NearToken::from_yoctonear(new_disputed);

            // Clear active_claim_id when moving to Disputed so future independent available credits
            // remain claimable without cross-contaminating this historical disputed claim_id (F-03).
            credit.active_claim_id = None;
            let _ = Self::refresh_credit_status(credit);
        }

        self.total_in_flight_liability = self
            .total_in_flight_liability
            .checked_sub(amount_yocto)
            .expect("ERR_UNDERFLOW: total_in_flight_liability underflow");
        self.total_disputed_liability = self
            .total_disputed_liability
            .checked_add(amount_yocto)
            .expect("ERR_OVERFLOW: total_disputed_liability overflow");

        self.flush_storage();
        self.assert_solvency_invariant();

        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"refund_claim_disputed\",\"claim_id\":{},\"claimant\":\"{}\",\"amount\":\"{}\",\"disputed_at_block\":{},\"note\":\"Transfer outcome unverified; held non-payable\"}}",
            claim_id, claimant, amount_yocto, now_block
        ));
    }

    fn close_settled_stuck_claim(
        &mut self,
        claim_id: u64,
        claimant: &AccountId,
        amount_yocto: u128,
        was_disputed: bool,
    ) {
        let should_remove_credit = {
            let credit = match self.pending_refunds.get_mut(claimant) {
                Some(c) => c,
                None => env::panic_str(&format!(
                    "ERR_CREDIT_NOT_FOUND: no credit for claimant {}",
                    claimant
                )),
            };

            if was_disputed {
                require!(
                    credit.disputed_amount.as_yoctonear() >= amount_yocto
                        && self.total_disputed_liability >= amount_yocto,
                    "ERR_INSUFFICIENT_DISPUTED_AMOUNT"
                );
                let new_disputed = credit
                    .disputed_amount
                    .as_yoctonear()
                    .checked_sub(amount_yocto)
                    .expect("ERR_UNDERFLOW: disputed_amount underflow");
                credit.disputed_amount = NearToken::from_yoctonear(new_disputed);
                self.total_disputed_liability = self
                    .total_disputed_liability
                    .checked_sub(amount_yocto)
                    .expect("ERR_UNDERFLOW: total_disputed_liability underflow");
                if credit.active_claim_id == Some(claim_id) {
                    credit.active_claim_id = None;
                }
            } else {
                require!(
                    credit.active_claim_id == Some(claim_id),
                    format!(
                        "ERR_CLAIM_ID_MISMATCH: credit active_claim_id ({:?}) does not match claim_id ({})",
                        credit.active_claim_id, claim_id
                    )
                );
                require!(
                    credit.in_flight_amount.as_yoctonear() >= amount_yocto
                        && self.total_in_flight_liability >= amount_yocto,
                    "ERR_INSUFFICIENT_IN_FLIGHT_AMOUNT"
                );
                let new_in_flight = credit
                    .in_flight_amount
                    .as_yoctonear()
                    .checked_sub(amount_yocto)
                    .expect("ERR_UNDERFLOW: in_flight_amount underflow");
                credit.in_flight_amount = NearToken::from_yoctonear(new_in_flight);
                self.total_in_flight_liability = self
                    .total_in_flight_liability
                    .checked_sub(amount_yocto)
                    .expect("ERR_UNDERFLOW: total_in_flight_liability underflow");
                credit.active_claim_id = None;
            }

            self.total_refund_liability = self
                .total_refund_liability
                .checked_sub(amount_yocto)
                .expect("ERR_UNDERFLOW: total_refund_liability underflow");

            Self::refresh_credit_status(credit)
        };

        if should_remove_credit {
            self.pending_refunds.remove(claimant);
            self.active_refund_credit_count = self.active_refund_credit_count.saturating_sub(1);
        }

        self.claims.remove(&claim_id);
        self.active_claim_count = self.active_claim_count.saturating_sub(1);
        self.flush_storage();
        self.assert_solvency_invariant();
    }

    pub(crate) fn compute_backed_refund_liability(&self) -> u128 {
        self.total_refund_liability
            .checked_sub(self.total_in_flight_liability)
            .and_then(|v| v.checked_sub(self.total_disputed_liability))
            .expect("ERR_LIABILITY_UNDERFLOW: backed refund liability underflow")
    }

    pub(crate) fn compute_required_backing(&self) -> u128 {
        let backed = self.compute_backed_refund_liability();
        self.minimum_operating_reserve
            .checked_add(self.refund_reserve_balance)
            .and_then(|v| v.checked_add(self.insurance_reserve_balance))
            .and_then(|v| v.checked_add(self.insurance_return_pending_balance))
            .and_then(|v| v.checked_add(backed))
            .expect("ERR_SOLVENCY_OVERFLOW: required backing sum overflow")
    }

    /// Compute the full encumbered backing floor including all in-flight and disputed/unresolved
    /// refund and insurance liabilities. Used by `admin_designate_refund_reserve` so returning
    /// system refunds from failed transfers can never be designated into another reserve before
    /// their callbacks settle.
    pub(crate) fn compute_encumbered_backing_floor(&self) -> u128 {
        self.compute_required_backing()
            .checked_add(self.total_in_flight_liability)
            .and_then(|v| v.checked_add(self.total_disputed_liability))
            .and_then(|v| v.checked_add(self.total_in_flight_insurance_liability))
            .and_then(|v| v.checked_add(self.total_unresolved_insurance_liability))
            .expect("ERR_SOLVENCY_OVERFLOW: encumbered backing floor overflow")
    }

    pub(crate) fn assert_solvency_invariant(&self) {
        let spendable = compute_spendable_balance();
        let required = self.compute_required_backing();
        require!(
            spendable >= required,
            format!(
                "SOLVENCY_INVARIANT_VIOLATION: spendable contract balance ({} yocto) < required backing ({} yocto = minimum_operating_reserve {} + refund_reserve_balance {} + insurance_reserve_balance {} + insurance_return_pending {} + backed_refund_liability {})",
                spendable,
                required,
                self.minimum_operating_reserve,
                self.refund_reserve_balance,
                self.insurance_reserve_balance,
                self.insurance_return_pending_balance,
                self.compute_backed_refund_liability()
            )
        );
    }

    /// Verify solvency after flushing storage and immediately prior to constructing an outbound
    /// `Promise::new(recipient).transfer(outbound_amount_yocto)` (before the host deducts
    /// `outbound_amount_yocto` from `env::account_balance()`).
    pub(crate) fn assert_solvency_invariant_with_outbound_transfer(
        &self,
        outbound_amount_yocto: u128,
    ) {
        let spendable = compute_spendable_balance();
        let required = self
            .compute_required_backing()
            .checked_add(outbound_amount_yocto)
            .expect("ERR_SOLVENCY_OVERFLOW: outbound transfer required backing overflow");
        require!(
            spendable >= required,
            format!(
                "SOLVENCY_INVARIANT_VIOLATION: spendable contract balance ({} yocto) < required backing including outbound transfer ({} yocto = required_backing {} + outbound_transfer {})",
                spendable,
                required,
                self.compute_required_backing(),
                outbound_amount_yocto
            )
        );
    }

    pub(crate) fn record_refund_credit(
        &mut self,
        requester: &AccountId,
        amount: NearToken,
        nonce: u64,
    ) {
        let yocto = amount.as_yoctonear();
        if yocto == 0 {
            return;
        }

        if let Some(existing) = self.pending_refunds.get_mut(requester) {
            let new_available = existing
                .available_amount
                .as_yoctonear()
                .checked_add(yocto)
                .expect("ERR_REFUND_OVERFLOW: available_amount overflow");
            existing.available_amount = NearToken::from_yoctonear(new_available);
            existing.from_nonce = nonce;
            let _ = Self::refresh_credit_status(existing);
        } else {
            self.pending_refunds.insert(
                requester.clone(),
                RefundCredit {
                    requester: requester.clone(),
                    available_amount: amount,
                    in_flight_amount: NearToken::from_yoctonear(0),
                    return_pending_amount: NearToken::from_yoctonear(0),
                    disputed_amount: NearToken::from_yoctonear(0),
                    status: RefundStatus::Available,
                    from_nonce: nonce,
                    active_claim_id: None,
                },
            );
            self.active_refund_credit_count = self
                .active_refund_credit_count
                .checked_add(1)
                .expect("ERR_CREDIT_COUNT_OVERFLOW: active_refund_credit_count overflow");
        }

        self.total_refund_liability = self
            .total_refund_liability
            .checked_add(yocto)
            .expect("ERR_LIABILITY_OVERFLOW: total_refund_liability overflow");
    }
}

/// Compute the contract's maximum transferable unlocked balance while strictly
/// preserving protocol storage staking requirements.
///
/// ## NEAR Runtime Balance Semantics
/// In NEAR Protocol (NEP-0008 / Runtime Economics):
/// - `env::account_balance()` returns the current unlocked liquid balance of the contract account
///   (`account.amount`), including any attached deposit for the current transaction.
/// - `env::account_locked_balance()` returns validator staking balance (`account.locked`), which is
///   completely separate from `account.amount` and cannot be used for storage staking.
///   Therefore, `account_locked_balance()` MUST NOT be subtracted from `account_balance()`.
/// - Storage staking requires: `account.amount >= storage_usage * storage_byte_cost`.
/// - Transferring an amount `T` decreases `account.amount` by `T`.
///
/// Hence, the maximum spendable/transferable balance without triggering a runtime storage panic is:
/// `spendable = account_balance.saturating_sub(storage_usage * storage_byte_cost)`.
pub fn compute_spendable_balance() -> u128 {
    let total_unlocked = env::account_balance().as_yoctonear();
    let storage_locked =
        (env::storage_usage() as u128).saturating_mul(env::storage_byte_cost().as_yoctonear());
    total_unlocked.saturating_sub(storage_locked)
}

/// Alias for backwards compatibility with earlier interface callers.
pub fn compute_available_liquid_balance() -> u128 {
    compute_spendable_balance()
}

/// Pure validation function verifying worker output against market rules,
/// price boundaries, divergence limits, and timestamp monotonic freshness.
///
/// Returns `Ok(WorkerOutput)` if all validations pass, or `Err(String)` describing
/// the exact violation without panicking.
pub fn validate_worker_output(
    data: &[u8],
    market: &str,
    market_cfg: &MarketConfig,
    existing_price: Option<&PriceObservation>,
    now_sec: u64,
) -> Result<WorkerOutput, String> {
    // 1. Extract worker output from raw JSON, string, or OutLayer response wrapper
    let worker_output = extract_worker_output(data)?;

    // 2. Schema version validation
    if worker_output.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "ERR_INVALID_SCHEMA_VERSION: {}",
            worker_output.schema_version
        ));
    }

    // 3. Market symbol validation
    if worker_output.market != market {
        return Err(format!(
            "ERR_WRONG_MARKET: expected {}, got {}",
            market, worker_output.market
        ));
    }

    // 4. Physical unit validation
    if worker_output.unit != market_cfg.unit {
        return Err(format!(
            "ERR_WRONG_UNIT: expected {}, got {}",
            market_cfg.unit, worker_output.unit
        ));
    }

    // 5. Decimal precision validation
    if worker_output.decimals != market_cfg.decimals {
        return Err(format!(
            "ERR_WRONG_DECIMALS: expected {}, got {}",
            market_cfg.decimals, worker_output.decimals
        ));
    }

    // 6. Source metadata & length bounds (UTF-8 byte lengths, L-03 storage safety)
    if worker_output.source_count < 1 {
        return Err("ERR_ZERO_SOURCE_COUNT: source_count must be at least 1".to_string());
    }
    if worker_output.source.len() > MAX_WORKER_SOURCE_BYTES {
        return Err(format!(
            "ERR_SOURCE_TOO_LONG: source UTF-8 byte length ({}) exceeds maximum ({} bytes)",
            worker_output.source.len(),
            MAX_WORKER_SOURCE_BYTES
        ));
    }
    if worker_output.data_class.len() > MAX_WORKER_DATA_CLASS_BYTES {
        return Err(format!(
            "ERR_DATA_CLASS_TOO_LONG: data_class UTF-8 byte length ({}) exceeds maximum ({} bytes)",
            worker_output.data_class.len(),
            MAX_WORKER_DATA_CLASS_BYTES
        ));
    }

    // 7. Price bounds validation
    let price_u128 = worker_output
        .parse_price()
        .map_err(|e| format!("ERR_MALFORMED_PRICE: {e}"))?;

    if price_u128 == 0 {
        return Err("ERR_ZERO_PRICE: price must be greater than zero".to_string());
    }
    if price_u128 < market_cfg.min_price.0 || price_u128 > market_cfg.max_price.0 {
        return Err(format!(
            "ERR_PRICE_OUT_OF_BOUNDS: price {} is outside permitted limits [{}, {}]",
            price_u128, market_cfg.min_price.0, market_cfg.max_price.0
        ));
    }

    // 8. Excessive price movement (divergence) validation
    if let Some(existing) = existing_price {
        if market_cfg.max_divergence_bps > 0 {
            let div_bps = calculate_divergence_bps(existing.price.0, price_u128);
            if div_bps > market_cfg.max_divergence_bps {
                return Err(format!(
                    "ERR_EXCESSIVE_PRICE_DIVERGENCE: price movement of {} bps exceeds max permitted {} bps",
                    div_bps, market_cfg.max_divergence_bps
                ));
            }
        }
    }

    // 9. Timestamp freshness and monotonic validation
    if worker_output.observed_at == 0 {
        if worker_output.data_class != DATA_CLASS_SYNTHETIC {
            return Err("ERR_NON_DEMO_ZERO_TIMESTAMP: observed_at=0 only permitted for synthetic demo fixtures".to_string());
        }
    } else {
        if worker_output.observed_at > now_sec + DEFAULT_MAX_CLOCK_DRIFT_SEC {
            return Err(format!(
                "ERR_FUTURE_TIMESTAMP: observed_at {} is in the future compared to block time {}",
                worker_output.observed_at, now_sec
            ));
        }
        if worker_output.observed_at < now_sec.saturating_sub(market_cfg.max_price_age_sec) {
            return Err(format!(
                "ERR_STALE_TIMESTAMP: observed_at {} is older than max age of {}s (now: {})",
                worker_output.observed_at, market_cfg.max_price_age_sec, now_sec
            ));
        }
        if let Some(existing) = existing_price {
            if existing.observed_at_sec > 0 && worker_output.observed_at < existing.observed_at_sec
            {
                return Err(format!(
                    "ERR_NON_MONOTONIC_TIMESTAMP: observed_at {} is older than existing {}",
                    worker_output.observed_at, existing.observed_at_sec
                ));
            }
        }
    }

    Ok(worker_output)
}

#[near]
impl RoastakeOracleAdapter {
    // -------------------------------------------------------------------------
    // Admin Management Methods
    // -------------------------------------------------------------------------

    fn assert_admin(&self) {
        require!(
            env::predecessor_account_id() == self.admin,
            "Only admin can call this method"
        );
    }

    /// Propose a new administrator account (Step 1 of two-step admin transfer).
    ///
    /// The proposed administrator must explicitly call `accept_admin()` before control transfers.
    pub fn propose_admin(&mut self, new_admin: AccountId) {
        self.assert_admin();
        validate_testnet_account_id(new_admin.as_str())
            .unwrap_or_else(|e| env::panic_str(&format!("new admin validation failed: {e}")));
        require!(
            new_admin != self.admin,
            "ERR_SAME_ADMIN: new_admin is already the active admin"
        );
        let pre_state_len = near_sdk::borsh::to_vec(self)
            .expect("ERR_STATE_SERIALIZE")
            .len() as u64;
        self.pending_admin = Some(new_admin.clone());
        let post_state_len = near_sdk::borsh::to_vec(self)
            .expect("ERR_STATE_SERIALIZE")
            .len() as u64;
        let growth_bytes = post_state_len.saturating_sub(pre_state_len);
        self.flush_storage();
        self.assert_solvency_invariant();
        self.assert_state_growth_margin(growth_bytes);
        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"admin_proposed\",\"current_admin\":\"{}\",\"pending_admin\":\"{}\"}}",
            self.admin, new_admin
        ));
    }

    /// Accept a pending administrator transfer (Step 2 of two-step admin transfer).
    ///
    /// Must be called by the exact `pending_admin` account nominated via `propose_admin`.
    pub fn accept_admin(&mut self) {
        let pending = self.pending_admin.clone().unwrap_or_else(|| {
            env::panic_str("ERR_NO_PENDING_ADMIN: no admin transfer is pending")
        });
        let caller = env::predecessor_account_id();
        require!(
            caller == pending,
            "ERR_ONLY_PENDING_ADMIN_CAN_ACCEPT: caller is not the pending admin"
        );
        let pre_state_len = near_sdk::borsh::to_vec(self)
            .expect("ERR_STATE_SERIALIZE")
            .len() as u64;
        let previous_admin = self.admin.clone();
        self.admin = pending.clone();
        self.pending_admin = None;
        let post_state_len = near_sdk::borsh::to_vec(self)
            .expect("ERR_STATE_SERIALIZE")
            .len() as u64;
        let growth_bytes = post_state_len.saturating_sub(pre_state_len);
        self.flush_storage();
        self.assert_solvency_invariant();
        if growth_bytes > 0 {
            self.assert_storage_headroom_bytes(growth_bytes, "STATE growth margin");
        }
        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"admin_updated\",\"previous_admin\":\"{}\",\"new_admin\":\"{}\"}}",
            previous_admin, pending
        ));
    }

    /// Cancel an unaccepted administrator transfer proposal.
    pub fn cancel_admin_proposal(&mut self) {
        self.assert_admin();
        let cancelled = self.pending_admin.take().unwrap_or_else(|| {
            env::panic_str("ERR_NO_PENDING_ADMIN: no admin transfer is pending")
        });
        self.flush_storage();
        self.assert_solvency_invariant();
        env::log_str(&format!(
            "EVENT_JSON:{{\"standard\":\"roastake-oracle\",\"event\":\"admin_proposal_cancelled\",\"admin\":\"{}\",\"cancelled_pending_admin\":\"{}\"}}",
            self.admin, cancelled
        ));
    }

    /// Disabled single-step admin setter: always rejects mainnet accounts and panics requiring
    /// the two-step `propose_admin` + `accept_admin` flow (F-01 remediation).
    pub fn set_admin(&mut self, new_admin: AccountId) {
        self.assert_admin();
        validate_testnet_account_id(new_admin.as_str())
            .unwrap_or_else(|e| env::panic_str(&format!("new admin validation failed: {e}")));
        env::panic_str(
            "ERR_SINGLE_STEP_SET_ADMIN_DISABLED: use propose_admin(new_admin) followed by accept_admin()",
        );
    }

    pub fn set_outlayer_contract(&mut self, outlayer_contract: AccountId) {
        self.assert_admin();
        validate_testnet_account_id(outlayer_contract.as_str()).unwrap_or_else(|e| {
            env::panic_str(&format!("outlayer contract validation failed: {e}"))
        });
        let pre_state_len = near_sdk::borsh::to_vec(self)
            .expect("ERR_STATE_SERIALIZE")
            .len() as u64;
        self.outlayer_contract = outlayer_contract;
        let post_state_len = near_sdk::borsh::to_vec(self)
            .expect("ERR_STATE_SERIALIZE")
            .len() as u64;
        let growth_bytes = post_state_len.saturating_sub(pre_state_len);
        self.flush_storage();
        self.assert_solvency_invariant();
        self.assert_state_growth_margin(growth_bytes);
    }

    pub fn add_market(&mut self, config: MarketConfig) {
        self.assert_admin();
        config.validate().unwrap_or_else(|e| {
            env::panic_str(&format!("market configuration validation failed: {e}"))
        });
        let is_new = !self.markets.contains_key(&config.symbol);
        if is_new {
            require!(
                self.market_count < HARD_MAX_MARKETS,
                format!(
                    "ERR_MARKET_LIMIT_REACHED: market_count ({}) >= HARD_MAX_MARKETS ({})",
                    self.market_count, HARD_MAX_MARKETS
                )
            );
            self.assert_storage_margin_for_new_record();
        }
        self.markets.insert(config.symbol.clone(), config);
        if is_new {
            self.market_count = self
                .market_count
                .checked_add(1)
                .expect("ERR_MARKET_COUNT_OVERFLOW: market_count overflow");
        }
        self.flush_storage();
        self.assert_solvency_invariant();
    }

    pub fn update_market_limits(
        &mut self,
        market: String,
        min_price: U128,
        max_price: U128,
        max_price_age_sec: u64,
        max_divergence_bps: u32,
    ) {
        self.assert_admin();
        require!(
            min_price.0 < max_price.0,
            "min_price must be less than max_price"
        );
        require!(min_price.0 > 0, "min_price must be greater than zero");

        let mut config = self
            .markets
            .get(&market)
            .cloned()
            .unwrap_or_else(|| env::panic_str(&format!("Market '{}' not found", market)));

        config.min_price = min_price;
        config.max_price = max_price;
        config.max_price_age_sec = max_price_age_sec;
        config.max_divergence_bps = max_divergence_bps;

        self.markets.insert(market, config);
        self.flush_storage();
        self.assert_solvency_invariant();
    }

    pub fn update_market_wasm(
        &mut self,
        market: String,
        wasm_url: String,
        wasm_sha256: String,
        build_target: String,
    ) {
        self.assert_admin();

        let mut config = self
            .markets
            .get(&market)
            .cloned()
            .unwrap_or_else(|| env::panic_str(&format!("Market '{}' not found", market)));

        config.wasm_url = wasm_url;
        config.wasm_sha256 = wasm_sha256;
        config.build_target = build_target;
        config.validate().unwrap_or_else(|e| {
            env::panic_str(&format!("market configuration validation failed: {e}"))
        });

        self.markets.insert(market, config);
        self.flush_storage();
        self.assert_solvency_invariant();
    }

    pub fn set_market_active(&mut self, market: String, active: bool) {
        self.assert_admin();
        let mut config = self
            .markets
            .get(&market)
            .cloned()
            .unwrap_or_else(|| env::panic_str(&format!("Market '{}' not found", market)));

        config.active = active;
        self.markets.insert(market, config);
        self.flush_storage();
        self.assert_solvency_invariant();
    }

    pub fn add_requester(&mut self, account_id: AccountId) {
        self.assert_admin();
        validate_testnet_account_id(account_id.as_str())
            .unwrap_or_else(|e| env::panic_str(&format!("requester validation failed: {e}")));
        if !self.authorized_requesters.contains(&account_id) {
            require!(
                self.authorized_requester_count < HARD_MAX_AUTHORIZED_REQUESTERS,
                format!(
                    "ERR_REQUESTER_LIMIT_REACHED: authorized_requester_count ({}) >= HARD_MAX_AUTHORIZED_REQUESTERS ({})",
                    self.authorized_requester_count, HARD_MAX_AUTHORIZED_REQUESTERS
                )
            );
            self.assert_storage_margin_for_new_record();
            self.authorized_requesters.insert(account_id);
            self.authorized_requester_count = self
                .authorized_requester_count
                .checked_add(1)
                .expect("ERR_REQUESTER_COUNT_OVERFLOW: authorized_requester_count overflow");
        }
        self.flush_storage();
        self.assert_solvency_invariant();
    }

    pub fn remove_requester(&mut self, account_id: AccountId) {
        self.assert_admin();
        if self.authorized_requesters.remove(&account_id) {
            self.authorized_requester_count = self.authorized_requester_count.saturating_sub(1);
        }
        self.flush_storage();
        self.assert_solvency_invariant();
    }

    // -------------------------------------------------------------------------
    // View Methods
    // -------------------------------------------------------------------------

    pub fn get_price(&self, market: String) -> Option<PriceObservation> {
        self.prices.get(&market).cloned()
    }

    pub fn get_market_config(&self, market: String) -> Option<MarketConfig> {
        self.markets.get(&market).cloned()
    }

    pub fn list_markets(&self) -> Vec<String> {
        self.markets.keys().cloned().collect()
    }

    pub fn get_admin(&self) -> AccountId {
        self.admin.clone()
    }

    pub fn get_pending_admin(&self) -> Option<AccountId> {
        self.pending_admin.clone()
    }

    pub fn get_outlayer_contract(&self) -> AccountId {
        self.outlayer_contract.clone()
    }

    pub fn is_requester(&self, account_id: AccountId) -> bool {
        account_id == self.admin || self.authorized_requesters.contains(&account_id)
    }

    pub fn get_pending_request(&self, nonce: u64) -> Option<PendingRequest> {
        self.pending_requests.get(&nonce).cloned()
    }

    pub fn get_unreconciled_failure(&self, nonce: u64) -> Option<UnreconciledFailure> {
        self.unreconciled_failures.get(&nonce).cloned()
    }

    pub fn get_next_nonce(&self) -> u64 {
        self.next_nonce
    }

    pub fn get_pending_refund(&self, account_id: AccountId) -> Option<RefundCredit> {
        self.pending_refunds.get(&account_id).cloned()
    }

    pub fn get_total_refund_liability(&self) -> U128 {
        U128(self.total_refund_liability)
    }

    pub fn get_refund_reserve_balance(&self) -> U128 {
        U128(self.refund_reserve_balance)
    }

    pub fn get_minimum_operating_reserve(&self) -> U128 {
        U128(self.minimum_operating_reserve)
    }

    pub fn get_required_backing_balance(&self) -> U128 {
        U128(self.compute_required_backing())
    }

    pub fn verify_solvency_invariant(&self) -> bool {
        compute_spendable_balance() >= self.compute_required_backing()
    }

    pub fn get_available_liquid_balance(&self) -> U128 {
        U128(compute_available_liquid_balance())
    }

    pub fn get_spendable_balance(&self) -> U128 {
        U128(compute_spendable_balance())
    }

    pub fn get_claim(&self, claim_id: u64) -> Option<RefundClaim> {
        self.claims.get(&claim_id).cloned()
    }

    pub fn get_next_claim_id(&self) -> u64 {
        self.next_claim_id
    }

    pub fn get_insurance_compensation(
        &self,
        compensation_id: u64,
    ) -> Option<InsuranceCompensation> {
        self.insurance_compensations.get(&compensation_id).cloned()
    }

    pub fn get_insurance_reserve_balance(&self) -> U128 {
        U128(self.insurance_reserve_balance)
    }

    pub fn get_insurance_in_flight_liability(&self) -> U128 {
        U128(self.total_in_flight_insurance_liability)
    }

    pub fn get_insurance_unresolved_liability(&self) -> U128 {
        U128(self.total_unresolved_insurance_liability)
    }

    pub fn is_insurance_enabled(&self) -> bool {
        self.insurance_enabled
    }

    pub fn get_open_request_counts(&self) -> (u32, u32, u32) {
        (
            self.pending_request_count,
            self.unreconciled_failure_count,
            self.max_open_requests,
        )
    }

    pub fn get_backed_refund_liability(&self) -> U128 {
        U128(self.compute_backed_refund_liability())
    }

    pub fn get_accounting_snapshot(&self) -> AccountingSnapshot {
        let account_balance = env::account_balance().as_yoctonear();
        let storage_usage_bytes = env::storage_usage();
        let storage_locked =
            (storage_usage_bytes as u128).saturating_mul(env::storage_byte_cost().as_yoctonear());
        let spendable = account_balance.saturating_sub(storage_locked);
        let backed = self.compute_backed_refund_liability();
        let required = self.compute_required_backing();
        let encumbered_floor = self.compute_encumbered_backing_floor();
        AccountingSnapshot {
            account_balance: U128(account_balance),
            storage_usage_bytes,
            storage_locked_balance: U128(storage_locked),
            spendable_balance: U128(spendable),
            minimum_operating_reserve: U128(self.minimum_operating_reserve),
            refund_reserve_balance: U128(self.refund_reserve_balance),
            insurance_reserve_balance: U128(self.insurance_reserve_balance),
            insurance_in_flight_liability: U128(self.total_in_flight_insurance_liability),
            insurance_return_pending_liability: U128(self.insurance_return_pending_balance),
            insurance_unresolved_liability: U128(self.total_unresolved_insurance_liability),
            insurance_enabled: self.insurance_enabled,
            total_refund_liability: U128(self.total_refund_liability),
            in_flight_liability: U128(self.total_in_flight_liability),
            disputed_liability: U128(self.total_disputed_liability),
            backed_refund_liability: U128(backed),
            required_backing: U128(required),
            encumbered_backing_floor: U128(encumbered_floor),
            unallocated_surplus: U128(spendable.saturating_sub(required)),
            invariant_holds: spendable >= required,
            pending_request_count: self.pending_request_count,
            unreconciled_failure_count: self.unreconciled_failure_count,
            active_refund_credit_count: self.active_refund_credit_count,
            active_claim_count: self.active_claim_count,
            open_request_slots: self.compute_open_request_slots(),
            max_open_requests: self.max_open_requests,
            active_insurance_compensation_count: self.active_insurance_compensation_count,
            max_active_insurance_compensations: self.max_active_insurance_compensations,
            market_count: self.market_count,
            authorized_requester_count: self.authorized_requester_count,
            next_claim_id: self.next_claim_id,
            next_compensation_id: self.next_compensation_id,
        }
    }

    /// Alias for `get_accounting_snapshot` used by solvency/storage verification suites.
    pub fn get_solvency_snapshot(&self) -> AccountingSnapshot {
        self.get_accounting_snapshot()
    }

    pub fn estimate_execution_cost(
        &self,
        resource_limits: Option<ResourceLimits>,
        input_len: Option<u64>,
    ) -> NearToken {
        let limits = resource_limits.unwrap_or_default();
        let len = input_len.unwrap_or(120);
        calculate_execution_cost(&limits, len)
    }
}
