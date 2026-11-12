//! State and data transfer types for the ROASTAKE Oracle Adapter.

use near_sdk::json_types::U128;
use near_sdk::{near, AccountId, NearToken};
use roastake_shared::{
    validate_testnet_account_id, SecretsReference, WorkerInput, DECIMALS_COFFEE_INDEX,
    DEFAULT_LIVE_INDEX_MAX_PRICE_AGE_SEC, DEFAULT_MAX_PRICE_AGE_SEC,
    HISTORICAL_MONTHLY_MAX_PRICE_AGE_SEC, IMF_PCPS_ATTRIBUTION_NOTICE, MARKET_ARABICA_MONTHLY_IMF,
    MARKET_GESHA_DEMO, MODE_FIXTURE, MODE_LIVE_INDEX, MODE_PUBLIC_BENCHMARK, UNIT_USD_PER_KG,
};

/// Validate a SHA-256 WASM digest: exact 64 ASCII hex chars and never all-zero placeholder.
pub fn validate_wasm_sha256(hash: &str) -> Result<(), String> {
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("wasm_sha256 must be a valid 64-character hex string".to_string());
    }
    if hash.bytes().all(|b| b == b'0') {
        return Err("wasm_sha256 cannot be an all-zero placeholder (0000...0000)".to_string());
    }
    Ok(())
}

/// Maximum UTF-8 byte length for a market symbol (e.g., "GESHA-DEMO/USD").
pub const MAX_MARKET_SYMBOL_BYTES: usize = 64;

/// Maximum UTF-8 byte length for a human-readable market description.
pub const MAX_MARKET_DESCRIPTION_BYTES: usize = 256;

/// Maximum UTF-8 byte length for a physical unit string (e.g., "USD/kg").
pub const MAX_MARKET_UNIT_BYTES: usize = 16;

/// Maximum UTF-8 byte length for a pinned worker WASM HTTPS URL.
pub const MAX_WASM_URL_BYTES: usize = 256;

/// Maximum UTF-8 byte length for an OutLayer WASI build target identifier (e.g., "wasm32-wasip2").
pub const MAX_BUILD_TARGET_BYTES: usize = 32;

/// Maximum UTF-8 byte length for `MarketConfig.worker_mode`.
pub const MAX_WORKER_MODE_BYTES: usize = 32;

/// Maximum UTF-8 byte length for `MarketSecretsRef.profile`.
pub const MAX_SECRETS_PROFILE_BYTES: usize = 64;

/// Maximum UTF-8 byte length for `MarketSecretsRef.account_id`.
pub const MAX_SECRETS_ACCOUNT_ID_BYTES: usize = 64;

/// Hard upper bound (100% = 10,000 bps) on `max_source_spread_bps`.
pub const MAX_SOURCE_SPREAD_BPS: u32 = 10_000;

/// Maximum UTF-8 byte length for `WorkerOutput.source` / `PriceObservation.source`.
/// Bounded so a newly inserted `PriceObservation` in `IterableMap<String, PriceObservation>`
/// (`keys` entry <= 114 bytes + `values` entry <= 379 bytes = <= 493 bytes worst-case) never
/// exceeds `OPEN_REQUEST_STORAGE_BUDGET_BYTES` (512 bytes).
pub const MAX_WORKER_SOURCE_BYTES: usize = roastake_shared::MAX_WORKER_SOURCE_BYTES;

/// Maximum UTF-8 byte length for `WorkerOutput.data_class` / `PriceObservation.data_class`.
pub const MAX_WORKER_DATA_CLASS_BYTES: usize = roastake_shared::MAX_WORKER_DATA_CLASS_BYTES;

/// Reference to encrypted OutLayer TEE credentials stored on-chain (contains no secret values).
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketSecretsRef {
    /// OutLayer secrets profile name (e.g., "prod")
    pub profile: String,
    /// Owner Testnet account holding the encrypted OutLayer secrets profile
    pub account_id: String,
}

