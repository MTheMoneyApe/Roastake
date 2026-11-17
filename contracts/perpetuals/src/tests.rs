// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

//! Comprehensive unit and adversarial tests for ROASTAKE Perpetuals (`roastake-perpetuals`).

use near_sdk::json_types::U128;
use near_sdk::test_utils::{accounts, VMContextBuilder};
use near_sdk::{testing_env, AccountId, NearToken, PromiseOrValue, PromiseResult};
use roastake_perps_core::{PRICE_SCALE, SIZE_SCALE, USD_SCALE};

use crate::types::{
    CollateralTokenConfig, ExternalPriceObservation, FungibleTokenMetadata, OracleSourceConfig,
    StoredMarketMode, StoredOraclePolicy, StoredPositionSide, ACCOUNT_STORAGE_DEPOSIT_YOCTO,
    CIRCLE_MAINNET_USDC_ACCOUNT_ID, CIRCLE_TESTNET_DISCLOSURE, CIRCLE_TESTNET_USDC_ACCOUNT_ID,
    DATA_CLASS_TESTNET_SANDBOX, TIDEPOOL_TESTNET_USDC_FIXTURE_ID,
};
use crate::PerpsContract;

fn admin_id() -> AccountId {
    "roastake-admin.testnet".parse().unwrap()
}

fn treasury_id() -> AccountId {
    "roastake-treasury.testnet".parse().unwrap()
}

fn usdc_id() -> AccountId {
    CIRCLE_TESTNET_USDC_ACCOUNT_ID.parse().unwrap()
}

fn tidepool_id() -> AccountId {
    TIDEPOOL_TESTNET_USDC_FIXTURE_ID.parse().unwrap()
}

fn operator_id() -> AccountId {
    "roastake-feed-op.testnet".parse().unwrap()
}

fn oracle_id() -> AccountId {
    "roastake-oracle-v1.testnet".parse().unwrap()
}

fn alice_id() -> AccountId {
    "alice.testnet".parse().unwrap()
}

fn bob_id() -> AccountId {
    "bob.testnet".parse().unwrap()
}

fn keeper_id() -> AccountId {
    "keeper.testnet".parse().unwrap()
}

fn lp_id() -> AccountId {
    "lp-provider.testnet".parse().unwrap()
}

const MARKET_SANDBOX: &str = "COFFEE-ARABICA-TESTNET/USD";
const MARKET_XCC: &str = "COFFEE-LIVE-TESTNET/USD";
const DISCLOSURE: &str = CIRCLE_TESTNET_DISCLOSURE;

fn set_ctx(predecessor: AccountId, block_ts_sec: u64, attached_yocto: u128) {
    let mut b = VMContextBuilder::new();
    b.current_account_id("perps.testnet".parse().unwrap());
    b.predecessor_account_id(predecessor);
    b.block_timestamp(block_ts_sec * 1_000_000_000);
    b.attached_deposit(NearToken::from_yoctonear(attached_yocto));
    testing_env!(b.build());
}

fn set_ctx_with_promises(
    predecessor: AccountId,
    block_ts_sec: u64,
    promise_results: Vec<PromiseResult>,
) {
    let mut b = VMContextBuilder::new();
    b.current_account_id("perps.testnet".parse().unwrap());
    b.predecessor_account_id(predecessor);
    b.block_timestamp(block_ts_sec * 1_000_000_000);
    testing_env!(
        b.build(),
        near_sdk::test_vm_config(),
        near_sdk::RuntimeFeesConfig::test(),
        Default::default(),
        promise_results
    );
}

fn setup_contract(ts_sec: u64) -> PerpsContract {
    set_ctx(admin_id(), ts_sec, 0);
    let mut c = PerpsContract::init(
        admin_id(),
        treasury_id(),
        usdc_id(),
        "USDC".to_string(),
        6,
        DISCLOSURE.to_string(),
    );
    c.register_market(
        MARKET_SANDBOX.to_string(),
        OracleSourceConfig::TestnetSandboxOperator {
            oracle_operator: operator_id(),
            min_price_e8: U128(PRICE_SCALE),
            max_price_e8: U128(500 * PRICE_SCALE),
        },
        None,
        None,
        Some(0),
    );
    c
}

fn register_user(c: &mut PerpsContract, user: AccountId, ts_sec: u64) {
    set_ctx(user.clone(), ts_sec, ACCOUNT_STORAGE_DEPOSIT_YOCTO);
    c.register_account(Some(user));
}

fn push_tick(c: &mut PerpsContract, price_e8: u128, obs_sec: u64, seq: u64, block_sec: u64) {
    set_ctx(operator_id(), block_sec, 0);
    c.push_sandbox_oracle_price(MARKET_SANDBOX.to_string(), U128(price_e8), obs_sec, seq);
}

fn deposit_usdc(
    c: &mut PerpsContract,
    sender: AccountId,
    units_e6: u128,
    msg: &str,
    ts_sec: u64,
) -> PromiseOrValue<U128> {
    set_ctx(usdc_id(), ts_sec, 0);
    c.ft_on_transfer(sender, U128(units_e6), msg.to_string())
}

// =========================================================================
// 1. Initialization, Mainnet Guards & Allowlist Metadata Verification
// =========================================================================

#[test]
#[should_panic(expected = "ERR_MAINNET_REJECTED")]
fn test_init_rejects_mainnet_collateral_token() {
    set_ctx(admin_id(), 1_000_000, 0);
    PerpsContract::init(
        admin_id(),
        treasury_id(),
        "usdc.near".parse().unwrap(),
        "USDC".to_string(),
        6,
        DISCLOSURE.to_string(),
    );
}

#[test]
#[should_panic(expected = "ERR_COLLATERAL_DECIMALS_GT_8_UNSUPPORTED")]
fn test_add_collateral_rejects_decimals_above_8() {
    let mut c = setup_contract(1_000);
    set_ctx(admin_id(), 1_000, 0);
    c.add_collateral_token(
        "bad18.testnet".parse().unwrap(),
        "WNEAR".to_string(),
        18,
        DISCLOSURE.to_string(),
    );
}

#[test]
fn test_collateral_metadata_verification_and_mismatch_deactivation() {
    let mut c = setup_contract(1_000);

    // Matching Circle NEAR Testnet metadata (`ft-1.0.0`, `USDC`, decimals=6, inline SVG icon >1.6 KB) marks verified & active.
    let circle_svg_icon = format!("data:image/svg+xml,{}", "a".repeat(1_700));
    let valid_meta = serde_json::to_vec(&FungibleTokenMetadata {
        spec: "ft-1.0.0".to_string(),
        name: "USDC".to_string(),
        symbol: "USDC".to_string(),
        icon: Some(circle_svg_icon),
        reference: None,
        reference_hash: None,
        decimals: 6,
    })
    .unwrap();
    assert!(valid_meta.len() > 1_700);
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_001,
        vec![PromiseResult::Successful(valid_meta)],
    );
    assert!(c.on_verify_collateral_metadata(usdc_id()));
    let view = c.get_collateral_token(usdc_id()).unwrap();
    assert!(view.active && view.metadata_verified_on_chain);
    assert_eq!(view.scale_to_usd_e8.0, 100);

    // Mismatched metadata (`decimals=8` or non-standard `spec`) fails closed & deactivates token!
    let spoof_meta = serde_json::to_vec(&FungibleTokenMetadata {
        spec: "ft-1.0.0".to_string(),
        name: "Fake".to_string(),
        symbol: "USDC".to_string(),
        icon: None,
        reference: None,
        reference_hash: None,
        decimals: 8,
    })
    .unwrap();
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_002,
        vec![PromiseResult::Successful(spoof_meta)],
    );
    assert!(!c.on_verify_collateral_metadata(usdc_id()));
    let view2 = c.get_collateral_token(usdc_id()).unwrap();
    assert!(!view2.active && !view2.metadata_verified_on_chain);
}

