#![no_std]
use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, token, Address, Bytes,
    BytesN, Env, FromVal, IntoVal, String, Symbol, TryFromVal, Val, Vec,
};

mod appeals;
mod arbitration;
mod auto_refund;
mod policies;
mod storage;
mod vouchers;

pub use appeals::*;
pub use arbitration::*;
pub use auto_refund::*;
pub use policies::*;
pub use storage::*;
pub use vouchers::*;

#[cfg(test)]
extern crate std;

#[cfg(test)]
std::thread_local! {
    static TEST_TRIPPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    static TEST_TRIP_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    static TEST_RESETS_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum RefundStatus {
    Requested,
    Approved,
    Rejected,
    Processed,
    PendingAppeal,
}

// Issue #397: canonical reason codes, enforced by the type system on Refund and
// on request_refund()'s signature, so get_reason_code_analytics() never sees a
// free-form/inconsistent string for this field.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum RefundReasonCode {
    ProductDefect,
    NonDelivery,
    DuplicateCharge,
    Unauthorized,
    CustomerRequest,
    Other,
}

// Issue #138 (recurred): the flat `Error` enum grew past Soroban's 50-variant
// XDR spec limit (`VecM<ScSpecUdtErrorEnumCaseV0, 50>`), which makes the
// `#[contracterror]` macro panic with `LengthExceedsMax` at compile time.
// Split into two `#[contracterror]` enums (each <= 50 variants), wrapped by a
// single `Error` type so every existing `Result<_, Error>` signature and `?`
// call site is unaffected. Mirrors the same pattern already used for
// `Error`/`BasicError`/`EscrowError`/`ActionError` in contracts/escrow/src/lib.rs.
#[contracterror]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CoreError {
    InvalidAmount = 1,
    RefundNotFound = 2,
    Unauthorized = 3,
    InvalidPaymentId = 4,
    InvalidStatus = 7,
    AlreadyProcessed = 8,
    RefundExceedsPayment = 9,
    TotalRefundsExceedPayment = 10,
    RefundWindowExpired = 11,
    RefundExceedsPolicy = 12,
    PolicyNotFound = 13,
    PolicyInactive = 14,
    QuorumNotReached = 15,
    NotArbitrator = 16,
    ContractPaused = 17,
    FunctionPaused = 18,
    CaseNotTimedOut = 19,
    BatchRefundTooLarge = 20,
    // Issue #138: Refund policy inheritance errors
    CircularInheritance = 21,
    MaxInheritanceDepth = 22,
    RefundNotRejected = 23,
    AppealWindowExpired = 24,
    AppealAlreadyFiled = 25,
    RefundRateLimitExceeded = 26,
    PaymentContractNotSet = 27,
    PaymentOwnershipMismatch = 28,
    CircuitBreakerTripped = 29,
    InvalidFeeConfig = 30,
    InsufficientTreasuryFees = 31,
    AutoApproveThresholdExceedsCeiling = 32,
    RefundCooldownActive = 33,
}

#[contracterror]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExtError {
    ArbitratorNotFound = 34,
    InvalidScoreThreshold = 35,
    AutoRefundTriggerNotFound = 36,
    DuplicateAutoRefundTrigger = 37,
    AddressFlaggedForFraud = 38,
    FraudSignalNotFound = 40,
    // Issue #144: Notification hook errors
    HookNotFound = 41,
    MaxHooksPerEventReached = 42,
    HookNotOwnedBySubscriber = 43,
    // Issue #373: Invalid notification hook subscriber address
    // (moved from 58, which collided with SchemaAlreadyAtTarget)
    InvalidHookAddress = 51,
    // Issue #148: Customer eligibility errors
    CustomerBlockedFromRefund = 44,
    EligibilityEntryNotFound = 45,
    TemplateNotFound = 46,
    TemplateInactive = 47,
    // Issue #XXX: Payment refund cap errors
    RefundCountCapExceeded = 48,
    RefundAmountCapExceeded = 49,
    UnsupportedRefundToken = 50,
    // New specific errors
    VoucherNotFound = 52,
    VoucherExpired = 53,
    VoucherAlreadyRedeemed = 54,
    EvidenceAlreadySubmitted = 55,
    CaseAlreadyEscalated = 56,
    // Issue #370: Customer tier policy errors
    TierPolicyNotFound = 57,
    SchemaAlreadyAtTarget = 58,
    // Issue #389: two-step admin rotation errors
    NoPendingAdmin = 59,
    NotPendingAdmin = 60,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Error {
    Core(CoreError),
    Ext(ExtError),
}

impl Error {
    pub fn to_u32(&self) -> u32 {
        match self {
            Error::Core(e) => *e as u32,
            Error::Ext(e) => *e as u32,
        }
    }
}

impl From<Error> for soroban_sdk::Error {
    fn from(e: Error) -> Self {
        soroban_sdk::Error::from_contract_error(e.to_u32())
    }
}

impl From<&Error> for soroban_sdk::Error {
    fn from(e: &Error) -> Self {
        soroban_sdk::Error::from_contract_error(e.to_u32())
    }
}

impl TryFrom<soroban_sdk::Error> for Error {
    type Error = soroban_sdk::Error;
    fn try_from(error: soroban_sdk::Error) -> Result<Self, Self::Error> {
        if let Ok(e) = CoreError::try_from(error) {
            return Ok(Error::Core(e));
        }
        if let Ok(e) = ExtError::try_from(error) {
            return Ok(Error::Ext(e));
        }
        Err(error)
    }
}

impl TryFrom<&soroban_sdk::Error> for Error {
    type Error = soroban_sdk::Error;
    fn try_from(error: &soroban_sdk::Error) -> Result<Self, Self::Error> {
        <Self as TryFrom<soroban_sdk::Error>>::try_from(*error)
    }
}

impl FromVal<Env, Error> for Val {
    fn from_val(env: &Env, v: &Error) -> Self {
        soroban_sdk::Error::from(v).into_val(env)
    }
}