impl MarketSecretsRef {
    /// Validate secrets reference bounds and strictly enforce Testnet + OutLayer profile rules (`[A-Za-z0-9_-]{1,64}`).
    pub fn validate(&self) -> Result<(), String> {
        if self.profile.is_empty() || self.profile.len() > MAX_SECRETS_PROFILE_BYTES {
            return Err(format!(
                "secrets_ref.profile must be 1..={} bytes of ASCII letters, digits, '-' or '_'",
                MAX_SECRETS_PROFILE_BYTES
            ));
        }
        if let Some(bad) = self
            .profile
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_'))
        {
            return Err(format!(
                "secrets_ref.profile must contain only ASCII alphanumeric characters, '-' or '_' (invalid char {:?})",
                bad
            ));
        }
        if self.account_id.trim().is_empty() || self.account_id.len() > MAX_SECRETS_ACCOUNT_ID_BYTES
        {
            return Err(format!(
                "secrets_ref.account_id must be 1..={} UTF-8 bytes",
                MAX_SECRETS_ACCOUNT_ID_BYTES
            ));
        }
        self.account_id
            .parse::<AccountId>()
            .map_err(|e| format!("secrets_ref.account_id is not a valid NEAR account ID: {e}"))?;
        validate_testnet_account_id(&self.account_id)?;
        Ok(())
    }

    /// Convert to OutLayer's `SecretsReference` ABI type for `request_execution`.
    pub fn to_outlayer_secrets_ref(&self) -> SecretsReference {
        SecretsReference {
            profile: self.profile.clone(),
            account_id: self.account_id.clone(),
        }
    }
}

/// Configuration for a supported coffee market index.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketConfig {
    /// Market symbol (e.g. "GESHA-DEMO/USD")
    pub symbol: String,
    /// Human-readable description
    pub description: String,
    /// Fixed-point decimal precision (standard is 8)
    pub decimals: u8,
    /// Physical unit of measure (standard is "USD/kg")
    pub unit: String,
    /// Minimum allowed price in fixed-point integer (bounds check)
    pub min_price: U128,
    /// Maximum allowed price in fixed-point integer (bounds check)
    pub max_price: U128,
    /// Maximum age in seconds before an observation is considered stale
    pub max_price_age_sec: u64,
    /// Maximum allowed divergence in basis points from previous price (e.g. 2000 = 20%)
    pub max_divergence_bps: u32,
    /// HTTPS URL for pinned WASM worker artifact
    pub wasm_url: String,
    /// Expected SHA-256 hex hash of the pinned WASM worker
    pub wasm_sha256: String,
    /// OutLayer supported WASI build target (e.g. "wasm32-wasip2")
    pub build_target: String,
    /// Whether the market is currently active for updates
    pub active: bool,
    /// Execution routing mode ("fixture" for GESHA demo, "live_index" for real markets)
    pub worker_mode: String,
    /// Optional OutLayer encrypted secrets profile reference (None for fixture markets)
    pub secrets_ref: Option<MarketSecretsRef>,
    /// Minimum participating sources required by on-chain validation and live Worker policy
    pub min_source_count: u32,
    /// Explicit permission for `SYNTHETIC_DEMO` data class (permitted ONLY on GESHA-DEMO/USD fixture)
    pub allow_synthetic: bool,
    /// Maximum participating-source dispersion policy forwarded to live Worker (in bps)
    pub max_source_spread_bps: u32,
}