// =========================================================================
// 2. Collateral Deposits, Conversions, Residues & Async Withdrawal Safety
// =========================================================================

#[test]
#[should_panic(expected = "ERR_INVALID_COLLATERAL_TOKEN")]
fn test_unapproved_fake_usdc_rejected_even_if_symbol_named_usdc() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, alice_id(), 1_000);
    set_ctx("fake-usdc.testnet".parse().unwrap(), 1_000, 0);
    let _ = c.ft_on_transfer(
        alice_id(),
        U128(100_000_000),
        r#"{"action":"trader_collateral"}"#.to_string(),
    );
}

#[test]
#[should_panic(expected = "ERR_ACCOUNT_NOT_REGISTERED")]
fn test_deposit_requires_storage_registration_first() {
    let mut c = setup_contract(1_000);
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        100_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );
}

#[test]
#[should_panic(expected = "ERR_INSUFFICIENT_STORAGE_DEPOSIT")]
fn test_register_account_rejects_underfunded_storage_deposit() {
    let mut c = setup_contract(1_000);
    set_ctx(alice_id(), 1_000, 1_000);
    let _ = c.register_account(Some(alice_id()));
}

#[test]
#[should_panic(expected = "ERR_INVALID_DEPOSIT_AMOUNT")]
fn test_deposit_rejects_zero_amount() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, alice_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        0,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );
}

#[test]
fn test_6_to_8_conversion_overflow_and_sub_micro_unit_residue_preservation() {
    let cfg = CollateralTokenConfig {
        token_id: usdc_id(),
        symbol: "USDC".to_string(),
        decimals: 6,
        scale_to_usd_e8: 100,
        non_monetary_testnet_disclosure: DISCLOSURE.to_string(),
        active: true,
        metadata_verified_on_chain: true,
        tracked_balance_units: 0,
    };
    assert_eq!(cfg.checked_usd_e8(1_000_000).unwrap(), USD_SCALE);
    assert!(cfg.checked_usd_e8(u128::MAX).is_err());

    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);

    // LP seeds $10,000 USDC (`10_000 * 10^6`)
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        10_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    );
    // Alice deposits 500 USDC (`500 * 10^6`) = 50_000_000_000 e8
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        500_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );

    // Simulate fractional price move leaving Alice with an odd sub-micro-USDC fee/PnL residue (`% 100 != 0`)
    push_tick(&mut c, 8 * PRICE_SCALE, 1_000, 1, 1_000);
    set_ctx(alice_id(), 1_001, 0);
    // Open `33.33333333 kg` @ `$8/kg` => notional has odd `e8` digits (`*10 bps` fee = indivisible by 100!)
    let size = 3_333_333_333u128;
    let margin = 100 * USD_SCALE;
    let pos_res = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Long,
        U128(size),
        U128(margin),
        None,
    );
    let pos_id = match pos_res {
        PromiseOrValue::Value(Some(id)) => id,
        _ => panic!("expected synchronous sandbox open"),
    };

    push_tick(&mut c, 811_111_111_u128, 1_002, 2, 1_002); // $8.11111111/kg
    set_ctx(alice_id(), 1_003, 0);
    let _ = c.close_position(pos_id, None);

    let acc = c.get_account(alice_id()).unwrap();
    let free_e8 = acc.free_collateral_e8.0;
    let expected_units = free_e8 / 100;
    let expected_residue = free_e8 % 100;
    assert_ne!(
        expected_residue, 0,
        "Test requires sub-micro-USDC (<100 e8) residue to verify conservation"
    );
    assert_eq!(acc.withdrawable_token_units_at_scale_100.0, expected_units);
    assert_eq!(acc.sub_micro_unit_residue_e8.0, expected_residue);

    // Withdraw None (maximum whole USDC units): withdraws floor(`free_e8 / 100`), keeps EXACT residue intact!
    set_ctx(alice_id(), 1_004, 0);
    let _ = c.withdraw_collateral(usdc_id(), None);
    let acc_in_flight = c.get_account(alice_id()).unwrap();
    assert_eq!(
        acc_in_flight.free_collateral_e8.0, expected_residue,
        "Residue must stay intact in free_collateral_e8, never silently rounded to protocol"
    );
    assert!(acc_in_flight.has_pending_withdrawal);

    // Settle transfer callback OK -> global solvency (`sum(buckets) == tracked_collateral_e8`) holds exact!
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_005,
        vec![PromiseResult::Successful(vec![])],
    );
    c.on_withdraw_transfer(1);
    assert!(c.verify_solvency_invariant());
    let acc_after = c.get_account(alice_id()).unwrap();
    assert_eq!(acc_after.free_collateral_e8.0, expected_residue);
    assert!(!acc_after.has_pending_withdrawal);
}

#[test]
#[should_panic(expected = "ERR_WITHDRAWAL_IN_PROGRESS")]
fn test_double_withdrawal_rejected_while_pending() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, alice_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        200_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );

    set_ctx(alice_id(), 1_001, 0);
    let _ = c.withdraw_collateral(usdc_id(), Some(U128(100_000_000)));
    set_ctx(alice_id(), 1_001, 0);
    let _ = c.withdraw_collateral(usdc_id(), Some(U128(50_000_000)));
}

#[test]
fn test_failed_withdrawal_callback_restores_balance_and_ignores_duplicate_callback() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, alice_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        200_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );

    set_ctx(alice_id(), 1_001, 0);
    let _ = c.withdraw_collateral(usdc_id(), Some(U128(150_000_000)));
    assert_eq!(
        c.get_account(alice_id()).unwrap().free_collateral_e8.0,
        50 * USD_SCALE
    );

    // Promise failed (e.g. receiver unregistered NEP-145 storage on Tidepool): balance restores 100%!
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_002,
        vec![PromiseResult::Failed],
    );
    c.on_withdraw_transfer(1);
    assert!(c.verify_solvency_invariant());
    assert_eq!(
        c.get_account(alice_id()).unwrap().free_collateral_e8.0,
        200 * USD_SCALE
    );

    // Duplicate callback replay must be ignored without panicking or double-crediting!
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_003,
        vec![PromiseResult::Failed],
    );
    c.on_withdraw_transfer(1);
    assert!(c.verify_solvency_invariant());
    assert_eq!(
        c.get_account(alice_id()).unwrap().free_collateral_e8.0,
        200 * USD_SCALE
    );
}

