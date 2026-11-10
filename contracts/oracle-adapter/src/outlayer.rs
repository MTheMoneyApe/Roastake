#![allow(clippy::too_many_arguments)]
//! OutLayer cross-contract call interface and cost estimation.
//!
//! Provides the external contract interface for invoking `outlayer.testnet::request_execution`
//! and calculates estimated execution costs matching the official OutLayer dynamic pricing model.

use near_sdk::{ext_contract, AccountId, NearToken};
use roastake_shared::{ExecutionSource, RequestParams, ResourceLimits, SecretsReference};

/// External contract interface for OutLayer's `request_execution` method on NEAR Testnet.
///
/// Verified against OutLayer official sources and deployed `outlayer.testnet` contract:
/// 7 canonical parameters:
/// 1. `source`: Target WASM source specification (`WasmUrl`, `Project`, `GitHub`)
/// 2. `resource_limits`: Explicit execution resource bounds (instructions, memory, seconds)
/// 3. `input_data`: Input payload passed to WASI worker via stdin (JSON string)
/// 4. `secrets_ref`: Optional reference to encrypted TEE secrets profile
/// 5. `response_format`: Serialized output encoding (e.g. "Json", "Text", "Bytes")
/// 6. `payer_account_id`: Destination account for automatic refund of unspent execution fees
/// 7. `params`: Optional advanced execution control parameters (`RequestParams`)
#[ext_contract(ext_outlayer)]
pub trait OutLayer {
    /// Request verifiable off-chain execution inside an OutLayer TEE worker.
    fn request_execution(
        &mut self,
        source: ExecutionSource,
        resource_limits: Option<ResourceLimits>,
        input_data: Option<String>,
        secrets_ref: Option<SecretsReference>,
        response_format: Option<String>,
        payer_account_id: Option<AccountId>,
        params: Option<RequestParams>,
    );
}

// -----------------------------------------------------------------------------
// Official OutLayer Dynamic Pricing Rates (NEAR Testnet)
//
// Verified via empirical RPC queries against `outlayer.testnet` (get_pricing_full):
// - Base fee: 0.001 NEAR (1 milliNEAR = 10^21 yoctoNEAR)
// - Per million instructions fee: 10^14 yoctoNEAR (100_000_000_000_000 yoctoNEAR)
// - Per millisecond fee: 10^17 yoctoNEAR (100_000_000_000_000_000 yoctoNEAR)
// - Per compile millisecond fee: 10^17 yoctoNEAR
// - Free input data threshold: 32,768 bytes (32 KB)
// - Storage surcharge for input > 32 KB: 0.00001 NEAR (10^19 yoctoNEAR) per byte
// -----------------------------------------------------------------------------

/// OutLayer base execution fee: 0.001 NEAR (1 milliNEAR = 1,000,000,000,000,000,000,000 yoctoNEAR).
pub const BASE_FEE_YOCTO: u128 = 1_000_000_000_000_000_000_000;

/// Instruction rate in yoctoNEAR per 1,000,000 instructions (10^14 yoctoNEAR = 0.0000000001 NEAR per million).
pub const PER_MILLION_INSTRUCTIONS_FEE_YOCTO: u128 = 100_000_000_000_000;

/// Execution time rate in yoctoNEAR per millisecond (10^17 yoctoNEAR = 0.0000001 NEAR per ms).
pub const PER_MS_FEE_YOCTO: u128 = 100_000_000_000_000_000;

/// Input payload threshold before temporary byte storage surcharge applies (32 KB).
pub const FREE_INPUT_BYTES_THRESHOLD: u64 = 32_768;

/// Surcharge rate per byte of input exceeding 32 KB: 0.00001 NEAR (10^19 yoctoNEAR / byte).
pub const STORAGE_PRICE_PER_BYTE_YOCTO: u128 = 10_000_000_000_000_000_000;

/// Recommended provisional minimum deposit threshold for triggering an index refresh (10 milliNEAR = 0.01 NEAR).
///
/// Standard worker run (1B instructions + 60s timeout + <32KB input) evaluates to exactly
/// 7,000,100,000,000,000,000,000 yoctoNEAR (~7.0001 milliNEAR). Attaching 10 milliNEAR
/// provides a safety buffer while OutLayer automatically refunds unspent deposit to `payer_account_id`.
pub const PROVISIONAL_MIN_DEPOSIT_MILLINEAR: u128 = 10;

/// Compute estimated cost for an OutLayer execution based on official verified rates.
///
/// Formula:
/// Execution cost = base_fee (0.001 NEAR)
///                + (max_instructions / 1_000_000) * 10^14 yoctoNEAR
///                + (max_execution_seconds * 1,000) * 10^17 yoctoNEAR
///                + max(0, input_len - 32768) * 10^19 yoctoNEAR
pub fn calculate_execution_cost(limits: &ResourceLimits, input_len: u64) -> NearToken {
    let instr = limits.max_instructions.unwrap_or(1_000_000_000) as u128;
    let time_sec = limits.max_execution_seconds.unwrap_or(60) as u128;

    let base_yocto = BASE_FEE_YOCTO;
    let instr_yocto = (instr / 1_000_000).saturating_mul(PER_MILLION_INSTRUCTIONS_FEE_YOCTO);
    let time_ms = time_sec.saturating_mul(1_000);
    let time_yocto = time_ms.saturating_mul(PER_MS_FEE_YOCTO);

    let input_surcharge_yocto = if input_len > FREE_INPUT_BYTES_THRESHOLD {
        ((input_len - FREE_INPUT_BYTES_THRESHOLD) as u128)
            .saturating_mul(STORAGE_PRICE_PER_BYTE_YOCTO)
    } else {
        0
    };

    let total_yocto = base_yocto
        .saturating_add(instr_yocto)
        .saturating_add(time_yocto)
        .saturating_add(input_surcharge_yocto);

    NearToken::from_yoctonear(total_yocto)
}
