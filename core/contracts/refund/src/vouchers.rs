//! Store-credit refund vouchers: issuance, redemption, and expiry.

use crate::*;

// Issue #192: Refund credit vouchers
#[derive(Clone)]
#[contracttype]
pub struct RefundVoucher {
    pub voucher_id: u64,
    pub refund_id: u64,
    pub customer: Address,
    pub merchant: Address,
    pub amount: i128,
    pub token: Address,
    pub issued_at: u64,
    pub expires_at: u64,
    pub redeemed: bool,
}

#[contractimpl]
impl RefundContract {
    /// Issue a refund credit voucher for an approved refund.
    ///
    /// Creates a voucher with the refund amount that the customer can redeem
    /// against a future payment.
    ///
    /// # Arguments
    /// * `admin` - The contract admin issuing the voucher.
    /// * `refund_id` - The ID of the refund to create a voucher for.
    /// * `expiry_seconds` - The number of seconds until the voucher expires.
    ///
    /// # Returns
    /// The ID of the newly created voucher.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `RefundNotFound` if the refund does not exist.
    pub fn issue_refund_voucher(
        env: Env,
        admin: Address,
        refund_id: u64,
        expiry_seconds: u64,
    ) -> Result<u64, Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        let refund: Refund = env
            .storage()
            .instance()
            .get(&DataKey::Refund(refund_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;

        // Validate refund status is Approved
        if refund.status != RefundStatus::Approved {
            return Err(Error::Core(CoreError::InvalidAmount));
        }

        // Prevent duplicate vouchers for the same refund
        if env
            .storage()
            .instance()
            .get::<_, bool>(&VoucherKey::RefundVoucherIssued(refund_id))
            .unwrap_or(false)
        {
            return Err(Error::Core(CoreError::InvalidAmount));
        }

        let counter: u64 = env
            .storage()
            .instance()
            .get(&VoucherKey::VoucherCounter)
            .unwrap_or(0);
        let voucher_id = counter + 1;

        let now = env.ledger().timestamp();
        let voucher = RefundVoucher {
            voucher_id,
            refund_id,
            customer: refund.customer.clone(),
            merchant: refund.merchant.clone(),
            amount: refund.amount,
            token: refund.token.clone(),
            issued_at: now,
            expires_at: now.saturating_add(expiry_seconds),
            redeemed: false,
        };

        env.storage()
            .instance()
            .set(&VoucherKey::Voucher(voucher_id), &voucher);
        env.storage()
            .instance()
            .set(&VoucherKey::VoucherCounter, &voucher_id);

        // Mark voucher as issued for this refund
        env.storage()
            .instance()
            .set(&VoucherKey::RefundVoucherIssued(refund_id), &true);

        let customer_count: u64 = env
            .storage()
            .instance()
            .get(&VoucherKey::CustomerVoucherCount(refund.customer.clone()))
            .unwrap_or(0);
        env.storage().instance().set(
            &VoucherKey::CustomerVoucher(refund.customer.clone(), customer_count),
            &voucher_id,
        );
        env.storage().instance().set(
            &VoucherKey::CustomerVoucherCount(refund.customer.clone()),
            &(customer_count + 1),
        );

        Ok(voucher_id)
    }

    /// Redeem a refund credit voucher for a customer.
    ///
    /// # Arguments
    /// * `customer` - The customer redeeming the voucher (must authenticate).
    /// * `voucher_id` - The ID of the voucher to redeem.
    /// * `_payment_id` - The payment ID to apply the voucher to (reserved for future use).
    ///
    /// # Errors
    /// Returns `VoucherNotFound` if the voucher does not exist.
    /// Returns `Unauthorized` if the caller is not the voucher's customer.
    /// Returns `VoucherAlreadyRedeemed` if the voucher has already been used.
    /// Returns `VoucherExpired` if the voucher has expired.
    pub fn redeem_refund_voucher(
        env: Env,
        customer: Address,
        voucher_id: u64,
        _payment_id: u64,
    ) -> Result<(), Error> {
        customer.require_auth();

        let mut voucher: RefundVoucher = env
            .storage()
            .instance()
            .get(&VoucherKey::Voucher(voucher_id))
            .ok_or(Error::Ext(ExtError::VoucherNotFound))?;

        if voucher.customer != customer {
            return Err(Error::Core(CoreError::Unauthorized));
        }
        if voucher.redeemed {
            return Err(Error::Ext(ExtError::VoucherAlreadyRedeemed));
        }
        if env.ledger().timestamp() > voucher.expires_at {
            return Err(Error::Ext(ExtError::VoucherExpired));
        }

        token::Client::new(&env, &voucher.token).transfer(
            &env.current_contract_address(),
            &customer,
            &voucher.amount,
        );

        voucher.redeemed = true;
        env.storage()
            .instance()
            .set(&VoucherKey::Voucher(voucher_id), &voucher);

        Ok(())
    }

    /// Get a refund voucher by its ID.
    ///
    /// # Arguments
    /// * `voucher_id` - The ID of the voucher to retrieve.
    ///
    /// # Returns
    /// The `RefundVoucher` if found, `None` otherwise.
    pub fn get_voucher(env: Env, voucher_id: u64) -> Option<RefundVoucher> {
        env.storage()
            .instance()
            .get(&VoucherKey::Voucher(voucher_id))
    }

    /// Get all refund vouchers issued to a customer.
    ///
    /// # Arguments
    /// * `customer` - The customer address to query.
    ///
    /// # Returns
    /// A vector of `RefundVoucher` entries for the customer.
    pub fn get_customer_vouchers(env: Env, customer: Address) -> Vec<RefundVoucher> {
        let count: u64 = env
            .storage()
            .instance()
            .get(&VoucherKey::CustomerVoucherCount(customer.clone()))
            .unwrap_or(0);
        let mut results = Vec::new(&env);
        let mut i = 0u64;
        while i < count {
            if let Some(vid) = env
                .storage()
                .instance()
                .get::<_, u64>(&VoucherKey::CustomerVoucher(customer.clone(), i))
            {
                if let Some(v) = env
                    .storage()
                    .instance()
                    .get::<_, RefundVoucher>(&VoucherKey::Voucher(vid))
                {
                    results.push_back(v);
                }
            }
            i += 1;
        }
        results
    }
}