#[test]
#[should_panic(expected = "ERR_WITHDRAWAL_BELOW_MIN_TOKEN_UNIT")]
fn test_sub_micro_residue_cannot_withdraw_zero_token_units() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        10_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        100_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );
    push_tick(&mut c, 8 * PRICE_SCALE, 1_000, 1, 1_000);

    // Create tiny fractional fee residue by trading non-round quantity & withdrawing whole units
    set_ctx(alice_id(), 1_001, 0);
    let PromiseOrValue::Value(Some(pid)) = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Long,
        U128(1_234_567_891),
        U128(50 * USD_SCALE),
        None,
    ) else {
        panic!("expected pos id");
    };
    set_ctx(alice_id(), 1_002, 0);
    let _ = c.close_position(pid, None);
    set_ctx(alice_id(), 1_003, 0);
    let _ = c.withdraw_collateral(usdc_id(), None);
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_004,
        vec![PromiseResult::Successful(vec![])],
    );
    c.on_withdraw_transfer(1);

    // Remaining balance is < 100 e8 (< 1 micro-USDC); trying another None withdraw fails safely with 0 loss!
    let rem = c.get_account(alice_id()).unwrap().free_collateral_e8.0;
    assert!(rem > 0 && rem < 100);
    set_ctx(alice_id(), 1_005, 0);
    let _ = c.withdraw_collateral(usdc_id(), None);
}

// =========================================================================
// 3. Perpetuals Lifecycle, Funding, Max Profit Cap & Utilization Cap
// =========================================================================

#[test]
fn test_open_long_short_profit_cap_funding_and_fees_solvency() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    register_user(&mut c, bob_id(), 1_000);

    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        10_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    ); // $10k LP
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        2_000_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    ); // $2k
    let _ = deposit_usdc(
        &mut c,
        bob_id(),
        2_000_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    ); // $2k

    push_tick(&mut c, 10 * PRICE_SCALE, 1_000, 1, 1_000); // $10/kg

    // Alice opens Long 100kg ($1,000 notional, $200 IM + $1 fee)
    set_ctx(alice_id(), 1_001, 0);
    let PromiseOrValue::Value(Some(long_id)) = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Long,
        U128(100 * SIZE_SCALE),
        U128(200 * USD_SCALE),
        None,
    ) else {
        panic!("expected id");
    };

    // Bob opens Short 50kg ($500 notional, $100 IM + $0.50 fee) -> Long skew exists!
    set_ctx(bob_id(), 1_002, 0);
    let PromiseOrValue::Value(Some(short_id)) = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Short,
        U128(50 * SIZE_SCALE),
        U128(100 * USD_SCALE),
        None,
    ) else {
        panic!("expected id");
    };

    // Advance 60s smoothly and step price +4% ($10.40/kg -> within 500 bps jump limit)
    push_tick(&mut c, 1_040_000_000_u128, 1_062, 2, 1_062);
    set_ctx(keeper_id(), 1_062, 0);
    let _ = c.accrue_funding(MARKET_SANDBOX.to_string());

    let market = c.get_market(MARKET_SANDBOX.to_string()).unwrap();
    assert!(
        market.cum_funding_index_e12.parse::<i128>().unwrap() > 0,
        "Positive Long skew must accrue positive funding index (Longs pay Shorts/LP)"
    );

    // Step price in <500 bps increments up to >2x ($22.00/kg) to verify max_pnl_bps = 10_000 (+100% notional) cap!
    let mut t = 1_100;
    let steps = [
        1_080_000_000_u128,
        1_130_000_000,
        1_185_000_000,
        1_240_000_000,
        1_300_000_000,
        1_360_000_000,
        1_420_000_000,
        1_490_000_000,
        1_560_000_000,
        1_630_000_000,
        1_700_000_000,
        1_780_000_000,
        1_860_000_000,
        1_950_000_000,
        2_040_000_000,
        2_130_000_000,
        2_200_000_000,
    ];
    for (seq, p) in (3_u64..).zip(steps) {
        push_tick(&mut c, p, t, seq, t);
        t += 10;
    }

    // Bob's Short is deeply underwater! First register keeper and liquidate Bob's underwater Short
    register_user(&mut c, keeper_id(), t);
    set_ctx(keeper_id(), t, 0);
    let _ = c.liquidate_position(short_id);
    assert!(c.verify_solvency_invariant());
    assert!(c.get_position(short_id).is_none());

    // Now Alice closes winning Long at $22.00/kg: raw PnL = +$1,200 > 100% of $1,000 entry notional => CAPPED AT $1,000!
    set_ctx(alice_id(), t + 1, 0);
    let free_before_close = c.get_account(alice_id()).unwrap().free_collateral_e8.0;
    let _ = c.close_position(long_id, None);
    let free_after_close = c.get_account(alice_id()).unwrap().free_collateral_e8.0;
    // Gain net of fee & small negative funding cannot exceed $200 margin + $1,000 max gain cap = +$1,200!
    assert!(free_after_close - free_before_close <= 1_200 * USD_SCALE);
    assert!(free_after_close - free_before_close > 1_190 * USD_SCALE);
    assert!(c.verify_solvency_invariant());
}

#[test]
#[should_panic(expected = "ERR_MAX_UTILIZATION_EXCEEDED")]
fn test_utilization_cap_75_percent_enforced() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        1_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    ); // $1,000 LP -> max $750 skew
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        500_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );
    push_tick(&mut c, 10 * PRICE_SCALE, 1_000, 1, 1_000);

    set_ctx(alice_id(), 1_001, 0);
    let _ = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Long,
        U128(80 * SIZE_SCALE), // $800 > $750 (80% utilization)
        U128(200 * USD_SCALE),
        None,
    );
}

#[test]
#[should_panic(expected = "ERR_EXCEEDS_AVAILABLE_MARGIN")]
fn test_remove_margin_blocks_extracting_unrealized_profit() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        5_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        500_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );
    push_tick(&mut c, 10 * PRICE_SCALE, 1_000, 1, 1_000);

    set_ctx(alice_id(), 1_001, 0);
    let PromiseOrValue::Value(Some(pid)) = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Long,
        U128(100 * SIZE_SCALE), // $1k notional, min IM = $200
        U128(200 * USD_SCALE),
        None,
    ) else {
        panic!("expected id");
    };

    // Price rises +4% (+$40 unrealized profit), yet locked_margin == $200 == entry_IM -> 0 removable!
    push_tick(&mut c, 1_040_000_000_u128, 1_010, 2, 1_010);
    set_ctx(alice_id(), 1_011, 0);
    let _ = c.remove_margin(pid, U128(10 * USD_SCALE));
}

// =========================================================================
// 4. Liquidation, Keeper Reward, Insurance Arbitrage & Bad Debt Cascade
// =========================================================================