impl MarketConfig {
    /// Create the default configuration for GESHA-DEMO/USD.
    pub fn gesha_demo_default(wasm_url: String, wasm_sha256: String) -> Self {
        Self {
            symbol: MARKET_GESHA_DEMO.to_string(),
            description: "Specialty Gesha Coffee Price Index (Testnet Demonstration Fixture)"
                .to_string(),
            decimals: DECIMALS_COFFEE_INDEX,
            unit: UNIT_USD_PER_KG.to_string(),
            // Min $1.00/kg (100,000,000), Max $500.00/kg (50,000,000,000)
            min_price: U128(100_000_000),
            max_price: U128(50_000_000_000),
            max_price_age_sec: DEFAULT_MAX_PRICE_AGE_SEC,
            max_divergence_bps: 2000, // 20% max price swing
            wasm_url,
            wasm_sha256,
            build_target: "wasm32-wasip2".to_string(),
            active: true,
            worker_mode: MODE_FIXTURE.to_string(),
            secrets_ref: None,
            min_source_count: 1,
            allow_synthetic: true,
            max_source_spread_bps: 0,
        }
    }

    /// Create the default Testnet configuration for `ARABICA-MONTHLY-IMF/USD`
    /// (IMF PCPS `PCOFFOTM` historical monthly benchmark; 90-day staleness limit; no API secrets required).
    pub fn arabica_monthly_imf_default(wasm_url: String, wasm_sha256: String) -> Self {
        Self {
            symbol: MARKET_ARABICA_MONTHLY_IMF.to_string(),
            description: IMF_PCPS_ATTRIBUTION_NOTICE.to_string(),
            decimals: DECIMALS_COFFEE_INDEX,
            unit: UNIT_USD_PER_KG.to_string(),
            // Min $1.00/kg (100,000,000), Max $500.00/kg (50,000,000,000)
            min_price: U128(100_000_000),
            max_price: U128(50_000_000_000),
            max_price_age_sec: HISTORICAL_MONTHLY_MAX_PRICE_AGE_SEC,
            max_divergence_bps: 2000,
            wasm_url,
            wasm_sha256,
            build_target: "wasm32-wasip2".to_string(),
            active: true,
            worker_mode: MODE_PUBLIC_BENCHMARK.to_string(),
            secrets_ref: None,
            min_source_count: 1,
            allow_synthetic: false,
            max_source_spread_bps: 0,
        }
    }

    /// Construct a typed `WorkerInput` from this market's configured policy.
    ///
    /// Fails closed on any policy inconsistency so live/benchmark markets can never fall back to `fixture`.
    pub fn build_worker_input(&self) -> Result<WorkerInput, String> {
        self.validate()?;
        let input = match self.worker_mode.as_str() {
            MODE_FIXTURE => WorkerInput::new_fixture(&self.symbol),
            MODE_PUBLIC_BENCHMARK => {
                WorkerInput::new_public_benchmark(&self.symbol, self.max_price_age_sec)
            }
            MODE_LIVE_INDEX => WorkerInput::new_live_index(
                &self.symbol,
                self.min_source_count,
                self.max_price_age_sec,
                self.max_source_spread_bps,
            ),
            other => {
                return Err(format!("unsupported worker_mode '{}'", other));
            }
        };
        input.validate()?;
        Ok(input)
    }

