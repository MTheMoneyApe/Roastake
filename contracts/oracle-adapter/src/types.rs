//! State and data transfer types for the ROASTAKE Oracle Adapter.

use near_sdk::json_types::U128;
use near_sdk::{near, AccountId, NearToken};
use roastake_shared::DECIMALS_COFFEE_INDEX;

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

/// Maximum UTF-8 byte length for `WorkerOutput.source` / `PriceObservation.source`.
/// Bounded so a newly inserted `PriceObservation` in `IterableMap<String, PriceObservation>`
/// (`keys` entry <= 114 bytes + `values` entry <= 379 bytes = <= 493 bytes worst-case) never
/// exceeds `OPEN_REQUEST_STORAGE_BUDGET_BYTES` (512 bytes).
pub const MAX_WORKER_SOURCE_BYTES: usize = 128;

/// Maximum UTF-8 byte length for `WorkerOutput.data_class` / `PriceObservation.data_class`.
pub const MAX_WORKER_DATA_CLASS_BYTES: usize = 32;

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
}

impl MarketConfig {
    /// Create the default configuration for GESHA-DEMO/USD.
    pub fn gesha_demo_default(wasm_url: String, wasm_sha256: String) -> Self {
        Self {
            symbol: "GESHA-DEMO/USD".to_string(),
            description: "Specialty Gesha Coffee Price Index (Testnet Demonstration Fixture)"
                .to_string(),
            decimals: DECIMALS_COFFEE_INDEX,
            unit: "USD/kg".to_string(),
            // Min $1.00/kg (100,000,000), Max $500.00/kg (50,000,000,000)
            min_price: U128(100_000_000),
            max_price: U128(50_000_000_000),
            max_price_age_sec: 3600,
            max_divergence_bps: 2000, // 20% max price swing
            wasm_url,
            wasm_sha256,
            build_target: "wasm32-wasip2".to_string(),
            active: true,
        }
    }

    /// Validate the sanity of this market configuration.
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
        if self.wasm_sha256.len() != 64 || !self.wasm_sha256.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Err("wasm_sha256 must be a valid 64-character hex string".to_string());
        }
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