#[test]
fn test_normal_liquidation_keeper_reward_and_insurance_fee() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    register_user(&mut c, keeper_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        10_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        1_000_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );
    push_tick(&mut c, 10 * PRICE_SCALE, 1_000, 1, 1_000);

    set_ctx(alice_id(), 1_001, 0);
    let PromiseOrValue::Value(Some(pid)) = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Long,
        U128(100 * SIZE_SCALE),
        U128(200 * USD_SCALE),
        None,
    ) else {
        panic!("expected id");
    };

    // Drop ~13.5% in bounded <=500 bps steps to enter MM threshold (`eq <= 8%` of mark) without bad debt!
    push_tick(&mut c, 955_000_000_u128, 1_010, 2, 1_010);
    push_tick(&mut c, 910_000_000_u128, 1_020, 3, 1_020);
    push_tick(&mut c, 868_000_000_u128, 1_030, 4, 1_030); // Long PnL = -$132 -> Eq = $68; MM(868) = $69.44 -> liquidatable!

    let ins_before = c.get_accounting_snapshot().insurance_fund_e8.0;
    set_ctx(keeper_id(), 1_031, 0);
    let _ = c.liquidate_position(pid);

    let keeper_bal = c.get_account(keeper_id()).unwrap().free_collateral_e8.0;
    let ins_after = c.get_accounting_snapshot().insurance_fund_e8.0;
    assert!(keeper_bal > 0, "Keeper must receive liquidation reward");
    assert!(
        ins_after > ins_before,
        "Insurance fund must grow from liquidation fee"
    );
    assert!(c.verify_solvency_invariant());
}

#[test]
fn test_adversarial_pre_liquidation_insurance_arbitrage_defeated_on_contract() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    register_user(&mut c, bob_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        10_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    ); // $10k LP
    let _ = deposit_usdc(
        &mut c,
        bob_id(),
        2_000_000_000,
        r#"{"action":"insurance_fund"}"#,
        1_000,
    ); // $2k Insurance
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        2_000_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );

    push_tick(&mut c, 10 * PRICE_SCALE, 1_000, 1, 1_000);
    set_ctx(alice_id(), 1_001, 0);
    let _pos = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Long,
        U128(100 * SIZE_SCALE),
        U128(200 * USD_SCALE),
        None,
    );

    // Simulate gradual drop to $7.00/kg -> Alice has $100 unrealized bad debt (Eq = -$100), covered by $2k Insurance!
    push_tick(&mut c, 955_000_000_u128, 1_010, 2, 1_010);
    push_tick(&mut c, 910_000_000_u128, 1_020, 3, 1_020);
    push_tick(&mut c, 868_000_000_u128, 1_030, 4, 1_030);
    push_tick(&mut c, 825_000_000_u128, 1_040, 5, 1_040);
    push_tick(&mut c, 785_000_000_u128, 1_050, 6, 1_050);
    push_tick(&mut c, 747_000_000_u128, 1_060, 7, 1_060);
    push_tick(&mut c, 711_000_000_u128, 1_070, 8, 1_070);
    push_tick(&mut c, 700_000_000_u128, 1_080, 9, 1_080);
    register_user(&mut c, keeper_id(), 1_080);
    set_ctx(keeper_id(), 1_080, 0);
    let _ = c.accrue_funding(MARKET_SANDBOX.to_string());

    let snap = c.get_lp_snapshot();
    assert!(snap.total_unrealized_bad_debt_e8.0 >= 100 * USD_SCALE);
    // LP MTM NAV MUST reflect full loss covered by ($200 locked margin + $100+ bad debt reimbursed by Insurance)!
    let lp_mtm: i128 = snap.lp_mtm_equity_e8.parse().unwrap();
    let expected_mtm = (snap.lp_vault_realized_equity_e8.0
        + 200 * USD_SCALE
        + snap.total_unrealized_bad_debt_e8.0) as i128;
    assert_eq!(
        lp_mtm, expected_mtm,
        "Insurance offset must neutralize temporary pre-liquidation bad debt depression in LP NAV"
    );

    // Liquidate at same timestamp 1_080 and confirm realized LP Vault equity equals pre-liquidation MTM NAV bit-for-bit (0 arbitrage profit!)
    set_ctx(keeper_id(), 1_080, 0);
    let _ = c.liquidate_position(1);
    let realized_after = c.get_lp_snapshot().lp_vault_realized_equity_e8.0 as i128;
    assert_eq!(
        realized_after, lp_mtm,
        "Post-liquidation LP realized equity must equal pre-liquidation Insurance-adjusted MTM NAV"
    );
}

#[test]
fn test_bad_debt_cascade_insolvency_pauses_market_safely_without_touching_others() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    register_user(&mut c, bob_id(), 1_000);
    register_user(&mut c, keeper_id(), 1_000);

    // Small LP vault + tiny Insurance -> test extreme uncovered bad debt waterfall
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        2_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    ); // $2k LP
    let _ = deposit_usdc(
        &mut c,
        bob_id(),
        1_000_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    ); // Bob bystander $1k
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        1_000_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );

    push_tick(&mut c, 10 * PRICE_SCALE, 1_000, 1, 1_000);
    set_ctx(alice_id(), 1_001, 0);
    let PromiseOrValue::Value(Some(pid)) = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Short,
        U128(150 * SIZE_SCALE), // $1,500 Short (<= 75% util), $300 IM
        U128(300 * USD_SCALE),
        None,
    ) else {
        panic!("expected id");
    };

    // Ramp price high via 4.5% steps until Short bad debt (`$3,500 - $300 = $3,200`) exceeds Ins + $2,000 LP Vault!
    let mut p = 10 * PRICE_SCALE;
    let mut t = 1_020;
    for seq in 2_u64..38 {
        p = p * 1045 / 1000;
        push_tick(&mut c, p, t, seq, t);
        t += 10;
    }

    set_ctx(keeper_id(), t, 0);
    let _ = c.liquidate_position(pid);

    let m = c.get_market(MARKET_SANDBOX.to_string()).unwrap();
    assert_eq!(m.mode, StoredMarketMode::Paused);
    assert!(m.paused_for_insolvency);
    assert_eq!(c.get_accounting_snapshot().insurance_fund_e8.0, 0);
    // Bystander Bob's $1,000 free_collateral remains 100% untouched!
    assert_eq!(
        c.get_account(bob_id()).unwrap().free_collateral_e8.0,
        1_000 * USD_SCALE
    );
    assert!(c.verify_solvency_invariant());
}

// =========================================================================
// 5. Uniform Global Emergency Settlement, Price Freeze & LP Exit Lock
// =========================================================================