    /// Validate the sanity and security invariants of this market configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.symbol.trim().is_empty() {
            return Err("market symbol cannot be empty".to_string());
        }
        if self.symbol.len() > MAX_MARKET_SYMBOL_BYTES {
            return Err(format!(
                "market symbol UTF-8 byte length ({}) exceeds maximum ({} bytes)",
                self.symbol.len(),
                MAX_MARKET_SYMBOL_BYTES
            ));
        }
        if self.description.len() > MAX_MARKET_DESCRIPTION_BYTES {
            return Err(format!(
                "market description UTF-8 byte length ({}) exceeds maximum ({} bytes)",
                self.description.len(),
                MAX_MARKET_DESCRIPTION_BYTES
            ));
        }
        if self.unit.trim().is_empty() || self.unit.len() > MAX_MARKET_UNIT_BYTES {
            return Err(format!(
                "market unit must be 1..={} UTF-8 bytes",
                MAX_MARKET_UNIT_BYTES
            ));
        }
        if self.decimals != DECIMALS_COFFEE_INDEX {
            return Err(format!(
                "invalid decimals {}: standard is {}",
                self.decimals, DECIMALS_COFFEE_INDEX
            ));
        }
        if self.min_price.0 >= self.max_price.0 {
            return Err("min_price must be strictly less than max_price".to_string());
        }
        if self.min_price.0 == 0 {
            return Err("min_price must be greater than zero".to_string());
        }
        validate_wasm_sha256(&self.wasm_sha256)?;
        if !self.wasm_url.starts_with("https://") {
            return Err("wasm_url must use https protocol".to_string());
        }
        if self.wasm_url.len() > MAX_WASM_URL_BYTES {
            return Err(format!(
                "wasm_url UTF-8 byte length ({}) exceeds maximum ({} bytes)",
                self.wasm_url.len(),
                MAX_WASM_URL_BYTES
            ));
        }
        if self.build_target.trim().is_empty() || self.build_target.len() > MAX_BUILD_TARGET_BYTES {
            return Err(format!(
                "build_target must be 1..={} UTF-8 bytes",
                MAX_BUILD_TARGET_BYTES
            ));
        }
        if self.worker_mode.trim().is_empty() || self.worker_mode.len() > MAX_WORKER_MODE_BYTES {
            return Err(format!(
                "worker_mode must be 1..={} UTF-8 bytes",
                MAX_WORKER_MODE_BYTES
            ));
        }
        match self.worker_mode.as_str() {
            MODE_FIXTURE => {
                if self.symbol != MARKET_GESHA_DEMO {
                    return Err(format!(
                        "fixture mode is restricted strictly to '{}'",
                        MARKET_GESHA_DEMO
                    ));
                }
                if !self.allow_synthetic {
                    return Err("fixture market must set allow_synthetic = true".to_string());
                }
                if self.min_source_count != 1 {
                    return Err("fixture market requires min_source_count == 1".to_string());
                }
                if self.max_price_age_sec == 0 || self.max_price_age_sec > DEFAULT_MAX_PRICE_AGE_SEC
                {
                    return Err(format!(
                        "fixture market requires max_price_age_sec in 1..={} seconds",
                        DEFAULT_MAX_PRICE_AGE_SEC
                    ));
                }
                if self.secrets_ref.is_some() {
                    return Err("fixture market must have secrets_ref == None".to_string());
                }
                if self.max_source_spread_bps != 0 {
                    return Err("fixture market requires max_source_spread_bps == 0".to_string());
                }
            }
            MODE_PUBLIC_BENCHMARK => {
                if self.symbol != MARKET_ARABICA_MONTHLY_IMF {
                    return Err(format!(
                        "public_benchmark mode is restricted strictly to '{}'",
                        MARKET_ARABICA_MONTHLY_IMF
                    ));
                }
                if self.allow_synthetic {
                    return Err(
                        "public_benchmark market must set allow_synthetic = false".to_string()
                    );
                }
                if self.min_source_count != 1 {
                    return Err(
                        "public_benchmark market requires min_source_count == 1".to_string()
                    );
                }
                if self.max_price_age_sec == 0
                    || self.max_price_age_sec > HISTORICAL_MONTHLY_MAX_PRICE_AGE_SEC
                {
                    return Err(format!(
                        "public_benchmark market requires max_price_age_sec in 1..={} seconds (90 days)",
                        HISTORICAL_MONTHLY_MAX_PRICE_AGE_SEC
                    ));
                }
                if self.max_source_spread_bps != 0 {
                    return Err(
                        "public_benchmark market requires max_source_spread_bps == 0".to_string(),
                    );
                }
                if let Some(ref sr) = self.secrets_ref {
                    sr.validate()?;
                }
            }
            MODE_LIVE_INDEX => {
                if self.symbol == MARKET_GESHA_DEMO || self.symbol == MARKET_ARABICA_MONTHLY_IMF {
                    return Err(format!(
                        "'{}' cannot be configured as a live_index market",
                        self.symbol
                    ));
                }
                if self.allow_synthetic {
                    return Err("live_index market must set allow_synthetic = false".to_string());
                }
                if self.min_source_count < 1 {
                    return Err("live_index market requires min_source_count >= 1".to_string());
                }
                if self.max_price_age_sec == 0
                    || self.max_price_age_sec > DEFAULT_LIVE_INDEX_MAX_PRICE_AGE_SEC
                {
                    return Err(format!(
                        "live_index market requires max_price_age_sec in 1..={} seconds",
                        DEFAULT_LIVE_INDEX_MAX_PRICE_AGE_SEC
                    ));
                }
                if self.max_source_spread_bps > MAX_SOURCE_SPREAD_BPS {
                    return Err(format!(
                        "max_source_spread_bps ({}) exceeds maximum ({})",
                        self.max_source_spread_bps, MAX_SOURCE_SPREAD_BPS
                    ));
                }
                if let Some(ref sr) = self.secrets_ref {
                    sr.validate()?;
                }
            }
            other => {
                return Err(format!(
                    "unsupported worker_mode '{}': expected '{}', '{}' or '{}'",
                    other, MODE_FIXTURE, MODE_PUBLIC_BENCHMARK, MODE_LIVE_INDEX
                ));
            }
        }
        Ok(())
    }
}

