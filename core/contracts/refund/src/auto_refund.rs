//! Automated refund triggers: registration, rules, and evaluation.

use crate::*;

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutoRefundTriggered {
    pub trigger_id: u64,
    pub payment_id: u64,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TriggerRegistered {
    pub trigger_id: u64,
    pub payment_id: u64,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct AutoRefundTrigger {
    pub trigger_id: u64,
    pub payment_id: u64,
    pub condition: AutoRefundCondition,
    pub refund_bps: u32,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct FulfillmentTimeoutCondition {
    pub fulfillment_deadline: u64,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct ContractStateMatchCondition {
    pub contract: Address,
    pub key: BytesN<32>,
    pub expected: Bytes,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum AutoRefundCondition {
    FulfillmentTimeout(FulfillmentTimeoutCondition),
    ContractStateMatch(ContractStateMatchCondition),
}

#[contractimpl]
impl RefundContract {
    /// Register an automatic refund trigger for a payment.
    ///
    /// Creates a trigger that automatically initiates a refund when a condition is met
    /// (e.g., fulfillment timeout or contract state match).
    ///
    /// # Arguments
    /// * `merchant` - The merchant registering the trigger (must be authorized).
    /// * `payment_id` - The payment ID to attach the trigger to.
    /// * `condition` - The condition that triggers the automatic refund.
    /// * `refund_bps` - The refund amount in basis points of the original payment (1-10000).
    ///
    /// # Returns
    /// The ID of the newly created trigger.
    ///
    /// # Errors
    /// Returns `InvalidPaymentId` if `payment_id` is 0.
    /// Returns `RefundExceedsPolicy` if `refund_bps` is out of valid range.
    /// Returns `Unauthorized` if the caller is not the payment's merchant.
    /// Returns `DuplicateAutoRefundTrigger` if an identical active trigger exists.
    pub fn register_auto_refund_trigger(
        env: Env,
        merchant: Address,
        payment_id: u64,
        condition: AutoRefundCondition,
        refund_bps: u32,
    ) -> Result<u64, Error> {
        merchant.require_auth();

        if payment_id == 0 {
            return Err(Error::Core(CoreError::InvalidPaymentId));
        }

        if let Err(_) = Self::validate_bps(refund_bps) {
            return Err(Error::Core(CoreError::RefundExceedsPolicy));
        }

        let payment = Self::get_external_payment(&env, payment_id)?;
        if payment.merchant != merchant {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        let trigger_count: u64 = env
            .storage()
            .instance()
            .get(&PolicyKey::AutoRefundTriggerCounter)
            .unwrap_or(0);

        let mut trigger_id = 1u64;
        while trigger_id <= trigger_count {
            if let Some(existing) = env
                .storage()
                .instance()
                .get::<PolicyKey, AutoRefundTrigger>(&PolicyKey::AutoRefundTrigger(trigger_id))
            {
                if existing.active
                    && existing.payment_id == payment_id
                    && existing.condition == condition
                {
                    return Err(Error::Ext(ExtError::DuplicateAutoRefundTrigger));
                }
            }
            trigger_id += 1;
        }

        let new_trigger_id = trigger_count + 1;
        let trigger = AutoRefundTrigger {
            trigger_id: new_trigger_id,
            payment_id,
            condition,
            refund_bps,
            active: true,
        };

        env.storage()
            .instance()
            .set(&PolicyKey::AutoRefundTrigger(new_trigger_id), &trigger);
        env.storage()
            .instance()
            .set(&PolicyKey::AutoRefundTriggerCounter, &new_trigger_id);

        (TriggerRegistered {
            trigger_id: new_trigger_id,
            payment_id,
        })
        .publish(&env);

        Ok(new_trigger_id)
    }

    /// Evaluate an automatic refund trigger and execute it if the condition is met.
    ///
    /// Checks the trigger's condition, creates and processes a refund if satisfied,
    /// and deactivates the trigger. Emits an `AutoRefundTriggered` event.
    ///
    /// # Arguments
    /// * `trigger_id` - The ID of the trigger to evaluate.
    ///
    /// # Returns
    /// `true` if the refund was triggered, `false` otherwise.
    ///
    /// # Errors
    /// Returns `InvalidAmount` if the calculated refund amount is non-positive.
    pub fn evaluate_auto_refund(env: Env, trigger_id: u64) -> Result<bool, Error> {
        let mut trigger = Self::get_auto_refund_trigger(env.clone(), trigger_id)?;
        if !trigger.active {
            return Ok(false);
        }

        let condition_met = Self::evaluate_auto_refund_condition(&env, &trigger.condition)?;
        if !condition_met {
            return Ok(false);
        }

        let payment = Self::get_external_payment(&env, trigger.payment_id)?;
        let refund_amount = payment
            .amount
            .checked_mul(trigger.refund_bps as i128)
            .and_then(|value| value.checked_div(10000))
            .ok_or(Error::Core(CoreError::InvalidAmount))?;
        if refund_amount <= 0 {
            return Err(Error::Core(CoreError::InvalidAmount));
        }

        let refund_id = Self::create_refund(
            env.clone(),
            payment.merchant.clone(),
            payment.id,
            payment.customer.clone(),
            refund_amount,
            payment.amount,
            payment.token.clone(),
            String::from_str(&env, "Automatic refund trigger executed"),
            RefundReasonCode::Other,
            payment.created_at,
            true,
        )?;
        Self::process_refund_internal(&env, env.current_contract_address(), refund_id)?;

        trigger.active = false;
        env.storage()
            .instance()
            .set(&PolicyKey::AutoRefundTrigger(trigger_id), &trigger);

        (AutoRefundTriggered {
            trigger_id,
            payment_id: payment.id,
            amount: refund_amount,
        })
        .publish(&env);

        Ok(true)
    }

    /// Get an automatic refund trigger by its ID.
    ///
    /// # Arguments
    /// * `trigger_id` - The unique identifier of the trigger.
    ///
    /// # Returns
    /// The `AutoRefundTrigger` record if found.
    ///
    /// # Errors
    /// Returns `AutoRefundTriggerNotFound` if no trigger exists with the given ID.
    pub fn get_auto_refund_trigger(env: Env, trigger_id: u64) -> Result<AutoRefundTrigger, Error> {
        env.storage()
            .instance()
            .get(&PolicyKey::AutoRefundTrigger(trigger_id))
            .ok_or(Error::Ext(ExtError::AutoRefundTriggerNotFound))
    }

    pub(crate) fn evaluate_auto_refund_condition(
        env: &Env,
        condition: &AutoRefundCondition,
    ) -> Result<bool, Error> {
        match condition {
            AutoRefundCondition::FulfillmentTimeout(config) => {
                Ok(env.ledger().timestamp() >= config.fulfillment_deadline)
            }
            AutoRefundCondition::ContractStateMatch(config) => {
                let args = (config.key.clone(),).into_val(env);
                let func = Symbol::new(env, "get_contract_state");
                match env.try_invoke_contract::<Bytes, soroban_sdk::InvokeError>(
                    &config.contract,
                    &func,
                    args,
                ) {
                    Ok(Ok(actual)) => Ok(actual == config.expected),
                    _ => Ok(false),
                }
            }
        }
    }
}