#[test]
fn test_uniform_emergency_settlement_symmetric_freeze_and_lp_exit_lock() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    register_user(&mut c, bob_id(), 1_000);

    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        10_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        1_000_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        bob_id(),
        1_000_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );

    push_tick(&mut c, 8 * PRICE_SCALE, 1_000, 1, 1_000);
    set_ctx(alice_id(), 1_001, 0);
    let PromiseOrValue::Value(Some(long_id)) = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Long,
        U128(100 * SIZE_SCALE),
        U128(200 * USD_SCALE),
        None,
    ) else {
        panic!("expected long id");
    };
    set_ctx(bob_id(), 1_002, 0);
    let PromiseOrValue::Value(Some(short_id)) = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Short,
        U128(100 * SIZE_SCALE),
        U128(200 * USD_SCALE),
        None,
    ) else {
        panic!("expected short id");
    };

    // Valid tick to $8.35/kg (+4.375%) -> Alice ITM (+$35), Bob OTM (-$35)
    push_tick(&mut c, 835_000_000_u128, 1_010, 2, 1_010);

    // At +350s: stale -> calling open fails closed and moves market to ReduceOnly!
    let r_open = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        set_ctx(alice_id(), 1_360, 0);
        let _ = c.open_position(
            MARKET_SANDBOX.to_string(),
            StoredPositionSide::Long,
            U128(10 * SIZE_SCALE),
            U128(50 * USD_SCALE),
            None,
        );
    }));
    assert!(r_open.is_err());

    // Cannot trigger emergency before 3600s outage!
    let r_early = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        set_ctx(keeper_id(), 1_400, 0);
        c.trigger_emergency_settlement(MARKET_SANDBOX.to_string());
    }));
    assert!(r_early.is_err());

    // At 1_010 + 3_600 = 4_610s: anyone may trigger Uniform Global Emergency Settlement!
    set_ctx(keeper_id(), 4_610, 0);
    c.trigger_emergency_settlement(MARKET_SANDBOX.to_string());

    let m = c.get_market(MARKET_SANDBOX.to_string()).unwrap();
    assert_eq!(m.mode, StoredMarketMode::Emergency);
    assert_eq!(m.frozen_settlement_price_e8.unwrap().0, 835_000_000_u128);

    // Impossible to modify price AFTER freezing (`push_sandbox_oracle_price` AND `set_emergency_recovery_price` fail!)
    let r_push = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        push_tick(&mut c, PRICE_SCALE, 4_611, 3, 4_611);
    }));
    assert!(r_push.is_err());
    let r_recover = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        set_ctx(admin_id(), 4_611, 0);
        c.set_emergency_recovery_price(MARKET_SANDBOX.to_string(), U128(5 * PRICE_SCALE));
    }));
    assert!(r_recover.is_err());

    // LP withdrawals MUST stay locked while open positions remain!
    let lp_shares = c.get_account(lp_id()).unwrap().lp_shares.0;
    assert!(c.get_lp_snapshot().withdrawals_locked_for_emergency);
    let r_lp = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        set_ctx(lp_id(), 4_612, 0);
        let _ = c.withdraw_lp(usdc_id(), U128(lp_shares), None);
    }));
    assert!(r_lp.is_err());

    // Both Alice (Long, +$35) and Bob (Short, -$35) settle symmetrically with ZERO close fee (`close_fee = 0`)!
    let prev_long = c.preview_emergency_settlement(long_id).unwrap();
    let prev_short = c.preview_emergency_settlement(short_id).unwrap();
    assert_eq!(prev_long.fee_e8, 0);
    assert_eq!(prev_short.fee_e8, 0);
    // PnL cancels out at frozen price ($35 + -$35 == 0); net vault delta matches pre-match 1s funding paid by Long (`889 e8`)
    assert_eq!(
        prev_long.settlement_pnl_e8 + prev_short.settlement_pnl_e8,
        0
    );
    assert_eq!(
        prev_long.vault_delta_e8 + prev_short.vault_delta_e8,
        -(prev_long.settlement_funding_e8 + prev_short.settlement_funding_e8)
    );

    set_ctx(alice_id(), 4_615, 0);
    c.claim_emergency_settlement(long_id);
    set_ctx(bob_id(), 4_616, 0);
    c.claim_emergency_settlement(short_id);

    // Once open_positions == 0, LP withdrawal unlocks cleanly!
    assert!(!c.get_lp_snapshot().withdrawals_locked_for_emergency);
    set_ctx(lp_id(), 4_620, 0);
    let _p = c.withdraw_lp(usdc_id(), U128(lp_shares), None);
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        4_621,
        vec![PromiseResult::Successful(vec![])],
    );
    c.on_withdraw_transfer(1);
    assert!(c.verify_solvency_invariant());
}

// =========================================================================
// 6. Async External XCC Oracle Safety, Prohibited Markets & Spoofed Callbacks
// =========================================================================

#[test]
#[should_panic(expected = "ERR_ORACLE_CLASS_REJECTED")]
fn test_register_market_rejects_imf_and_gesha_benchmarks() {
    let mut c = setup_contract(1_000);
    set_ctx(admin_id(), 1_000, 0);
    c.register_market(
        "IMFTEST/USD".to_string(),
        OracleSourceConfig::ExternalXcc {
            oracle_contract: oracle_id(),
            oracle_market: "ARABICA-MONTHLY-IMF/USD".to_string(),
        },
        None,
        None,
        None,
    );
}

#[test]
fn test_async_xcc_callbacks_reject_imf_promise_failure_and_replay_safely() {
    set_ctx(admin_id(), 1_000, 0);
    let mut c = PerpsContract::init(
        admin_id(),
        treasury_id(),
        usdc_id(),
        "USDC".to_string(),
        6,
        DISCLOSURE.to_string(),
    );
    c.register_market(
        MARKET_XCC.to_string(),
        OracleSourceConfig::ExternalXcc {
            oracle_contract: oracle_id(),
            oracle_market: "LIVE-ARABICA-TEST/USD".to_string(),
        },
        None,
        None,
        None,
    );
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        10_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        1_000_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );

    // 1. Open via async XCC locks account & stores action_id=1
    set_ctx(alice_id(), 1_005, 0);
    let _ = c.open_position(
        MARKET_XCC.to_string(),
        StoredPositionSide::Long,
        U128(50 * SIZE_SCALE),
        U128(100 * USD_SCALE),
        None,
    );
    assert!(c.get_account(alice_id()).unwrap().pending_action_lock);

    // 2. XCC returns banned `PUBLIC_BENCHMARK_MONTHLY` -> fails closed without panic AND unlocks Alice cleanly!
    let banned_obs = serde_json::to_vec(&Some(ExternalPriceObservation {
        market: "LIVE-ARABICA-TEST/USD".to_string(),
        price: U128(738_863_238),
        decimals: 8,
        unit: "USD/kg".to_string(),
        source: "imf".to_string(),
        source_count: 1,
        observed_at_sec: 1_004,
        stored_at_sec: 1_005,
        nonce: 1,
        data_class: "PUBLIC_BENCHMARK_MONTHLY".to_string(),
    }))
    .unwrap();
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_006,
        vec![PromiseResult::Successful(banned_obs)],
    );
    let _ = c.on_oracle_action(1);
    assert!(!c.get_account(alice_id()).unwrap().pending_action_lock);
    assert_eq!(c.open_position_ids.len(), 0);
    assert!(c.verify_solvency_invariant());

    // 3. Valid XCC response opens position cleanly & replay callback #2 is harmless!
    set_ctx(alice_id(), 1_010, 0);
    let _ = c.open_position(
        MARKET_XCC.to_string(),
        StoredPositionSide::Long,
        U128(50 * SIZE_SCALE),
        U128(100 * USD_SCALE),
        None,
    );
    let valid_obs = serde_json::to_vec(&Some(ExternalPriceObservation {
        market: "LIVE-ARABICA-TEST/USD".to_string(),
        price: U128(8 * PRICE_SCALE),
        decimals: 8,
        unit: "USD/kg".to_string(),
        source: "agg-test".to_string(),
        source_count: 2,
        observed_at_sec: 1_009,
        stored_at_sec: 1_010,
        nonce: 2,
        data_class: "LIVE_AGGREGATED".to_string(),
    }))
    .unwrap();
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_011,
        vec![PromiseResult::Successful(valid_obs.clone())],
    );
    let _ = c.on_oracle_action(2);
    assert_eq!(c.open_position_ids.len(), 1);

    // Duplicate replay of action_id=2 does nothing!
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_012,
        vec![PromiseResult::Successful(valid_obs)],
    );
    let _ = c.on_oracle_action(2);
    assert_eq!(c.open_position_ids.len(), 1);
    assert!(c.verify_solvency_invariant());
}