/// An on-chain verified coffee index observation.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceObservation {
    /// Market symbol (e.g. "GESHA-DEMO/USD")
    pub market: String,
    /// Verified price in integer fixed-point (8 decimals)
    pub price: U128,
    /// Decimals (always 8)
    pub decimals: u8,
    /// Unit of measure (e.g. "USD/kg")
    pub unit: String,
    /// Source metadata tag (e.g. "roastake-fixture")
    pub source: String,
    /// Number of independent sources contributing to the price
    pub source_count: u32,
    /// Observation timestamp from worker in epoch seconds (0 for synthetic fixture)
    pub observed_at_sec: u64,
    /// On-chain block timestamp in epoch seconds when validated and stored
    pub stored_at_sec: u64,
    /// Request nonce that generated this observation
    pub nonce: u64,
    /// Data classification (e.g. "SYNTHETIC_DEMO")
    pub data_class: String,
}

/// State tracking an in-flight OutLayer execution request awaiting resolution.
///
/// Created when `request_price_update` is initiated, and safely removed when the
/// private callback `on_outlayer_execution` completes (success, failure, or rejection).
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRequest {
    /// Monotonically increasing request identifier
    pub nonce: u64,
    /// Account that initiated the request
    pub requester: AccountId,
    /// Market requested
    pub market: String,
    /// Timestamp when request was initiated in epoch seconds
    pub requested_at_sec: u64,
    /// Expected SHA-256 of the worker WASM binary
    pub expected_wasm_hash: String,
    /// NEAR deposit attached to the OutLayer execution (preserved for potential credit on failure)
    pub deposit_attached: NearToken,
}