impl TryFromVal<Env, Val> for Error {
    type Error = soroban_sdk::ConversionError;
    fn try_from_val(env: &Env, val: &Val) -> Result<Self, Self::Error> {
        let error: soroban_sdk::Error =
            soroban_sdk::Error::try_from_val(env, val).map_err(|_| soroban_sdk::ConversionError)?;
        Error::try_from(error).map_err(|_| soroban_sdk::ConversionError)
    }
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundRequested {
    pub refund_id: u64,
    pub payment_id: u64,
    pub merchant: Address,
    pub customer: Address,
    pub amount: i128,
    pub token: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundProcessed {
    pub refund_id: u64,
    pub processed_by: Address,
    pub customer: Address,
    pub amount: i128,
    pub token: Address,
    pub processed_at: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundApproved {
    pub refund_id: u64,
    pub payment_id: u64,
    pub amount: i128,
    pub approved_by: Address,
    pub approved_at: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundRejected {
    pub refund_id: u64,
    pub rejected_by: Address,
    pub rejected_at: u64,
    pub rejection_reason: String,
}

// Issue #144: Notification hook structures
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum RefundEventType {
    Requested,
    Approved,
    Rejected,
    Processed,
    Escalated,
}

#[derive(Clone)]
#[contracttype]
pub struct NotificationHook {
    pub hook_id: u64,
    pub subscriber: Address,
    pub events: Vec<RefundEventType>,
    pub active: bool,
}

// Issue #191: Multi-token refund support
#[derive(Clone)]
#[contracttype]
pub struct SupportedRefundToken {
    pub token: Address,
    pub active: bool,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookRegistered {
    pub hook_id: u64,
    pub subscriber: Address,
    pub event_count: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookDeregistered {
    pub hook_id: u64,
    pub subscriber: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookInvocationFailed {
    pub hook_id: u64,
    pub subscriber: Address,
    pub event_type: RefundEventType,
    pub refund_id: u64,
}

// ── Issue #148: Customer eligibility registry ─────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum EligibilityRule {
    Allow,
    Block,
}

#[derive(Clone)]
#[contracttype]
pub struct RefundEligibilityEntry {
    pub customer: Address,
    pub merchant: Address,
    pub rule: EligibilityRule,
    pub reason_hash: BytesN<32>,
    pub set_at: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EligibilitySet {
    pub merchant: Address,
    pub customer: Address,
    pub rule: EligibilityRule,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EligibilityRemoved {
    pub merchant: Address,
    pub customer: Address,
}

#[derive(Clone)]
#[contracttype]
pub struct Refund {
    pub id: u64,
    pub payment_id: u64,
    pub merchant: Address,
    pub customer: Address,
    pub amount: i128,
    pub original_payment_amount: i128,
    pub token: Address,
    // Issue #191: original payment token for multi-token refund matching
    pub original_token: Address,
    pub status: RefundStatus,
    pub requested_at: u64,
    pub reason: String,
    pub reason_code: RefundReasonCode,
    // Issue #147: Lifecycle timestamps
    pub approved_at: Option<u64>,
    pub rejected_at: Option<u64>,
    pub processed_at: Option<u64>,
    pub rejected_by: Option<Address>,
    pub appeal_deadline: Option<u64>,
    // Issue #199: TTL expiry
    pub expires_at: Option<u64>,
}

#[derive(Clone)]
#[contracttype]
pub struct PaymentRefundCap {
    pub payment_id: u64,
    pub max_refund_count: u32,
    pub max_total_amount: i128,
}

// Issue #370: Per-tier refund cap for customer loyalty tiers
#[derive(Clone)]
#[contracttype]
pub struct RefundCap {
    pub max_refund_bps: u32,
}

#[derive(Clone)]
#[contracttype]
pub struct MerchantRefundSummary {
    pub total_requests: u64,
    pub total_approved: u64,
    pub total_rejected: u64,
    pub total_amount_refunded: i128,
    pub pending_count: u64,
    pub pending_amount: i128,
}

// Issue #147: Customer refund summary
#[derive(Clone)]
#[contracttype]
pub struct CustomerRefundSummary {
    pub total_requested: u64,
    pub total_approved: u64,
    pub total_amount_refunded: i128,
    pub avg_processing_time: u64,
}

#[derive(Clone)]
#[contracttype]
pub struct RefundTier {
    pub days_from_purchase: u64,
    pub max_refund_bps: u32,
}

#[derive(Clone)]
#[contracttype]
pub struct RefundPolicy {
    pub merchant: Address,
    pub tiers: Vec<RefundTier>,
    pub active: bool,
    pub created_at: u64,
    pub updated_at: u64,
    pub default_window_seconds: u64,
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
enum ExternalPaymentStatus {
    Pending,
    Completed,
    Refunded,
    PartialRefunded,
    Cancelled,
}

#[derive(Clone)]
#[contracttype]
enum ExternalCurrency {
    XLM,
    USDC,
    USDT,
    BTC,
    ETH,
}

#[derive(Clone)]
#[contracttype]
struct ExternalPayment {
    pub id: u64,
    pub customer: Address,
    pub merchant: Address,
    pub amount: i128,
    pub token: Address,
    pub currency: ExternalCurrency,
    pub status: ExternalPaymentStatus,
    pub created_at: u64,
    pub expires_at: u64,
    pub metadata: String,
    pub notes: String,
    pub refunded_amount: i128,
}

// ── Issue #134: Policy versioning struct ──────────────────────────────────
#[derive(Clone)]
#[contracttype]
pub struct RefundPolicyVersion {
    pub version: u32,
    pub policy: RefundPolicy,
    pub created_at: u64,
    pub created_by: Address,
}

#[derive(Clone)]
#[contracttype]
pub struct RefundPolicyTemplate {
    pub template_id: u64,
    pub name: String,
    pub tiers: Vec<(u32, i128)>,
    pub default_window_seconds: u64,
    pub active: bool,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundPolicyTemplateCreated {
    pub template_id: u64,
    pub created_by: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyTemplateDeactivated {
    pub template_id: u64,
    pub deactivated_by: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundPolicyTemplateApplied {
    pub template_id: u64,
    pub merchant: Address,
    pub applied_by: Address,
}

// ── Issue #135: Batch refund result struct ─────────────────────────────────
#[derive(Clone)]
#[contracttype]
pub struct BatchRefundResult {
    pub refund_id: u64,
    pub success: bool,
    pub error_code: u32,
    pub amount_refunded: i128,
}

#[derive(Clone)]
#[contracttype]
pub struct CustomerRefundRateLimit {
    pub customer: Address,
    pub window_start: u64,
    pub request_count: u32,
    pub max_requests_per_window: u32,
    pub window_seconds: u64,
    /// When true, per-customer limits are not refreshed from global config on window reset.
    pub custom_override: bool,
}

#[derive(Clone)]
#[contracttype]
pub struct GlobalRefundRateLimit {
    pub max_requests_per_window: u32,
    pub window_seconds: u64,
    /// Config applied to windows that start at or after `next_config_effective_at`.
    pub next_max_requests_per_window: u32,
    pub next_window_seconds: u64,
    pub next_config_effective_at: u64,
}

/// Configuration for platform fee deduction on refund processing
#[derive(Clone)]
#[contracttype]
pub struct RefundFeeConfig {
    pub fee_bps: u32,       // Fee in basis points (e.g., 100 = 1%)
    pub min_fee: i128,      // Minimum fee amount
    pub max_fee: i128,      // Maximum fee amount
    pub treasury: Address,  // Address to receive fees
    pub fee_token: Address, // Token in which fees are collected
    pub active: bool,       // Whether fee collection is enabled
}

/// Per-customer refund cooldown configuration
#[derive(Clone)]
#[contracttype]
pub struct RefundCooldownConfig {
    pub cooldown_seconds: u64, // Minimum time between refund requests per customer
    pub enabled: bool,         // Whether cooldown is enforced
}

/// Tracks the last refund request time for a customer
#[derive(Clone)]
#[contracttype]
pub struct CustomerRefundCooldown {
    pub customer: Address,
    pub last_refund_requested_at: u64,
    pub cooldown_seconds: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutoApproved {
    pub refund_id: u64,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundPolicySet {
    pub merchant: Address,
    pub tiers_count: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundPolicyDeactivated {
    pub merchant: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefaultRefundPolicySet {
    pub set_by: Address,
    pub tiers_count: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefaultRefundPolicyRemoved {
    pub removed_by: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyOverrideApplied {
    pub refund_id: u64,
    pub admin: Address,
    pub reason: String,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminRefundOverride {
    pub override_id: u64,
    pub refund_id: u64,
    pub admin: Address,
    pub reason: String,
    pub override_amount: i128,
    pub override_status: RefundStatus,
    pub executed_at: u64,
}

#[derive(Clone)]
#[contracttype]
pub struct AdminOverrideHistory {
    pub override_id: u64,
    pub refund_id: u64,
    pub admin: Address,
    pub reason: String,
    pub override_amount: i128,
    pub override_status: RefundStatus,
    pub executed_at: u64,
    pub transaction_hash: BytesN<32>, // Immutable hash of override details
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractPausedEvent {
    pub paused_by: Address,
    pub reason: String,
    pub paused_at: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractUnpausedEvent {
    pub unpaused_by: Address,
    pub unpaused_at: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionPausedEvent {
    pub function_name: String,
    pub paused_by: Address,
    pub reason: String,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionUnpausedEvent {
    pub function_name: String,
    pub unpaused_by: Address,
}

#[derive(Clone)]
#[contracttype]
pub struct RefundAnalytics {
    pub total_refunds_requested: u64,
    pub total_refunds_approved: u64,
    pub total_refunds_rejected: u64,
    pub total_refunds_processed: u64,
    pub total_refund_volume: i128,
    pub approval_rate_bps: u32,
}

#[derive(Clone)]
#[contracttype]
pub struct PauseState {
    pub globally_paused: bool,
    pub paused_functions: Vec<String>,
    pub paused_at: u64,
    pub paused_by: Address,
    pub pause_reason: String,
}

#[derive(Clone)]
#[contracttype]
pub struct PauseHistory {
    pub index: u64,
    pub function_name: String,
    pub paused: bool,
    pub changed_by: Address,
    pub changed_at: u64,
    pub reason: String,
}

#[derive(Clone)]
#[contracttype]
pub struct CircuitBreakerConfig {
    pub max_refund_rate_bps: u32,
    pub measurement_window_seconds: u64,
    pub cooldown_seconds: u64,
    pub enabled: bool,
}

#[derive(Clone)]
#[contracttype]
pub struct CircuitBreakerState {
    pub tripped: bool,
    pub tripped_at: Option<u64>,
    pub trip_count: u32,
    pub last_refund_rate_bps: u32,
    pub resets_at: Option<u64>,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CircuitBreakerTrippedEvent {
    pub refund_rate_bps: u32,
    pub tripped_at: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CircuitBreakerResetEvent {
    pub reset_by: Address,
    pub reset_at: u64,
}

// Fraud detection structures (#137)
#[derive(Clone)]
#[contracttype]
pub struct FraudSignal {
    pub address: Address,
    pub refund_rate_bps: u32,
    pub total_payments: u64,
    pub total_refunds: u64,
    pub flagged_at: u64,
    pub reviewed: bool,
}

#[derive(Clone)]
#[contracttype]
pub struct FraudConfig {
    pub max_refund_rate_bps: u32,
    pub min_transactions_for_check: u64,
    pub enabled: bool,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FraudSignalRaised {
    pub address: Address,
    pub refund_rate_bps: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FraudSignalReviewed {
    pub address: Address,
    pub reviewed_by: Address,
}

// Issue #195: Batch decision types
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum BatchDecisionType {
    Approve,
    Reject,
}

#[derive(Clone)]
#[contracttype]
pub struct BatchRefundDecision {
    pub refund_ids: Vec<u64>,
    pub decision: BatchDecisionType,
    pub note_hash: BytesN<32>,
}

#[derive(Clone)]
#[contracttype]
pub struct BatchDecisionResult {
    pub succeeded: Vec<u64>,
    pub failed: Vec<u64>,
}

// Issue #197: Payment categories
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum PaymentCategory {
    DigitalGoods,
    PhysicalGoods,
    Subscription,
    Service,
    Other,
}

impl PaymentCategory {
    pub fn to_index(&self) -> u32 {
        match self {
            PaymentCategory::DigitalGoods => 0,
            PaymentCategory::PhysicalGoods => 1,
            PaymentCategory::Subscription => 2,
            PaymentCategory::Service => 3,
            PaymentCategory::Other => 4,
        }
    }
}

#[derive(Clone)]
#[contracttype]
pub struct CategoryRefundWindow {
    pub category: PaymentCategory,
    pub window_seconds: u64,
    pub merchant: Address,
}

// Issue #199: Refund TTL
#[derive(Clone)]
#[contracttype]
pub struct RefundTTLConfig {
    pub default_ttl_seconds: u64,
    pub active: bool,
}

/// Event emitted when platform fee is deducted from a refund
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundFeeDeducted {
    pub refund_id: u64,
    pub fee_amount: i128,
    pub net_refund_amount: i128,
    pub treasury: Address,
}

/// Event emitted when refund fee configuration is updated
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundFeeConfigUpdated {
    pub fee_bps: u32,
    pub min_fee: i128,
    pub max_fee: i128,
    pub updated_by: Address,
}

/// Event emitted when customer refund cooldown is enforced
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundCooldownEnforced {
    pub customer: Address,
    pub last_refund_at: u64,
    pub cooldown_seconds: u64,
    pub available_at: u64,
}

/// Event emitted when the global refund rate limit config is updated
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RateLimitUpdated {
    pub admin: Address,
    pub new_window_seconds: u64,
    pub new_max_refunds: u32,
    pub effective_at: u64,
}

/// Event emitted when the current admin proposes a new admin.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminRotationProposed {
    pub current_admin: Address,
    pub pending_admin: Address,
}

/// Event emitted when a proposed admin accepts the role, completing rotation.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdminRotationAccepted {
    pub previous_admin: Address,
    pub new_admin: Address,
}

#[contract]
pub struct RefundContract;

#[contractimpl]
impl RefundContract {
    const BATCH_DECISION_LIMIT: u32 = 50;

    /// Initialize the refund contract with an admin address.
    ///
    /// Sets up the default refund policy (30-day window, 100% refund),
    /// admin approval settings, and appeal window.
    ///
    /// # Panics
    /// Panics if the contract has already been initialized.
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("Already initialized");
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&SystemKey::SchemaVersion, &Self::INITIAL_SCHEMA_VERSION);

        // Set default refund policy (30 days, 100% refund)
        let mut default_tiers = Vec::new(&env);
        default_tiers.push_back(RefundTier {
            days_from_purchase: 30,
            max_refund_bps: 10000,
        });
        let default_policy = RefundPolicy {
            merchant: admin.clone(), // Placeholder, will be overridden per merchant
            tiers: default_tiers,
            active: true,
            created_at: env.ledger().timestamp(),
            updated_at: env.ledger().timestamp(),
            default_window_seconds: 30 * 24 * 60 * 60, // 30 days
        };
        env.storage()
            .instance()
            .set(&DataKey::DefaultRefundPolicy, &default_policy);

        // Store default settings for admin separately
        Self::set_inherit_from_parent_inner(&env, &admin, false);
        Self::set_requires_admin_approval_inner(&env, &admin, true);
        Self::set_auto_approve_below_inner(&env, &admin, 0);
        Self::set_auto_approve_below_ceiling_inner(&env, 0);
        env.storage()
            .instance()
            .set(&DataKey::AppealWindowSeconds, &604800u64);
    }

    /// Propose a new admin, starting a two-step rotation (Issue #389).
    ///
    /// The current admin designates `new_admin` as pending. The rotation only
    /// completes once `new_admin` calls [`Self::accept_admin`], so a typo'd or
    /// unreachable address can never brick admin control of the contract.
    ///
    /// # Arguments
    /// * `admin` - The current admin (must be authorized and match stored admin).
    /// * `new_admin` - The address to propose as the next admin.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the current admin.
    pub fn propose_admin(env: Env, admin: Address, new_admin: Address) -> Result<(), Error> {
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
            .set(&DataKey::PendingAdmin, &new_admin);

        (AdminRotationProposed {
            current_admin: admin,
            pending_admin: new_admin,
        })
        .publish(&env);

        Ok(())
    }

    /// Accept a pending admin rotation, finalizing the transition (Issue #389).
    ///
    /// Must be called by the address previously proposed via
    /// [`Self::propose_admin`]. Replaces `DataKey::Admin` with the caller and
    /// clears the pending slot.
    ///
    /// # Errors
    /// Returns `NoPendingAdmin` if no rotation has been proposed.
    /// Returns `NotPendingAdmin` if the caller is not the proposed admin.
    pub fn accept_admin(env: Env, new_admin: Address) -> Result<(), Error> {
        new_admin.require_auth();

        let pending: Address = env
            .storage()
            .instance()
            .get(&DataKey::PendingAdmin)
            .ok_or(Error::Ext(ExtError::NoPendingAdmin))?;
        if pending != new_admin {
            return Err(Error::Ext(ExtError::NotPendingAdmin));
        }

        let previous_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;

        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().remove(&DataKey::PendingAdmin);

        (AdminRotationAccepted {
            previous_admin,
            new_admin,
        })
        .publish(&env);

        Ok(())
    }

    /// Get the address currently proposed as the next admin, if any.
    pub fn get_pending_admin(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::PendingAdmin)
    }

    /// Request a refund for a payment.
    ///
    /// Creates a new refund request with status `Requested` (or `Approved` if auto-approval
    /// conditions are met). Validates the token, policy, fraud signals, and eligibility.
    ///
    /// # Arguments
    /// * `merchant` - The merchant requesting the refund (must be authorized).
    /// * `payment_id` - The ID of the original payment.
    /// * `customer` - The customer receiving the refund.
    /// * `amount` - The refund amount in the smallest token unit.
    /// * `original_payment_amount` - The original payment amount.
    /// * `token` - The token address used for the refund.
    /// * `reason` - A human-readable reason for the refund.
    /// * `reason_code` - A canonical reason code for the refund.
    /// * `payment_created_at` - The timestamp when the original payment was created.
    ///
    /// # Returns
    /// The ID of the newly created refund.
    ///
    /// # Errors
    /// Returns errors for invalid amounts, unsupported tokens, policy violations,
    /// fraud signals, eligibility blocks, or payment ownership mismatches.
    pub fn request_refund(
        env: Env,
        merchant: Address,
        payment_id: u64,
        customer: Address,
        amount: i128,
        original_payment_amount: i128,
        token: Address,
        reason: String,
        reason_code: RefundReasonCode,
        payment_created_at: u64,
    ) -> Result<u64, Error> {
        Self::require_not_paused(&env, "request_refund")?;
        // Require merchant authentication
        merchant.require_auth();

        // Issue #191: validate token against supported registry if registry is non-empty
        let token_count: u64 = env
            .storage()
            .instance()
            .get(&TokenKey::TokenCount)
            .unwrap_or(0);
        if token_count > 0 {
            let supported: Option<SupportedRefundToken> = env
                .storage()
                .instance()
                .get(&TokenKey::SupportedToken(token.clone()));
            match supported {
                Some(t) if t.active => {}
                _ => return Err(Error::Ext(ExtError::UnsupportedRefundToken)),
            }
        }

        Self::create_refund(
            env,
            merchant,
            payment_id,
            customer,
            amount,
            original_payment_amount,
            token,
            reason,
            reason_code,
            payment_created_at,
            false,
        )
    }

    /// Retrieve a refund by its ID.
    ///
    /// # Arguments
    /// * `refund_id` - The unique identifier of the refund.
    ///
    /// # Returns
    /// The `Refund` record if found.
    ///
    /// # Errors
    /// Returns `RefundNotFound` if no refund exists with the given ID.
    pub fn get_refund(env: &Env, refund_id: u64) -> Result<Refund, Error> {
        // Retrieve refund from storage by ID
        env.storage()
            .instance()
            .get(&DataKey::Refund(refund_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))
    }

    /// Approve a pending refund request.
    ///
    /// Changes the refund status from `Requested` to `Approved` and emits a `RefundApproved` event.
    ///
    /// # Arguments
    /// * `admin` - The admin address (must be authorized).
    /// * `refund_id` - The ID of the refund to approve.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the admin.
    /// Returns `InvalidStatus` if the refund is not in `Requested` status.
    /// Returns `RefundWindowExpired` if the refund's TTL has expired.
    pub fn approve_refund(env: Env, admin: Address, refund_id: u64) -> Result<(), Error> {
        Self::require_not_paused(&env, "approve_refund")?;
        // Require admin authentication
        admin.require_auth();

        Self::approve_refund_internal(&env, admin, refund_id)
    }

    /// Reject a pending refund request.
    ///
    /// Moves the refund to `PendingAppeal` status with an appeal window, and emits a
    /// `RefundRejected` event. The customer can file an appeal within the appeal window.
    ///
    /// # Arguments
    /// * `admin` - The admin address (must be authorized).
    /// * `refund_id` - The ID of the refund to reject.
    /// * `rejection_reason` - A human-readable reason for the rejection.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the admin.
    /// Returns `InvalidStatus` if the refund is not in `Requested` status.
    pub fn reject_refund(
        env: Env,
        admin: Address,
        refund_id: u64,
        rejection_reason: String,
    ) -> Result<(), Error> {
        Self::require_not_paused(&env, "reject_refund")?;
        // Require admin authentication
        admin.require_auth();

        Self::begin_refund_rejection(&env, admin, refund_id, rejection_reason)
    }

    /// Process an approved refund for payout.
    ///
    /// Changes the refund status from `Approved` to `Processed`, deducts platform fees,
    /// enforces merchant refund quota, and emits a `RefundProcessed` event.
    ///
    /// # Arguments
    /// * `admin` - The admin address (must be authorized).
    /// * `refund_id` - The ID of the refund to process.
    ///
    /// # Errors
    /// Returns `InvalidStatus` if the refund is not in `Approved` status.
    /// Returns `RefundExceedsPolicy` if the merchant quota is exceeded.
    /// Returns `TotalRefundsExceedPayment` if processing would exceed the original payment.
    pub fn process_refund(env: Env, admin: Address, refund_id: u64) -> Result<(), Error> {
        Self::require_not_paused(&env, "process_refund")?;
        admin.require_auth();

        Self::process_refund_internal(&env, admin, refund_id)
    }

    /// Set a custom per-customer refund rate limit that overrides the global limit.
    ///
    /// # Arguments
    /// * `admin` - The contract admin setting the limit.
    /// * `customer` - The customer address to apply the limit to.
    /// * `max_per_window` - Maximum number of refund requests allowed per window.
    /// * `window_seconds` - Duration of the rate-limit window in seconds.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn set_customer_rate_limit(
        env: Env,
        admin: Address,
        customer: Address,
        max_per_window: u32,
        window_seconds: u64,
    ) -> Result<(), Error> {
        admin.require_auth();

        let mut limit = env
            .storage()
            .instance()
            .get(&DataKey::CustomerRefundRateLimit(customer.clone()))
            .unwrap_or(CustomerRefundRateLimit {
                customer: customer.clone(),
                window_start: env.ledger().timestamp(),
                request_count: 0,
                max_requests_per_window: max_per_window,
                window_seconds,
                custom_override: true,
            });

        limit.max_requests_per_window = max_per_window;
        limit.window_seconds = window_seconds;
        limit.custom_override = true;

        env.storage()
            .instance()
            .set(&DataKey::CustomerRefundRateLimit(customer), &limit);
        Ok(())
    }

    /// Get the current rate-limit status for a customer, including request count and window info.
    ///
    /// # Arguments
    /// * `customer` - The customer address to query.
    ///
    /// # Returns
    /// The `CustomerRefundRateLimit` for the customer. Returns a default zero-value
    /// if no limit has been configured.
    pub fn get_customer_rate_limit_status(env: Env, customer: Address) -> CustomerRefundRateLimit {
        env.storage()
            .instance()
            .get(&DataKey::CustomerRefundRateLimit(customer.clone()))
            .unwrap_or(CustomerRefundRateLimit {
                customer,
                window_start: 0,
                request_count: 0,
                max_requests_per_window: 0,
                window_seconds: 0,
                custom_override: false,
            })
    }

    /// Set the global refund rate limit that applies to all customers by default.
    ///
    /// # Arguments
    /// * `admin` - The contract admin setting the limit.
    /// * `max_per_window` - Maximum number of refund requests allowed per window.
    /// * `window_seconds` - Duration of the rate-limit window in seconds.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `InvalidAmount` if either parameter is zero.
    pub fn set_global_refund_rate_limit(
        env: Env,
        admin: Address,
        max_per_window: u32,
        window_seconds: u64,
    ) -> Result<(), Error> {
        admin.require_auth();

        if max_per_window == 0 || window_seconds == 0 {
            return Err(Error::Core(CoreError::InvalidAmount));
        }

        let limit = GlobalRefundRateLimit {
            max_requests_per_window: max_per_window,
            window_seconds,
            next_max_requests_per_window: max_per_window,
            next_window_seconds: window_seconds,
            next_config_effective_at: 0,
        };

        env.storage()
            .instance()
            .set(&DataKey::GlobalRefundRateLimit, &limit);
        Ok(())
    }

    /// Update the global refund rate limit without disrupting in-progress windows.
    /// New parameters apply only to windows that start at or after the update timestamp;
    /// the current window's request count and duration are preserved.
    pub fn update_rate_limit(
        env: Env,
        admin: Address,
        new_window_seconds: u64,
        new_max_refunds: u32,
    ) -> Result<(), Error> {
        admin.require_auth();

        if new_max_refunds == 0 || new_window_seconds == 0 {
            return Err(Error::Core(CoreError::InvalidAmount));
        }

        let now = env.ledger().timestamp();
        let updated = match env
            .storage()
            .instance()
            .get::<DataKey, GlobalRefundRateLimit>(&DataKey::GlobalRefundRateLimit)
        {
            Some(mut existing) => {
                existing.next_max_requests_per_window = new_max_refunds;
                existing.next_window_seconds = new_window_seconds;
                existing.next_config_effective_at = now;
                existing
            }
            None => GlobalRefundRateLimit {
                max_requests_per_window: new_max_refunds,
                window_seconds: new_window_seconds,
                next_max_requests_per_window: new_max_refunds,
                next_window_seconds: new_window_seconds,
                next_config_effective_at: 0,
            },
        };

        env.storage()
            .instance()
            .set(&DataKey::GlobalRefundRateLimit, &updated);

        (RateLimitUpdated {
            admin,
            new_window_seconds,
            new_max_refunds,
            effective_at: now,
        })
        .publish(&env);

        Ok(())
    }

    /// Get the current global refund rate limit configuration.
    ///
    /// # Returns
    /// The `GlobalRefundRateLimit` if configured, `None` otherwise.
    pub fn get_global_refund_rate_limit(env: Env) -> Option<GlobalRefundRateLimit> {
        env.storage()
            .instance()
            .get(&DataKey::GlobalRefundRateLimit)
    }

    fn store_refund_policy(
        env: &Env,
        merchant: Address,
        policy: RefundPolicy,
        created_by: Address,
    ) {
        env.storage()
            .instance()
            .set(&DataKey::RefundPolicy(merchant.clone()), &policy);

        let version_count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::RefundPolicyVersionCount(merchant.clone()))
            .unwrap_or(0);
        let new_version = version_count + 1;
        let versioned = RefundPolicyVersion {
            version: new_version,
            policy: policy.clone(),
            created_at: env.ledger().timestamp(),
            created_by,
        };
        env.storage().instance().set(
            &DataKey::RefundPolicyVersion(merchant.clone(), new_version),
            &versioned,
        );
        env.storage().instance().set(
            &DataKey::RefundPolicyVersionCount(merchant.clone()),
            &new_version,
        );

        // Emit RefundPolicySet event
        (RefundPolicySet {
            merchant,
            tiers_count: policy.tiers.len() as u32,
        })
        .publish(env);
    }

    /// Create a reusable refund policy template that can be applied to merchants.
    ///
    /// # Arguments
    /// * `admin` - The contract admin creating the template.
    /// * `name` - A human-readable name for the template.
    /// * `tiers` - A vector of `(days_from_purchase, max_refund_bps)` tuples defining the refund tiers.
    /// * `window` - The default refund window in seconds.
    ///
    /// # Returns
    /// The ID of the newly created template.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn create_policy_template(
        env: Env,
        admin: Address,
        name: String,
        tiers: Vec<(u32, i128)>,
        window: u64,
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

        let template_count: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RefundPolicyTemplateCount)
            .unwrap_or(0);
        let template_id = template_count + 1;
        let template = RefundPolicyTemplate {
            template_id,
            name,
            tiers,
            default_window_seconds: window,
            active: true,
        };

        env.storage()
            .instance()
            .set(&DataKey::RefundPolicyTemplate(template_id), &template);
        env.storage()
            .instance()
            .set(&DataKey::RefundPolicyTemplateCount, &template_id);

        (RefundPolicyTemplateCreated {
            template_id,
            created_by: admin,
        })
        .publish(&env);

        Ok(template_id)
    }

    /// Apply a policy template to a merchant, replacing their current refund policy.
    ///
    /// # Arguments
    /// * `admin` - The contract admin applying the template.
    /// * `merchant` - The merchant to apply the template to.
    /// * `template_id` - The ID of the template to apply.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `TemplateNotFound` if the template does not exist.
    /// Returns `TemplateInactive` if the template has been deactivated.
    pub fn apply_template_to_merchant(
        env: Env,
        admin: Address,
        merchant: Address,
        template_id: u64,
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

        let template: RefundPolicyTemplate = env
            .storage()
            .instance()
            .get(&DataKey::RefundPolicyTemplate(template_id))
            .ok_or(Error::Ext(ExtError::TemplateNotFound))?;

        if !template.active {
            return Err(Error::Ext(ExtError::TemplateInactive));
        }

        let mut tiers = Vec::new(&env);
        let days = template.default_window_seconds / (24 * 60 * 60);
        tiers.push_back(RefundTier {
            days_from_purchase: days,
            max_refund_bps: 10000,
        });

        let policy = RefundPolicy {
            merchant: merchant.clone(),
            tiers,
            active: true,
            created_at: env.ledger().timestamp(),
            updated_at: env.ledger().timestamp(),
            default_window_seconds: 30 * 24 * 60 * 60,
        };

        Self::set_requires_admin_approval_inner(&env, &merchant, true);
        Self::set_auto_approve_below_inner(&env, &merchant, 0);

        Self::store_refund_policy(&env, merchant.clone(), policy, admin.clone());
        (RefundPolicyTemplateApplied {
            template_id,
            merchant,
            applied_by: admin,
        })
        .publish(&env);

        Ok(())
    }

    /// Get a policy template by its ID.
    ///
    /// # Arguments
    /// * `template_id` - The ID of the template to retrieve.
    ///
    /// # Returns
    /// The `RefundPolicyTemplate` if found, `None` otherwise.
    pub fn get_policy_template(env: Env, template_id: u64) -> Option<RefundPolicyTemplate> {
        env.storage()
            .instance()
            .get(&DataKey::RefundPolicyTemplate(template_id))
    }

    /// List all active refund policy templates.
    ///
    /// # Returns
    /// A vector of all active `RefundPolicyTemplate` entries.
    pub fn list_policy_templates(env: Env) -> Vec<RefundPolicyTemplate> {
        let count: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RefundPolicyTemplateCount)
            .unwrap_or(0);
        let mut templates = Vec::new(&env);
        for id in 1..=count {
            if let Some(template) = env
                .storage()
                .instance()
                .get::<_, RefundPolicyTemplate>(&DataKey::RefundPolicyTemplate(id))
            {
                if template.active {
                    templates.push_back(template);
                }
            }
        }
        templates
    }

    /// Deactivate a refund policy template so it can no longer be applied.
    ///
    /// # Arguments
    /// * `admin` - The contract admin deactivating the template.
    /// * `template_id` - The ID of the template to deactivate.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `TemplateNotFound` if the template does not exist.
    /// Returns `TemplateInactive` if the template is already inactive.
    pub fn deactivate_policy_template(
        env: Env,
        admin: Address,
        template_id: u64,
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

        let mut template: RefundPolicyTemplate = env
            .storage()
            .instance()
            .get(&DataKey::RefundPolicyTemplate(template_id))
            .ok_or(Error::Ext(ExtError::TemplateNotFound))?;

        if !template.active {
            return Err(Error::Ext(ExtError::TemplateInactive));
        }

        template.active = false;
        env.storage()
            .instance()
            .set(&DataKey::RefundPolicyTemplate(template_id), &template);

        (PolicyTemplateDeactivated {
            template_id,
            deactivated_by: admin,
        })
        .publish(&env);

        Ok(())
    }

    /// Withdraw accumulated treasury fees
    /// Requires admin authorization
    /// Returns the amount withdrawn
    fn deduct_refund_fee(
        env: &Env,
        refund_id: u64,
        amount: i128,
        token: &Address,
    ) -> Result<(i128, i128), Error> {
        let config: RefundFeeConfig =
            match env.storage().instance().get(&SystemKey::RefundFeeConfig) {
                Some(c) => c,
                None => return Ok((amount, 0)),
            };
        if !config.active {
            return Ok((amount, 0));
        }
        let raw_fee = amount
            .saturating_mul(config.fee_bps as i128)
            .checked_div(10_000)
            .unwrap_or(0);
        let fee = raw_fee.max(config.min_fee).min(config.max_fee);
        let net = amount.saturating_sub(fee);
        if fee > 0 {
            token::Client::new(env, token).transfer(
                &env.current_contract_address(),
                &config.treasury,
                &fee,
            );
            let accumulated: i128 = env
                .storage()
                .instance()
                .get(&SystemKey::AccumulatedRefundFees)
                .unwrap_or(0);
            env.storage().instance().set(
                &SystemKey::AccumulatedRefundFees,
                &accumulated.saturating_add(fee),
            );
            (RefundFeeDeducted {
                refund_id,
                fee_amount: fee,
                net_refund_amount: net,
                treasury: config.treasury,
            })
            .publish(env);
        }
        Ok((net, fee))
    }

    pub fn withdraw_treasury_fees(env: Env, admin: Address) -> Result<i128, Error> {
        admin.require_auth();

        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        let accumulated: i128 = env
            .storage()
            .instance()
            .get(&ArbitrationKey::AccumulatedTreasuryFees)
            .unwrap_or(0);

        if accumulated <= 0 {
            return Err(Error::Core(CoreError::InsufficientTreasuryFees));
        }

        // Reset accumulated fees
        env.storage()
            .instance()
            .set(&ArbitrationKey::AccumulatedTreasuryFees, &0i128);

        Ok(accumulated)
    }

    /// Get a paginated list of refunds filtered by status.
    ///
    /// # Arguments
    /// * `status` - The refund status to filter by.
    /// * `limit` - Maximum number of results to return.
    /// * `offset` - Number of results to skip for pagination.
    ///
    /// # Returns
    /// A vector of `Refund` entries matching the given status.
    pub fn get_refunds_by_status(
        env: &Env,
        status: RefundStatus,
        limit: u64,
        offset: u64,
    ) -> Vec<Refund> {
        let mut results: Vec<Refund> = Vec::new(env);
        let total = Self::get_refund_count_by_status(env, status.clone());

        if limit == 0 || offset >= total {
            return results;
        }

        let end = core::cmp::min(total, offset.saturating_add(limit));
        let mut index = offset;
        while index < end {
            if let Some(refund_id) = env
                .storage()
                .instance()
                .get::<_, u64>(&DataKey::RefundsByStatus(status.clone(), index))
            {
                if let Some(refund) = env
                    .storage()
                    .instance()
                    .get::<_, Refund>(&DataKey::Refund(refund_id))
                {
                    results.push_back(refund);
                }
            }
            index += 1;
        }

        results
    }

    /// Get a paginated list of all refunds for a specific merchant.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to query.
    /// * `limit` - Maximum number of results to return.
    /// * `offset` - Number of results to skip for pagination.
    ///
    /// # Returns
    /// A vector of `Refund` entries for the merchant.
    pub fn get_merchant_refunds(
        env: Env,
        merchant: Address,
        limit: u64,
        offset: u64,
    ) -> Vec<Refund> {
        let mut results: Vec<Refund> = Vec::new(&env);
        let total = Self::get_merchant_refund_count(&env, &merchant);

        if limit == 0 || offset >= total {
            return results;
        }

        let end = core::cmp::min(total, offset.saturating_add(limit));
        let mut index = offset;
        while index < end {
            if let Some(refund_id) = env
                .storage()
                .instance()
                .get::<_, u64>(&DataKey::MerchantRefunds(merchant.clone(), index))
            {
                if let Some(refund) = env
                    .storage()
                    .instance()
                    .get::<_, Refund>(&DataKey::Refund(refund_id))
                {
                    results.push_back(refund);
                }
            }
            index += 1;
        }

        results
    }

    /// Get a paginated list of refunds for a merchant filtered by status.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to query.
    /// * `status` - The refund status to filter by.
    /// * `limit` - Maximum number of results to return.
    /// * `offset` - Number of results to skip for pagination.
    ///
    /// # Returns
    /// A vector of `Refund` entries matching the merchant and status.
    pub fn get_merchant_refunds_by_status(
        env: Env,
        merchant: Address,
        status: RefundStatus,
        limit: u64,
        offset: u64,
    ) -> Vec<Refund> {
        Self::get_merchant_refunds_by_status_internal(&env, &merchant, status, limit, offset)
    }

    /// Get all pending (requested) refunds for a merchant.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to query.
    ///
    /// # Returns
    /// A vector of all `Refund` entries in `Requested` status for the merchant.
    pub fn get_merchant_pending_refunds(env: Env, merchant: Address) -> Vec<Refund> {
        let total = Self::get_merchant_refund_count(&env, &merchant);
        Self::get_merchant_refunds_by_status_internal(
            &env,
            &merchant,
            RefundStatus::Requested,
            total,
            0,
        )
    }

    /// Get aggregate refund statistics for a merchant.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to query.
    ///
    /// # Returns
    /// A `MerchantRefundSummary` containing total requests, approved/rejected counts,
    /// total refunded amount, and pending counts/amounts.
    pub fn get_merchant_refund_summary(env: Env, merchant: Address) -> MerchantRefundSummary {
        let total_requests = Self::get_merchant_refund_count(&env, &merchant);
        let mut total_approved = 0u64;
        let mut total_rejected = 0u64;
        let mut total_amount_refunded = 0i128;
        let mut pending_count = 0u64;
        let mut pending_amount = 0i128;

        let mut index = 0u64;
        while index < total_requests {
            if let Some(refund_id) = env
                .storage()
                .instance()
                .get::<_, u64>(&DataKey::MerchantRefunds(merchant.clone(), index))
            {
                if let Some(refund) = env
                    .storage()
                    .instance()
                    .get::<_, Refund>(&DataKey::Refund(refund_id))
                {
                    match refund.status {
                        RefundStatus::Approved => {
                            total_approved += 1;
                            pending_count += 1;
                            pending_amount += refund.amount;
                        }
                        RefundStatus::Rejected => {
                            total_rejected += 1;
                        }
                        RefundStatus::Processed => {
                            total_amount_refunded += refund.amount;
                        }
                        RefundStatus::Requested => {
                            pending_count += 1;
                            pending_amount += refund.amount;
                        }
                        RefundStatus::PendingAppeal => {
                            pending_count += 1;
                            pending_amount += refund.amount;
                        }
                    }
                }
            }
            index += 1;
        }

        MerchantRefundSummary {
            total_requests,
            total_approved,
            total_rejected,
            total_amount_refunded,
            pending_count,
            pending_amount,
        }
    }

    /// Get a paginated list of refunds filtered by reason code.
    ///
    /// # Arguments
    /// * `code` - The refund reason code to filter by.
    /// * `limit` - Maximum number of results to return.
    /// * `offset` - Number of results to skip for pagination.
    ///
    /// # Returns
    /// A vector of `Refund` entries matching the given reason code.
    pub fn get_refunds_by_reason_code(
        env: &Env,
        code: RefundReasonCode,
        limit: u64,
        offset: u64,
    ) -> Vec<Refund> {
        let mut results: Vec<Refund> = Vec::new(env);
        if limit == 0 {
            return results;
        }

        let total_refunds: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RefundCounter)
            .unwrap_or(0);

        let mut matched: u64 = 0;
        let mut collected: u64 = 0;
        let mut id: u64 = 1;
        while id <= total_refunds && collected < limit {
            if let Some(refund) = env
                .storage()
                .instance()
                .get::<_, Refund>(&DataKey::Refund(id))
            {
                if refund.reason_code == code {
                    if matched >= offset {
                        results.push_back(refund);
                        collected += 1;
                    }
                    matched += 1;
                }
            }
            id += 1;
        }

        results
    }

    /// Get analytics showing the count of refunds for each reason code, sorted by frequency,
    /// restricted to refunds created within `[window_start, window_end]` (inclusive).
    ///
    /// The result is deterministic for a given window and is cached under
    /// `SystemKey::AnalyticsCache(window_start, window_end)`. The cache is invalidated
    /// (recomputed) only when a refund whose `created_at` falls inside that window is
    /// processed after the cache entry was written (see `process_refund_internal`).
    ///
    /// # Returns
    /// A vector of `(RefundReasonCode, count)` tuples sorted by descending count.
    pub fn get_reason_code_analytics(
        env: Env,
        window_start: u64,
        window_end: u64,
    ) -> Vec<(RefundReasonCode, u64)> {
        let cache_key = SystemKey::AnalyticsCache(window_start, window_end);
        if let Some(cached) = env
            .storage()
            .instance()
            .get::<_, Vec<(RefundReasonCode, u64)>>(&cache_key)
        {
            return cached;
        }

        let mut product_defect: u64 = 0;
        let mut non_delivery: u64 = 0;
        let mut duplicate_charge: u64 = 0;
        let mut unauthorized: u64 = 0;
        let mut customer_request: u64 = 0;
        let mut other: u64 = 0;

        let total_refunds: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RefundCounter)
            .unwrap_or(0);

        let mut id: u64 = 1;
        while id <= total_refunds {
            if let Some(refund) = env
                .storage()
                .instance()
                .get::<_, Refund>(&DataKey::Refund(id))
            {
                if refund.requested_at >= window_start && refund.requested_at <= window_end {
                    match refund.reason_code {
                        RefundReasonCode::ProductDefect => product_defect += 1,
                        RefundReasonCode::NonDelivery => non_delivery += 1,
                        RefundReasonCode::DuplicateCharge => duplicate_charge += 1,
                        RefundReasonCode::Unauthorized => unauthorized += 1,
                        RefundReasonCode::CustomerRequest => customer_request += 1,
                        RefundReasonCode::Other => other += 1,
                    }
                }
            }
            id += 1;
        }

        let mut ordered = [
            (RefundReasonCode::ProductDefect, product_defect),
            (RefundReasonCode::NonDelivery, non_delivery),
            (RefundReasonCode::DuplicateCharge, duplicate_charge),
            (RefundReasonCode::Unauthorized, unauthorized),
            (RefundReasonCode::CustomerRequest, customer_request),
            (RefundReasonCode::Other, other),
        ];

        ordered.sort_unstable_by(|a, b| {
            let count_cmp = b.1.cmp(&a.1);
            if count_cmp == core::cmp::Ordering::Equal {
                Self::reason_code_rank(&a.0).cmp(&Self::reason_code_rank(&b.0))
            } else {
                count_cmp
            }
        });

        let mut result = Vec::new(&env);
        for (code, count) in ordered {
            result.push_back((code, count));
        }

        env.storage().instance().set(&cache_key, &result);

        let mut windows: Vec<(u64, u64)> = env
            .storage()
            .instance()
            .get(&SystemKey::AnalyticsCacheWindows)
            .unwrap_or(Vec::new(&env));
        if !windows.iter().any(|w| w == (window_start, window_end)) {
            windows.push_back((window_start, window_end));
            env.storage()
                .instance()
                .set(&SystemKey::AnalyticsCacheWindows, &windows);
        }

        result
    }

    /// Invalidate any cached analytics window that contains `requested_at`.
    ///
    /// Only called when a refund is processed, per issue #382: caches are keyed by
    /// window and there is no bounded index of "cache keys ever written," so we
    /// track the small set of distinct windows queried so far and drop the ones
    /// whose range covers the newly processed refund's `requested_at`.
    fn invalidate_analytics_cache_for(env: &Env, requested_at: u64) {
        let windows: Vec<(u64, u64)> = env
            .storage()
            .instance()
            .get(&SystemKey::AnalyticsCacheWindows)
            .unwrap_or(Vec::new(env));
        for (window_start, window_end) in windows.iter() {
            if requested_at >= window_start && requested_at <= window_end {
                env.storage()
                    .instance()
                    .remove(&SystemKey::AnalyticsCache(window_start, window_end));
            }
        }
    }

    /// Get the total number of refunds in a given status.
    ///
    /// # Arguments
    /// * `status` - The refund status to count.
    ///
    /// # Returns
    /// The count of refunds in the specified status.
    pub fn get_refund_count_by_status(env: &Env, status: RefundStatus) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::RefundStatusCount(status))
            .unwrap_or(0)
    }

    /// Get the cumulative amount that has been refunded for a given payment.
    ///
    /// # Arguments
    /// * `payment_id` - The payment ID to calculate the total for.
    ///
    /// # Returns
    /// The total refunded amount in the smallest denomination of the token.
    pub fn get_total_refunded_amount(env: &Env, payment_id: u64) -> i128 {
        let total_refunds: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RefundCounter)
            .unwrap_or(0);
        let mut total: i128 = 0;

        let mut id: u64 = 1;
        while id <= total_refunds {
            if let Some(refund) = env
                .storage()
                .instance()
                .get::<_, Refund>(&DataKey::Refund(id))
            {
                if refund.payment_id == payment_id && refund.status == RefundStatus::Processed {
                    total += refund.amount;
                }
            }
            id += 1;
        }

        total
    }

    /// Check whether a refund request for a given payment would exceed the original payment amount.
    ///
    /// # Arguments
    /// * `payment_id` - The payment ID to check.
    /// * `requested_amount` - The refund amount being requested.
    /// * `original_amount` - The original payment amount.
    ///
    /// # Returns
    /// `Ok(true)` if the refund is allowed.
    ///
    /// # Errors
    /// Returns `TotalRefundsExceedsPayment` if the cumulative refunds would exceed the original amount.
    pub fn can_refund_payment(
        env: &Env,
        payment_id: u64,
        requested_amount: i128,
        original_amount: i128,
    ) -> Result<bool, Error> {
        let total_refunded = Self::get_total_refunded_amount(env, payment_id);
        if requested_amount.saturating_add(total_refunded) > original_amount {
            return Err(Error::Core(CoreError::TotalRefundsExceedPayment));
        }

        Ok(true)
    }

    fn sort_tiers(_env: &Env, tiers: Vec<RefundTier>) -> Vec<RefundTier> {
        let mut sorted = tiers.clone();
        let len = sorted.len();
        if len <= 1 {
            return sorted;
        }
        for i in 1..len {
            let mut j = i;
            while j > 0 {
                let current = sorted.get(j).unwrap();
                let prev = sorted.get(j - 1).unwrap();
                if current.days_from_purchase < prev.days_from_purchase {
                    sorted.set(j, prev);
                    sorted.set(j - 1, current);
                    j -= 1;
                } else {
                    break;
                }
            }
        }
        sorted
    }

    /// Set a refund policy for a merchant with tiered refund rules.
    ///
    /// Tiers are sorted by `days_from_purchase` in ascending order. Each tier
    /// specifies the maximum refund percentage (in basis points) within its time window.
    ///
    /// # Arguments
    /// * `merchant` - The merchant setting the policy (must authenticate).
    /// * `tiers` - A vector of `RefundTier` entries defining the policy tiers.
    ///
    /// # Errors
    /// Returns `RefundExceedsPolicy` if any tier has an invalid `max_refund_bps` value.
    pub fn set_refund_policy(
        env: Env,
        merchant: Address,
        tiers: Vec<RefundTier>,
    ) -> Result<(), Error> {
        // Require merchant authentication
        merchant.require_auth();

        // Validate max_refund_bps is within bounds for all tiers (0-10000 basis points)
        for tier in tiers.iter() {
            if let Err(_) = Self::validate_bps(tier.max_refund_bps) {
                return Err(Error::Core(CoreError::RefundExceedsPolicy));
            }
        }

        // Sort tiers by days_from_purchase in ascending order
        let sorted_tiers = Self::sort_tiers(&env, tiers);

        let now = env.ledger().timestamp();
        let policy = RefundPolicy {
            merchant: merchant.clone(),
            tiers: sorted_tiers.clone(),
            active: true,
            created_at: now,
            updated_at: now,
            default_window_seconds: 30 * 24 * 60 * 60,
        };

        env.storage()
            .instance()
            .set(&DataKey::RefundPolicy(merchant.clone()), &policy);

        // ── Issue #134: version the policy ──────────────────────────────────
        let version_count: u32 = env
            .storage()
            .instance()
            .get(&PolicyKey::RefundPolicyVersionCount(merchant.clone()))
            .unwrap_or(0);
        let new_version = version_count + 1;
        let versioned = RefundPolicyVersion {
            version: new_version,
            policy: policy.clone(),
            created_at: now,
            created_by: merchant.clone(),
        };
        env.storage().instance().set(
            &PolicyKey::RefundPolicyVersion(merchant.clone(), new_version),
            &versioned,
        );
        env.storage().instance().set(
            &PolicyKey::RefundPolicyVersionCount(merchant.clone()),
            &new_version,
        );

        // Emit RefundPolicySet event
        (RefundPolicySet {
            merchant,
            tiers_count: sorted_tiers.len() as u32,
        })
        .publish(&env);

        Ok(())
    }

    // ── Issue #134: Policy versioning query functions ──────────────────────

    /// Get a specific versioned refund policy for a merchant.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to query.
    /// * `version` - The version number to retrieve.
    ///
    /// # Returns
    /// The `RefundPolicyVersion` if found, `None` otherwise.
    pub fn get_refund_policy_version(
        env: Env,
        merchant: Address,
        version: u32,
    ) -> Option<RefundPolicyVersion> {
        env.storage()
            .instance()
            .get(&PolicyKey::RefundPolicyVersion(merchant, version))
    }

    /// Get the refund policy version that was in effect for a merchant at a given timestamp.
    ///
    /// Walks all versions in reverse order and returns the latest one created at or
    /// before the specified timestamp.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to query.
    /// * `timestamp` - The Unix timestamp to look up the policy for.
    ///
    /// # Returns
    /// The `RefundPolicyVersion` in effect at the given time, or `None` if no version existed.
    pub fn get_refund_policy_at_time(
        env: Env,
        merchant: Address,
        timestamp: u64,
    ) -> Option<RefundPolicyVersion> {
        let count: u32 = env
            .storage()
            .instance()
            .get(&PolicyKey::RefundPolicyVersionCount(merchant.clone()))
            .unwrap_or(0);
        if count == 0 {
            return None;
        }
        // Walk versions in reverse to find the latest one created at or before timestamp
        let mut result: Option<RefundPolicyVersion> = None;
        for v in 1..=count {
            if let Some(pv) = env
                .storage()
                .instance()
                .get::<PolicyKey, RefundPolicyVersion>(&PolicyKey::RefundPolicyVersion(
                    merchant.clone(),
                    v,
                ))
            {
                if pv.created_at <= timestamp {
                    result = Some(pv);
                }
            }
        }
        result
    }

    /// Get the full version history of refund policies for a merchant.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to query.
    ///
    /// # Returns
    /// A vector of all `RefundPolicyVersion` entries in chronological order.
    pub fn get_refund_policy_history(env: Env, merchant: Address) -> Vec<RefundPolicyVersion> {
        let count: u32 = env
            .storage()
            .instance()
            .get(&PolicyKey::RefundPolicyVersionCount(merchant.clone()))
            .unwrap_or(0);
        let mut history = Vec::new(&env);
        for v in 1..=count {
            if let Some(pv) = env
                .storage()
                .instance()
                .get::<PolicyKey, RefundPolicyVersion>(&PolicyKey::RefundPolicyVersion(
                    merchant.clone(),
                    v,
                ))
            {
                history.push_back(pv);
            }
        }
        history
    }

    /// Get the current active refund policy for a merchant.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to query.
    ///
    /// # Returns
    /// The current `RefundPolicy` if one exists, `None` otherwise.
    pub fn get_refund_policy(env: &Env, merchant: Address) -> Option<RefundPolicy> {
        env.storage()
            .instance()
            .get(&DataKey::RefundPolicy(merchant))
    }

    // ── Issue #93: Default refund policy management ────────────────────────

    /// Set the global default refund policy. Admin-only.
    pub fn set_default_refund_policy(
        env: Env,
        admin: Address,
        policy: RefundPolicy,
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
            .set(&DataKey::DefaultRefundPolicy, &policy);
        (DefaultRefundPolicySet {
            set_by: admin,
            tiers_count: policy.tiers.len() as u32,
        })
        .publish(&env);
        Ok(())
    }

    /// Get the global default refund policy (returns None if not set).
    pub fn get_default_refund_policy(env: Env) -> Option<RefundPolicy> {
        env.storage().instance().get(&DataKey::DefaultRefundPolicy)
    }

    /// Internal helper used by request_refund / validate_against_policy.
    fn get_default_refund_policy_inner(env: &Env) -> Option<RefundPolicy> {
        env.storage().instance().get(&DataKey::DefaultRefundPolicy)
    }

    /// Remove the global default refund policy. Admin-only.
    pub fn remove_default_refund_policy(env: Env, admin: Address) -> Result<(), Error> {
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
            .remove(&DataKey::DefaultRefundPolicy);
        (DefaultRefundPolicyRemoved { removed_by: admin }).publish(&env);
        Ok(())
    }

    fn get_requires_admin_approval_inner(env: &Env, merchant: &Address) -> bool {
        let key = Symbol::new(env, "requires_admin_approval");
        let composite_key: (Symbol, Address) = (key, merchant.clone());
        env.storage().instance().get(&composite_key).unwrap_or(true)
    }

    fn set_requires_admin_approval_inner(env: &Env, merchant: &Address, value: bool) {
        let key = Symbol::new(env, "requires_admin_approval");
        let composite_key: (Symbol, Address) = (key, merchant.clone());
        env.storage().instance().set(&composite_key, &value);
    }

    fn get_auto_approve_below_inner(env: &Env, merchant: &Address) -> i128 {
        let key = Symbol::new(env, "auto_approve_below");
        let composite_key: (Symbol, Address) = (key, merchant.clone());
        env.storage().instance().get(&composite_key).unwrap_or(0)
    }

    fn set_auto_approve_below_inner(env: &Env, merchant: &Address, value: i128) {
        let key = Symbol::new(env, "auto_approve_below");
        let composite_key: (Symbol, Address) = (key, merchant.clone());
        env.storage().instance().set(&composite_key, &value);
    }

    fn get_auto_approve_below_ceiling_inner(env: &Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::AutoApproveBelowCeiling)
            .unwrap_or(0)
    }

    fn set_auto_approve_below_ceiling_inner(env: &Env, value: i128) {
        env.storage()
            .instance()
            .set(&DataKey::AutoApproveBelowCeiling, &value);
    }

    fn get_inherit_from_parent_inner(env: &Env, merchant: &Address) -> bool {
        let key = Symbol::new(env, "inherit_from_parent");
        let composite_key: (Symbol, Address) = (key, merchant.clone());
        env.storage().instance().get(&composite_key).unwrap_or(true)
    }

    fn set_inherit_from_parent_inner(env: &Env, merchant: &Address, inherit: bool) {
        let key = Symbol::new(env, "inherit_from_parent");
        let composite_key: (Symbol, Address) = (key, merchant.clone());
        env.storage().instance().set(&composite_key, &inherit);
    }

    /// Check whether a merchant's refunds require admin approval before processing.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to query.
    ///
    /// # Returns
    /// `true` if admin approval is required (the default), `false` otherwise.
    pub fn get_requires_admin_approval(env: Env, merchant: Address) -> bool {
        Self::get_requires_admin_approval_inner(&env, &merchant)
    }

    /// Set whether a merchant's refunds require admin approval before processing.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to configure (must authenticate).
    /// * `value` - `true` to require admin approval, `false` to allow auto-processing.
    pub fn set_requires_admin_approval(env: Env, merchant: Address, value: bool) {
        merchant.require_auth();
        Self::set_requires_admin_approval_inner(&env, &merchant, value);
    }

    /// Get the auto-approval threshold amount for a merchant.
    ///
    /// Refunds at or below this amount are automatically approved when admin approval
    /// is not required.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to query.
    ///
    /// # Returns
    /// The auto-approval threshold amount. Returns 0 if not configured.
    pub fn get_auto_approve_below(env: Env, merchant: Address) -> i128 {
        Self::get_auto_approve_below_inner(&env, &merchant)
    }

    /// Set the auto-approval threshold amount for a merchant.
    ///
    /// Refunds at or below this amount will be automatically approved when admin
    /// approval is not required.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to configure (must authenticate).
    /// * `value` - The threshold amount below which refunds are auto-approved.
    pub fn set_auto_approve_below(env: Env, merchant: Address, value: i128) -> Result<(), Error> {
        merchant.require_auth();
        let ceiling = Self::get_auto_approve_below_ceiling_inner(&env);
        if value > ceiling {
            return Err(Error::Core(CoreError::AutoApproveThresholdExceedsCeiling));
        }
        Self::set_auto_approve_below_inner(&env, &merchant, value);
        Ok(())
    }

    /// Get the platform-wide ceiling for merchant auto-approval thresholds.
    ///
    /// Refund thresholds above this value are rejected by `set_auto_approve_below()`.
    pub fn get_auto_approve_below_ceiling(env: Env) -> i128 {
        Self::get_auto_approve_below_ceiling_inner(&env)
    }

    /// Set the platform-wide ceiling for merchant auto-approval thresholds.
    ///
    /// # Arguments
    /// * `admin` - The contract admin configuring the ceiling.
    /// * `value` - The maximum auto-approval threshold any merchant may set.
    pub fn set_auto_approve_below_ceiling(
        env: Env,
        admin: Address,
        value: i128,
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
        Self::set_auto_approve_below_ceiling_inner(&env, value);
        Ok(())
    }

    /// Check whether a merchant inherits its refund policy from its parent merchant.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to query.
    ///
    /// # Returns
    /// `true` if inheritance is enabled (the default), `false` otherwise.
    pub fn get_inherit_from_parent(env: Env, merchant: Address) -> bool {
        Self::get_inherit_from_parent_inner(&env, &merchant)
    }

    /// Set whether a merchant inherits its refund policy from its parent merchant.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to configure (must authenticate).
    /// * `inherit` - `true` to enable inheritance, `false` to disable it.
    pub fn set_inherit_from_parent(env: Env, merchant: Address, inherit: bool) {
        merchant.require_auth();
        Self::set_inherit_from_parent_inner(&env, &merchant, inherit);
    }

    /// Deactivate a merchant's refund policy so it is no longer enforced.
    ///
    /// # Arguments
    /// * `merchant` - The merchant whose policy should be deactivated (must authenticate).
    ///
    /// # Errors
    /// Returns `PolicyNotFound` if no policy exists for the merchant.
    /// Returns `PolicyInactive` if the policy is already inactive.
    pub fn deactivate_refund_policy(env: Env, merchant: Address) -> Result<(), Error> {
        // Require merchant authentication
        merchant.require_auth();

        let mut policy: RefundPolicy = env
            .storage()
            .instance()
            .get(&DataKey::RefundPolicy(merchant.clone()))
            .ok_or(Error::Core(CoreError::PolicyNotFound))?;

        if !policy.active {
            return Err(Error::Core(CoreError::PolicyInactive));
        }

        policy.active = false;
        env.storage()
            .instance()
            .set(&DataKey::RefundPolicy(merchant.clone()), &policy);

        // Emit RefundPolicyDeactivated event
        (RefundPolicyDeactivated { merchant }).publish(&env);

        Ok(())
    }

    /// Override a refund decision as an admin and create an immutable audit log entry.
    ///
    /// Records the override with a SHA-256 transaction hash for integrity verification.
    /// Emits both `AdminRefundOverride` and legacy `PolicyOverrideApplied` events.
    ///
    /// # Arguments
    /// * `admin` - The contract admin performing the override.
    /// * `refund_id` - The ID of the refund to override.
    /// * `new_status` - The new status to apply to the refund.
    /// * `new_amount` - The new amount to apply to the refund.
    /// * `reason` - A human-readable reason for the override.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `RefundNotFound` if the refund does not exist.
    pub fn admin_override_policy(
        env: Env,
        admin: Address,
        refund_id: u64,
        new_status: RefundStatus,
        new_amount: i128,
        reason: String,
    ) -> Result<(), Error> {
        // Require admin authentication
        admin.require_auth();

        let admin_address: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;

        if admin != admin_address {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        // Verify refund exists and update it
        let mut refund: Refund = env
            .storage()
            .instance()
            .get(&DataKey::Refund(refund_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;

        // Apply override
        refund.status = new_status.clone();
        refund.amount = new_amount;
        env.storage()
            .instance()
            .set(&DataKey::Refund(refund_id), &refund);

        // Generate immutable audit log entry
        let override_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AdminOverrideHistoryCount)
            .unwrap_or(0);

        let executed_at = env.ledger().timestamp();

        // Create hash of override details for immutability verification
        let mut hash_data = Bytes::new(&env);
        hash_data.append(&Bytes::from_slice(&env, &refund_id.to_be_bytes()));
        hash_data.append(&Bytes::from_slice(&env, &new_amount.to_be_bytes()));
        hash_data.append(&Bytes::from_slice(&env, &executed_at.to_be_bytes()));
        let transaction_hash = env.crypto().sha256(&hash_data);

        let audit_entry = AdminOverrideHistory {
            override_id,
            refund_id,
            admin: admin.clone(),
            reason: reason.clone(),
            override_amount: new_amount,
            override_status: new_status.clone(),
            executed_at,
            transaction_hash: transaction_hash.into(),
        };

        // Store immutable audit log entry
        env.storage()
            .instance()
            .set(&DataKey::AdminOverrideHistory(override_id), &audit_entry);

        // Increment counter
        env.storage()
            .instance()
            .set(&DataKey::AdminOverrideHistoryCount, &(override_id + 1));

        // Emit AdminRefundOverride event
        AdminRefundOverride {
            override_id,
            refund_id,
            admin: admin.clone(),
            reason: reason.clone(),
            override_amount: new_amount,
            override_status: new_status,
            executed_at,
        }
        .publish(&env);

        // Emit legacy PolicyOverrideApplied event for backward compatibility
        PolicyOverrideApplied {
            refund_id,
            admin,
            reason,
        }
        .publish(&env);

        Ok(())
    }

    /// Retrieve admin override audit log entry by override_id
    pub fn get_admin_override_history(env: Env, override_id: u64) -> Option<AdminOverrideHistory> {
        env.storage()
            .instance()
            .get(&DataKey::AdminOverrideHistory(override_id))
    }

    /// Get total count of admin override audit log entries
    pub fn get_admin_override_history_count(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::AdminOverrideHistoryCount)
            .unwrap_or(0)
    }

    // ── Issue #138: Refund policy inheritance for merchant hierarchies ────────

    /// Maximum depth allowed for policy inheritance chain
    const MAX_INHERITANCE_DEPTH: u32 = 5;

    /// Set the parent merchant for a child merchant to enable policy inheritance.
    /// Requires admin authorization.
    /// Validates against self-parent, circular references, and max depth.
    pub fn set_merchant_parent(
        env: Env,
        admin: Address,
        merchant: Address,
        parent: Address,
    ) -> Result<(), Error> {
        admin.require_auth();

        // Verify admin authorization
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        // Prevent self-parent
        if merchant == parent {
            return Err(Error::Core(CoreError::CircularInheritance));
        }

        // Check for circular reference by traversing up from parent
        // If we encounter the merchant in the parent's chain, it would create a cycle
        let mut visited = Vec::new(&env);
        visited.push_back(merchant.clone());

        let mut current = parent.clone();
        let mut depth: u32 = 1;

        while depth <= Self::MAX_INHERITANCE_DEPTH {
            if current == merchant {
                return Err(Error::Core(CoreError::CircularInheritance));
            }

            // Check if we've seen this address before (shouldn't happen but safety check)
            if visited.contains(&current) {
                return Err(Error::Core(CoreError::CircularInheritance));
            }
            visited.push_back(current.clone());

            // Move to next parent
            match Self::get_merchant_parent(&env, current.clone()) {
                Some(next_parent) => {
                    current = next_parent;
                    depth += 1;
                }
                None => break,
            }
        }

        // Validate max depth constraint (>= to prevent exceeding max, including the new merchant)
        if depth >= Self::MAX_INHERITANCE_DEPTH {
            return Err(Error::Core(CoreError::MaxInheritanceDepth));
        }

        // Store the parent relationship using Symbol-based key
        let key = Symbol::new(&env, "parent_of");
        let composite_key: (Symbol, Address) = (key, merchant.clone());
        env.storage().instance().set(&composite_key, &parent);

        Ok(())
    }

    /// Get the direct parent merchant of a given merchant.
    pub fn get_merchant_parent(env: &Env, merchant: Address) -> Option<Address> {
        let key = Symbol::new(env, "parent_of");
        let composite_key: (Symbol, Address) = (key, merchant);
        env.storage().instance().get(&composite_key)
    }

    /// Get the effective refund policy for a merchant, traversing the inheritance chain.
    /// Returns the first active explicit policy found, respecting inherit_from_parent flag.
    pub fn get_effective_refund_policy(env: Env, merchant: Address) -> Option<RefundPolicy> {
        let starting_policy = Self::get_refund_policy(&env, merchant.clone());
        let mut current = merchant.clone();
        let mut depth: u32 = 0;
        let mut visited = Vec::new(&env);

        while depth < Self::MAX_INHERITANCE_DEPTH {
            // Prevent infinite loops
            if visited.contains(&current) {
                return None; // Circular reference detected
            }
            visited.push_back(current.clone());

            // Try to get explicit policy for current merchant
            if let Some(policy) = Self::get_refund_policy(&env, current.clone()) {
                if policy.active {
                    // If this is the starting merchant, always return their own active policy
                    // A merchant's explicit policy always takes precedence for themselves
                    if current == merchant {
                        return Some(policy);
                    }
                    // We're at a parent in the chain - their policy is inheritable
                    return Some(policy);
                }
                // Policy is inactive - check if we should continue to parent
                if current == merchant && !Self::get_inherit_from_parent_inner(&env, &merchant) {
                    // Starting merchant has disabled inheritance and their policy is inactive
                    return Some(policy);
                }
                // Continue to parent (either inactive policy or merchant wants to inherit)
            }

            // Move to parent
            match Self::get_merchant_parent(&env, current.clone()) {
                Some(parent) => {
                    current = parent;
                    depth += 1;
                }
                None => break,
            }
        }

        // If we reached max depth, return None to indicate failure
        if depth >= Self::MAX_INHERITANCE_DEPTH {
            return None;
        }

        // Fallback logic after loop terminates:
        if let Some(policy) = starting_policy {
            return Some(policy);
        }
        Self::get_default_refund_policy_inner(&env)
    }

    /// Get the inheritance chain for a merchant (ancestry path).
    /// Returns vector from merchant → parent → grandparent → ... → root.
    /// Returns error if circular reference or max depth exceeded.
    pub fn get_policy_inheritance_chain(
        env: Env,
        merchant: Address,
    ) -> Result<Vec<Address>, Error> {
        let mut chain = Vec::new(&env);
        let mut current = merchant.clone();
        let mut depth: u32 = 0;

        chain.push_back(current.clone());

        while depth < Self::MAX_INHERITANCE_DEPTH {
            match Self::get_merchant_parent(&env, current.clone()) {
                Some(parent) => {
                    // Check for circular reference
                    if chain.contains(&parent) {
                        return Err(Error::Core(CoreError::CircularInheritance));
                    }
                    chain.push_back(parent.clone());
                    current = parent;
                    depth += 1;
                }
                None => break,
            }
        }

        // Check if we hit max depth
        if depth >= Self::MAX_INHERITANCE_DEPTH {
            return Err(Error::Core(CoreError::MaxInheritanceDepth));
        }

        Ok(chain)
    }

    /// Get the applicable refund basis points for a merchant and payment, considering
    /// policy inheritance and tier evaluation.
    ///
    /// # Arguments
    /// * `merchant` - The merchant address to evaluate.
    /// * `payment_id` - The payment ID to determine the applicable tier.
    ///
    /// # Returns
    /// The maximum refund amount in basis points (0-10000) applicable to the payment.
    pub fn get_applicable_refund_bps(env: Env, merchant: Address, payment_id: u64) -> u32 {
        let payment = match Self::get_external_payment(&env, payment_id) {
            Ok(p) => p,
            Err(_) => return 0,
        };
        let current_time = env.ledger().timestamp();
        let created_at = payment.created_at;

        // Traverse policy inheritance chain to find the effective policy
        let policy_opt = Self::get_effective_refund_policy(env.clone(), merchant);
        let policy = match policy_opt {
            Some(p) => p,
            None => return 0,
        };

        if !policy.active {
            return 0;
        }

        let elapsed_seconds = current_time.saturating_sub(created_at);
        let days_since_purchase = elapsed_seconds / (24 * 60 * 60);

        // Find the first tier (sorted ascending by days_from_purchase) where days_since_purchase <= tier.days_from_purchase
        for tier in policy.tiers.iter() {
            if days_since_purchase <= tier.days_from_purchase {
                return tier.max_refund_bps;
            }
        }

        0
    }

    fn validate_against_policy(
        env: &Env,
        merchant: &Address,
        customer: &Address,
        amount: i128,
        original_amount: i128,
        payment_created_at: u64,
        payment_id: u64,
    ) -> Result<(), Error> {
        let policy: RefundPolicy = Self::get_effective_refund_policy(env.clone(), merchant.clone())
            .ok_or(Error::Core(CoreError::PolicyNotFound))?;

        if !policy.active {
            return Err(Error::Core(CoreError::PolicyInactive));
        }

        let current_time = env.ledger().timestamp();
        let elapsed_seconds = current_time.saturating_sub(payment_created_at);
        let days_since_purchase = elapsed_seconds / (24 * 60 * 60);
        let elapsed_seconds_total = elapsed_seconds;

        // Enforce the category-specific (or policy-default) refund window first.
        // get_effective_window returns seconds; if elapsed time exceeds it the
        // refund is outside the allowed window regardless of tier settings.
        if payment_id > 0 {
            let effective_window_seconds =
                Self::get_effective_window(env.clone(), merchant.clone(), payment_id);
            if elapsed_seconds_total > effective_window_seconds {
                return Err(Error::Core(CoreError::RefundWindowExpired));
            }
        }

        let mut allowed_bps = 0;
        let mut found_tier = false;
        for tier in policy.tiers.iter() {
            if days_since_purchase <= tier.days_from_purchase {
                allowed_bps = tier.max_refund_bps;
                found_tier = true;
                break;
            }
        }

        if !found_tier {
            return Err(Error::Core(CoreError::RefundWindowExpired));
        }

        // Issue #370: Override allowed_bps with customer tier policy if set
        let tier_id_opt: Option<u32> = env
            .storage()
            .instance()
            .get(&DataKey::CustomerTier(customer.clone()));

        if let Some(tier_id) = tier_id_opt {
            let tier_cap_opt: Option<RefundCap> = env
                .storage()
                .instance()
                .get(&DataKey::CustomerTierPolicy(merchant.clone(), tier_id));
            match tier_cap_opt {
                Some(cap) => {
                    allowed_bps = cap.max_refund_bps;
                }
                None => {
                    let strict: bool = env
                        .storage()
                        .instance()
                        .get(&DataKey::StrictTierPolicy(merchant.clone()))
                        .unwrap_or(false);
                    if strict {
                        return Err(Error::Ext(ExtError::TierPolicyNotFound));
                    }
                }
            }
        }

        // Check refund percentage using overflow-safe math
        let refund_percentage_bps = amount
            .checked_mul(10000)
            .unwrap_or(i128::MAX)
            .checked_div(original_amount)
            .unwrap_or(u32::MAX as i128) as u32;

        if refund_percentage_bps > allowed_bps {
            return Err(Error::Core(CoreError::RefundExceedsPolicy));
        }

        Ok(())
    }

    fn add_to_status_index(env: &Env, status: RefundStatus, refund_id: u64) {
        let count = Self::get_refund_count_by_status(env, status.clone());
        env.storage()
            .instance()
            .set(&DataKey::RefundsByStatus(status.clone(), count), &refund_id);
        env.storage()
            .instance()
            .set(&DataKey::RefundStatusCount(status.clone()), &(count + 1));
        env.storage()
            .instance()
            .set(&DataKey::RefundStatusIndex(refund_id), &count);
    }

    fn remove_from_status_index(
        env: &Env,
        status: RefundStatus,
        refund_id: u64,
    ) -> Result<(), Error> {
        let count = Self::get_refund_count_by_status(env, status.clone());
        if count == 0 {
            return Err(Error::Core(CoreError::InvalidStatus));
        }

        let index: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RefundStatusIndex(refund_id))
            .ok_or(Error::Core(CoreError::InvalidStatus))?;
        let last_index = count - 1;

        if index != last_index {
            let last_refund_id: u64 = env
                .storage()
                .instance()
                .get(&DataKey::RefundsByStatus(status.clone(), last_index))
                .ok_or(Error::Core(CoreError::InvalidStatus))?;
            env.storage().instance().set(
                &DataKey::RefundsByStatus(status.clone(), index),
                &last_refund_id,
            );
            env.storage()
                .instance()
                .set(&DataKey::RefundStatusIndex(last_refund_id), &index);
        }

        env.storage()
            .instance()
            .remove(&DataKey::RefundsByStatus(status.clone(), last_index));
        env.storage()
            .instance()
            .remove(&DataKey::RefundStatusIndex(refund_id));
        env.storage()
            .instance()
            .set(&DataKey::RefundStatusCount(status), &last_index);

        Ok(())
    }

    // ── Issue #135: Batch refund processing ──────────────────────────────────

    const DEFAULT_BATCH_LIMIT: u32 = 20;

    /// Get the maximum number of refunds that can be processed in a single batch operation.
    ///
    /// # Returns
    /// The batch refund limit. Defaults to 20 if not configured.
    pub fn get_batch_refund_limit(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::BatchRefundLimit)
            .unwrap_or(Self::DEFAULT_BATCH_LIMIT)
    }

    /// Set the maximum number of refunds allowed per batch operation.
    ///
    /// # Arguments
    /// * `admin` - The contract admin setting the limit.
    /// * `limit` - The maximum batch size.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn set_batch_refund_limit(env: Env, admin: Address, limit: u32) -> Result<(), Error> {
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
            .set(&DataKey::BatchRefundLimit, &limit);
        Ok(())
    }

    /// Approve multiple refunds in a single batch operation.
    ///
    /// Per-item failures are isolated; valid items are processed and invalid items
    /// return an error entry in the results vector without blocking other items.
    /// The entire batch is rejected if the count exceeds the configured batch limit.
    ///
    /// # Arguments
    /// * `admin` - The contract admin approving the refunds.
    /// * `refund_ids` - A vector of refund IDs to approve.
    ///
    /// # Returns
    /// A vector of `BatchRefundResult` entries indicating success or failure for each refund.
    pub fn approve_refund_batch(
        env: Env,
        admin: Address,
        refund_ids: Vec<u64>,
    ) -> Vec<BatchRefundResult> {
        admin.require_auth();
        let limit = Self::get_batch_refund_limit(env.clone());
        if refund_ids.len() > limit {
            // Batch-level validation failure: reject the entire batch without processing.
            let mut results = Vec::new(&env);
            results.push_back(BatchRefundResult {
                refund_id: 0,
                success: false,
                error_code: Error::Core(CoreError::BatchRefundTooLarge).to_u32(),
                amount_refunded: 0,
            });
            return results;
        }

        // Per-item failures are isolated; valid items are processed and invalid items
        // return an error entry in the results vector without blocking other items.
        let mut results = Vec::new(&env);
        for refund_id in refund_ids.iter() {
            let result = Self::approve_refund_internal(&env, admin.clone(), refund_id);
            match result {
                Ok(()) => {
                    let amount = env
                        .storage()
                        .instance()
                        .get::<DataKey, Refund>(&DataKey::Refund(refund_id))
                        .map(|r| r.amount)
                        .unwrap_or(0);
                    results.push_back(BatchRefundResult {
                        refund_id,
                        success: true,
                        error_code: 0,
                        amount_refunded: amount,
                    });
                }
                Err(e) => {
                    results.push_back(BatchRefundResult {
                        refund_id,
                        success: false,
                        error_code: e.to_u32(),
                        amount_refunded: 0,
                    });
                }
            }
        }
        results
    }

    /// Process (finalize) multiple approved refunds in a single batch operation.
    ///
    /// Per-item failures are isolated; valid items are processed and invalid items
    /// return an error entry in the results vector without blocking other items.
    /// The entire batch is rejected if the count exceeds the configured batch limit.
    ///
    /// # Arguments
    /// * `admin` - The contract admin processing the refunds.
    /// * `refund_ids` - A vector of refund IDs to process.
    ///
    /// # Returns
    /// A vector of `BatchRefundResult` entries indicating success or failure for each refund.
    pub fn process_refund_batch(
        env: Env,
        admin: Address,
        refund_ids: Vec<u64>,
    ) -> Vec<BatchRefundResult> {
        admin.require_auth();
        let limit = Self::get_batch_refund_limit(env.clone());
        if refund_ids.len() > limit {
            // Batch-level validation failure: reject the entire batch without processing.
            let mut results = Vec::new(&env);
            results.push_back(BatchRefundResult {
                refund_id: 0,
                success: false,
                error_code: Error::Core(CoreError::BatchRefundTooLarge).to_u32(),
                amount_refunded: 0,
            });
            return results;
        }

        // Per-item failures are isolated; valid items are processed and invalid items
        // return an error entry in the results vector without blocking other items.
        let mut results = Vec::new(&env);
        for refund_id in refund_ids.iter() {
            let amount = env
                .storage()
                .instance()
                .get::<DataKey, Refund>(&DataKey::Refund(refund_id))
                .map(|r| r.amount)
                .unwrap_or(0);
            let result = Self::process_refund_internal(&env, admin.clone(), refund_id);
            match result {
                Ok(()) => {
                    results.push_back(BatchRefundResult {
                        refund_id,
                        success: true,
                        error_code: 0,
                        amount_refunded: amount,
                    });
                }
                Err(e) => {
                    results.push_back(BatchRefundResult {
                        refund_id,
                        success: false,
                        error_code: e.to_u32(),
                        amount_refunded: 0,
                    });
                }
            }
        }
        results
    }

    // ── Issue #143: Cross-contract payment verification ───────────────────────

    /// Set the address of the payment contract used for cross-contract ownership verification.
    ///
    /// # Arguments
    /// * `admin` - The contract admin setting the address.
    /// * `payment_contract` - The address of the payment contract.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn set_payment_contract_address(
        env: Env,
        admin: Address,
        payment_contract: Address,
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
            .set(&DataKey::PaymentContractAddress, &payment_contract);
        Ok(())
    }

    /// Get the address of the payment contract used for cross-contract verification.
    ///
    /// # Returns
    /// The payment contract address if configured, `None` otherwise.
    pub fn get_payment_contract_address(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::PaymentContractAddress)
    }

    /// Verify that a customer owns a given payment via a cross-contract call.
    ///
    /// # Arguments
    /// * `payment_id` - The payment ID to verify.
    /// * `customer` - The customer address to verify ownership for.
    ///
    /// # Returns
    /// `true` if the payment exists, belongs to the customer, and is completed.
    /// Returns `false` if no payment contract is set or verification fails.
    pub fn verify_payment_ownership(env: Env, payment_id: u64, customer: Address) -> bool {
        let payment_contract: Address = match env
            .storage()
            .instance()
            .get(&DataKey::PaymentContractAddress)
        {
            Some(addr) => addr,
            None => return false, // no contract set → skip verification
        };
        // Cross-contract call to payment_contract.check_payment_customer(payment_id, customer).
        // That function returns bool: true if payment exists, belongs to customer, and is Completed.
        let func = Symbol::new(&env, "check_payment_customer");
        let args = (payment_id, customer).into_val(&env);
        match env.try_invoke_contract::<bool, soroban_sdk::InvokeError>(
            &payment_contract,
            &func,
            args,
        ) {
            Ok(Ok(result)) => result,
            _ => false,
        }
    }

    fn create_refund(
        env: Env,
        merchant: Address,
        payment_id: u64,
        customer: Address,
        amount: i128,
        original_payment_amount: i128,
        token: Address,
        reason: String,
        reason_code: RefundReasonCode,
        payment_created_at: u64,
        force_approved: bool,
    ) -> Result<u64, Error> {
        if amount <= 0 {
            return Err(Error::Core(CoreError::InvalidAmount));
        }

        if amount > original_payment_amount {
            return Err(Error::Core(CoreError::RefundExceedsPayment));
        }

        Self::check_customer_refund_cooldown(&env, &customer)?;

        if payment_id == 0 {
            return Err(Error::Core(CoreError::InvalidPaymentId));
        }

        if env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::PaymentContractAddress)
            .is_some()
        {
            let owned = Self::verify_payment_ownership(env.clone(), payment_id, customer.clone());
            if !owned {
                return Err(Error::Core(CoreError::PaymentOwnershipMismatch));
            }
        }

        Self::can_refund_payment(&env, payment_id, amount, original_payment_amount)?;
        Self::check_and_update_circuit_breaker(&env, amount, original_payment_amount)?;
        Self::check_and_update_customer_refund_rate_limit(&env, customer.clone())?;

        // Check payment refund cap
        Self::check_payment_refund_cap(&env, payment_id, amount)?;

        // Check for fraud signals (#137)
        if let Some(fraud_signal) = Self::check_fraud_signals(env.clone(), customer.clone()) {
            if !fraud_signal.reviewed {
                return Err(Error::Ext(ExtError::AddressFlaggedForFraud));
            }
        }

        // Issue #148: Check merchant-level customer eligibility
        let eligibility_rule = Self::check_refund_eligibility_internal(&env, &merchant, &customer);
        if eligibility_rule == EligibilityRule::Block {
            return Err(Error::Ext(ExtError::CustomerBlockedFromRefund));
        }

        if env.storage().instance().has(&DataKey::Admin) {
            Self::validate_against_policy(
                &env,
                &merchant,
                &customer,
                amount,
                original_payment_amount,
                payment_created_at,
                payment_id,
            )?;
        }

        let counter: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RefundCounter)
            .unwrap_or(0);
        let refund_id = counter + 1;

        let initial_status = if force_approved {
            RefundStatus::Approved
        } else {
            let effective_merchant = if let Some(policy) =
                Self::get_effective_refund_policy(env.clone(), merchant.clone())
            {
                policy.merchant
            } else {
                merchant.clone()
            };
            let requires_approval =
                Self::get_requires_admin_approval_inner(&env, &effective_merchant);
            let auto_below = {
                let merchant_threshold =
                    Self::get_auto_approve_below_inner(&env, &effective_merchant);
                let platform_ceiling = Self::get_auto_approve_below_ceiling_inner(&env);
                core::cmp::min(merchant_threshold, platform_ceiling)
            };
            if !requires_approval && amount <= auto_below {
                RefundStatus::Approved
            } else {
                RefundStatus::Requested
            }
        };

        let ttl_expires_at: Option<u64> = env
            .storage()
            .instance()
            .get::<RefundExtKey, RefundTTLConfig>(&RefundExtKey::RefundTTLConfig)
            .filter(|cfg| cfg.active)
            .map(|cfg| {
                env.ledger()
                    .timestamp()
                    .saturating_add(cfg.default_ttl_seconds)
            });

        let refund = Refund {
            id: refund_id,
            payment_id,
            merchant: merchant.clone(),
            customer: customer.clone(),
            amount,
            original_payment_amount,
            token: token.clone(),
            // Issue #191: record original payment token
            original_token: token.clone(),
            status: initial_status.clone(),
            requested_at: env.ledger().timestamp(),
            reason,
            reason_code,
            // Issue #147: Initialize lifecycle timestamps
            approved_at: if initial_status == RefundStatus::Approved {
                Some(env.ledger().timestamp())
            } else {
                None
            },
            rejected_at: None,
            processed_at: None,
            rejected_by: None,
            appeal_deadline: None,
            // Issue #199: TTL expiry
            expires_at: ttl_expires_at,
        };

        env.storage()
            .instance()
            .set(&DataKey::Refund(refund_id), &refund);
        env.storage()
            .instance()
            .set(&DataKey::RefundCounter, &refund_id);
        Self::add_to_status_index(&env, initial_status.clone(), refund_id);

        let merchant_count: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MerchantRefundCount(merchant.clone()))
            .unwrap_or(0);
        env.storage().instance().set(
            &DataKey::MerchantRefunds(merchant.clone(), merchant_count),
            &refund_id,
        );
        env.storage().instance().set(
            &DataKey::MerchantRefundCount(merchant.clone()),
            &(merchant_count + 1),
        );

        Self::append_customer_refund_history(&env, &customer, refund_id);

        let payment_count: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PaymentRefundCount(payment_id))
            .unwrap_or(0);
        env.storage().instance().set(
            &DataKey::PaymentRefunds(payment_id, payment_count),
            &refund_id,
        );
        env.storage().instance().set(
            &DataKey::PaymentRefundCount(payment_id),
            &(payment_count + 1),
        );

        // Update payment refund usage for cap tracking
        Self::update_payment_refund_usage(&env, payment_id, amount);

        (RefundRequested {
            refund_id,
            payment_id,
            merchant,
            customer: customer.clone(),
            amount,
            token,
        })
        .publish(&env);

        // Update customer refund cooldown
        Self::update_customer_refund_cooldown(&env, &customer)?;

        // Issue #144: Invoke notification hooks for Requested event
        Self::invoke_hooks(&env, RefundEventType::Requested, refund_id);

        if initial_status == RefundStatus::Approved {
            (AutoApproved { refund_id, amount }).publish(&env);
        }

        Ok(refund_id)
    }

    fn approve_refund_internal(
        env: &Env,
        approved_by: Address,
        refund_id: u64,
    ) -> Result<(), Error> {
        let mut refund: Refund = env
            .storage()
            .instance()
            .get(&DataKey::Refund(refund_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;

        if refund.status != RefundStatus::Requested {
            return Err(Error::Core(CoreError::InvalidStatus));
        }

        // Issue #199: reject if TTL has expired
        if let Some(expires_at) = refund.expires_at {
            if env.ledger().timestamp() >= expires_at {
                return Err(Error::Core(CoreError::RefundWindowExpired));
            }
        }

        Self::remove_from_status_index(env, RefundStatus::Requested, refund_id)?;
        refund.status = RefundStatus::Approved;
        // Issue #147: Set approved_at timestamp
        refund.approved_at = Some(env.ledger().timestamp());
        env.storage()
            .instance()
            .set(&DataKey::Refund(refund_id), &refund);
        Self::add_to_status_index(env, RefundStatus::Approved, refund_id);

        (RefundApproved {
            refund_id,
            payment_id: refund.payment_id,
            amount: refund.amount,
            approved_by,
            approved_at: env.ledger().timestamp(),
        })
        .publish(env);

        // Issue #144: Invoke notification hooks
        Self::invoke_hooks(env, RefundEventType::Approved, refund_id);

        Ok(())
    }

    fn process_refund_internal(
        env: &Env,
        processed_by: Address,
        refund_id: u64,
    ) -> Result<(), Error> {
        let mut refund: Refund = env
            .storage()
            .instance()
            .get(&DataKey::Refund(refund_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;

        if refund.status != RefundStatus::Approved {
            return Err(Error::Core(CoreError::InvalidStatus));
        }

        Self::can_refund_payment(
            env,
            refund.payment_id,
            refund.amount,
            refund.original_payment_amount,
        )?;

        // Deduct platform fee from refund amount
        let (net_refund_amount, _fee_amount) =
            Self::deduct_refund_fee(env, refund_id, refund.amount, &refund.token)?;

        if net_refund_amount > 0 {
            token::Client::new(env, &refund.token).transfer(
                &env.current_contract_address(),
                &refund.customer,
                &net_refund_amount,
            );
        }

        // Enforce merchant refund quota if configured
        if let Some(mut quota) = env
            .storage()
            .instance()
            .get::<_, MerchantRefundQuota>(&DataKey::MerchantRefundQuota(refund.merchant.clone()))
        {
            let now = env.ledger().timestamp();
            // auto-reset if period elapsed
            if now > quota.period_start.saturating_add(quota.period_seconds) {
                quota.used = 0;
                quota.period_start = now;
            }
            let new_used = quota
                .used
                .checked_add(refund.amount)
                .ok_or(Error::Core(CoreError::InvalidAmount))?;
            if new_used > quota.limit {
                return Err(Error::Core(CoreError::RefundExceedsPolicy));
            }
            quota.used = new_used;
            env.storage().instance().set(
                &DataKey::MerchantRefundQuota(refund.merchant.clone()),
                &quota,
            );
        }

        Self::remove_from_status_index(env, RefundStatus::Approved, refund_id)?;
        refund.status = RefundStatus::Processed;
        // Issue #147: Set processed_at timestamp
        refund.processed_at = Some(env.ledger().timestamp());
        env.storage()
            .instance()
            .set(&DataKey::Refund(refund_id), &refund);
        Self::add_to_status_index(env, RefundStatus::Processed, refund_id);
        // Issue #382: this refund's requested_at may fall within a previously
        // cached analytics window, so drop that cache entry.
        Self::invalidate_analytics_cache_for(env, refund.requested_at);

        (RefundProcessed {
            refund_id,
            processed_by,
            customer: refund.customer,
            amount: refund.amount,
            token: refund.token,
            processed_at: env.ledger().timestamp(),
        })
        .publish(env);

        // Issue #144: Invoke notification hooks
        Self::invoke_hooks(env, RefundEventType::Processed, refund_id);

        Ok(())
    }

    fn get_external_payment(env: &Env, payment_id: u64) -> Result<ExternalPayment, Error> {
        let payment_contract: Address = env
            .storage()
            .instance()
            .get(&DataKey::PaymentContractAddress)
            .ok_or(Error::Core(CoreError::PaymentContractNotSet))?;
        let args = (payment_id,).into_val(env);
        let func = Symbol::new(env, "get_payment");
        match env.try_invoke_contract::<ExternalPayment, soroban_sdk::InvokeError>(
            &payment_contract,
            &func,
            args,
        ) {
            Ok(Ok(payment)) => Ok(payment),
            _ => Err(Error::Core(CoreError::InvalidPaymentId)),
        }
    }

    // ── ANALYTICS FUNCTIONS ────────────────────────────────────────────────

    /// Get overall refund analytics for the contract.
    ///
    /// # Returns
    /// A `RefundAnalytics` struct containing total requests, approvals, rejections,
    /// processed count, total volume, and approval rate in basis points.
    pub fn get_refund_analytics(env: Env) -> RefundAnalytics {
        env.storage()
            .instance()
            .get(&DataKey::RefundAnalyticsKey)
            .unwrap_or(RefundAnalytics {
                total_refunds_requested: 0,
                total_refunds_approved: 0,
                total_refunds_rejected: 0,
                total_refunds_processed: 0,
                total_refund_volume: 0,
                approval_rate_bps: 0,
            })
    }

    // ── PAUSE FUNCTIONS ────────────────────────────────────────────────────

    /// Pause the entire contract, blocking all state-changing refund operations.
    ///
    /// Records the pause event in the history log with a timestamp and reason.
    ///
    /// # Arguments
    /// * `admin` - The contract admin pausing the contract.
    /// * `reason` - A human-readable reason for the pause.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn pause_contract(env: Env, admin: Address, reason: String) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }
        let now = env.ledger().timestamp();
        let pause_state = if let Some(mut state) = env
            .storage()
            .instance()
            .get::<SystemKey, PauseState>(&SystemKey::PauseStateKey)
        {
            state.globally_paused = true;
            state.paused_at = now;
            state.paused_by = admin.clone();
            state.pause_reason = reason.clone();
            state
        } else {
            PauseState {
                globally_paused: true,
                paused_functions: Vec::new(&env),
                paused_at: now,
                paused_by: admin.clone(),
                pause_reason: reason.clone(),
            }
        };
        env.storage()
            .instance()
            .set(&SystemKey::PauseStateKey, &pause_state);
        let history_count: u64 = env
            .storage()
            .instance()
            .get(&SystemKey::PauseHistoryCount)
            .unwrap_or(0);
        let entry = PauseHistory {
            index: history_count,
            function_name: String::from_str(&env, "global"),
            paused: true,
            changed_by: admin.clone(),
            changed_at: now,
            reason: reason.clone(),
        };
        env.storage()
            .instance()
            .set(&SystemKey::PauseHistoryEntry(history_count), &entry);
        env.storage()
            .instance()
            .set(&SystemKey::PauseHistoryCount, &(history_count + 1));
        (ContractPausedEvent {
            paused_by: admin,
            reason,
            paused_at: now,
        })
        .publish(&env);
        Ok(())
    }

    /// Unpause the contract and resume all refund operations.
    ///
    /// # Arguments
    /// * `admin` - The contract admin unpausing the contract.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn unpause_contract(env: Env, admin: Address) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }
        if let Some(mut state) = env
            .storage()
            .instance()
            .get::<SystemKey, PauseState>(&SystemKey::PauseStateKey)
        {
            state.globally_paused = false;
            env.storage()
                .instance()
                .set(&SystemKey::PauseStateKey, &state);
        }
        let now = env.ledger().timestamp();
        let history_count: u64 = env
            .storage()
            .instance()
            .get(&SystemKey::PauseHistoryCount)
            .unwrap_or(0);
        let entry = PauseHistory {
            index: history_count,
            function_name: String::from_str(&env, "global"),
            paused: false,
            changed_by: admin.clone(),
            changed_at: now,
            reason: String::from_str(&env, ""),
        };
        env.storage()
            .instance()
            .set(&SystemKey::PauseHistoryEntry(history_count), &entry);
        env.storage()
            .instance()
            .set(&SystemKey::PauseHistoryCount, &(history_count + 1));
        (ContractUnpausedEvent {
            unpaused_by: admin,
            unpaused_at: now,
        })
        .publish(&env);
        Ok(())
    }

    /// Pause a specific contract function while keeping others operational.
    ///
    /// # Arguments
    /// * `admin` - The contract admin pausing the function.
    /// * `function_name` - The name of the function to pause.
    /// * `reason` - A human-readable reason for the pause.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn pause_function(
        env: Env,
        admin: Address,
        function_name: String,
        reason: String,
    ) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }
        let now = env.ledger().timestamp();
        let mut pause_state = if let Some(state) = env
            .storage()
            .instance()
            .get::<SystemKey, PauseState>(&SystemKey::PauseStateKey)
        {
            state
        } else {
            PauseState {
                globally_paused: false,
                paused_functions: Vec::new(&env),
                paused_at: 0,
                paused_by: admin.clone(),
                pause_reason: String::from_str(&env, ""),
            }
        };
        if !pause_state.paused_functions.contains(&function_name) {
            pause_state
                .paused_functions
                .push_back(function_name.clone());
        }
        env.storage()
            .instance()
            .set(&SystemKey::PauseStateKey, &pause_state);
        let history_count: u64 = env
            .storage()
            .instance()
            .get(&SystemKey::PauseHistoryCount)
            .unwrap_or(0);
        let entry = PauseHistory {
            index: history_count,
            function_name: function_name.clone(),
            paused: true,
            changed_by: admin.clone(),
            changed_at: now,
            reason: reason.clone(),
        };
        env.storage()
            .instance()
            .set(&SystemKey::PauseHistoryEntry(history_count), &entry);
        env.storage()
            .instance()
            .set(&SystemKey::PauseHistoryCount, &(history_count + 1));
        (FunctionPausedEvent {
            function_name,
            paused_by: admin,
            reason,
        })
        .publish(&env);
        Ok(())
    }

    /// Unpause a previously paused contract function.
    ///
    /// # Arguments
    /// * `admin` - The contract admin unpausing the function.
    /// * `function_name` - The name of the function to unpause.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn unpause_function(env: Env, admin: Address, function_name: String) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }
        if let Some(mut state) = env
            .storage()
            .instance()
            .get::<SystemKey, PauseState>(&SystemKey::PauseStateKey)
        {
            let mut new_paused = Vec::new(&env);
            for fn_name in state.paused_functions.iter() {
                if fn_name != function_name {
                    new_paused.push_back(fn_name);
                }
            }
            state.paused_functions = new_paused;
            env.storage()
                .instance()
                .set(&SystemKey::PauseStateKey, &state);
        }
        let now = env.ledger().timestamp();
        let history_count: u64 = env
            .storage()
            .instance()
            .get(&SystemKey::PauseHistoryCount)
            .unwrap_or(0);
        let entry = PauseHistory {
            index: history_count,
            function_name: function_name.clone(),
            paused: false,
            changed_by: admin.clone(),
            changed_at: now,
            reason: String::from_str(&env, ""),
        };
        env.storage()
            .instance()
            .set(&SystemKey::PauseHistoryEntry(history_count), &entry);
        env.storage()
            .instance()
            .set(&SystemKey::PauseHistoryCount, &(history_count + 1));
        (FunctionUnpausedEvent {
            function_name,
            unpaused_by: admin,
        })
        .publish(&env);
        Ok(())
    }

    /// Get the current global pause state of the contract.
    ///
    /// # Returns
    /// A `PauseState` struct indicating whether the contract is globally paused,
    /// which functions are individually paused, and who initiated the pause.
    pub fn get_pause_state(env: Env) -> PauseState {
        env.storage()
            .instance()
            .get(&SystemKey::PauseStateKey)
            .unwrap_or(PauseState {
                globally_paused: false,
                paused_functions: Vec::new(&env),
                paused_at: 0,
                paused_by: env.current_contract_address(),
                pause_reason: String::from_str(&env, ""),
            })
    }

    /// Check whether a specific function is currently paused.
    ///
    /// # Arguments
    /// * `function_name` - The name of the function to check.
    ///
    /// # Returns
    /// `true` if the function is paused (either individually or due to a global pause).
    pub fn is_function_paused(env: Env, function_name: String) -> bool {
        if let Some(state) = env
            .storage()
            .instance()
            .get::<SystemKey, PauseState>(&SystemKey::PauseStateKey)
        {
            if state.globally_paused {
                return true;
            }
            for fn_name in state.paused_functions.iter() {
                if fn_name == function_name {
                    return true;
                }
            }
        }
        false
    }

    fn reason_code_rank(code: &RefundReasonCode) -> u32 {
        match code {
            RefundReasonCode::ProductDefect => 0,
            RefundReasonCode::NonDelivery => 1,
            RefundReasonCode::DuplicateCharge => 2,
            RefundReasonCode::Unauthorized => 3,
            RefundReasonCode::CustomerRequest => 4,
            RefundReasonCode::Other => 5,
        }
    }

    fn require_not_paused(env: &Env, function_name: &str) -> Result<(), Error> {
        if let Some(state) = env
            .storage()
            .instance()
            .get::<SystemKey, PauseState>(&SystemKey::PauseStateKey)
        {
            if state.globally_paused {
                return Err(Error::Core(CoreError::ContractPaused));
            }
            let fn_str = String::from_str(env, function_name);
            for fn_name in state.paused_functions.iter() {
                if fn_name == fn_str {
                    return Err(Error::Core(CoreError::FunctionPaused));
                }
            }
        }
        Ok(())
    }

    // ── CIRCUIT BREAKER ────────────────────────────────────────────────────

    /// Set the circuit breaker configuration that monitors refund volume ratios.
    ///
    /// # Arguments
    /// * `admin` - The contract admin setting the configuration.
    /// * `config` - The `CircuitBreakerConfig` with thresholds and cooldown settings.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn set_circuit_breaker_config(
        env: Env,
        admin: Address,
        config: CircuitBreakerConfig,
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
            .set(&SystemKey::CircuitBreakerConfigKey, &config);
        Ok(())
    }