#[test]
fn test_async_lp_deposit_refunds_on_promise_failure_without_unlock_leak() {
    set_ctx(admin_id(), 1_000, 0);
    let mut c = PerpsContract::init(
        admin_id(),
        treasury_id(),
        usdc_id(),
        "USDC".to_string(),
        6,
        DISCLOSURE.to_string(),
    );
    c.register_market(
        MARKET_XCC.to_string(),
        OracleSourceConfig::ExternalXcc {
            oracle_contract: oracle_id(),
            oracle_market: "LIVE-ARABICA-TEST/USD".to_string(),
        },
        None,
        None,
        None,
    );
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        10_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        1_000_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );

    // Open position via XCC action_id=1
    set_ctx(alice_id(), 1_005, 0);
    let _ = c.open_position(
        MARKET_XCC.to_string(),
        StoredPositionSide::Long,
        U128(50 * SIZE_SCALE),
        U128(100 * USD_SCALE),
        None,
    );
    let valid_obs = serde_json::to_vec(&Some(ExternalPriceObservation {
        market: "LIVE-ARABICA-TEST/USD".to_string(),
        price: U128(8 * PRICE_SCALE),
        decimals: 8,
        unit: "USD/kg".to_string(),
        source: "agg-test".to_string(),
        source_count: 2,
        observed_at_sec: 1_004,
        stored_at_sec: 1_005,
        nonce: 1,
        data_class: "LIVE_AGGREGATED".to_string(),
    }))
    .unwrap();
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_006,
        vec![PromiseResult::Successful(valid_obs)],
    );
    let _ = c.on_oracle_action(1);

    // Bob submits LpDeposit while open XCC market active -> Promise returned (action_id=2)
    register_user(&mut c, bob_id(), 1_007);
    let res = deposit_usdc(
        &mut c,
        bob_id(),
        500_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_008,
    );
    assert!(matches!(res, PromiseOrValue::Promise(_)));
    assert!(c.get_account(bob_id()).unwrap().pending_action_lock);

    // Oracle Promise Failed -> on_oracle_lp_deposit returns full refund U128(500_000_000) & unlocks Bob cleanly!
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_009,
        vec![PromiseResult::Failed],
    );
    let refund = c.on_oracle_lp_deposit(2);
    assert_eq!(refund.0, 500_000_000);
    assert!(!c.get_account(bob_id()).unwrap().pending_action_lock);
    assert_eq!(c.get_account(bob_id()).unwrap().lp_shares.0, 0);
    assert!(c.verify_solvency_invariant());
}

#[test]
#[should_panic(expected = "Method on_oracle_action is private")]
fn test_malicious_external_call_to_private_oracle_callback_rejected() {
    let mut c = setup_contract(1_000);
    set_ctx(accounts(2), 1_001, 0);
    let _ = c.on_oracle_action(1);
}

#[test]
#[should_panic(expected = "Method on_withdraw_transfer is private")]
fn test_malicious_external_call_to_private_withdraw_callback_rejected() {
    let mut c = setup_contract(1_000);
    set_ctx(accounts(2), 1_001, 0);
    c.on_withdraw_transfer(1);
}

#[test]
#[should_panic(expected = "ERR_PRICE_JUMP_CIRCUIT_BREAKER")]
fn test_sandbox_jump_circuit_breaker_gates_to_reduce_only_and_rejects_stale() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        5_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        500_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );

    push_tick(&mut c, 10 * PRICE_SCALE, 1_000, 1, 1_000);
    // +10% jump (>500 bps) triggers ReduceOnly!
    push_tick(&mut c, 11 * PRICE_SCALE, 1_005, 2, 1_005);
    assert_eq!(
        c.get_market(MARKET_SANDBOX.to_string()).unwrap().mode,
        StoredMarketMode::ReduceOnly
    );

    // Direct check: also evaluate DATA_CLASS_TESTNET_SANDBOX sanity helper
    let m = c.get_market(MARKET_SANDBOX.to_string()).unwrap();
    assert_eq!(m.feed_classification, DATA_CLASS_TESTNET_SANDBOX);
    assert!(m.is_synthetic_testnet_sandbox);

    // Attempt open while ReduceOnly fails closed (first test jump rejection path explicitly)
    set_ctx(admin_id(), 1_006, 0);
    c.set_market_mode(MARKET_SANDBOX.to_string(), StoredMarketMode::Normal);
    set_ctx(operator_id(), 1_007, 0);
    // Re-trigger via evaluate directly by resetting previous jump difference
    push_tick(&mut c, 13 * PRICE_SCALE, 1_007, 3, 1_007);
    set_ctx(alice_id(), 1_008, 0);
    // Manually test jump breaker error message via oracle helper
    let mut ms = c.markets.get(&MARKET_SANDBOX.to_string()).unwrap().clone();
    ms.last_valid_price_e8 = Some(10 * PRICE_SCALE);
    c.markets.insert(MARKET_SANDBOX.to_string(), ms.clone());
    ms.mode = StoredMarketMode::Normal;
    c.markets.insert(MARKET_SANDBOX.to_string(), ms);
    let _ = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Long,
        U128(10 * SIZE_SCALE),
        U128(50 * USD_SCALE),
        None,
    );
}

#[test]
#[should_panic(expected = "InsufficientLiquidity")]
fn test_lp_withdraw_restricted_by_cash_available_liquidity() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        1_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        500_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );
    push_tick(&mut c, 10 * PRICE_SCALE, 1_000, 1, 1_000);
    set_ctx(alice_id(), 1_001, 0);
    let _ = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Long,
        U128(60 * SIZE_SCALE), // $600 reserved OI against $1,000 LP -> <= $400 available
        U128(200 * USD_SCALE),
        None,
    );

    let shares = c.get_account(lp_id()).unwrap().lp_shares.0;
    set_ctx(lp_id(), 1_002, 0);
    let _ = c.withdraw_lp(usdc_id(), U128(shares * 9 / 10), None); // Requests ~$900 > ~$400 free
}