/// An ambiguous execution failure awaiting explicit backing verification or external resolution.
///
/// ## Why `UnreconciledFailure` Exists
///
/// When `on_outlayer_execution` receives `Err(PromiseError::Failed)`, NEAR Protocol's runtime does
/// not distinguish between:
/// 1. **Pre-acceptance failure (Domain 1):** The initial `request_execution` call failed before
///    OutLayer accepted the request. NEAR issues a system-refund receipt returning the attached
///    deposit to ROASTAKE.
/// 2. **Post-acceptance failure (Domain 2):** OutLayer accepted `request_execution` and retained
///    the attached deposit, but the yielded execution promise subsequently failed (e.g., callback
///    panic, gas exhaustion, or timeout). NEAR does NOT return the deposit to ROASTAKE, and
///    OutLayer may have either retained the deposit or already refunded `payer_account_id`
///    (the original requester) directly.
///
/// Automatically recording a payable `RefundCredit` on `PromiseError::Failed` would create an
/// **unbacked refund liability** (or a **double refund** if OutLayer already refunded the user).
///
/// Therefore, ambiguous failures are recorded in `unreconciled_failures` with zero payable
/// liability until:
/// - **Backed Promotion (`promote_unreconciled_refund`):** Promoted against explicitly allocated
///   `refund_reserve_balance` (either funded via `fund_refund_reserve` or designated via
///   `admin_designate_refund_reserve` after verifying a NEAR system-refund receipt).
/// - **External Resolution (`admin_resolve_unreconciled_external` + `acknowledge_unreconciled_resolved_external`):**
///   Administrator records a governed external resolution proposal with SHA-256 attestation, and the
///   original requester accepts/acknowledges on-chain to close the record without local liability.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreconciledFailure {
    /// Request nonce of the ambiguous failed execution
    pub nonce: u64,
    /// Original requester account
    pub requester: AccountId,
    /// Market requested
    pub market: String,
    /// Deposit amount at risk in yoctoNEAR
    pub deposit_at_risk: NearToken,
    /// Timestamp when the failure callback executed in epoch seconds
    pub failed_at_sec: u64,
    /// Failure reason classification
    pub reason: String,
    /// Optional externally observed OutLayer request ID, if safely correlated
    pub outlayer_request_id: Option<u64>,
    /// Whether the administrator has proposed external resolution (awaiting requester acceptance)
    pub external_resolution_proposed: bool,
    /// Optional SHA-256 evidence digest recorded with an external resolution proposal
    pub resolution_evidence_sha256: Option<String>,
    /// Block height at which external resolution was proposed by the administrator
    pub resolution_proposed_at_block: Option<u64>,
}

/// Status of an account's refund claim lifecycle.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefundStatus {
    /// Credit has been backed and is available to be claimed by the requester.
    Available,
    /// Transfer promise has been dispatched and is currently in-flight on NEAR.
    InFlight,
    /// Outbound transfer failed; awaiting verification that the system-refund receipt has restored backing.
    ReturnPending,
    /// Transfer outcome could not be observed on-chain (the confirmation callback did not complete).
    ///
    /// The entitlement is preserved as a liability but is NON-PAYABLE. It can only leave this state
    /// through (a) a late objective callback, (b) claimant acknowledgement of receipt, or
    /// (c) an explicitly enabled, fully backed insurance compensation. It is never restored to
    /// `Available` or unilaterally deleted by an administrative assertion.
    Disputed,
}

/// Status of an active refund payout claim.
///
/// Terminal claims (`Settled`, `ReconciledSuccess`, or restored after failure) are removed from
/// contract storage to prevent unbounded storage accumulation.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimStatus {
    /// Transfer promise is in flight on NEAR
    InFlight,
    /// Outbound transfer failed; awaiting `reconcile_returned_refund` once system refund arrives
    ReturnPending,
    /// Transfer successfully settled via confirmation callback (removed upon settlement)
    Settled,
    /// Transfer failed on NEAR and credit was restored to available (removed upon restoration)
    RestoredOnFailure,
    /// Manual administrator reconciliation: confirmed transferred successfully off-chain
    ReconciledSuccess,
    /// Manual administrator reconciliation: confirmed transfer failed off-chain
    ReconciledFailure,
    /// Transfer outcome unobservable on-chain (callback did not complete); non-payable until a
    /// late objective callback, claimant acknowledgement, or discretionary insurance compensation.
    Disputed,
}

