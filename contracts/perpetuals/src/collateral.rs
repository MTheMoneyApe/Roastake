// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

//! NEP-141 / NEP-145 collateral adapter, 6<->8 decimal conversion, and async withdrawal safety.

use near_sdk::{env, ext_contract, json_types::U128, NearToken, Promise, PromiseOrValue};

use crate::events::{
    emit_event, CollateralDepositedEvent, CollateralWithdrawnEvent, InsuranceUpdatedEvent,
    SecurityAnomalyEvent,
};
use crate::types::{
    FungibleTokenMetadata, WithdrawalKind, WithdrawalRecord, FT_TRANSFER_GAS,
    MAX_METADATA_PAYLOAD_BYTES, WITHDRAW_CALLBACK_GAS,
};

#[ext_contract(ext_ft)]
pub trait FungibleTokenCore {
    fn ft_transfer(&mut self, receiver_id: near_sdk::AccountId, amount: U128, memo: Option<String>);
    fn ft_metadata(&self) -> FungibleTokenMetadata;
}

impl crate::PerpsContract {
    pub(crate) fn internal_credit_free_collateral_deposit(
        &mut self,
        sender_id: &near_sdk::AccountId,
        token_id: &near_sdk::AccountId,
        raw_units: u128,
        amount_e8: u128,
    ) -> PromiseOrValue<U128> {
        let mut account = self.accounts.get(sender_id).cloned().unwrap_or_default();
        if !account.registered {
            env::panic_str("ERR_ACCOUNT_NOT_REGISTERED");
        }
        if account.pending_action_lock {
            env::panic_str("ERR_ACCOUNT_LOCKED");
        }

        account.free_collateral_e8 = account
            .free_collateral_e8
            .checked_add(amount_e8)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        self.accounts.insert(sender_id.clone(), account.clone());

        self.buckets.total_trader_free_collateral_e8 = self
            .buckets
            .total_trader_free_collateral_e8
            .checked_add(amount_e8)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        self.tracked_collateral_e8 = self
            .tracked_collateral_e8
            .checked_add(amount_e8)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));

        let mut token_cfg = self
            .collateral_tokens
            .get(token_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_INVALID_COLLATERAL_TOKEN"));
        token_cfg.tracked_balance_units = token_cfg
            .tracked_balance_units
            .checked_add(raw_units)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        self.collateral_tokens.insert(token_id.clone(), token_cfg);

        self.assert_solvency();

        emit_event(
            "collateral_deposited",
            &CollateralDepositedEvent {
                account_id: sender_id.as_str(),
                token_id: token_id.as_str(),
                amount_token_units: raw_units.to_string(),
                amount_e8: amount_e8.to_string(),
                new_free_collateral_e8: account.free_collateral_e8.to_string(),
            },
        );

        PromiseOrValue::Value(U128(0))
    }

    pub(crate) fn internal_credit_insurance_deposit(
        &mut self,
        token_id: &near_sdk::AccountId,
        raw_units: u128,
        amount_e8: u128,
    ) -> PromiseOrValue<U128> {
        self.buckets.insurance_fund_e8 = self
            .buckets
            .insurance_fund_e8
            .checked_add(amount_e8)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        self.tracked_collateral_e8 = self
            .tracked_collateral_e8
            .checked_add(amount_e8)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));

        let mut token_cfg = self
            .collateral_tokens
            .get(token_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_INVALID_COLLATERAL_TOKEN"));
        token_cfg.tracked_balance_units = token_cfg
            .tracked_balance_units
            .checked_add(raw_units)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        self.collateral_tokens.insert(token_id.clone(), token_cfg);

        self.assert_solvency();

        emit_event(
            "insurance_updated",
            &InsuranceUpdatedEvent {
                reason: "funded",
                delta_e8: amount_e8.to_string(),
                new_balance_e8: self.buckets.insurance_fund_e8.to_string(),
                uncovered_insolvency_e8: "0".to_string(),
            },
        );

        PromiseOrValue::Value(U128(0))
    }

    pub(crate) fn internal_credit_treasury_deposit(
        &mut self,
        token_id: &near_sdk::AccountId,
        raw_units: u128,
        amount_e8: u128,
    ) -> PromiseOrValue<U128> {
        self.buckets.protocol_fees_e8 = self
            .buckets
            .protocol_fees_e8
            .checked_add(amount_e8)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        self.tracked_collateral_e8 = self
            .tracked_collateral_e8
            .checked_add(amount_e8)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));

        let mut token_cfg = self
            .collateral_tokens
            .get(token_id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("ERR_INVALID_COLLATERAL_TOKEN"));
        token_cfg.tracked_balance_units = token_cfg
            .tracked_balance_units
            .checked_add(raw_units)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        self.collateral_tokens.insert(token_id.clone(), token_cfg);

        self.assert_solvency();
        PromiseOrValue::Value(U128(0))
    }

    /// Initiate async NEP-141 `ft_transfer` for user free collateral, LP proceeds, or protocol treasury fees.
    pub(crate) fn internal_dispatch_withdrawal(
        &mut self,
        kind: WithdrawalKind,
        account_id: near_sdk::AccountId,
        token_id: near_sdk::AccountId,
        amount_token_units: u128,
        amount_usd_e8: u128,
    ) -> Promise {
        self.next_withdrawal_id = self
            .next_withdrawal_id
            .checked_add(1)
            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        let withdrawal_id = self.next_withdrawal_id;

        let record = WithdrawalRecord {
            id: withdrawal_id,
            kind,
            account_id: account_id.clone(),
            token_id: token_id.clone(),
            amount_token_units,
            amount_usd_e8,
            created_at_sec: env::block_timestamp_ms() / 1_000,
        };
        self.pending_withdrawals.insert(withdrawal_id, record);

        self.assert_solvency();

        emit_event(
            "collateral_withdrawn",
            &CollateralWithdrawnEvent {
                account_id: account_id.as_str(),
                token_id: token_id.as_str(),
                withdrawal_id,
                amount_token_units: amount_token_units.to_string(),
                amount_e8: amount_usd_e8.to_string(),
                status: "initiated",
            },
        );

        ext_ft::ext(token_id)
            .with_attached_deposit(NearToken::from_yoctonear(1))
            .with_static_gas(FT_TRANSFER_GAS)
            .ft_transfer(account_id, U128(amount_token_units), None)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(WITHDRAW_CALLBACK_GAS)
                    .on_withdraw_transfer(withdrawal_id),
            )
    }

    pub(crate) fn internal_handle_withdraw_callback(
        &mut self,
        withdrawal_id: u64,
        succeeded: bool,
    ) {
        let Some(record) = self.pending_withdrawals.remove(&withdrawal_id) else {
            emit_event(
                "security_anomaly",
                &SecurityAnomalyEvent {
                    kind: "duplicate_or_unknown_withdraw_callback",
                    actor: env::predecessor_account_id().as_str(),
                    detail: &format!("withdrawal_id={withdrawal_id}"),
                },
            );
            return;
        };

        self.buckets.total_in_flight_withdrawals_e8 = self
            .buckets
            .total_in_flight_withdrawals_e8
            .checked_sub(record.amount_usd_e8)
            .unwrap_or_else(|| env::panic_str("ERR_SOLVENCY_IN_FLIGHT_UNDERFLOW"));

        match record.kind {
            WithdrawalKind::TraderFreeCollateral | WithdrawalKind::LpProceeds => {
                if let Some(mut acc) = self.accounts.get(&record.account_id).cloned() {
                    acc.has_pending_withdrawal = false;
                    if !succeeded {
                        acc.free_collateral_e8 = acc
                            .free_collateral_e8
                            .checked_add(record.amount_usd_e8)
                            .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
                    }
                    self.accounts.insert(record.account_id.clone(), acc);
                } else if !succeeded {
                    env::panic_str("ERR_WITHDRAW_RESTORE_MISSING_ACCOUNT");
                }
            }
            WithdrawalKind::ProtocolTreasury => {
                self.treasury_withdrawal_pending = false;
                if !succeeded {
                    self.buckets.protocol_fees_e8 = self
                        .buckets
                        .protocol_fees_e8
                        .checked_add(record.amount_usd_e8)
                        .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
                }
            }
        }

        if succeeded {
            self.tracked_collateral_e8 = self
                .tracked_collateral_e8
                .checked_sub(record.amount_usd_e8)
                .unwrap_or_else(|| env::panic_str("ERR_TRACKED_BALANCE_UNDERFLOW"));
            if let Some(mut cfg) = self.collateral_tokens.get(&record.token_id).cloned() {
                cfg.tracked_balance_units = cfg
                    .tracked_balance_units
                    .checked_sub(record.amount_token_units)
                    .unwrap_or_else(|| env::panic_str("ERR_TOKEN_BALANCE_UNDERFLOW"));
                self.collateral_tokens.insert(record.token_id.clone(), cfg);
            }
        } else if matches!(
            record.kind,
            WithdrawalKind::TraderFreeCollateral | WithdrawalKind::LpProceeds
        ) {
            self.buckets.total_trader_free_collateral_e8 = self
                .buckets
                .total_trader_free_collateral_e8
                .checked_add(record.amount_usd_e8)
                .unwrap_or_else(|| env::panic_str("ERR_OVERFLOW"));
        }

        self.assert_solvency();

        emit_event(
            "collateral_withdrawn",
            &CollateralWithdrawnEvent {
                account_id: record.account_id.as_str(),
                token_id: record.token_id.as_str(),
                withdrawal_id,
                amount_token_units: record.amount_token_units.to_string(),
                amount_e8: record.amount_usd_e8.to_string(),
                status: if succeeded { "settled" } else { "restored" },
            },
        );
    }

    pub(crate) fn internal_handle_verify_metadata_callback(
        &mut self,
        token_id: near_sdk::AccountId,
        raw_result: Result<Vec<u8>, near_sdk::PromiseError>,
    ) -> bool {
        let Ok(bytes) = raw_result else {
            return false;
        };
        if bytes.len() > MAX_METADATA_PAYLOAD_BYTES {
            return false;
        }
        let Ok(meta) = serde_json::from_slice::<FungibleTokenMetadata>(&bytes) else {
            return false;
        };
        let Some(mut cfg) = self.collateral_tokens.get(&token_id).cloned() else {
            return false;
        };
        if meta.spec != "ft-1.0.0" || meta.decimals != cfg.decimals || meta.symbol != cfg.symbol {
            cfg.active = false;
            cfg.metadata_verified_on_chain = false;
            self.collateral_tokens.insert(token_id, cfg);
            emit_event(
                "security_anomaly",
                &SecurityAnomalyEvent {
                    kind: "collateral_metadata_mismatch_deactivated",
                    actor: env::current_account_id().as_str(),
                    detail: &format!(
                        "meta.spec={}, meta.symbol={}, meta.decimals={}",
                        meta.spec, meta.symbol, meta.decimals
                    ),
                },
            );
            return false;
        }
        cfg.metadata_verified_on_chain = true;
        self.collateral_tokens.insert(token_id, cfg);
        true
    }
}