#[test]
fn test_negative_funding_indicative_liq_price_and_increase_reduce_continuity() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    register_user(&mut c, bob_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        10_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        2_000_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );
    push_tick(&mut c, 10 * PRICE_SCALE, 1_000, 1, 1_000);

    // Alice opens Short -> Short skew > Long skew -> funding accrues NEGATIVE (`Short` pays `LP/Long`)
    set_ctx(alice_id(), 1_001, 0);
    let PromiseOrValue::Value(Some(short_id)) = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Short,
        U128(100 * SIZE_SCALE),
        U128(250 * USD_SCALE),
        None,
    ) else {
        panic!("expected id");
    };

    let h0 = c.get_position_health(short_id).unwrap();
    let liq0 = h0.indicative_liquidation_price_e8.unwrap().0;

    push_tick(&mut c, 10 * PRICE_SCALE, 1_201, 2, 1_201);
    set_ctx(keeper_id(), 1_201, 0);
    let _ = c.accrue_funding(MARKET_SANDBOX.to_string());
    let m = c.get_market(MARKET_SANDBOX.to_string()).unwrap();
    assert!(
        m.cum_funding_index_e12.parse::<i128>().unwrap() < 0,
        "Short skew must produce negative cumulative funding index"
    );

    // Short payer's indicative liquidation price must drop closer to current price due to negative funding net!
    let h1 = c.get_position_health(short_id).unwrap();
    assert!(h1.funding_net_e8.parse::<i128>().unwrap() < 0);
    assert!(h1.indicative_liquidation_price_e8.unwrap().0 < liq0);

    // Increase Short by +50kg + $150 margin -> settles old slice's funding into margin cleanly
    set_ctx(alice_id(), 1_202, 0);
    let _ = c.increase_position(short_id, U128(50 * SIZE_SCALE), U128(150 * USD_SCALE), None);
    let pos_inc = c.get_position(short_id).unwrap();
    assert_eq!(pos_inc.size_e8.0, 150 * SIZE_SCALE);

    // Accrue more negative funding then partial reduce 50kg (leaving 100kg retaining entry_funding_index)
    push_tick(&mut c, 10 * PRICE_SCALE, 1_302, 3, 1_302);
    set_ctx(keeper_id(), 1_302, 0);
    let _ = c.accrue_funding(MARKET_SANDBOX.to_string());
    set_ctx(alice_id(), 1_303, 0);
    let _ = c.reduce_position(short_id, U128(50 * SIZE_SCALE), None);
    assert_eq!(
        c.get_position(short_id).unwrap().size_e8.0,
        100 * SIZE_SCALE
    );
    let _ = c.close_position(short_id, None);
    assert!(c.verify_solvency_invariant());
}

#[test]
fn test_liquidation_before_winner_ordering_blocks_early_payout_then_succeeds_after_sweep() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    register_user(&mut c, bob_id(), 1_000);
    register_user(&mut c, keeper_id(), 1_000);

    // Fund LP vault with $1,000 + Insurance with $500; Alice Long + Bob Short matched $700 each (<= 75% util!)
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        1_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        bob_id(),
        500_000_000,
        r#"{"action":"insurance_fund"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        1_000_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        bob_id(),
        1_000_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );

    push_tick(&mut c, 10 * PRICE_SCALE, 1_000, 1, 1_000);
    set_ctx(alice_id(), 1_001, 0);
    let PromiseOrValue::Value(Some(long_id)) = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Long,
        U128(70 * SIZE_SCALE), // $700 Long <= $750 (75% utilization!)
        U128(250 * USD_SCALE),
        None,
    ) else {
        panic!("expected long");
    };
    set_ctx(bob_id(), 1_001, 0);
    let PromiseOrValue::Value(Some(short_id)) = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Short,
        U128(70 * SIZE_SCALE), // Matched $700 Short -> `R_LP = 2 * 700 * 0.25 = $350` -> `$650` free!
        U128(250 * USD_SCALE),
        None,
    ) else {
        panic!("expected short");
    };

    // LP withdraws $500 of excess cash, leaving ~$500 cash in `E_LP`!
    let shares = c.get_account(lp_id()).unwrap().lp_shares.0;
    set_ctx(lp_id(), 1_002, 0);
    let _ = c.withdraw_lp(usdc_id(), U128(shares / 2), None);
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_003,
        vec![PromiseResult::Successful(vec![])],
    );
    c.on_withdraw_transfer(1);

    // Advance price smoothly to $19.50/kg (+$665 winner payout > ~$500 current `E_LP_realized`, while Bob Short is underwater!)
    let mut p = 10 * PRICE_SCALE;
    let mut t = 1_010;
    for seq in 2_u64..18 {
        p = (p * 1045 / 1000).min(1_950_000_000);
        push_tick(&mut c, p, t, seq, t);
        t += 10;
    }

    // Attempting to close Alice's +$665 winner BEFORE sweeping Bob's underwater Short rejects with `ERR_LIQUIDATE_UNDERWATER_POSITIONS_FIRST`!
    let r_early = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        set_ctx(alice_id(), t, 0);
        let _ = c.close_position(long_id, None);
    }));
    assert!(r_early.is_err());

    // After Keeper sweeps Bob's underwater Short (depositing Bob's $250 margin + Insurance into `E_LP_realized`), Alice closes cleanly!
    set_ctx(keeper_id(), t, 0);
    let _ = c.liquidate_position(short_id);
    set_ctx(alice_id(), t + 1, 0);
    let _ = c.close_position(long_id, None);
    assert!(c.verify_solvency_invariant());
}

// =========================================================================
// 7. M04.5B.1 — Circle USDC Migration, Pre-Deploy Validation & Edge Cases
// =========================================================================

#[test]
fn test_m045b1_circle_mainnet_usdc_and_intents_uri_rejected() {
    // Circle NEAR Testnet account parses and passes validate_testnet_account_id + contract init.
    assert!(roastake_shared::validate_testnet_account_id(CIRCLE_TESTNET_USDC_ACCOUNT_ID).is_ok());

    // Defuse/Intents asset URI (`nep141:...`) is NOT a NEAR AccountId.
    let intents_id = format!("nep141:{CIRCLE_TESTNET_USDC_ACCOUNT_ID}");
    assert!(intents_id.parse::<AccountId>().is_err());

    // Circle NEAR Mainnet USDC (17208628...) MUST panic on init!
    let r = std::panic::catch_unwind(|| {
        set_ctx(admin_id(), 1_000, 0);
        PerpsContract::init(
            admin_id(),
            treasury_id(),
            CIRCLE_MAINNET_USDC_ACCOUNT_ID.parse().unwrap(),
            "USDC".to_string(),
            6,
            CIRCLE_TESTNET_DISCLOSURE.to_string(),
        );
    });
    assert!(r.is_err(), "Mainnet Circle USDC ID must fail closed");
}

#[test]
fn test_m045b1_circle_metadata_rejects_wrong_spec_and_symbol() {
    let mut c = setup_contract(1_000);
    let bad_spec = serde_json::to_vec(&FungibleTokenMetadata {
        spec: "ft-0.9.0".to_string(),
        name: "USDC".to_string(),
        symbol: "USDC".to_string(),
        icon: None,
        reference: None,
        reference_hash: None,
        decimals: 6,
    })
    .unwrap();
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_001,
        vec![PromiseResult::Successful(bad_spec)],
    );
    assert!(!c.on_verify_collateral_metadata(usdc_id()));
    let view = c.get_collateral_token(usdc_id()).unwrap();
    assert!(!view.active && !view.metadata_verified_on_chain);
}

#[test]
fn test_m045b1_tidepool_rejected_by_default_until_explicit_alternative_fixture_added() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, alice_id(), 1_000);

    // By default, ONLY Circle Testnet USDC is registered -> Tidepool transfers fail closed!
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        set_ctx(tidepool_id(), 1_000, 0);
        let _ = c.ft_on_transfer(
            alice_id(),
            U128(100_000_000),
            r#"{"action":"trader_collateral"}"#.to_string(),
        );
    }));
    assert!(
        res.is_err(),
        "Unapproved Tidepool must fail closed by default"
    );

    // Admin may explicitly allowlist Tidepool strictly as an alternative test fixture.
    set_ctx(admin_id(), 1_001, 0);
    c.add_collateral_token(
        tidepool_id(),
        "USDC".to_string(),
        6,
        "Tidepool alternative test fixture only".to_string(),
    );
    set_ctx(tidepool_id(), 1_002, 0);
    let accepted = c.ft_on_transfer(
        alice_id(),
        U128(25_000_000),
        r#"{"action":"trader_collateral"}"#.to_string(),
    );
    assert!(matches!(accepted, PromiseOrValue::Value(U128(0))));
    assert_eq!(
        c.get_account(alice_id()).unwrap().free_collateral_e8.0,
        2_500_000_000
    );
    assert!(c.verify_solvency_invariant());
}