/// A globally identified refund payout claim with full provenance and auditability.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefundClaim {
    /// Monotonically increasing unique claim identifier
    pub claim_id: u64,
    /// Entitled claimant account
    pub claimant: AccountId,
    /// Exact amount claimed in yoctoNEAR
    pub amount: NearToken,
    /// Nonce of the originating failed OutLayer execution request
    pub originating_nonce: u64,
    /// Timestamp when claim was dispatched in epoch seconds
    pub initiated_at_sec: u64,
    /// Block height at which the claim (and its transfer promise) was created
    pub initiated_at_block: u64,
    /// Block height at which the claim was marked `Disputed`, if ever
    pub disputed_at_block: Option<u64>,
    /// Current settlement status
    pub status: ClaimStatus,
    /// Whether the administrator has proposed external/paid settlement (never deletes claim until claimant accepts)
    pub admin_settlement_proposed: bool,
    /// Block height at which the administrator proposed settlement, if ever
    pub settlement_proposed_at_block: Option<u64>,
    /// Optional SHA-256 evidence digest recorded with the administrator's settlement proposal
    pub settlement_evidence_sha256: Option<String>,
}

/// Administrative reconciliation input for a claim whose confirmation callback did not complete.
///
/// IMPORTANT: neither variant is treated as proof. A NEAR contract cannot query historical receipt
/// outcomes, so an administrator's statement is an attestation only.
/// - `ConfirmedSuccess` → governed *paid settlement proposal*; allowed only for a `Disputed` claim after
///   `DISPUTE_RESOLUTION_DELAY_BLOCKS`. Records a settlement proposal on the claim without deleting
///   the user's entitlement; closure requires claimant acceptance via `acknowledge_disputed_transfer_received`.
/// - `ConfirmedFailure` → recorded as an *unverified failure assertion*; the claim moves to
///   `Disputed` (non-payable). It NEVER restores the entitlement to `Available`.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileOutcome {
    /// Administrator proposes that the recipient was paid (proposal only; requires claimant acceptance)
    ConfirmedSuccess,
    /// Administrator asserts the transfer failed (unverified; results in `Disputed`, never `Available`)
    ConfirmedFailure,
}

/// Full accounting snapshot separating every liability and reserve bucket.
///
/// Backed (physically required at-rest) liabilities are `available + return_pending`. In-flight amounts
/// have already left the account (NEAR deducts a `Transfer` amount from the sender's balance when
/// the action is created), and disputed/unresolved amounts have an unknown physical location; all of
/// them remain included in `encumbered_backing_floor` so returning system refunds can never be
/// designated into another reserve.
#[near(serializers = [json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountingSnapshot {
    pub account_balance: U128,
    pub storage_usage_bytes: u64,
    pub storage_locked_balance: U128,
    pub spendable_balance: U128,
    pub minimum_operating_reserve: U128,
    pub refund_reserve_balance: U128,
    pub insurance_reserve_balance: U128,
    pub insurance_in_flight_liability: U128,
    pub insurance_return_pending_liability: U128,
    pub insurance_unresolved_liability: U128,
    pub insurance_enabled: bool,
    pub total_refund_liability: U128,
    pub in_flight_liability: U128,
    pub disputed_liability: U128,
    pub backed_refund_liability: U128,
    pub required_backing: U128,
    pub encumbered_backing_floor: U128,
    pub unallocated_surplus: U128,
    pub invariant_holds: bool,
    pub pending_request_count: u32,
    pub unreconciled_failure_count: u32,
    pub active_refund_credit_count: u32,
    pub active_claim_count: u32,
    pub open_request_slots: u32,
    pub max_open_requests: u32,
    pub active_insurance_compensation_count: u32,
    pub max_active_insurance_compensations: u32,
    pub market_count: u32,
    pub authorized_requester_count: u32,
    pub next_claim_id: u64,
    pub next_compensation_id: u64,
}

/// Status of a discretionary insurance compensation payout (only active while unconfirmed).
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsuranceCompensationStatus {
    /// Outbound compensation transfer is in flight on NEAR.
    InFlight,
    /// Outbound compensation transfer failed on-chain; awaiting system refund verification.
    ReturnPending,
    /// Outbound compensation callback did not complete; held unresolved until late callback or claimant acknowledgement.
    Disputed,
}

