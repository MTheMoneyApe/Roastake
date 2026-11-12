// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

/*!
Reusable ROASTAKE Coffee Asset contract (NEP-141 / NEP-145 / NEP-148) for NEAR Testnet.

Each instance represents a single coffee variant where 1 token (`10^8` base units,
`decimals = 8`) is the reference unit for 1 kg of that variant. Initial Testnet instances
carry `SIMULATED_TESTNET_INVENTORY` only, enforce an explicit `supply_cap`, disable
physical redemption fail-closed, and keep an optional `reserve_registry_id` hook ready
for a later Model C independently verifiable reserve registry.
*/

use near_sdk::collections::LookupMap;
use near_sdk::json_types::{Base64VecU8, U128};
use near_sdk::{
    env, ext_contract, near, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseOrValue,
};
use serde::Serialize;

pub const COFFEE_TOKEN_DECIMALS: u8 = 8;
pub const ONE_KG_BASE_UNITS: u128 = 100_000_000;
pub const STORAGE_BOUND_YOCTO: u128 = 1_250_000_000_000_000_000_000; // 0.00125 NEAR
pub const BACKING_CLASSIFICATION_SIMULATED: &str = "SIMULATED_TESTNET_INVENTORY";
pub const UNIT_REPRESENTATION_LABEL: &str =
    "1 token (10^8 base units, 8 decimals) = 1 kg coffee variant reference unit; fractional balances permitted";
pub const SIMULATED_INVENTORY_DISCLOSURE: &str =
    "SIMULATED_TESTNET_INVENTORY: Testnet supply is capped against simulated demonstration inventory only. Tokens do not represent currently verified or physically custodied coffee, and physical redemption is disabled.";

const FT_RESOLVE_TRANSFER_GAS: Gas = Gas::from_tgas(20);
const FT_TRANSFER_CALL_OVERHEAD_GAS: Gas = Gas::from_tgas(35);
const MIN_FT_ON_TRANSFER_GAS: Gas = Gas::from_tgas(10);

fn is_disallowed_mainnet_account(account_id: &AccountId) -> bool {
    let raw = account_id.as_str().to_ascii_lowercase();
    raw == "near" || (raw.ends_with(".near") && !raw.ends_with(".test.near"))
}

fn assert_testnet_or_sandbox_account(account_id: &AccountId, context: &str) {
    assert!(
        !is_disallowed_mainnet_account(account_id),
        "ERR_MAINNET_ACCOUNT_DISALLOWED:{context}"
    );
}

#[ext_contract(ext_ft_receiver)]
pub trait FungibleTokenReceiver {
    fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128>;
}

#[near(serializers = [json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FungibleTokenMetadata {
    pub spec: String,
    pub name: String,
    pub symbol: String,
    pub icon: Option<String>,
    pub reference: Option<String>,
    pub reference_hash: Option<Base64VecU8>,
    pub decimals: u8,
}

#[near(serializers = [json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageBalance {
    pub total: U128,
    pub available: U128,
}

#[near(serializers = [json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageBalanceBounds {
    pub min: U128,
    pub max: U128,
}

#[near(serializers = [json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoffeeAssetBackingInfo {
    pub variant: String,
    pub symbol: String,
    pub decimals: u8,
    pub total_supply: U128,
    pub supply_cap: U128,
    pub remaining_mintable_supply: U128,
    pub reference_price_usd_e8: U128,
    pub unit_representation: String,
    pub backing_classification: String,
    pub physical_redemption_enabled: bool,
    pub reserve_registry_id: Option<AccountId>,
    pub disclosure: String,
    pub owner_id: AccountId,
    pub minter_id: AccountId,
}

#[derive(Serialize)]
struct Nep297Event<'a, T: Serialize> {
    standard: &'a str,
    version: &'a str,
    event: &'a str,
    data: &'a [T],
}

#[derive(Serialize)]
struct FtMintEvent<'a> {
    owner_id: &'a str,
    amount: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    memo: Option<&'a str>,
}

