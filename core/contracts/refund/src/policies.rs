//! Customer refund tiers and merchant refund quotas.

use crate::*;

#[derive(Clone)]
#[contracttype]
pub struct MerchantRefundQuota {
    pub merchant: Address,
    pub limit: i128,
    pub period_seconds: u64,
    pub used: i128,
    pub period_start: u64,
}

#[contractimpl]
impl RefundContract {
    /// Set a refund quota for a merchant within a time period.
    ///
    /// Limits the total refund amount a merchant can process within the specified period.
    ///
    /// # Arguments
    /// * `admin` - The admin address (must be authorized).
    /// * `merchant` - The merchant to set the quota for.
    /// * `limit` - The maximum refund amount allowed in the period.
    /// * `period_seconds` - The duration of the quota period in seconds.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the admin.
    pub fn set_merchant_refund_quota(
        env: Env,
        admin: Address,
        merchant: Address,
        limit: i128,
        period_seconds: u64,
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

        let now = env.ledger().timestamp();
        let quota = match env
            .storage()
            .instance()
            .get::<_, MerchantRefundQuota>(&DataKey::MerchantRefundQuota(merchant.clone()))
        {
            Some(mut existing) => {
                existing.limit = limit;
                existing.period_seconds = period_seconds;
                existing
            }
            None => MerchantRefundQuota {
                merchant: merchant.clone(),
                limit,
                period_seconds,
                used: 0,
                period_start: now,
            },
        };
        env.storage()
            .instance()
            .set(&DataKey::MerchantRefundQuota(merchant), &quota);
        Ok(())
    }

    /// Get the refund quota configuration for a merchant.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to query.
    ///
    /// # Returns
    /// The `MerchantRefundQuota` if configured, `None` otherwise.
    pub fn get_merchant_refund_quota(env: Env, merchant: Address) -> Option<MerchantRefundQuota> {
        env.storage()
            .instance()
            .get(&DataKey::MerchantRefundQuota(merchant))
    }

    /// Reset a merchant's refund quota usage counter to zero and restart the quota period.
    ///
    /// # Arguments
    /// * `admin` - The contract admin executing the reset.
    /// * `merchant` - The merchant whose quota should be reset.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `PolicyNotFound` if no quota is configured for the merchant.
    pub fn reset_merchant_quota(env: Env, admin: Address, merchant: Address) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        let mut quota: MerchantRefundQuota = env
            .storage()
            .instance()
            .get(&DataKey::MerchantRefundQuota(merchant.clone()))
            .ok_or(Error::Core(CoreError::PolicyNotFound))?;
        quota.used = 0;
        quota.period_start = env.ledger().timestamp();
        env.storage()
            .instance()
            .set(&DataKey::MerchantRefundQuota(merchant), &quota);
        Ok(())
    }

    /// Assign a tier level to a customer for tier-based refund cap policies.
    ///
    /// # Arguments
    /// * `admin` - The contract admin setting the tier.
    /// * `customer` - The customer address to assign the tier to.
    /// * `tier_id` - The tier level to assign.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn set_customer_tier(
        env: Env,
        admin: Address,
        customer: Address,
        tier_id: u32,
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
        env.storage()
            .instance()
            .set(&DataKey::CustomerTier(customer), &tier_id);
        Ok(())
    }

    /// Get the tier level assigned to a customer.
    ///
    /// # Arguments
    /// * `customer` - The customer address to query.
    ///
    /// # Returns
    /// The tier ID if assigned, `None` otherwise.
    pub fn get_customer_tier(env: Env, customer: Address) -> Option<u32> {
        env.storage()
            .instance()
            .get(&DataKey::CustomerTier(customer))
    }

    /// Set the refund cap policy for a specific customer tier under a merchant.
    ///
    /// # Arguments
    /// * `merchant` - The merchant setting the policy (must authenticate).
    /// * `tier_id` - The tier level to configure.
    /// * `max_refund_bps` - The maximum refund percentage in basis points (0-10000).
    ///
    /// # Errors
    /// Returns `InvalidAmount` if `max_refund_bps` exceeds 10000.
    pub fn set_customer_tier_policy(
        env: Env,
        merchant: Address,
        tier_id: u32,
        max_refund_bps: u32,
    ) -> Result<(), Error> {
        merchant.require_auth();
        if max_refund_bps > 10000 {
            return Err(Error::Core(CoreError::InvalidAmount));
        }
        let cap = RefundCap { max_refund_bps };
        env.storage()
            .instance()
            .set(&DataKey::CustomerTierPolicy(merchant, tier_id), &cap);
        Ok(())
    }

    /// Get the refund cap policy for a specific customer tier under a merchant.
    ///
    /// # Arguments
    /// * `merchant` - The merchant to query.
    /// * `tier_id` - The tier level to look up.
    ///
    /// # Returns
    /// The `RefundCap` for the tier if configured, `None` otherwise.
    pub fn get_customer_tier_policy(
        env: Env,
        merchant: Address,
        tier_id: u32,
    ) -> Option<RefundCap> {
        env.storage()
            .instance()
            .get(&DataKey::CustomerTierPolicy(merchant, tier_id))
    }

    /// Enable or disable strict tier policy enforcement for a merchant.
    ///
    /// When strict mode is enabled, customers without an assigned tier are
    /// denied refunds instead of falling back to default behavior.
    ///
    /// # Arguments
    /// * `merchant` - The merchant to configure (must authenticate).
    /// * `strict` - `true` to enable strict mode, `false` to disable it.
    pub fn set_strict_tier_policy(env: Env, merchant: Address, strict: bool) -> Result<(), Error> {
        merchant.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::StrictTierPolicy(merchant), &strict);
        Ok(())
    }

    /// Check whether strict tier policy enforcement is enabled for a merchant.
    ///
    /// # Arguments
    /// * `merchant` - The merchant to query.
    ///
    /// # Returns
    /// `true` if strict mode is enabled, `false` otherwise (the default).
    pub fn get_strict_tier_policy(env: Env, merchant: Address) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::StrictTierPolicy(merchant))
            .unwrap_or(false)
    }
}