/// Active insurance compensation payout record (removed upon terminal settlement/return).
///
/// Separate from `RefundClaim` and uses a separate `compensation_id` namespace so discretionary
/// insurance payouts for `Disputed` claims are never conflated with verified refund restorations.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsuranceCompensation {
    pub compensation_id: u64,
    pub originating_claim_id: u64,
    pub originating_nonce: u64,
    pub recipient: AccountId,
    pub amount: NearToken,
    pub initiated_at_sec: u64,
    pub initiated_at_block: u64,
    pub disputed_at_block: Option<u64>,
    pub evidence_sha256: String,
    pub status: InsuranceCompensationStatus,
}

/// A failure-safe refund credit tracking available, in-flight, and return-pending entitlements.
///
/// ## State Transition Invariants
///
/// 1. **Backed Promotion:** An `UnreconciledFailure` is promoted via `promote_unreconciled_refund`
///    by atomically deducting `deposit_at_risk` from `refund_reserve_balance`, adding it to
///    `available_amount`, and incrementing `total_refund_liability` by the exact same amount.
/// 2. **Claim Initiation:** When the user calls `claim_refund()`, `available_amount` is moved to
///    `in_flight_amount` (with `available_amount` set to 0), and status transitions to `InFlight`.
///    `total_refund_liability` remains unchanged until confirmation.
/// 3. **Transfer Success:** When `on_refund_transfer` confirms `Ok(_)`, `in_flight_amount` and
///    `total_refund_liability` are decremented by the transferred amount, and the terminal claim
///    record is removed from `self.claims`. If no entitlement remains, the user's `RefundCredit`
///    is removed from `self.pending_refunds`.
/// 4. **Transfer Failure & Return Reconciliation:** If the transfer promise fails, `on_refund_transfer`
///    moves `in_flight_amount` to `return_pending_amount` (`RefundStatus::ReturnPending`,
///    `ClaimStatus::ReturnPending`), keeping `total_refund_liability` intact. Once the NEAR
///    system-refund receipt restores the contract's spendable backing under the formal solvency
///    invariant, `reconcile_returned_refund(claim_id)` restores `return_pending_amount` to
///    `available_amount` and removes the failed claim record from `self.claims`.
#[near(serializers = [borsh, json])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefundCredit {
    /// Account entitled to the refund
    pub requester: AccountId,
    /// Amount currently available to be claimed (in yoctoNEAR)
    pub available_amount: NearToken,
    /// Amount currently in-flight in an unconfirmed transfer (in yoctoNEAR)
    pub in_flight_amount: NearToken,
    /// Amount from a failed payout transfer awaiting system-refund backing confirmation (in yoctoNEAR)
    pub return_pending_amount: NearToken,
    /// Amount whose transfer outcome is unobservable on-chain; non-payable (in yoctoNEAR)
    pub disputed_amount: NearToken,
    /// Current lifecycle status
    pub status: RefundStatus,
    /// Request nonce of the most recent failure contributing to this credit
    pub from_nonce: u64,
    /// Active in-flight or return-pending claim ID
    pub active_claim_id: Option<u64>,
}

impl RefundCredit {
    /// Total outstanding entitlement (available + in-flight + return-pending + disputed).
    pub fn total_entitlement(&self) -> NearToken {
        let sum = self
            .available_amount
            .as_yoctonear()
            .checked_add(self.in_flight_amount.as_yoctonear())
            .and_then(|v| v.checked_add(self.return_pending_amount.as_yoctonear()))
            .and_then(|v| v.checked_add(self.disputed_amount.as_yoctonear()))
            .expect("ERR_OVERFLOW: total_entitlement overflow");
        NearToken::from_yoctonear(sum)
    }
}