#[derive(Serialize)]
struct FtTransferEvent<'a> {
    old_owner_id: &'a str,
    new_owner_id: &'a str,
    amount: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    memo: Option<&'a str>,
}

#[derive(Serialize)]
struct FtBurnEvent<'a> {
    owner_id: &'a str,
    amount: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    memo: Option<&'a str>,
}

fn emit_nep141_event<T: Serialize>(event_name: &str, item: &T) {
    let payload = Nep297Event {
        standard: "nep141",
        version: "1.0.0",
        event: event_name,
        data: std::slice::from_ref(item),
    };
    if let Ok(json) = serde_json::to_string(&payload) {
        env::log_str(&format!("EVENT_JSON:{json}"));
    }
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct CoffeeAssetToken {
    pub owner_id: AccountId,
    pub minter_id: AccountId,
    pub variant: String,
    pub name: String,
    pub symbol: String,
    pub icon: Option<String>,
    pub reference: Option<String>,
    pub reference_hash: Option<Base64VecU8>,
    pub total_supply: u128,
    pub supply_cap: u128,
    pub reference_price_usd_e8: u128,
    pub reserve_registry_id: Option<AccountId>,
    pub balances: LookupMap<AccountId, u128>,
}

#[near]
impl CoffeeAssetToken {
    #[init]
    pub fn new(
        owner_id: AccountId,
        minter_id: AccountId,
        variant: String,
        name: String,
        symbol: String,
        supply_cap: U128,
        reference_price_usd_e8: U128,
        icon: Option<String>,
        reference: Option<String>,
    ) -> Self {
        assert_testnet_or_sandbox_account(&owner_id, "owner_id");
        assert_testnet_or_sandbox_account(&minter_id, "minter_id");
        assert!(!variant.trim().is_empty(), "ERR_EMPTY_VARIANT");
        assert!(!name.trim().is_empty(), "ERR_EMPTY_NAME");
        assert!(!symbol.trim().is_empty(), "ERR_EMPTY_SYMBOL");
        assert!(supply_cap.0 > 0, "ERR_ZERO_SUPPLY_CAP");
        assert!(reference_price_usd_e8.0 > 0, "ERR_ZERO_REFERENCE_PRICE");

        Self {
            owner_id,
            minter_id,
            variant,
            name,
            symbol,
            icon,
            reference,
            reference_hash: None,
            total_supply: 0,
            supply_cap: supply_cap.0,
            reference_price_usd_e8: reference_price_usd_e8.0,
            reserve_registry_id: None,
            balances: LookupMap::new(b"b"),
        }
    }

    pub fn get_owner(&self) -> AccountId {
        self.owner_id.clone()
    }

    pub fn get_minter(&self) -> AccountId {
        self.minter_id.clone()
    }

    pub fn get_supply_cap(&self) -> U128 {
        U128(self.supply_cap)
    }

    pub fn get_backing_classification(&self) -> String {
        BACKING_CLASSIFICATION_SIMULATED.to_string()
    }

    pub fn is_physical_redemption_enabled(&self) -> bool {
        false
    }

    pub fn get_reserve_registry(&self) -> Option<AccountId> {
        self.reserve_registry_id.clone()
    }

    pub fn get_backing_info(&self) -> CoffeeAssetBackingInfo {
        CoffeeAssetBackingInfo {
            variant: self.variant.clone(),
            symbol: self.symbol.clone(),
            decimals: COFFEE_TOKEN_DECIMALS,
            total_supply: U128(self.total_supply),
            supply_cap: U128(self.supply_cap),
            remaining_mintable_supply: U128(self.supply_cap.saturating_sub(self.total_supply)),
            reference_price_usd_e8: U128(self.reference_price_usd_e8),
            unit_representation: UNIT_REPRESENTATION_LABEL.to_string(),
            backing_classification: BACKING_CLASSIFICATION_SIMULATED.to_string(),
            physical_redemption_enabled: false,
            reserve_registry_id: self.reserve_registry_id.clone(),
            disclosure: SIMULATED_INVENTORY_DISCLOSURE.to_string(),
            owner_id: self.owner_id.clone(),
            minter_id: self.minter_id.clone(),
        }
    }

    #[payable]
    pub fn set_minter(&mut self, minter_id: AccountId) {
        self.assert_one_yocto();
        self.assert_owner();
        assert_testnet_or_sandbox_account(&minter_id, "minter_id");
        self.minter_id = minter_id;
    }

    #[payable]
    pub fn set_reserve_registry(&mut self, reserve_registry_id: Option<AccountId>) {
        self.assert_one_yocto();
        self.assert_owner();
        if let Some(ref registry) = reserve_registry_id {
            assert_testnet_or_sandbox_account(registry, "reserve_registry_id");
        }
        self.reserve_registry_id = reserve_registry_id;
    }

    pub fn mint(&mut self, account_id: AccountId, amount: U128) {
        assert_eq!(
            env::predecessor_account_id(),
            self.minter_id,
            "ERR_UNAUTHORIZED_MINTER"
        );
        assert_testnet_or_sandbox_account(&account_id, "mint_account_id");
        assert!(amount.0 > 0, "ERR_ZERO_MINT_AMOUNT");
        let existing = self
            .balances
            .get(&account_id)
            .expect("ERR_ACCOUNT_NOT_REGISTERED");

        let new_total = self
            .total_supply
            .checked_add(amount.0)
            .expect("ERR_SUPPLY_OVERFLOW");
        assert!(new_total <= self.supply_cap, "ERR_SUPPLY_CAP_EXCEEDED");

        let new_balance = existing
            .checked_add(amount.0)
            .expect("ERR_BALANCE_OVERFLOW");
        self.balances.insert(&account_id, &new_balance);
        self.total_supply = new_total;

        let amount_str = amount.0.to_string();
        emit_nep141_event(
            "ft_mint",
            &FtMintEvent {
                owner_id: account_id.as_str(),
                amount: &amount_str,
                memo: Some(BACKING_CLASSIFICATION_SIMULATED),
            },
        );
    }

    #[payable]
    pub fn request_physical_redemption(&mut self, _amount: U128) {
        env::panic_str(
            "ERR_PHYSICAL_REDEMPTION_DISABLED: Token carries SIMULATED_TESTNET_INVENTORY only; physical custody and redemption are disabled.",
        );
    }

    #[payable]
    pub fn storage_deposit(
        &mut self,
        account_id: Option<AccountId>,
        _registration_only: Option<bool>,
    ) -> StorageBalance {
        let target = account_id.unwrap_or_else(env::predecessor_account_id);
        assert_testnet_or_sandbox_account(&target, "storage_deposit_target");
        let attached = env::attached_deposit().as_yoctonear();

        if self.balances.contains_key(&target) {
            if attached > 0 {
                Promise::new(env::predecessor_account_id())
                    .transfer(NearToken::from_yoctonear(attached))
                    .detach();
            }
            return StorageBalance {
                total: U128(STORAGE_BOUND_YOCTO),
                available: U128(0),
            };
        }

        assert!(
            attached >= STORAGE_BOUND_YOCTO,
            "The attached deposit is less than minimum storage balance"
        );
        let refund = attached - STORAGE_BOUND_YOCTO;
        if refund > 0 {
            Promise::new(env::predecessor_account_id())
                .transfer(NearToken::from_yoctonear(refund))
                .detach();
        }
        self.balances.insert(&target, &0);
        StorageBalance {
            total: U128(STORAGE_BOUND_YOCTO),
            available: U128(0),
        }
    }

    #[payable]
    pub fn storage_withdraw(&mut self, amount: Option<U128>) -> StorageBalance {
        self.assert_one_yocto();
        let caller = env::predecessor_account_id();
        assert!(
            self.balances.contains_key(&caller),
            "ERR_ACCOUNT_NOT_REGISTERED"
        );
        if let Some(requested) = amount {
            assert_eq!(requested.0, 0, "The amount is greater than the available storage balance");
        }
        StorageBalance {
            total: U128(STORAGE_BOUND_YOCTO),
            available: U128(0),
        }
    }

    #[payable]
    pub fn storage_unregister(&mut self, force: Option<bool>) -> bool {
        self.assert_one_yocto();
        let caller = env::predecessor_account_id();
        let Some(bal) = self.balances.get(&caller) else {
            return false;
        };
        if bal > 0 {
            assert!(
                force.unwrap_or(false),
                "Can't unregister the account with the positive balance without force"
            );
            self.total_supply = self.total_supply.saturating_sub(bal);
            let bal_str = bal.to_string();
            emit_nep141_event(
                "ft_burn",
                &FtBurnEvent {
                    owner_id: caller.as_str(),
                    amount: &bal_str,
                    memo: Some("force_storage_unregister"),
                },
            );
        }
        self.balances.remove(&caller);
        Promise::new(caller)
            .transfer(NearToken::from_yoctonear(STORAGE_BOUND_YOCTO))
            .detach();
        true
    }

    pub fn storage_balance_bounds(&self) -> StorageBalanceBounds {
        StorageBalanceBounds {
            min: U128(STORAGE_BOUND_YOCTO),
            max: U128(STORAGE_BOUND_YOCTO),
        }
    }

    pub fn storage_balance_of(&self, account_id: AccountId) -> Option<StorageBalance> {
        if self.balances.contains_key(&account_id) {
            Some(StorageBalance {
                total: U128(STORAGE_BOUND_YOCTO),
                available: U128(0),
            })
        } else {
            None
        }
    }

    pub fn ft_metadata(&self) -> FungibleTokenMetadata {
        FungibleTokenMetadata {
            spec: "ft-1.0.0".to_string(),
            name: self.name.clone(),
            symbol: self.symbol.clone(),
            icon: self.icon.clone(),
            reference: self.reference.clone(),
            reference_hash: self.reference_hash.clone(),
            decimals: COFFEE_TOKEN_DECIMALS,
        }
    }

    pub fn ft_total_supply(&self) -> U128 {
        U128(self.total_supply)
    }

    pub fn ft_balance_of(&self, account_id: AccountId) -> U128 {
        U128(self.balances.get(&account_id).unwrap_or(0))
    }

    #[payable]
    pub fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>) {
        self.assert_one_yocto();
        assert_testnet_or_sandbox_account(&receiver_id, "ft_transfer_receiver");
        let sender_id = env::predecessor_account_id();
        self.internal_transfer(&sender_id, &receiver_id, amount.0, memo.as_deref());
    }

    #[payable]
    pub fn ft_transfer_call(
        &mut self,
        receiver_id: AccountId,
        amount: U128,
        memo: Option<String>,
        msg: String,
    ) -> PromiseOrValue<U128> {
        self.assert_one_yocto();
        assert_testnet_or_sandbox_account(&receiver_id, "ft_transfer_call_receiver");
        let sender_id = env::predecessor_account_id();

        let reserved = FT_RESOLVE_TRANSFER_GAS
            .as_gas()
            .saturating_add(FT_TRANSFER_CALL_OVERHEAD_GAS.as_gas())
            .saturating_add(env::used_gas().as_gas());
        let on_transfer_gas = env::prepaid_gas().as_gas().saturating_sub(reserved);
        assert!(
            on_transfer_gas >= MIN_FT_ON_TRANSFER_GAS.as_gas(),
            "ERR_INSUFFICIENT_PREPAID_GAS"
        );

        self.internal_transfer(&sender_id, &receiver_id, amount.0, memo.as_deref());

        let promise = ext_ft_receiver::ext(receiver_id.clone())
            .with_static_gas(Gas::from_gas(on_transfer_gas))
            .ft_on_transfer(sender_id.clone(), amount, msg)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(FT_RESOLVE_TRANSFER_GAS)
                    .ft_resolve_transfer(sender_id, receiver_id, amount),
            );
        PromiseOrValue::Promise(promise)
    }

    #[private]
    pub fn ft_resolve_transfer(
        &mut self,
        sender_id: AccountId,
        receiver_id: AccountId,
        amount: U128,
    ) -> U128 {
        let unused = match env::promise_result_checked(0, 256) {
            Ok(bytes) => serde_json::from_slice::<U128>(&bytes)
                .map(|v| v.0.min(amount.0))
                .unwrap_or(0),
            Err(_) => amount.0,
        };
        if unused > 0 {
            let receiver_balance = self.balances.get(&receiver_id).unwrap_or(0);
            let refund = unused.min(receiver_balance);
            if refund > 0 {
                if let Some(sender_balance) = self.balances.get(&sender_id) {
                    self.balances
                        .insert(&receiver_id, &(receiver_balance - refund));
                    self.balances.insert(
                        &sender_id,
                        &sender_balance.checked_add(refund).expect("ERR_BALANCE_OVERFLOW"),
                    );
                    let refund_str = refund.to_string();
                    emit_nep141_event(
                        "ft_transfer",
                        &FtTransferEvent {
                            old_owner_id: receiver_id.as_str(),
                            new_owner_id: sender_id.as_str(),
                            amount: &refund_str,
                            memo: Some("refund"),
                        },
                    );
                    return U128(amount.0 - refund);
                }
            }
        }
        U128(amount.0)
    }

    fn assert_one_yocto(&self) {
        assert_eq!(
            env::attached_deposit().as_yoctonear(),
            1,
            "Requires attached deposit of exactly 1 yoctoNEAR"
        );
    }

    fn assert_owner(&self) {
        assert_eq!(
            env::predecessor_account_id(),
            self.owner_id,
            "ERR_UNAUTHORIZED_OWNER"
        );
    }

    fn internal_transfer(
        &mut self,
        sender_id: &AccountId,
        receiver_id: &AccountId,
        amount: u128,
        memo: Option<&str>,
    ) {
        assert_ne!(sender_id, receiver_id, "Sender and receiver should be different");
        assert!(amount > 0, "The amount should be a positive number");
        let sender_balance = self
            .balances
            .get(sender_id)
            .expect("The account is not registered");
        assert!(
            sender_balance >= amount,
            "The account doesn't have enough balance"
        );
        let receiver_balance = self
            .balances
            .get(receiver_id)
            .expect("The account is not registered");

        self.balances.insert(sender_id, &(sender_balance - amount));
        self.balances.insert(
            receiver_id,
            &receiver_balance
                .checked_add(amount)
                .expect("ERR_BALANCE_OVERFLOW"),
        );

        let amount_str = amount.to_string();
        emit_nep141_event(
            "ft_transfer",
            &FtTransferEvent {
                old_owner_id: sender_id.as_str(),
                new_owner_id: receiver_id.as_str(),
                amount: &amount_str,
                memo,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::{get_logs, VMContextBuilder};
    use near_sdk::{testing_env, VmPromiseResult};

    fn acct(name: &str) -> AccountId {
        name.parse().expect("valid account id")
    }

    fn setup_ctx(predecessor: &str, deposit_yocto: u128) -> VMContextBuilder {
        let mut builder = VMContextBuilder::new();
        builder
            .current_account_id(acct("arabica.roastake-dev-mta.testnet"))
            .predecessor_account_id(acct(predecessor))
            .attached_deposit(NearToken::from_yoctonear(deposit_yocto))
            .prepaid_gas(Gas::from_tgas(200));
        builder
    }

    fn init_arabica() -> CoffeeAssetToken {
        testing_env!(setup_ctx("roastake-dev-mta.testnet", 0).build());
        CoffeeAssetToken::new(
            acct("roastake-dev-mta.testnet"),
            acct("roastake-dev-mta.testnet"),
            "COFFEE-ARABICA".to_string(),
            "ROASTAKE Testnet Arabica".to_string(),
            "rARABICA".to_string(),
            U128(1_000_000_000), // 10.00 kg cap
            U128(1_000_000_000), // $10.00/kg
            None,
            Some("Simulated NEAR Testnet coffee asset benchmarked to $10/kg.".to_string()),
        )
    }

    #[test]
    fn metadata_and_backing_report_simulated_testnet_inventory() {
        let contract = init_arabica();
        let meta = contract.ft_metadata();
        assert_eq!(meta.spec, "ft-1.0.0");
        assert_eq!(meta.symbol, "rARABICA");
        assert_eq!(meta.decimals, 8);
        assert_eq!(
            contract.get_backing_classification(),
            "SIMULATED_TESTNET_INVENTORY"
        );
        assert!(!contract.is_physical_redemption_enabled());
        assert_eq!(contract.get_supply_cap(), U128(1_000_000_000));
        assert_eq!(contract.get_reserve_registry(), None);

        let info = contract.get_backing_info();
        assert_eq!(info.variant, "COFFEE-ARABICA");
        assert_eq!(info.backing_classification, "SIMULATED_TESTNET_INVENTORY");
        assert!(!info.physical_redemption_enabled);
        assert_eq!(info.reference_price_usd_e8, U128(1_000_000_000));
        assert!(info.disclosure.contains("SIMULATED_TESTNET_INVENTORY"));
    }

    #[test]
    #[should_panic(expected = "ERR_UNAUTHORIZED_MINTER")]
    fn unauthorized_mint_panics() {
        let mut contract = init_arabica();
        testing_env!(setup_ctx("roastake-dev-mta.testnet", STORAGE_BOUND_YOCTO).build());
        contract.storage_deposit(Some(acct("majorblog5570.testnet")), Some(true));

        testing_env!(setup_ctx("majorblog5570.testnet", 0).build());
        contract.mint(acct("majorblog5570.testnet"), U128(100_000_000));
    }

    #[test]
    #[should_panic(expected = "ERR_SUPPLY_CAP_EXCEEDED")]
    fn supply_cap_cannot_be_exceeded() {
        let mut contract = init_arabica();
        testing_env!(setup_ctx("roastake-dev-mta.testnet", STORAGE_BOUND_YOCTO).build());
        contract.storage_deposit(Some(acct("roastake-dev-mta.testnet")), Some(true));

        testing_env!(setup_ctx("roastake-dev-mta.testnet", 0).build());
        contract.mint(acct("roastake-dev-mta.testnet"), U128(600_000_000));
        contract.mint(acct("roastake-dev-mta.testnet"), U128(500_000_000));
    }

    #[test]
    #[should_panic(expected = "ERR_PHYSICAL_REDEMPTION_DISABLED")]
    fn physical_redemption_fails_closed() {
        let mut contract = init_arabica();
        testing_env!(setup_ctx("roastake-dev-mta.testnet", 1).build());
        contract.request_physical_redemption(U128(10_000_000));
    }

    #[test]
    #[should_panic(expected = "ERR_MAINNET_ACCOUNT_DISALLOWED")]
    fn mainnet_accounts_are_rejected_fail_closed() {
        let mut contract = init_arabica();
        testing_env!(setup_ctx("roastake-dev-mta.testnet", STORAGE_BOUND_YOCTO).build());
        contract.storage_deposit(Some(acct("attacker.near")), Some(true));
    }

    #[test]
    fn storage_and_transfer_update_balances_and_emit_nep297() {
        let mut contract = init_arabica();
        assert_eq!(
            contract.storage_balance_bounds(),
            StorageBalanceBounds {
                min: U128(STORAGE_BOUND_YOCTO),
                max: U128(STORAGE_BOUND_YOCTO)
            }
        );
        assert_eq!(contract.storage_balance_of(acct("majorblog5570.testnet")), None);

        testing_env!(setup_ctx("roastake-dev-mta.testnet", STORAGE_BOUND_YOCTO).build());
        let sb = contract.storage_deposit(Some(acct("roastake-dev-mta.testnet")), Some(true));
        assert_eq!(sb.total, U128(STORAGE_BOUND_YOCTO));

        testing_env!(setup_ctx("majorblog5570.testnet", STORAGE_BOUND_YOCTO * 2).build());
        contract.storage_deposit(None, Some(true));
        assert!(contract
            .storage_balance_of(acct("majorblog5570.testnet"))
            .is_some());

        testing_env!(setup_ctx("roastake-dev-mta.testnet", 0).build());
        contract.mint(acct("roastake-dev-mta.testnet"), U128(100_000_000)); // 1.00 kg
        assert_eq!(contract.ft_total_supply(), U128(100_000_000));

        testing_env!(setup_ctx("roastake-dev-mta.testnet", 1).build());
        contract.ft_transfer(
            acct("majorblog5570.testnet"),
            U128(25_000_000),
            Some("demo-lot-slice".to_string()),
        );
        assert_eq!(
            contract.ft_balance_of(acct("roastake-dev-mta.testnet")),
            U128(75_000_000)
        );
        assert_eq!(
            contract.ft_balance_of(acct("majorblog5570.testnet")),
            U128(25_000_000)
        );
        let logs = get_logs();
        assert!(logs.iter().any(|l| l.contains("\"event\":\"ft_transfer\"")));
    }

    #[test]
    #[should_panic(expected = "The attached deposit is less than minimum storage balance")]
    fn storage_deposit_rejects_underpayment() {
        let mut contract = init_arabica();
        testing_env!(setup_ctx("majorblog5570.testnet", STORAGE_BOUND_YOCTO - 1).build());
        contract.storage_deposit(None, Some(true));
    }

    #[test]
    fn storage_unregister_and_ft_resolve_transfer_refund_work() {
        let mut contract = init_arabica();
        testing_env!(setup_ctx("roastake-dev-mta.testnet", STORAGE_BOUND_YOCTO).build());
        contract.storage_deposit(Some(acct("roastake-dev-mta.testnet")), Some(true));
        contract.storage_deposit(Some(acct("ref-finance-101.testnet")), Some(true));
        contract.storage_deposit(Some(acct("temp.testnet")), Some(true));

        testing_env!(setup_ctx("roastake-dev-mta.testnet", 0).build());
        contract.mint(acct("roastake-dev-mta.testnet"), U128(100_000_000));

        // Simulate ft_transfer_call moving 10_000_000 then ref returning 4_000_000 unused.
        testing_env!(setup_ctx("roastake-dev-mta.testnet", 1).build());
        contract.ft_transfer_call(
            acct("ref-finance-101.testnet"),
            U128(10_000_000),
            None,
            "{}".to_string(),
        );
        assert_eq!(
            contract.ft_balance_of(acct("ref-finance-101.testnet")),
            U128(10_000_000)
        );

        let resolve_ctx = setup_ctx("arabica.roastake-dev-mta.testnet", 0).build();
        testing_env!(
            resolve_ctx,
            near_sdk::test_vm_config(),
            near_sdk::RuntimeFeesConfig::test(),
            Default::default(),
            vec![near_sdk::PromiseResult::Successful(
                b"\"4000000\"".to_vec()
            )]
        );
        let used = contract.ft_resolve_transfer(
            acct("roastake-dev-mta.testnet"),
            acct("ref-finance-101.testnet"),
            U128(10_000_000),
        );
        assert_eq!(used, U128(6_000_000));
        assert_eq!(
            contract.ft_balance_of(acct("roastake-dev-mta.testnet")),
            U128(94_000_000)
        );
        assert_eq!(
            contract.ft_balance_of(acct("ref-finance-101.testnet")),
            U128(6_000_000)
        );

        // Unregister empty account cleanly
        testing_env!(setup_ctx("temp.testnet", 1).build());
        assert!(contract.storage_unregister(Some(false)));
        assert_eq!(contract.storage_balance_of(acct("temp.testnet")), None);
    }

    #[test]
    fn owner_can_register_placeholder_reserve_registry_and_update_minter() {
        let mut contract = init_arabica();
        testing_env!(setup_ctx("roastake-dev-mta.testnet", 1).build());
        contract.set_reserve_registry(Some(acct("registry.roastake-dev-mta.testnet")));
        contract.set_minter(acct("minter.roastake-dev-mta.testnet"));
        assert_eq!(
            contract.get_reserve_registry(),
            Some(acct("registry.roastake-dev-mta.testnet"))
        );
        assert_eq!(contract.get_minter(), acct("minter.roastake-dev-mta.testnet"));
        assert!(!contract.is_physical_redemption_enabled());
    }
}
