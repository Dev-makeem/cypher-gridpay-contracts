//! Storage keys and schema versioning for the refund contract.

use crate::*;

// Issue #138 workaround: Using tuple-based storage keys with Symbol
// to avoid LengthExceedsMax error from large #[contracttype] enums
pub type StorageKey = (Symbol, Option<Address>, Option<u64>, Option<u32>);

/// Construct a tuple-based storage key from its components.
///
/// Uses `Symbol::new` with `Env::default()` to create the prefix symbol.
///
/// # Arguments
/// * `prefix` - A string prefix for the storage key.
/// * `addr` - An optional address component.
/// * `id` - An optional numeric ID component.
/// * `sub_id` - An optional sub-ID component.
///
/// # Returns
/// A `StorageKey` tuple suitable for use in contract storage.
pub fn make_key(
    prefix: &str,
    addr: Option<Address>,
    id: Option<u64>,
    sub_id: Option<u32>,
) -> StorageKey {
    (Symbol::new(&Env::default(), prefix), addr, id, sub_id)
}

// Legacy DataKey - split into functional groups to avoid LengthExceedsMax
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum DataKey {
    Admin,
    Refund(u64),
    RefundCounter,
    RefundsByStatus(RefundStatus, u64),
    RefundStatusCount(RefundStatus),
    RefundStatusIndex(u64),
    MerchantRefunds(Address, u64),
    MerchantRefundQuota(Address),
    MerchantRefundCount(Address),
    CustomerRefunds(Address, u64),
    CustomerRefundCount(Address),
    // Issue: bound unbounded per-customer history growth by archiving old entries
    CustomerRefundHistoryStart(Address),
    CustomerRefundsArchive(Address, u64),
    PaymentRefunds(u64, u64),
    PaymentRefundCount(u64),
    PoolToken(u64),
    DefaultRefundPolicy,
    RefundPolicy(Address),
    // Policy versioning (#134)
    RefundPolicyVersion(Address, u32),
    RefundPolicyVersionCount(Address),
    RefundPolicyTemplate(u64),
    RefundPolicyTemplateCount,
    // Payment contract address (#143)
    PaymentContractAddress,
    BatchRefundLimit,
    RefundAnalyticsKey,
    // Rate limiting
    CustomerRefundRateLimit(Address),
    GlobalRefundRateLimit,
    // Admin override audit log
    AdminOverrideHistory(u64),
    AdminOverrideHistoryCount,
    // Payment refund caps
    PaymentRefundCap(u64),
    PaymentRefundUsage(u64),
    AutoApproveBelowCeiling,
    // Issue #370: Customer-tier-based refund caps
    CustomerTier(Address),
    CustomerTierPolicy(Address, u32),
    StrictTierPolicy(Address),
    AppealWindowSeconds,
    // Issue #389: two-step admin rotation
    PendingAdmin,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum ArbitrationKey {
    ArbitrationCase(u64),
    ArbitrationCounter,
    ArbitratorList,
    ArbitratorsVoted(u64),
    ArbitratorVote(u64, Address),
    ArbitrationFeeConfig,
    AccumulatedTreasuryFees,
    ArbitrationStakeConfig,
    ArbitrationStake(u64),
    ArbitratorReputation(Address),
    ArbitratorScoreIndex(i128, u64),
    ArbitratorScoreCount,
    ArbitrationTimeoutConfig,
    // Issue #194: Tiered arbitration
    SeniorArbitratorList,
    ArbitrationTierConfig,
    CaseEscalated(u64),
    // Uniqueness guard: maps refund_id -> case_id so the same refund
    // cannot be escalated into multiple parallel arbitration cases.
    CaseByRefund(u64),
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum PolicyKey {
    RefundPolicyVersion(Address, u32),
    RefundPolicyVersionCount(Address),
    AutoRefundTrigger(u64),
    AutoRefundTriggerCounter,
}

// Maximum number of a customer's refund references kept in "hot" instance
// storage. Older entries are moved to persistent storage (archived) so a
// customer's history can grow indefinitely without bloating the instance
// storage footprint read/written on every contract invocation.
pub(crate) const CUSTOMER_HISTORY_HOT_CAP: u64 = 50;

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum SystemKey {
    PauseStateKey,
    PauseHistoryEntry(u64),
    PauseHistoryCount,
    CircuitBreakerConfigKey,
    CircuitBreakerStateKey,
    WindowStart,
    WindowRefundVolume,
    WindowPaymentVolume,
    FraudSignal(Address),
    FraudConfig,
    FlaggedAddressesIndex,
    // Ordered list of flagged addresses: FlaggedAddress(n) -> Address, paired
    // with the FlaggedAddressesIndex counter so get_flagged_addresses can
    // enumerate every entry without iterating over all storage keys.
    FlaggedAddress(u64),
    RefundRejectedAt(u64),
    Appeal(u64),
    AppealCounter,
    AppealByRefund(u64),
    AppealByCustomer(Address, u64),
    AppealByCustomerCount(Address),
    // Notification hooks
    NotificationHook(u64),
    NotificationHookCounter,
    HooksByEvent(RefundEventType, u64),
    HooksByEventCount(RefundEventType),
    SubscriberHooks(Address, u64),
    SubscriberHookCount(Address),
    // Platform fee deduction on refund processing
    RefundFeeConfig,
    AccumulatedRefundFees,
    // Per-customer refund cooldown
    CustomerRefundCooldown(Address),
    RefundCooldownConfig,
    SchemaVersion,
    // Issue #382: cached reason-code analytics for a given [window_start, window_end]
    // ledger-timestamp range, so repeated queries over the same window don't
    // re-scan the full refund history.
    AnalyticsCache(u64, u64),
    // Tracks the distinct (window_start, window_end) pairs cached above, so a newly
    // processed refund can invalidate only the windows it actually falls within.
    AnalyticsCacheWindows,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum EvidenceKey {
    Evidence(u64, Address),
    EvidenceIndex(u64, u64),
    EvidenceCount(u64),
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum VoucherKey {
    Voucher(u64),
    VoucherCounter,
    CustomerVoucher(Address, u64),
    CustomerVoucherCount(Address),
    RefundVoucherIssued(u64),
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum TokenKey {
    SupportedToken(Address),
    TokenCount,
    TokenByIndex(u64),
}

// Issues #195/#197/#198/#199: extended storage keys
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum RefundExtKey {
    CategoryWindow(Address, u32),
    PaymentCategoryTag(u64),
    AssignmentConfig,
    RotationIndex,
    RefundTTLConfig,
}

/// Storage key for eligibility entries: keyed by (merchant, customer).
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum EligibilityKey {
    /// The eligibility entry for a (merchant, customer) pair.
    Entry(Address, Address),
    /// Ordered index of customers for a merchant: (merchant, index) → customer.
    MerchantCustomerIndex(Address, u64),
    /// Total number of eligibility entries for a merchant.
    MerchantCustomerCount(Address),
}

#[contractimpl]
impl RefundContract {
    pub(crate) const INITIAL_SCHEMA_VERSION: u32 = 1;

    /// Get the current schema version of the contract.
    ///
    /// Returns the schema version number. Defaults to `INITIAL_SCHEMA_VERSION` if not set.
    pub fn get_schema_version(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&SystemKey::SchemaVersion)
            .unwrap_or(Self::INITIAL_SCHEMA_VERSION)
    }

    /// Migrate the contract schema to a new version.
    ///
    /// # Arguments
    /// * `admin` - The admin address (must be authorized and match stored admin).
    /// * `target_version` - The target schema version to migrate to.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the admin.
    /// Returns `SchemaAlreadyAtTarget` if the current version is already at or past the target.
    pub fn migrate_schema(env: Env, admin: Address, target_version: u32) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        let current = Self::get_schema_version(env.clone());
        if current >= target_version {
            return Err(Error::Ext(ExtError::SchemaAlreadyAtTarget));
        }

        env.storage()
            .instance()
            .set(&SystemKey::SchemaVersion, &target_version);
        Ok(())
    }
}