#[test]
fn test_m045b1_6_to_8_partial_withdrawals_over_withdraw_rejection_and_residues() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);

    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        10_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        500_000_000, // 500.000000 Circle USDC -> 500_00000000 e8
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );

    // Create sub-micro-USDC odd balance via fractional size open & close
    push_tick(&mut c, 8 * PRICE_SCALE, 1_000, 1, 1_000);
    set_ctx(alice_id(), 1_001, 0);
    let PromiseOrValue::Value(Some(pos_id)) = c.open_position(
        MARKET_SANDBOX.to_string(),
        StoredPositionSide::Long,
        U128(3_333_333_333),
        U128(100 * USD_SCALE),
        None,
    ) else {
        panic!("expected pos");
    };
    push_tick(&mut c, 811_111_111, 1_002, 2, 1_002);
    set_ctx(alice_id(), 1_003, 0);
    let _ = c.close_position(pos_id, None);

    let acc0 = c.get_account(alice_id()).unwrap();
    let total_e8 = acc0.free_collateral_e8.0;
    let total_units = acc0.withdrawable_token_units_at_scale_100.0;
    let residue_e8 = acc0.sub_micro_unit_residue_e8.0;
    assert_eq!(total_units * 100 + residue_e8, total_e8);
    assert!(residue_e8 > 0 && residue_e8 < 100);

    // Over-withdrawal of even +1 base unit MUST fail closed!
    let over_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        set_ctx(alice_id(), 1_004, 0);
        let _ = c.withdraw_collateral(usdc_id(), Some(U128(total_units + 1)));
    }));
    assert!(over_res.is_err());

    // Partial withdrawal of 50.000000 USDC succeeds and settles cleanly
    set_ctx(alice_id(), 1_004, 0);
    let _ = c.withdraw_collateral(usdc_id(), Some(U128(50_000_000)));
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_005,
        vec![PromiseResult::Successful(vec![])],
    );
    c.on_withdraw_transfer(1);
    let acc1 = c.get_account(alice_id()).unwrap();
    assert_eq!(acc1.free_collateral_e8.0, total_e8 - 5_000_000_000);
    assert_eq!(acc1.sub_micro_unit_residue_e8.0, residue_e8);

    // Second partial withdrawal (`None` -> all remaining whole units) leaves exact `<100 e8` residue!
    set_ctx(alice_id(), 1_006, 0);
    let _ = c.withdraw_collateral(usdc_id(), None);
    set_ctx_with_promises(
        "perps.testnet".parse().unwrap(),
        1_007,
        vec![PromiseResult::Successful(vec![])],
    );
    c.on_withdraw_transfer(2);
    let acc2 = c.get_account(alice_id()).unwrap();
    assert_eq!(acc2.free_collateral_e8.0, residue_e8);
    assert!(c.verify_solvency_invariant());
}

#[test]
fn test_m045b1_sandbox_feed_fail_closed_when_missing_or_stale_and_market_view_transparent() {
    let mut c = setup_contract(1_000);
    register_user(&mut c, lp_id(), 1_000);
    register_user(&mut c, alice_id(), 1_000);
    let _ = deposit_usdc(
        &mut c,
        lp_id(),
        10_000_000_000,
        r#"{"action":"lp_deposit"}"#,
        1_000,
    );
    let _ = deposit_usdc(
        &mut c,
        alice_id(),
        500_000_000,
        r#"{"action":"trader_collateral"}"#,
        1_000,
    );

    // No price pushed yet -> open_position MUST fail closed with ERR_ORACLE_PRICE_MISSING!
    let miss_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        set_ctx(alice_id(), 1_000, 0);
        let _ = c.open_position(
            MARKET_SANDBOX.to_string(),
            StoredPositionSide::Long,
            U128(10 * SIZE_SCALE),
            U128(20 * USD_SCALE),
            None,
        );
    }));
    assert!(miss_res.is_err());

    // Push valid tick and verify transparent TESTNET_SANDBOX_FEED view metadata
    push_tick(&mut c, 850_000_000, 1_000, 1, 1_000);
    let mv = c.get_market(MARKET_SANDBOX.to_string()).unwrap();
    assert_eq!(mv.feed_classification, DATA_CLASS_TESTNET_SANDBOX);
    assert!(mv.is_synthetic_testnet_sandbox);
    assert_eq!(mv.sandbox_operator, Some(operator_id()));
    assert_eq!(mv.sandbox_min_price_e8, Some(U128(PRICE_SCALE)));
    assert_eq!(mv.sandbox_max_price_e8, Some(U128(500 * PRICE_SCALE)));
    assert_eq!(mv.sandbox_sequence, Some(1));
    assert_eq!(mv.last_valid_observed_sec, Some(1_000));

    // Stale >300s later -> open_position fails closed!
    let stale_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        set_ctx(alice_id(), 1_305, 0);
        let _ = c.open_position(
            MARKET_SANDBOX.to_string(),
            StoredPositionSide::Long,
            U128(10 * SIZE_SCALE),
            U128(20 * USD_SCALE),
            None,
        );
    }));
    assert!(stale_res.is_err());

    // Price out of bounds ($0.50 < $1.00 min) -> push_sandbox_oracle_price fails closed!
    let oob_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        push_tick(&mut c, 50_000_000, 1_005, 2, 1_005);
    }));
    assert!(oob_res.is_err());
}

#[test]
fn test_m045b1_u128_json_abi_deserialization() {
    // Verify exact JSON strings emitted by scripts/deploy-perpetuals-testnet.ps1 deserialize cleanly!
    let src_json = r#"{"TestnetSandboxOperator":{"oracle_operator":"roastake-dev-mta.testnet","min_price_e8":"100000000","max_price_e8":"50000000000"}}"#;
    let src: OracleSourceConfig =
        serde_json::from_str(src_json).expect("valid U128 OracleSourceConfig");
    match src {
        OracleSourceConfig::TestnetSandboxOperator {
            min_price_e8,
            max_price_e8,
            ..
        } => {
            assert_eq!(min_price_e8.0, 100_000_000);
            assert_eq!(max_price_e8.0, 50_000_000_000);
        }
        _ => panic!("expected TestnetSandboxOperator"),
    }

    let policy_json = r#"{"allow_testnet_sandbox":true,"max_execution_age_sec":300,"emergency_close_age_sec":3600,"max_jump_bps":500,"max_clock_drift_sec":60}"#;
    let policy: StoredOraclePolicy = serde_json::from_str(policy_json).expect("valid policy");
    assert_eq!(policy.max_execution_age_sec, 300);
    assert_eq!(policy.emergency_close_age_sec, 3600);
}
