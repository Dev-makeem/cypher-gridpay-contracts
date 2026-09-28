//! Appeal filing, resolution, and supporting evidence.

use crate::*;

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppealFiled {
    pub appeal_id: u64,
    pub refund_id: u64,
    pub appellant: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppealResolved {
    pub appeal_id: u64,
    pub upheld: bool,
    pub resolved_at: u64,
}

// Issue #190: Dispute evidence attachment
#[derive(Clone)]
#[contracttype]
pub struct RefundEvidence {
    pub refund_id: u64,
    pub submitter: Address,
    pub evidence_hash: BytesN<32>,
    pub submitted_at: u64,
}

#[derive(Clone)]
#[contracttype]
pub struct RefundAppeal {
    pub appeal_id: u64,
    pub refund_id: u64,
    pub appellant: Address,
    pub reason: String,
    pub filed_at: u64,
    pub resolved: bool,
    pub outcome: Option<bool>,
}

#[contractimpl]
impl RefundContract {
    /// Finalize a denied refund after its appeal window has expired.
    ///
    /// Moves the refund from `PendingAppeal` to `Rejected` status if the appeal window
    /// has elapsed, and emits a `RefundRejected` event.
    ///
    /// # Arguments
    /// * `refund_id` - The ID of the refund to finalize.
    ///
    /// # Errors
    /// Returns `RefundNotFound` if the refund does not exist.
    /// Returns `RefundNotRejected` if the refund is not in `PendingAppeal` status.
    /// Returns `InvalidStatus` if the appeal window has not yet expired.
    pub fn finalize_denial(env: Env, refund_id: u64) -> Result<(), Error> {
        let mut refund: Refund = env
            .storage()
            .instance()
            .get(&DataKey::Refund(refund_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;

        if refund.status != RefundStatus::PendingAppeal {
            return Err(Error::Core(CoreError::RefundNotRejected));
        }

        let appeal_deadline = refund
            .appeal_deadline
            .ok_or(Error::Core(CoreError::RefundNotRejected))?;
        let now = env.ledger().timestamp();
        if now < appeal_deadline {
            return Err(Error::Core(CoreError::InvalidStatus));
        }

        Self::remove_from_status_index(&env, RefundStatus::PendingAppeal, refund_id)?;

        refund.status = RefundStatus::Rejected;
        refund.rejected_at = Some(now);
        env.storage()
            .instance()
            .set(&DataKey::Refund(refund_id), &refund);
        Self::add_to_status_index(&env, RefundStatus::Rejected, refund_id);
        Self::release_payment_refund_usage(&env, refund.payment_id, refund.amount);
        env.storage()
            .instance()
            .set(&SystemKey::RefundRejectedAt(refund_id), &now);

        let rejected_by = refund
            .rejected_by
            .clone()
            .unwrap_or(env.current_contract_address());

        (RefundRejected {
            refund_id,
            rejected_by,
            rejected_at: now,
            rejection_reason: soroban_sdk::String::from_str(&env, "appeal window expired"),
        })
        .publish(&env);

        Self::invoke_hooks(&env, RefundEventType::Rejected, refund_id);

        Ok(())
    }

    pub(crate) fn begin_refund_rejection(
        env: &Env,
        admin: Address,
        refund_id: u64,
        rejection_reason: String,
    ) -> Result<(), Error> {
        let mut refund: Refund = env
            .storage()
            .instance()
            .get(&DataKey::Refund(refund_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;

        if refund.status != RefundStatus::Requested {
            return Err(Error::Core(CoreError::InvalidStatus));
        }

        Self::remove_from_status_index(env, RefundStatus::Requested, refund_id)?;

        let appeal_window: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AppealWindowSeconds)
            .unwrap_or(604800);
        let now = env.ledger().timestamp();

        refund.status = RefundStatus::PendingAppeal;
        refund.rejected_by = Some(admin.clone());
        refund.appeal_deadline = Some(now.saturating_add(appeal_window));

        env.storage()
            .instance()
            .set(&DataKey::Refund(refund_id), &refund);
        Self::add_to_status_index(env, RefundStatus::PendingAppeal, refund_id);

        (RefundRejected {
            refund_id,
            rejected_by: admin,
            rejected_at: now,
            rejection_reason,
        })
        .publish(env);

        Ok(())
    }

    /// File an appeal against a rejected or pending-appeal refund.
    ///
    /// Creates a new appeal record and emits an `AppealFiled` event. The customer
    /// must be the refund's customer and the refund must be in a rejected/pending-appeal state.
    ///
    /// # Arguments
    /// * `customer` - The customer filing the appeal (must be authorized).
    /// * `refund_id` - The ID of the refund being appealed.
    /// * `reason` - A human-readable reason for the appeal.
    ///
    /// # Returns
    /// The ID of the newly created appeal.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the refund's customer.
    /// Returns `RefundNotRejected` if the refund is not in an appealable state.
    /// Returns `AppealAlreadyFiled` if an appeal already exists for this refund.
    /// Returns `AppealWindowExpired` if the appeal window has passed.
    pub fn file_appeal(
        env: Env,
        customer: Address,
        refund_id: u64,
        reason: String,
    ) -> Result<u64, Error> {
        Self::require_not_paused(&env, "file_appeal")?;
        customer.require_auth();

        let refund: Refund = env
            .storage()
            .instance()
            .get(&DataKey::Refund(refund_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;

        if refund.customer != customer {
            return Err(Error::Core(CoreError::Unauthorized));
        }
        if refund.status != RefundStatus::Rejected && refund.status != RefundStatus::PendingAppeal {
            return Err(Error::Core(CoreError::RefundNotRejected));
        }
        if env
            .storage()
            .instance()
            .has(&SystemKey::AppealByRefund(refund_id))
        {
            return Err(Error::Core(CoreError::AppealAlreadyFiled));
        }

        let now = env.ledger().timestamp();
        if refund.status == RefundStatus::PendingAppeal {
            let appeal_deadline = refund
                .appeal_deadline
                .ok_or(Error::Core(CoreError::RefundNotRejected))?;
            if now > appeal_deadline {
                return Err(Error::Core(CoreError::AppealWindowExpired));
            }
        } else {
            let rejected_at: u64 = env
                .storage()
                .instance()
                .get(&SystemKey::RefundRejectedAt(refund_id))
                .ok_or(Error::Core(CoreError::RefundNotRejected))?;
            let appeal_window: u64 = env
                .storage()
                .instance()
                .get(&DataKey::AppealWindowSeconds)
                .unwrap_or(604800);
            if now > rejected_at.saturating_add(appeal_window) {
                return Err(Error::Core(CoreError::AppealWindowExpired));
            }
        }

        let counter: u64 = env
            .storage()
            .instance()
            .get(&SystemKey::AppealCounter)
            .unwrap_or(0);
        let appeal_id = counter + 1;
        let appeal = RefundAppeal {
            appeal_id,
            refund_id,
            appellant: customer.clone(),
            reason,
            filed_at: now,
            resolved: false,
            outcome: None,
        };
        env.storage()
            .instance()
            .set(&SystemKey::Appeal(appeal_id), &appeal);
        env.storage()
            .instance()
            .set(&SystemKey::AppealCounter, &appeal_id);
        env.storage()
            .instance()
            .set(&SystemKey::AppealByRefund(refund_id), &appeal_id);

        let customer_count: u64 = env
            .storage()
            .instance()
            .get(&SystemKey::AppealByCustomerCount(customer.clone()))
            .unwrap_or(0);
        env.storage().instance().set(
            &SystemKey::AppealByCustomer(customer.clone(), customer_count),
            &appeal_id,
        );
        env.storage().instance().set(
            &SystemKey::AppealByCustomerCount(customer.clone()),
            &(customer_count + 1),
        );

        (AppealFiled {
            appeal_id,
            refund_id,
            appellant: customer,
        })
        .publish(&env);

        Ok(appeal_id)
    }

    /// Resolve an appeal by upholding or denying it.
    ///
    /// If upheld, the refund is approved and processed. If denied, the rejection becomes final.
    /// Emits an `AppealResolved` event.
    ///
    /// # Arguments
    /// * `admin` - The admin address (must be authorized and be the contract admin).
    /// * `appeal_id` - The ID of the appeal to resolve.
    /// * `uphold` - `true` to uphold the appeal (approve refund), `false` to deny.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the admin.
    /// Returns `AlreadyProcessed` if the appeal is already resolved.
    /// Returns `RefundNotFound` if the appeal or refund does not exist.
    pub fn resolve_appeal(
        env: Env,
        admin: Address,
        appeal_id: u64,
        uphold: bool,
    ) -> Result<(), Error> {
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        let mut appeal: RefundAppeal = env
            .storage()
            .instance()
            .get(&SystemKey::Appeal(appeal_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;
        if appeal.resolved {
            return Err(Error::Core(CoreError::AlreadyProcessed));
        }

        if uphold {
            let mut refund: Refund = env
                .storage()
                .instance()
                .get(&DataKey::Refund(appeal.refund_id))
                .ok_or(Error::Core(CoreError::RefundNotFound))?;
            if refund.status != RefundStatus::Rejected
                && refund.status != RefundStatus::PendingAppeal
            {
                return Err(Error::Core(CoreError::RefundNotRejected));
            }

            let prior_status = refund.status.clone();
            Self::remove_from_status_index(&env, prior_status, refund.id)?;
            refund.status = RefundStatus::Approved;
            env.storage()
                .instance()
                .set(&DataKey::Refund(refund.id), &refund);
            Self::add_to_status_index(&env, RefundStatus::Approved, refund.id);

            Self::process_refund_internal(&env, admin.clone(), refund.id)?;
        } else {
            let mut refund: Refund = env
                .storage()
                .instance()
                .get(&DataKey::Refund(appeal.refund_id))
                .ok_or(Error::Core(CoreError::RefundNotFound))?;
            if refund.status != RefundStatus::Rejected
                && refund.status != RefundStatus::PendingAppeal
            {
                return Err(Error::Core(CoreError::RefundNotRejected));
            }

            // The appeal was explicitly denied, so the rejection is final
            // now — no need to wait out the rest of the appeal window.
            if refund.status == RefundStatus::PendingAppeal {
                Self::remove_from_status_index(&env, RefundStatus::PendingAppeal, refund.id)?;
                refund.status = RefundStatus::Rejected;
                refund.rejected_at = Some(env.ledger().timestamp());
                env.storage()
                    .instance()
                    .set(&DataKey::Refund(refund.id), &refund);
                Self::add_to_status_index(&env, RefundStatus::Rejected, refund.id);
                Self::release_payment_refund_usage(&env, refund.payment_id, refund.amount);
            }
        }

        appeal.resolved = true;
        appeal.outcome = Some(uphold);
        env.storage()
            .instance()
            .set(&SystemKey::Appeal(appeal_id), &appeal);

        (AppealResolved {
            appeal_id,
            upheld: uphold,
            resolved_at: env.ledger().timestamp(),
        })
        .publish(&env);

        Ok(())
    }

    /// Retrieve an appeal by its ID.
    ///
    /// # Arguments
    /// * `appeal_id` - The unique identifier of the appeal.
    ///
    /// # Returns
    /// The `RefundAppeal` record if found.
    ///
    /// # Errors
    /// Returns `RefundNotFound` if no appeal exists with the given ID.
    pub fn get_appeal(env: Env, appeal_id: u64) -> Result<RefundAppeal, Error> {
        env.storage()
            .instance()
            .get(&SystemKey::Appeal(appeal_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))
    }

    /// Get all appeals filed by a specific customer.
    ///
    /// # Arguments
    /// * `customer` - The customer address to query appeals for.
    ///
    /// # Returns
    /// A vector of `RefundAppeal` records filed by the customer.
    pub fn get_appeals_by_customer(env: Env, customer: Address) -> Vec<RefundAppeal> {
        let mut appeals = Vec::new(&env);
        let count: u64 = env
            .storage()
            .instance()
            .get(&SystemKey::AppealByCustomerCount(customer.clone()))
            .unwrap_or(0);

        let mut index = 0u64;
        while index < count {
            if let Some(appeal_id) = env
                .storage()
                .instance()
                .get::<_, u64>(&SystemKey::AppealByCustomer(customer.clone(), index))
            {
                if let Some(appeal) = env
                    .storage()
                    .instance()
                    .get::<_, RefundAppeal>(&SystemKey::Appeal(appeal_id))
                {
                    appeals.push_back(appeal);
                }
            }
            index += 1;
        }

        appeals
    }

    /// Submit evidence for a refund dispute as the customer or merchant.
    ///
    /// Each party (customer or merchant) can submit one evidence entry per refund.
    ///
    /// # Arguments
    /// * `submitter` - The address submitting the evidence (must be the customer or merchant).
    /// * `refund_id` - The ID of the refund to submit evidence for.
    /// * `evidence_hash` - SHA-256 hash of the evidence document.
    ///
    /// # Errors
    /// Returns `RefundNotFound` if the refund does not exist.
    /// Returns `Unauthorized` if the submitter is not the customer or merchant.
    /// Returns `EvidenceAlreadySubmitted` if this party has already submitted evidence.
    pub fn submit_refund_evidence(
        env: Env,
        submitter: Address,
        refund_id: u64,
        evidence_hash: BytesN<32>,
    ) -> Result<(), Error> {
        submitter.require_auth();

        let refund: Refund = env
            .storage()
            .instance()
            .get(&DataKey::Refund(refund_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;

        if submitter != refund.customer && submitter != refund.merchant {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        if env
            .storage()
            .instance()
            .has(&EvidenceKey::Evidence(refund_id, submitter.clone()))
        {
            return Err(Error::Ext(ExtError::EvidenceAlreadySubmitted));
        }

        let count: u64 = env
            .storage()
            .instance()
            .get(&EvidenceKey::EvidenceCount(refund_id))
            .unwrap_or(0);

        let evidence = RefundEvidence {
            refund_id,
            submitter: submitter.clone(),
            evidence_hash,
            submitted_at: env.ledger().timestamp(),
        };

        env.storage().instance().set(
            &EvidenceKey::Evidence(refund_id, submitter.clone()),
            &evidence,
        );
        env.storage()
            .instance()
            .set(&EvidenceKey::EvidenceIndex(refund_id, count), &submitter);
        env.storage()
            .instance()
            .set(&EvidenceKey::EvidenceCount(refund_id), &(count + 1));

        Ok(())
    }

    /// Get the evidence submitted by a specific party for a refund dispute.
    ///
    /// # Arguments
    /// * `refund_id` - The ID of the refund.
    /// * `submitter` - The address of the party who submitted the evidence.
    ///
    /// # Returns
    /// The `RefundEvidence` if found, `None` otherwise.
    pub fn get_refund_evidence(
        env: Env,
        refund_id: u64,
        submitter: Address,
    ) -> Option<RefundEvidence> {
        env.storage()
            .instance()
            .get(&EvidenceKey::Evidence(refund_id, submitter))
    }

    /// Get all evidence entries submitted for a refund dispute.
    ///
    /// # Arguments
    /// * `refund_id` - The ID of the refund.
    ///
    /// # Returns
    /// A vector of all `RefundEvidence` entries for the refund.
    pub fn get_all_refund_evidence(env: Env, refund_id: u64) -> Vec<RefundEvidence> {
        let count: u64 = env
            .storage()
            .instance()
            .get(&EvidenceKey::EvidenceCount(refund_id))
            .unwrap_or(0);
        let mut results = Vec::new(&env);
        let mut i = 0u64;
        while i < count {
            if let Some(submitter) = env
                .storage()
                .instance()
                .get::<_, Address>(&EvidenceKey::EvidenceIndex(refund_id, i))
            {
                if let Some(ev) = env
                    .storage()
                    .instance()
                    .get::<_, RefundEvidence>(&EvidenceKey::Evidence(refund_id, submitter))
                {
                    results.push_back(ev);
                }
            }
            i += 1;
        }
        results
    }
}