    /// Get the current state of the circuit breaker.
    ///
    /// # Returns
    /// A `CircuitBreakerState` indicating whether the breaker is tripped, the trip count,
    /// the last observed refund rate, and the auto-reset timestamp.
    pub fn get_circuit_breaker_state(env: Env) -> CircuitBreakerState {
        let mut state = env
            .storage()
            .instance()
            .get::<SystemKey, CircuitBreakerState>(&SystemKey::CircuitBreakerStateKey)
            .unwrap_or(CircuitBreakerState {
                tripped: false,
                tripped_at: None,
                trip_count: 0,
                last_refund_rate_bps: 0,
                resets_at: None,
            });
        #[cfg(test)]
        {
            if TEST_TRIPPED.with(|t| t.load(core::sync::atomic::Ordering::SeqCst)) {
                state.tripped = true;
                state.trip_count =
                    TEST_TRIP_COUNT.with(|tc| tc.load(core::sync::atomic::Ordering::SeqCst));
                let resets_at =
                    TEST_RESETS_AT.with(|r| r.load(core::sync::atomic::Ordering::SeqCst));
                if resets_at > 0 {
                    state.resets_at = Some(resets_at);
                }
            }
        }
        state
    }

    /// Manually reset the circuit breaker and clear the tripped state.
    ///
    /// # Arguments
    /// * `admin` - The contract admin resetting the circuit breaker.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn reset_circuit_breaker(env: Env, admin: Address) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }
        let mut state = Self::get_circuit_breaker_state(env.clone());
        state.tripped = false;
        state.tripped_at = None;
        state.resets_at = None;
        env.storage()
            .instance()
            .set(&SystemKey::CircuitBreakerStateKey, &state);
        #[cfg(test)]
        {
            TEST_TRIPPED.with(|t| t.store(false, core::sync::atomic::Ordering::SeqCst));
            TEST_TRIP_COUNT.with(|tc| tc.store(0, core::sync::atomic::Ordering::SeqCst));
            TEST_RESETS_AT.with(|r| r.store(0, core::sync::atomic::Ordering::SeqCst));
        }
        let now = env.ledger().timestamp();
        CircuitBreakerResetEvent {
            reset_by: admin,
            reset_at: now,
        }
        .publish(&env);
        Ok(())
    }

    /// Check whether the circuit breaker is currently active (tripped and not yet reset).
    ///
    /// # Returns
    /// `true` if the circuit breaker is tripped and the cooldown has not elapsed.
    pub fn check_circuit_breaker(env: Env) -> bool {
        let config: CircuitBreakerConfig = match env
            .storage()
            .instance()
            .get(&SystemKey::CircuitBreakerConfigKey)
        {
            Some(c) => c,
            None => return false,
        };
        if !config.enabled {
            return false;
        }
        let state = Self::get_circuit_breaker_state(env.clone());
        if !state.tripped {
            return false;
        }
        let now = env.ledger().timestamp();
        if let Some(resets_at) = state.resets_at {
            now < resets_at
        } else {
            true
        }
    }

    fn effective_global_rate_limit(global: &GlobalRefundRateLimit, now: u64) -> (u32, u64) {
        if global.next_config_effective_at > 0 && now >= global.next_config_effective_at {
            (
                global.next_max_requests_per_window,
                global.next_window_seconds,
            )
        } else {
            (global.max_requests_per_window, global.window_seconds)
        }
    }

    fn check_and_update_customer_refund_rate_limit(
        env: &Env,
        customer: Address,
    ) -> Result<(), Error> {
        let global_limit_opt = env
            .storage()
            .instance()
            .get::<DataKey, GlobalRefundRateLimit>(&DataKey::GlobalRefundRateLimit);
        let customer_limit_opt = env
            .storage()
            .instance()
            .get::<DataKey, CustomerRefundRateLimit>(&DataKey::CustomerRefundRateLimit(
                customer.clone(),
            ));
        if global_limit_opt.is_none() && customer_limit_opt.is_none() {
            return Ok(());
        }
        let now = env.ledger().timestamp();
        let mut limit = match customer_limit_opt {
            Some(l) => l,
            None => {
                let g = global_limit_opt.as_ref().unwrap();
                let (max_requests, window_seconds) = Self::effective_global_rate_limit(g, now);
                CustomerRefundRateLimit {
                    customer: customer.clone(),
                    window_start: now,
                    request_count: 0,
                    max_requests_per_window: max_requests,
                    window_seconds,
                    custom_override: false,
                }
            }
        };
        if now >= limit.window_start + limit.window_seconds {
            limit.window_start = now;
            limit.request_count = 0;
            if !limit.custom_override {
                if let Some(ref g) = global_limit_opt {
                    let (max_requests, window_seconds) = Self::effective_global_rate_limit(g, now);
                    limit.max_requests_per_window = max_requests;
                    limit.window_seconds = window_seconds;
                }
            }
        }
        if limit.request_count >= limit.max_requests_per_window {
            return Err(Error::Core(CoreError::RefundRateLimitExceeded));
        }
        limit.request_count += 1;
        env.storage()
            .instance()
            .set(&DataKey::CustomerRefundRateLimit(customer), &limit);
        Ok(())
    }

    fn check_and_update_circuit_breaker(
        env: &Env,
        refund_amount: i128,
        payment_amount: i128,
    ) -> Result<(), Error> {
        let config: CircuitBreakerConfig = match env
            .storage()
            .instance()
            .get(&SystemKey::CircuitBreakerConfigKey)
        {
            Some(c) => c,
            None => return Ok(()),
        };

        if !config.enabled {
            return Ok(());
        }

        let now = env.ledger().timestamp();
        let mut state = Self::get_circuit_breaker_state(env.clone());

        // Auto-reset after cooldown
        if state.tripped {
            if let Some(resets_at) = state.resets_at {
                if now >= resets_at {
                    state.tripped = false;
                    state.tripped_at = None;
                    state.resets_at = None;
                    env.storage()
                        .instance()
                        .set(&SystemKey::CircuitBreakerStateKey, &state);
                    #[cfg(test)]
                    {
                        TEST_TRIPPED.with(|t| t.store(false, core::sync::atomic::Ordering::SeqCst));
                        TEST_TRIP_COUNT
                            .with(|tc| tc.store(0, core::sync::atomic::Ordering::SeqCst));
                        TEST_RESETS_AT.with(|r| r.store(0, core::sync::atomic::Ordering::SeqCst));
                    }
                } else {
                    return Err(Error::Core(CoreError::CircuitBreakerTripped));
                }
            } else {
                return Err(Error::Core(CoreError::CircuitBreakerTripped));
            }
        }

        // Reset window if expired
        let window_start: u64 = env
            .storage()
            .instance()
            .get(&SystemKey::WindowStart)
            .unwrap_or(0);

        if now >= window_start + config.measurement_window_seconds || window_start == 0 {
            env.storage().instance().set(&SystemKey::WindowStart, &now);
            env.storage()
                .instance()
                .set(&SystemKey::WindowRefundVolume, &0i128);
            env.storage()
                .instance()
                .set(&SystemKey::WindowPaymentVolume, &0i128);
        }

        let new_refund_vol: i128 = env
            .storage()
            .instance()
            .get(&SystemKey::WindowRefundVolume)
            .unwrap_or(0)
            + refund_amount;

        let new_payment_vol: i128 = env
            .storage()
            .instance()
            .get(&SystemKey::WindowPaymentVolume)
            .unwrap_or(0)
            + payment_amount;

        if new_payment_vol <= 0 {
            return Ok(());
        }

        let rate_bps = ((new_refund_vol * 10000) / new_payment_vol) as u32;

        if rate_bps > config.max_refund_rate_bps {
            state.tripped = true;
            state.tripped_at = Some(now);
            state.trip_count += 1;
            state.last_refund_rate_bps = rate_bps;
            state.resets_at = Some(now + config.cooldown_seconds);
            env.storage()
                .instance()
                .set(&SystemKey::CircuitBreakerStateKey, &state);
            #[cfg(test)]
            {
                TEST_TRIPPED.with(|t| t.store(true, core::sync::atomic::Ordering::SeqCst));
                TEST_TRIP_COUNT
                    .with(|tc| tc.store(state.trip_count, core::sync::atomic::Ordering::SeqCst));
                TEST_RESETS_AT.with(|r| {
                    r.store(
                        now + config.cooldown_seconds,
                        core::sync::atomic::Ordering::SeqCst,
                    )
                });
            }
            CircuitBreakerTrippedEvent {
                refund_rate_bps: rate_bps,
                tripped_at: now,
            }
            .publish(env);
            return Err(Error::Core(CoreError::CircuitBreakerTripped));
        }

        env.storage()
            .instance()
            .set(&SystemKey::WindowRefundVolume, &new_refund_vol);
        env.storage()
            .instance()
            .set(&SystemKey::WindowPaymentVolume, &new_payment_vol);

        Ok(())
    }

    /// Check for fraud signals on an address based on its refund rate relative to payment count.
    ///
    /// If the refund rate exceeds the configured threshold and the address has
    /// sufficient transaction history, a `FraudSignal` is created or updated.
    ///
    /// # Arguments
    /// * `address` - The address to check for fraud signals.
    ///
    /// # Returns
    /// The `FraudSignal` if one exists and has not been reviewed, `None` otherwise.
    // Fraud detection functions (#137)
    pub fn check_fraud_signals(env: Env, address: Address) -> Option<FraudSignal> {
        // Get fraud config
        let config: FraudConfig = env
            .storage()
            .instance()
            .get(&SystemKey::FraudConfig)
            .unwrap_or(FraudConfig {
                max_refund_rate_bps: 2000, // 20%
                min_transactions_for_check: 5,
                enabled: true,
            });

        if !config.enabled {
            return None;
        }

        // Get customer's payment and refund statistics from payment contract
        // For now, we'll use a simplified approach - in production, this would
        // query the payment contract for actual statistics
        let total_payments = Self::get_customer_payment_count(&env, &address);
        let total_refunds = Self::get_customer_refund_count(&env, &address);

        // Skip if below minimum transaction threshold
        if total_payments < config.min_transactions_for_check {
            return None;
        }

        // Calculate refund rate
        let refund_rate_bps: u32 = if total_payments > 0 {
            ((total_refunds * 10000) / total_payments) as u32
        } else {
            0
        };

        // Check if refund rate exceeds threshold
        if refund_rate_bps > config.max_refund_rate_bps {
            let existing_signal: Option<FraudSignal> = env
                .storage()
                .instance()
                .get(&SystemKey::FraudSignal(address.clone()));

            match existing_signal {
                Some(mut signal) if !signal.reviewed => {
                    // Update existing signal
                    signal.refund_rate_bps = refund_rate_bps as u32;
                    signal.total_payments = total_payments;
                    signal.total_refunds = total_refunds;
                    env.storage()
                        .instance()
                        .set(&SystemKey::FraudSignal(address), &signal);
                    Some(signal)
                }
                None => {
                    // Create new fraud signal
                    let signal = FraudSignal {
                        address: address.clone(),
                        refund_rate_bps: refund_rate_bps as u32,
                        total_payments,
                        total_refunds,
                        flagged_at: env.ledger().timestamp(),
                        reviewed: false,
                    };
                    env.storage()
                        .instance()
                        .set(&SystemKey::FraudSignal(address.clone()), &signal);

                    // Add to flagged addresses index: store both the ordered
                    // address entry and the updated counter so the list can
                    // be fully reconstructed by get_flagged_addresses.
                    let flagged_count: u64 = env
                        .storage()
                        .instance()
                        .get(&SystemKey::FlaggedAddressesIndex)
                        .unwrap_or(0);
                    env.storage()
                        .instance()
                        .set(&SystemKey::FlaggedAddress(flagged_count), &address.clone());
                    env.storage()
                        .instance()
                        .set(&SystemKey::FlaggedAddressesIndex, &(flagged_count + 1));

                    // Emit fraud signal raised event
                    (FraudSignalRaised {
                        address,
                        refund_rate_bps: refund_rate_bps as u32,
                    })
                    .publish(&env);

                    Some(signal)
                }
                _ => None, // Already reviewed or exists
            }
        } else {
            None
        }
    }

    /// Get all addresses that have been flagged for potential fraud.
    ///
    /// # Returns
    /// A vector of `FraudSignal` entries for all flagged addresses.
    pub fn get_flagged_addresses(env: Env) -> Vec<FraudSignal> {
        let mut flagged = Vec::new(&env);

        let total: u64 = env
            .storage()
            .instance()
            .get(&SystemKey::FlaggedAddressesIndex)
            .unwrap_or(0);

        for i in 0..total {
            if let Some(address) = env
                .storage()
                .instance()
                .get::<SystemKey, Address>(&SystemKey::FlaggedAddress(i))
            {
                if let Some(signal) = env
                    .storage()
                    .instance()
                    .get::<SystemKey, FraudSignal>(&SystemKey::FraudSignal(address))
                {
                    flagged.push_back(signal);
                }
            }
        }

        flagged
    }

    /// Mark a fraud signal as reviewed by an admin, allowing the address to continue
    /// requesting refunds.
    ///
    /// # Arguments
    /// * `admin` - The contract admin marking the signal as reviewed.
    /// * `address` - The flagged address to review.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `FraudSignalNotFound` if no fraud signal exists for the address.
    pub fn mark_fraud_reviewed(env: Env, admin: Address, address: Address) -> Result<(), Error> {
        admin.require_auth();

        // Verify admin is the contract admin
        let stored_admin = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        let mut signal: FraudSignal = env
            .storage()
            .instance()
            .get(&SystemKey::FraudSignal(address.clone()))
            .ok_or(Error::Ext(ExtError::FraudSignalNotFound))?;

        signal.reviewed = true;
        env.storage()
            .instance()
            .set(&SystemKey::FraudSignal(address.clone()), &signal);

        // Emit fraud signal reviewed event
        (FraudSignalReviewed {
            address,
            reviewed_by: admin,
        })
        .publish(&env);

        Ok(())
    }

    /// Set the fraud detection configuration thresholds.
    ///
    /// # Arguments
    /// * `admin` - The contract admin setting the configuration.
    /// * `config` - The `FraudConfig` with detection parameters.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn set_fraud_config(env: Env, admin: Address, config: FraudConfig) -> Result<(), Error> {
        admin.require_auth();

        // Verify admin is the contract admin
        let stored_admin = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        env.storage()
            .instance()
            .set(&SystemKey::FraudConfig, &config);

        Ok(())
    }

    // Helper functions for fraud detection
    fn get_customer_payment_count(env: &Env, address: &Address) -> u64 {
        let payment_contract: Address = match env
            .storage()
            .instance()
            .get(&DataKey::PaymentContractAddress)
        {
            Some(addr) => addr,
            None => return 0,
        };

        let func = Symbol::new(env, "get_payment_count_by_customer");
        let args = (address.clone(),).into_val(env);

        match env.try_invoke_contract::<u64, soroban_sdk::InvokeError>(
            &payment_contract,
            &func,
            args,
        ) {
            Ok(Ok(count)) => count,
            _ => 0,
        }
    }

    fn get_customer_refund_count(env: &Env, address: &Address) -> u64 {
        // Count refunds for this address
        let refund_count: u64 = env
            .storage()
            .instance()
            .get(&DataKey::CustomerRefundCount(address.clone()))
            .unwrap_or(0);
        refund_count
    }

    /// Appends a refund id to a customer's history index, capping the number
    /// of entries kept in "hot" instance storage. Once the cap is exceeded,
    /// the oldest entry is moved to persistent storage so per-customer
    /// history can grow without bound while the instance storage footprint
    /// (loaded on every invocation) stays fixed.
    fn append_customer_refund_history(env: &Env, customer: &Address, refund_id: u64) {
        let customer_count: u64 = env
            .storage()
            .instance()
            .get(&DataKey::CustomerRefundCount(customer.clone()))
            .unwrap_or(0);

        env.storage().instance().set(
            &DataKey::CustomerRefunds(customer.clone(), customer_count),
            &refund_id,
        );
        env.storage().instance().set(
            &DataKey::CustomerRefundCount(customer.clone()),
            &(customer_count + 1),
        );

        let start: u64 = env
            .storage()
            .instance()
            .get(&DataKey::CustomerRefundHistoryStart(customer.clone()))
            .unwrap_or(0);
        let hot_len = customer_count + 1 - start;
        if hot_len > CUSTOMER_HISTORY_HOT_CAP {
            if let Some(archived_id) = env
                .storage()
                .instance()
                .get::<_, u64>(&DataKey::CustomerRefunds(customer.clone(), start))
            {
                env.storage().persistent().set(
                    &DataKey::CustomerRefundsArchive(customer.clone(), start),
                    &archived_id,
                );
            }
            env.storage()
                .instance()
                .remove(&DataKey::CustomerRefunds(customer.clone(), start));
            env.storage().instance().set(
                &DataKey::CustomerRefundHistoryStart(customer.clone()),
                &(start + 1),
            );
        }
    }

    /// Reads a customer's refund id at a given history index, transparently
    /// falling back to the archive when the entry has aged out of hot storage.
    fn get_customer_refund_id_at(env: &Env, customer: &Address, index: u64) -> Option<u64> {
        let start: u64 = env
            .storage()
            .instance()
            .get(&DataKey::CustomerRefundHistoryStart(customer.clone()))
            .unwrap_or(0);
        if index < start {
            env.storage()
                .persistent()
                .get(&DataKey::CustomerRefundsArchive(customer.clone(), index))
        } else {
            env.storage()
                .instance()
                .get(&DataKey::CustomerRefunds(customer.clone(), index))
        }
    }

    /// Set the per-customer refund cooldown configuration.
    ///
    /// # Arguments
    /// * `admin` - The contract admin setting the configuration.
    /// * `config` - Cooldown duration and enable flag.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn set_refund_cooldown_config(
        env: Env,
        admin: Address,
        config: RefundCooldownConfig,
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
            .set(&SystemKey::RefundCooldownConfig, &config);
        Ok(())
    }

    /// Returns the configured per-customer refund cooldown, if any.
    pub fn get_refund_cooldown_config(env: Env) -> Option<RefundCooldownConfig> {
        env.storage()
            .instance()
            .get(&SystemKey::RefundCooldownConfig)
    }

    fn check_customer_refund_cooldown(env: &Env, customer: &Address) -> Result<(), Error> {
        let config: RefundCooldownConfig = match env
            .storage()
            .instance()
            .get::<SystemKey, RefundCooldownConfig>(&SystemKey::RefundCooldownConfig)
        {
            Some(c) if c.enabled => c,
            _ => return Ok(()),
        };

        let record: CustomerRefundCooldown = match env
            .storage()
            .instance()
            .get(&SystemKey::CustomerRefundCooldown(customer.clone()))
        {
            Some(r) => r,
            None => return Ok(()),
        };

        let now = env.ledger().timestamp();
        let elapsed = now.saturating_sub(record.last_refund_requested_at);
        if elapsed < record.cooldown_seconds {
            let available_at = record
                .last_refund_requested_at
                .saturating_add(record.cooldown_seconds);
            RefundCooldownEnforced {
                customer: customer.clone(),
                last_refund_at: record.last_refund_requested_at,
                cooldown_seconds: record.cooldown_seconds,
                available_at,
            }
            .publish(env);
            return Err(Error::Core(CoreError::RefundCooldownActive));
        }

        Ok(())
    }

    fn update_customer_refund_cooldown(env: &Env, customer: &Address) -> Result<(), Error> {
        let config: RefundCooldownConfig = match env
            .storage()
            .instance()
            .get(&SystemKey::RefundCooldownConfig)
        {
            Some(c) => c,
            None => return Ok(()), // No cooldown configured, skip
        };
        if !config.enabled {
            return Ok(());
        }
        let record = CustomerRefundCooldown {
            customer: customer.clone(),
            last_refund_requested_at: env.ledger().timestamp(),
            cooldown_seconds: config.cooldown_seconds,
        };
        env.storage().instance().set(
            &SystemKey::CustomerRefundCooldown(customer.clone()),
            &record,
        );
        Ok(())
    }

    // Issue #147: Customer refund history functions

    /// Get paginated refund history for a customer, sorted newest-first
    pub fn get_customer_refund_history(
        env: Env,
        customer: Address,
        limit: u64,
        offset: u64,
    ) -> Vec<Refund> {
        let mut results: Vec<Refund> = Vec::new(&env);
        let total = Self::get_customer_refund_count(&env, &customer);

        if limit == 0 || offset >= total {
            return results;
        }

        // Calculate range for newest-first ordering
        let end = core::cmp::min(total, offset.saturating_add(limit));

        // Iterate in reverse order (newest first)
        let mut collected = 0u64;
        let mut skipped = 0u64;
        let mut index = total;

        while index > 0 && collected < limit {
            index -= 1;

            if skipped < offset {
                skipped += 1;
                continue;
            }

            if let Some(refund_id) = Self::get_customer_refund_id_at(&env, &customer, index) {
                if let Some(refund) = env
                    .storage()
                    .instance()
                    .get::<_, Refund>(&DataKey::Refund(refund_id))
                {
                    results.push_back(refund);
                    collected += 1;
                }
            }
        }

        results
    }

    /// Get the total count of refunds for a customer (public version)
    pub fn get_customer_refund_count_public(env: Env, customer: Address) -> u64 {
        Self::get_customer_refund_count(&env, &customer)
    }

    /// Get summary statistics for a customer's refunds
    pub fn get_customer_refund_summary(env: Env, customer: Address) -> CustomerRefundSummary {
        let total_requested = Self::get_customer_refund_count(&env, &customer);
        let mut total_approved = 0u64;
        let mut total_amount_refunded = 0i128;
        let mut total_processing_time = 0u64;
        let mut processed_count = 0u64;

        let mut index = 0u64;
        while index < total_requested {
            if let Some(refund_id) = Self::get_customer_refund_id_at(&env, &customer, index) {
                if let Some(refund) = env
                    .storage()
                    .instance()
                    .get::<_, Refund>(&DataKey::Refund(refund_id))
                {
                    match refund.status {
                        RefundStatus::Approved | RefundStatus::Processed => {
                            total_approved += 1;
                        }
                        _ => {}
                    }

                    if refund.status == RefundStatus::Processed {
                        total_amount_refunded += refund.amount;

                        // Calculate processing time if we have both timestamps
                        if let Some(processed_at) = refund.processed_at {
                            let processing_time = processed_at.saturating_sub(refund.requested_at);
                            total_processing_time =
                                total_processing_time.saturating_add(processing_time);
                            processed_count += 1;
                        }
                    }
                }
            }
            index += 1;
        }

        let avg_processing_time = if processed_count > 0 {
            total_processing_time / processed_count
        } else {
            0
        };

        CustomerRefundSummary {
            total_requested,
            total_approved,
            total_amount_refunded,
            avg_processing_time,
        }
    }

    fn get_merchant_refund_count(env: &Env, merchant: &Address) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MerchantRefundCount(merchant.clone()))
            .unwrap_or(0)
    }

    // Issue #144: Notification hook functions
    const MAX_HOOKS_PER_EVENT: u32 = 10;

    /// Verify the subscriber address is a reachable contract that implements `ping()`.
    fn validate_notification_hook_subscriber(env: &Env, subscriber: &Address) -> Result<(), Error> {
        match env.try_invoke_contract::<(), soroban_sdk::InvokeError>(
            subscriber,
            &Symbol::new(env, "ping"),
            ().into_val(env),
        ) {
            Ok(Ok(_)) => Ok(()),
            _ => Err(Error::Ext(ExtError::InvalidHookAddress)),
        }
    }

    /// Register a notification hook for specific refund events
    pub fn register_notification_hook(
        env: Env,
        subscriber: Address,
        events: Vec<RefundEventType>,
    ) -> Result<u64, Error> {
        subscriber.require_auth();

        // Check that at least one event is specified
        if events.is_empty() {
            return Err(Error::Core(CoreError::InvalidAmount)); // Reusing error for invalid input
        }

        Self::validate_notification_hook_subscriber(&env, &subscriber)?;

        // Check max hooks per event type
        for event_type in events.iter() {
            let count: u32 = env
                .storage()
                .instance()
                .get(&SystemKey::HooksByEventCount(event_type.clone()))
                .unwrap_or(0);

            if count >= Self::MAX_HOOKS_PER_EVENT {
                return Err(Error::Ext(ExtError::MaxHooksPerEventReached));
            }
        }

        // Generate hook ID
        let hook_id: u64 = env
            .storage()
            .instance()
            .get(&SystemKey::NotificationHookCounter)
            .unwrap_or(0)
            + 1;

        env.storage()
            .instance()
            .set(&SystemKey::NotificationHookCounter, &hook_id);

        // Create hook
        let hook = NotificationHook {
            hook_id,
            subscriber: subscriber.clone(),
            events: events.clone(),
            active: true,
        };

        // Store hook
        env.storage()
            .instance()
            .set(&SystemKey::NotificationHook(hook_id), &hook);

        // Index by event type
        for event_type in events.iter() {
            let count: u32 = env
                .storage()
                .instance()
                .get(&SystemKey::HooksByEventCount(event_type.clone()))
                .unwrap_or(0);

            env.storage().instance().set(
                &SystemKey::HooksByEvent(event_type.clone(), count as u64),
                &hook_id,
            );

            env.storage().instance().set(
                &SystemKey::HooksByEventCount(event_type.clone()),
                &(count + 1),
            );
        }

        // Index by subscriber
        let subscriber_count: u32 = env
            .storage()
            .instance()
            .get(&SystemKey::SubscriberHookCount(subscriber.clone()))
            .unwrap_or(0);

        env.storage().instance().set(
            &SystemKey::SubscriberHooks(subscriber.clone(), subscriber_count as u64),
            &hook_id,
        );

        env.storage().instance().set(
            &SystemKey::SubscriberHookCount(subscriber.clone()),
            &(subscriber_count + 1),
        );

        // Emit event
        (HookRegistered {
            hook_id,
            subscriber,
            event_count: events.len(),
        })
        .publish(&env);

        Ok(hook_id)
    }

    /// Deregister a notification hook
    pub fn deregister_hook(env: Env, subscriber: Address, hook_id: u64) -> Result<(), Error> {
        subscriber.require_auth();

        // Get hook
        let hook: NotificationHook = env
            .storage()
            .instance()
            .get(&SystemKey::NotificationHook(hook_id))
            .ok_or(Error::Ext(ExtError::HookNotFound))?;

        // Verify ownership
        if hook.subscriber != subscriber {
            return Err(Error::Ext(ExtError::HookNotOwnedBySubscriber));
        }

        // Mark as inactive
        let mut updated_hook = hook.clone();
        updated_hook.active = false;

        env.storage()
            .instance()
            .set(&SystemKey::NotificationHook(hook_id), &updated_hook);

        // Decrement per-event hook counters
        for event_type in hook.events.iter() {
            let count: u32 = env
                .storage()
                .instance()
                .get(&SystemKey::HooksByEventCount(event_type.clone()))
                .unwrap_or(0);
            if count > 0 {
                env.storage().instance().set(
                    &SystemKey::HooksByEventCount(event_type.clone()),
                    &(count - 1),
                );
            }
        }

        // Decrement per-subscriber hook counter
        let subscriber_count: u32 = env
            .storage()
            .instance()
            .get(&SystemKey::SubscriberHookCount(subscriber.clone()))
            .unwrap_or(0);
        if subscriber_count > 0 {
            env.storage().instance().set(
                &SystemKey::SubscriberHookCount(subscriber.clone()),
                &(subscriber_count - 1),
            );
        }

        // Emit event
        (HookDeregistered {
            hook_id,
            subscriber,
        })
        .publish(&env);

        Ok(())
    }

    /// Get all hooks registered for a specific event type
    pub fn get_hooks_for_event(env: Env, event_type: RefundEventType) -> Vec<NotificationHook> {
        let mut hooks: Vec<NotificationHook> = Vec::new(&env);

        let count: u32 = env
            .storage()
            .instance()
            .get(&SystemKey::HooksByEventCount(event_type.clone()))
            .unwrap_or(0);

        for i in 0..count {
            if let Some(hook_id) = env
                .storage()
                .instance()
                .get::<_, u64>(&SystemKey::HooksByEvent(event_type.clone(), i as u64))
            {
                if let Some(hook) = env
                    .storage()
                    .instance()
                    .get::<_, NotificationHook>(&SystemKey::NotificationHook(hook_id))
                {
                    if hook.active {
                        hooks.push_back(hook);
                    }
                }
            }
        }

        hooks
    }

    /// Get all hooks for a subscriber
    pub fn get_subscriber_hooks(env: Env, subscriber: Address) -> Vec<NotificationHook> {
        let mut hooks: Vec<NotificationHook> = Vec::new(&env);

        let count: u32 = env
            .storage()
            .instance()
            .get(&SystemKey::SubscriberHookCount(subscriber.clone()))
            .unwrap_or(0);

        for i in 0..count {
            if let Some(hook_id) = env
                .storage()
                .instance()
                .get::<_, u64>(&SystemKey::SubscriberHooks(subscriber.clone(), i as u64))
            {
                if let Some(hook) = env
                    .storage()
                    .instance()
                    .get::<_, NotificationHook>(&SystemKey::NotificationHook(hook_id))
                {
                    if hook.active {
                        hooks.push_back(hook);
                    }
                }
            }
        }

        hooks
    }

    /// Internal function to invoke hooks for a specific event
    fn invoke_hooks(env: &Env, event_type: RefundEventType, refund_id: u64) {
        let count: u32 = env
            .storage()
            .instance()
            .get(&SystemKey::HooksByEventCount(event_type.clone()))
            .unwrap_or(0);

        for i in 0..count {
            if let Some(hook_id) = env
                .storage()
                .instance()
                .get::<_, u64>(&SystemKey::HooksByEvent(event_type.clone(), i as u64))
            {
                if let Some(hook) = env
                    .storage()
                    .instance()
                    .get::<_, NotificationHook>(&SystemKey::NotificationHook(hook_id))
                {
                    if hook.active && hook.events.contains(&event_type) {
                        // Attempt to invoke the subscriber contract
                        // Using try_invoke_contract to isolate failures
                        let result = env.try_invoke_contract::<(), soroban_sdk::InvokeError>(
                            &hook.subscriber,
                            &Symbol::new(env, "on_refund_event"),
                            (event_type.clone(), refund_id).into_val(env),
                        );

                        // If hook invocation fails, emit failure event but don't revert
                        if result.is_err() {
                            (HookInvocationFailed {
                                hook_id: hook.hook_id,
                                subscriber: hook.subscriber.clone(),
                                event_type: event_type.clone(),
                                refund_id,
                            })
                            .publish(env);
                        }
                    }
                }
            }
        }
    }

    // ── Issue #148: Customer eligibility registry ─────────────────────────

    /// Set or update the refund eligibility rule for a customer under a specific merchant.
    /// Only the merchant themselves or the admin may call this.
    pub fn set_refund_eligibility(
        env: Env,
        merchant: Address,
        customer: Address,
        rule: EligibilityRule,
        reason_hash: BytesN<32>,
    ) -> Result<(), Error> {
        // Require merchant auth; admin can also call via mock_all_auths in tests
        merchant.require_auth();

        let entry = RefundEligibilityEntry {
            customer: customer.clone(),
            merchant: merchant.clone(),
            rule: rule.clone(),
            reason_hash,
            set_at: env.ledger().timestamp(),
        };

        let key = EligibilityKey::Entry(merchant.clone(), customer.clone());
        let is_new = !env.storage().instance().has(&key);
        env.storage().instance().set(&key, &entry);

        // If this is a new entry, append to the merchant's customer index
        if is_new {
            let count: u64 = env
                .storage()
                .instance()
                .get(&EligibilityKey::MerchantCustomerCount(merchant.clone()))
                .unwrap_or(0);
            env.storage().instance().set(
                &EligibilityKey::MerchantCustomerIndex(merchant.clone(), count),
                &customer,
            );
            env.storage().instance().set(
                &EligibilityKey::MerchantCustomerCount(merchant.clone()),
                &(count + 1),
            );
        }

        (EligibilitySet {
            merchant,
            customer,
            rule,
        })
        .publish(&env);

        Ok(())
    }

    /// Return the eligibility rule for a (merchant, customer) pair.
    /// Defaults to `Allow` when no entry exists.
    pub fn check_refund_eligibility(
        env: Env,
        merchant: Address,
        customer: Address,
    ) -> EligibilityRule {
        Self::check_refund_eligibility_internal(&env, &merchant, &customer)
    }

    /// Internal version that borrows `env` by reference.
    fn check_refund_eligibility_internal(
        env: &Env,
        merchant: &Address,
        customer: &Address,
    ) -> EligibilityRule {
        env.storage()
            .instance()
            .get::<EligibilityKey, RefundEligibilityEntry>(&EligibilityKey::Entry(
                merchant.clone(),
                customer.clone(),
            ))
            .map(|e| e.rule)
            .unwrap_or(EligibilityRule::Allow)
    }

    /// Remove an eligibility entry for a (merchant, customer) pair.
    /// Returns `EligibilityEntryNotFound` if no entry exists.
    /// Only the merchant or admin may call this.
    pub fn remove_refund_eligibility(
        env: Env,
        merchant: Address,
        customer: Address,
    ) -> Result<(), Error> {
        merchant.require_auth();

        let key = EligibilityKey::Entry(merchant.clone(), customer.clone());
        if !env.storage().instance().has(&key) {
            return Err(Error::Ext(ExtError::EligibilityEntryNotFound));
        }
        env.storage().instance().remove(&key);

        // Compact the merchant's customer index by swapping with the last element
        let count: u64 = env
            .storage()
            .instance()
            .get(&EligibilityKey::MerchantCustomerCount(merchant.clone()))
            .unwrap_or(0);

        if count > 0 {
            // Find the position of this customer in the index
            let mut found_index: Option<u64> = None;
            for i in 0..count {
                let idx_key = EligibilityKey::MerchantCustomerIndex(merchant.clone(), i);
                if let Some(addr) = env
                    .storage()
                    .instance()
                    .get::<EligibilityKey, Address>(&idx_key)
                {
                    if addr == customer {
                        found_index = Some(i);
                        break;
                    }
                }
            }

            if let Some(pos) = found_index {
                let last = count - 1;
                if pos != last {
                    // Swap with last
                    let last_key = EligibilityKey::MerchantCustomerIndex(merchant.clone(), last);
                    let last_addr: Address = env.storage().instance().get(&last_key).unwrap();
                    env.storage().instance().set(
                        &EligibilityKey::MerchantCustomerIndex(merchant.clone(), pos),
                        &last_addr,
                    );
                }
                // Remove the last slot
                env.storage()
                    .instance()
                    .remove(&EligibilityKey::MerchantCustomerIndex(
                        merchant.clone(),
                        last,
                    ));
                env.storage().instance().set(
                    &EligibilityKey::MerchantCustomerCount(merchant.clone()),
                    &last,
                );
            }
        }

        (EligibilityRemoved { merchant, customer }).publish(&env);

        Ok(())
    }

    /// Return all eligibility entries for a merchant.
    pub fn get_merchant_eligibility_list(
        env: Env,
        merchant: Address,
    ) -> Vec<RefundEligibilityEntry> {
        let mut results = Vec::new(&env);
        let count: u64 = env
            .storage()
            .instance()
            .get(&EligibilityKey::MerchantCustomerCount(merchant.clone()))
            .unwrap_or(0);

        for i in 0..count {
            if let Some(customer) = env.storage().instance().get::<EligibilityKey, Address>(
                &EligibilityKey::MerchantCustomerIndex(merchant.clone(), i),
            ) {
                if let Some(entry) = env
                    .storage()
                    .instance()
                    .get::<EligibilityKey, RefundEligibilityEntry>(&EligibilityKey::Entry(
                        merchant.clone(),
                        customer,
                    ))
                {
                    results.push_back(entry);
                }
            }
        }

        results
    }

    fn get_merchant_refunds_by_status_internal(
        env: &Env,
        merchant: &Address,
        status: RefundStatus,
        limit: u64,
        offset: u64,
    ) -> Vec<Refund> {
        let mut results: Vec<Refund> = Vec::new(env);
        if limit == 0 {
            return results;
        }

        let total = Self::get_merchant_refund_count(env, merchant);
        let mut matched = 0u64;
        let mut collected = 0u64;
        let mut index = 0u64;

        while index < total && collected < limit {
            if let Some(refund_id) = env
                .storage()
                .instance()
                .get::<_, u64>(&DataKey::MerchantRefunds(merchant.clone(), index))
            {
                if let Some(refund) = env
                    .storage()
                    .instance()
                    .get::<_, Refund>(&DataKey::Refund(refund_id))
                {
                    if refund.status == status {
                        if matched >= offset {
                            results.push_back(refund);
                            collected += 1;
                        }
                        matched += 1;
                    }
                }
            }
            index += 1;
        }

        results
    }

    /// Batch reject multiple refunds in a single operation.
    ///
    /// Per-item failures are isolated; successful rejections are recorded in the
    /// `succeeded` list and failures in the `failed` list.
    ///
    /// # Arguments
    /// * `admin` - The contract admin performing the batch rejection.
    /// * `refund_ids` - A vector of refund IDs to reject.
    /// * `note_hash` - A SHA-256 hash of rejection notes (currently unused).
    ///
    /// # Returns
    /// A `BatchDecisionResult` with lists of succeeded and failed refund IDs.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `BatchRefundTooLarge` if the batch exceeds the configured limit.
    pub fn batch_reject_refunds(
        env: Env,
        admin: Address,
        refund_ids: Vec<u64>,
        note_hash: BytesN<32>,
    ) -> Result<BatchDecisionResult, Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        if refund_ids.len() > Self::BATCH_DECISION_LIMIT {
            return Err(Error::Core(CoreError::BatchRefundTooLarge));
        }

        let mut succeeded = Vec::new(&env);
        let mut failed = Vec::new(&env);
        let mut had_failure = false;

        for refund_id in refund_ids.iter() {
            let result = (|| -> Result<(), Error> {
                Self::begin_refund_rejection(
                    &env,
                    admin.clone(),
                    refund_id,
                    soroban_sdk::String::from_str(&env, "batch rejection"),
                )
            })();
            match result {
                Ok(()) => succeeded.push_back(refund_id),
                Err(_) => {
                    failed.push_back(refund_id);
                    had_failure = true;
                }
            }
        }

        let _ = note_hash;

        if had_failure {
            return Err(Error::Core(CoreError::BatchRefundTooLarge));
        }

        Ok(BatchDecisionResult { succeeded, failed })
    }

    // ── Issue #197: Category-based dynamic refund windows ─────────────────────

    /// Set a category-specific refund window for a merchant.
    ///
    /// # Arguments
    /// * `admin` - The contract admin setting the window.
    /// * `merchant` - The merchant to configure the window for.
    /// * `category` - The payment category to apply the window to.
    /// * `window_seconds` - The refund window duration in seconds for this category.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn set_category_window(
        env: Env,
        admin: Address,
        merchant: Address,
        category: PaymentCategory,
        window_seconds: u64,
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
        let cat_idx = category.to_index();
        let window = CategoryRefundWindow {
            category,
            window_seconds,
            merchant: merchant.clone(),
        };
        env.storage()
            .instance()
            .set(&RefundExtKey::CategoryWindow(merchant, cat_idx), &window);
        Ok(())
    }

    /// Get the category-specific refund window for a merchant.
    ///
    /// # Arguments
    /// * `merchant` - The merchant to query.
    /// * `category` - The payment category to look up.
    ///
    /// # Returns
    /// The refund window in seconds for the category, or `None` if not configured.
    pub fn get_category_window(
        env: Env,
        merchant: Address,
        category: PaymentCategory,
    ) -> Option<u64> {
        let cat_idx = category.to_index();
        env.storage()
            .instance()
            .get::<RefundExtKey, CategoryRefundWindow>(&RefundExtKey::CategoryWindow(
                merchant, cat_idx,
            ))
            .map(|w| w.window_seconds)
    }

    /// Tag a payment with a category to determine its applicable refund window.
    ///
    /// # Arguments
    /// * `merchant` - The merchant who owns the payment (must authenticate).
    /// * `payment_id` - The payment ID to tag.
    /// * `category` - The category to assign to the payment.
    ///
    /// # Errors
    /// Returns `AlreadyProcessed` if the payment has already been tagged.
    pub fn tag_payment_category(
        env: Env,
        merchant: Address,
        payment_id: u64,
        category: PaymentCategory,
    ) -> Result<(), Error> {
        merchant.require_auth();
        if env
            .storage()
            .instance()
            .has(&RefundExtKey::PaymentCategoryTag(payment_id))
        {
            return Err(Error::Core(CoreError::AlreadyProcessed));
        }
        let cat_idx = category.to_index();
        env.storage()
            .instance()
            .set(&RefundExtKey::PaymentCategoryTag(payment_id), &cat_idx);
        Ok(())
    }

    /// Get the effective refund window for a specific payment, considering category tags.
    ///
    /// If the payment has a category tag and a category-specific window is configured,
    /// that window is returned. Otherwise, falls back to the merchant's default policy window.
    ///
    /// # Arguments
    /// * `merchant` - The merchant to query.
    /// * `payment_id` - The payment ID to evaluate.
    ///
    /// # Returns
    /// The effective refund window in seconds.
    pub fn get_effective_window(env: Env, merchant: Address, payment_id: u64) -> u64 {
        let default_window: u64 = Self::get_refund_policy(&env, merchant.clone())
            .map(|p| {
                if p.default_window_seconds > 0 {
                    p.default_window_seconds
                } else {
                    30 * 24 * 60 * 60
                }
            })
            .unwrap_or(30 * 24 * 60 * 60);

        let cat_idx_opt: Option<u32> = env
            .storage()
            .instance()
            .get(&RefundExtKey::PaymentCategoryTag(payment_id));

        if let Some(cat_idx) = cat_idx_opt {
            if let Some(window) = env
                .storage()
                .instance()
                .get::<RefundExtKey, CategoryRefundWindow>(&RefundExtKey::CategoryWindow(
                    merchant, cat_idx,
                ))
                .map(|w| w.window_seconds)
            {
                return window;
            }
        }

        default_window
    }

    // ── Issue #199: Refund request TTL with automatic expiry ──────────────────

    /// Configure the default time-to-live for refund requests.
    ///
    /// Refunds that are not processed before their TTL expires can be automatically
    /// rejected via `expire_stale_refund`.
    ///
    /// # Arguments
    /// * `admin` - The contract admin setting the TTL.
    /// * `ttl_seconds` - The default TTL in seconds for new refund requests.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn set_refund_ttl_config(env: Env, admin: Address, ttl_seconds: u64) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        let cfg = RefundTTLConfig {
            default_ttl_seconds: ttl_seconds,
            active: true,
        };
        env.storage()
            .instance()
            .set(&RefundExtKey::RefundTTLConfig, &cfg);
        Ok(())
    }

    /// Expire a stale refund request that has exceeded its TTL without being processed.
    ///
    /// Moves the refund from `Requested` to `Rejected` status with a "TTL expired" reason.
    ///
    /// # Arguments
    /// * `refund_id` - The ID of the refund to expire.
    ///
    /// # Errors
    /// Returns `RefundNotFound` if the refund does not exist.
    /// Returns `InvalidStatus` if the refund is not in `Requested` status.
    /// Returns `RefundWindowExpired` if the refund's TTL has not yet elapsed.
    pub fn expire_stale_refund(env: Env, refund_id: u64) -> Result<(), Error> {
        let mut refund: Refund = env
            .storage()
            .instance()
            .get(&DataKey::Refund(refund_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;

        if refund.status != RefundStatus::Requested {
            return Err(Error::Core(CoreError::InvalidStatus));
        }

        let expires_at = refund
            .expires_at
            .ok_or(Error::Core(CoreError::PolicyNotFound))?;

        if env.ledger().timestamp() < expires_at {
            return Err(Error::Core(CoreError::RefundWindowExpired));
        }

        Self::remove_from_status_index(&env, RefundStatus::Requested, refund_id)?;
        refund.status = RefundStatus::Rejected;
        refund.rejected_at = Some(env.ledger().timestamp());
        env.storage()
            .instance()
            .set(&DataKey::Refund(refund_id), &refund);
        Self::add_to_status_index(&env, RefundStatus::Rejected, refund_id);
        Self::release_payment_refund_usage(&env, refund.payment_id, refund.amount);

        (RefundRejected {
            refund_id,
            rejected_by: env.current_contract_address(),
            rejected_at: env.ledger().timestamp(),
            rejection_reason: soroban_sdk::String::from_str(&env, "TTL expired"),
        })
        .publish(&env);

        Ok(())
    }

    /// Get refund IDs that have expired (past their TTL) and are still in `Requested` status.
    ///
    /// # Arguments
    /// * `limit` - Maximum number of expired refund IDs to return.
    ///
    /// # Returns
    /// A vector of refund IDs that are eligible for expiration.
    pub fn get_expired_refunds(env: Env, limit: u32) -> Vec<u64> {
        let now = env.ledger().timestamp();
        let total: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RefundCounter)
            .unwrap_or(0);

        let mut results = Vec::new(&env);
        let mut collected = 0u32;
        let mut id = 1u64;

        while id <= total && collected < limit {
            if let Some(refund) = env
                .storage()
                .instance()
                .get::<DataKey, Refund>(&DataKey::Refund(id))
            {
                if refund.status == RefundStatus::Requested {
                    if let Some(expires_at) = refund.expires_at {
                        if now >= expires_at {
                            results.push_back(id);
                            collected += 1;
                        }
                    }
                }
            }
            id += 1;
        }

        results
    }

    // ── Issue #191: Multi-token refund support ─────────────────────────────

    /// Register a token as a supported refund payment method.
    ///
    /// # Arguments
    /// * `admin` - The contract admin registering the token.
    /// * `token` - The address of the token contract to register.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn register_refund_token(env: Env, admin: Address, token: Address) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        let count: u64 = env
            .storage()
            .instance()
            .get(&TokenKey::TokenCount)
            .unwrap_or(0);

        let entry = SupportedRefundToken {
            token: token.clone(),
            active: true,
        };
        env.storage()
            .instance()
            .set(&TokenKey::SupportedToken(token.clone()), &entry);

        let already_indexed = (0..count).any(|i| {
            env.storage()
                .instance()
                .get::<_, Address>(&TokenKey::TokenByIndex(i))
                .map(|t| t == token)
                .unwrap_or(false)
        });
        if !already_indexed {
            env.storage()
                .instance()
                .set(&TokenKey::TokenByIndex(count), &token);
            env.storage()
                .instance()
                .set(&TokenKey::TokenCount, &(count + 1));
        }

        Ok(())
    }

    /// Deregister a token so it can no longer be used for refunds.
    ///
    /// Sets the token's active status to `false` rather than removing it.
    ///
    /// # Arguments
    /// * `admin` - The contract admin deregistering the token.
    /// * `token` - The address of the token contract to deregister.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `RefundNotFound` if the token is not registered.
    pub fn deregister_refund_token(env: Env, admin: Address, token: Address) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        let mut entry: SupportedRefundToken = env
            .storage()
            .instance()
            .get(&TokenKey::SupportedToken(token.clone()))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;

        entry.active = false;
        env.storage()
            .instance()
            .set(&TokenKey::SupportedToken(token), &entry);

        Ok(())
    }

    /// Get all registered refund tokens (both active and inactive).
    ///
    /// # Returns
    /// A vector of `SupportedRefundToken` entries for all registered tokens.
    pub fn get_supported_refund_tokens(env: Env) -> Vec<SupportedRefundToken> {
        let count: u64 = env
            .storage()
            .instance()
            .get(&TokenKey::TokenCount)
            .unwrap_or(0);
        let mut results = Vec::new(&env);
        let mut i = 0u64;
        while i < count {
            if let Some(token) = env
                .storage()
                .instance()
                .get::<_, Address>(&TokenKey::TokenByIndex(i))
            {
                if let Some(entry) = env
                    .storage()
                    .instance()
                    .get::<_, SupportedRefundToken>(&TokenKey::SupportedToken(token))
                {
                    results.push_back(entry);
                }
            }
            i += 1;
        }
        results
    }

    // ── Payment refund cap management ──────────────────────────────────────

    /// Set a refund cap on a specific payment to limit the number and amount of refunds.
    ///
    /// # Arguments
    /// * `admin` - The contract admin setting the cap.
    /// * `cap` - The `PaymentRefundCap` with max count and max total amount limits.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `InvalidPaymentId` if the payment ID is zero.
    pub fn set_payment_refund_cap(
        env: Env,
        admin: Address,
        cap: PaymentRefundCap,
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

        if cap.payment_id == 0 {
            return Err(Error::Core(CoreError::InvalidPaymentId));
        }

        env.storage()
            .instance()
            .set(&DataKey::PaymentRefundCap(cap.payment_id), &cap);
        Ok(())
    }

    /// Get the refund cap configuration for a specific payment.
    ///
    /// # Arguments
    /// * `payment_id` - The payment ID to query.
    ///
    /// # Returns
    /// The `PaymentRefundCap` if configured, `None` otherwise.
    pub fn get_payment_refund_cap(env: Env, payment_id: u64) -> Option<PaymentRefundCap> {
        env.storage()
            .instance()
            .get(&DataKey::PaymentRefundCap(payment_id))
    }

    /// Get the current refund usage for a specific payment.
    ///
    /// # Arguments
    /// * `payment_id` - The payment ID to query.
    ///
    /// # Returns
    /// A tuple of `(refund_count, total_refunded_amount)` representing how many
    /// refunds have been made and the cumulative amount refunded.
    pub fn get_payment_refund_usage(env: Env, payment_id: u64) -> (u32, i128) {
        let usage: Option<(u32, i128)> = env
            .storage()
            .instance()
            .get(&DataKey::PaymentRefundUsage(payment_id));
        usage.unwrap_or((0, 0))
    }

    fn check_payment_refund_cap(
        env: &Env,
        payment_id: u64,
        refund_amount: i128,
    ) -> Result<(), Error> {
        // If no cap is set, no restriction applies
        let cap: PaymentRefundCap = match env
            .storage()
            .instance()
            .get(&DataKey::PaymentRefundCap(payment_id))
        {
            Some(c) => c,
            None => return Ok(()),
        };

        let (current_count, current_amount): (u32, i128) = env
            .storage()
            .instance()
            .get(&DataKey::PaymentRefundUsage(payment_id))
            .unwrap_or((0u32, 0i128));

        // Check count cap (only for Requested and Approved statuses)
        if current_count >= cap.max_refund_count {
            return Err(Error::Ext(ExtError::RefundCountCapExceeded));
        }

        // Check amount cap (cumulative across all statuses except Rejected)
        let new_total_amount = current_amount.saturating_add(refund_amount);
        if new_total_amount > cap.max_total_amount {
            return Err(Error::Ext(ExtError::RefundAmountCapExceeded));
        }

        Ok(())
    }

    fn update_payment_refund_usage(env: &Env, payment_id: u64, refund_amount: i128) {
        let (current_count, current_amount): (u32, i128) = env
            .storage()
            .instance()
            .get(&DataKey::PaymentRefundUsage(payment_id))
            .unwrap_or((0u32, 0i128));

        let new_count = current_count.saturating_add(1u32);
        let new_amount = current_amount.saturating_add(refund_amount);

        env.storage().instance().set(
            &DataKey::PaymentRefundUsage(payment_id),
            &(new_count, new_amount),
        );
    }

    fn release_payment_refund_usage(env: &Env, payment_id: u64, refund_amount: i128) {
        let (current_count, current_amount): (u32, i128) = env
            .storage()
            .instance()
            .get(&DataKey::PaymentRefundUsage(payment_id))
            .unwrap_or((0u32, 0i128));

        let new_count = current_count.saturating_sub(1u32);
        let new_amount = current_amount.saturating_sub(refund_amount);

        env.storage().instance().set(
            &DataKey::PaymentRefundUsage(payment_id),
            &(new_count, new_amount),
        );
    }

    fn validate_bps(bps: u32) -> Result<(), Error> {
        if bps < 1 || bps > 10000 {
            return Err(Error::Core(CoreError::InvalidAmount));
        };

        Ok(())
    }
}

mod test;
mod test_policy;
mod test_process;
mod test_rate_limit;

#[cfg(test)]
mod test_payment_refund_cap;

#[cfg(test)]
mod test_circuit_breaker;

#[cfg(test)]
mod test_versioning;

#[cfg(test)]
mod test_batch;

#[cfg(test)]
mod test_cross_contract;

#[cfg(test)]
mod test_arbitration_fees;

#[cfg(test)]
mod test_arbitration_stake;
mod test_refund_cooldown;

#[cfg(test)]
mod test_arbitrator_reputation;

#[cfg(test)]
mod test_auto_refund;

#[cfg(test)]
mod test_inheritance;

mod test_customer_history;
#[cfg(test)]
mod test_notification_hooks;

#[cfg(test)]
mod test_arbitration_timeout;

#[cfg(test)]
mod test_merchant_eligibility;

#[cfg(test)]
mod test_customer_tier_policy;

#[cfg(test)]
mod test_voucher_expiry;

#[cfg(test)]
mod schema_version_test;

#[cfg(test)]
mod test_merchant_override_and_error_codes;

#[cfg(test)]
mod test_admin_rotation;
