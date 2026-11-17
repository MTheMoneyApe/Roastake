// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

//! Local NEAR Sandbox NEP-141 / NEP-145 / NEP-148 fixture matching the live ABI
//! verified on `usdc.tidepool-sandbox.testnet` (6 decimals, 0.00125 NEAR storage bounds).

use near_sdk::collections::{LookupMap, LookupSet};
use near_sdk::json_types::U128;
use near_sdk::{
    env, ext_contract, near, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseOrValue,
};

const STORAGE_BOUND_YOCTO: u128 = 1_250_000_000_000_000_000_000; // 0.00125 NEAR
const FT_ON_TRANSFER_GAS: Gas = Gas::from_tgas(180);
const FT_RESOLVE_TRANSFER_GAS: Gas = Gas::from_tgas(30);

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
    pub reference_hash: Option<String>,
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

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct MockTidepoolFt {
    pub owner_id: AccountId,
    pub minters: LookupSet<AccountId>,
    pub balances: LookupMap<AccountId, u128>,
    pub total_supply: u128,
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    pub icon: Option<String>,
    pub fail_transfers: bool,
}

#[near]
impl MockTidepoolFt {
    #[init]
    pub fn new(
        owner_id: AccountId,
        name: String,
        symbol: String,
        decimals: u8,
        icon: Option<String>,
    ) -> Self {
        let mut minters = LookupSet::new(b"n");
        minters.insert(&owner_id);
        Self {
            owner_id,
            minters,
            balances: LookupMap::new(b"b"),
            total_supply: 0,
            name,
            symbol,
            decimals,
            icon,
            fail_transfers: false,
        }
    }

    pub fn set_fail_transfers(&mut self, fail: bool) {
        assert_eq!(
            env::predecessor_account_id(),
            self.owner_id,
            "ERR_UNAUTHORIZED"
        );
        self.fail_transfers = fail;
    }

    pub fn owner_id(&self) -> AccountId {
        self.owner_id.clone()
    }

    pub fn is_minter(&self, account_id: AccountId) -> bool {
        self.minters.contains(&account_id)
    }

    pub fn add_minter(&mut self, account_id: AccountId) {
        assert_eq!(
            env::predecessor_account_id(),
            self.owner_id,
            "ERR_UNAUTHORIZED"
        );
        self.minters.insert(&account_id);
    }

    pub fn mint(&mut self, account_id: AccountId, amount: U128) {
        assert!(
            self.is_minter(env::predecessor_account_id()),
            "ERR_NOT_MINTER"
        );
        assert!(
            self.balances.contains_key(&account_id),
            "The account is not registered"
        );
        let cur = self.balances.get(&account_id).unwrap_or(0);
        self.balances.insert(&account_id, &(cur + amount.0));
        self.total_supply += amount.0;
    }

    #[payable]
    pub fn storage_deposit(
        &mut self,
        account_id: Option<AccountId>,
        _registration_only: Option<bool>,
    ) -> StorageBalance {
        let target = account_id.unwrap_or_else(env::predecessor_account_id);
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
    pub fn storage_unregister(&mut self, force: Option<bool>) -> bool {
        assert_eq!(
            env::attached_deposit().as_yoctonear(),
            1,
            "Requires 1 yoctoNEAR"
        );
        let caller = env::predecessor_account_id();
        let Some(bal) = self.balances.get(&caller) else {
            return false;
        };
        if bal > 0 && !force.unwrap_or(false) {
            env::panic_str(
                "Can't unregister account with positive token balance without force=true",
            );
        }
        self.total_supply = self.total_supply.saturating_sub(bal);
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
            reference: None,
            reference_hash: None,
            decimals: self.decimals,
        }
    }

    pub fn ft_total_supply(&self) -> U128 {
        U128(self.total_supply)
    }

    pub fn ft_balance_of(&self, account_id: AccountId) -> U128 {
        U128(self.balances.get(&account_id).unwrap_or(0))
    }

    #[payable]
    pub fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, _memo: Option<String>) {
        assert_eq!(
            env::attached_deposit().as_yoctonear(),
            1,
            "Requires 1 yoctoNEAR"
        );
        assert!(!self.fail_transfers, "ERR_SIMULATED_FT_TRANSFER_FAILURE");
        let sender_id = env::predecessor_account_id();
        self.internal_transfer(&sender_id, &receiver_id, amount.0);
    }

    #[payable]
    pub fn ft_transfer_call(
        &mut self,
        receiver_id: AccountId,
        amount: U128,
        _memo: Option<String>,
        msg: String,
    ) -> PromiseOrValue<U128> {
        assert_eq!(
            env::attached_deposit().as_yoctonear(),
            1,
            "Requires 1 yoctoNEAR"
        );
        let sender_id = env::predecessor_account_id();
        self.internal_transfer(&sender_id, &receiver_id, amount.0);

        let p = ext_ft_receiver::ext(receiver_id.clone())
            .with_static_gas(FT_ON_TRANSFER_GAS)
            .ft_on_transfer(sender_id.clone(), amount, msg)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(FT_RESOLVE_TRANSFER_GAS)
                    .ft_resolve_transfer(sender_id, receiver_id, amount),
            );
        PromiseOrValue::Promise(p)
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
            let rec_bal = self.balances.get(&receiver_id).unwrap_or(0);
            let refund = unused.min(rec_bal);
            if refund > 0 {
                self.balances.insert(&receiver_id, &(rec_bal - refund));
                if let Some(s_bal) = self.balances.get(&sender_id) {
                    self.balances.insert(&sender_id, &(s_bal + refund));
                }
                return U128(amount.0 - refund);
            }
        }
        U128(amount.0)
    }

    fn internal_transfer(&mut self, sender_id: &AccountId, receiver_id: &AccountId, amount: u128) {
        assert_ne!(sender_id, receiver_id, "Sender and receiver must differ");
        assert!(amount > 0, "The amount should be a positive number");
        let s_bal = self
            .balances
            .get(sender_id)
            .expect("Sender is not registered");
        assert!(s_bal >= amount, "The account doesn't have enough balance");
        let r_bal = self
            .balances
            .get(receiver_id)
            .expect("The account is not registered");
        self.balances.insert(sender_id, &(s_bal - amount));
        self.balances.insert(receiver_id, &(r_bal + amount));
    }
}
