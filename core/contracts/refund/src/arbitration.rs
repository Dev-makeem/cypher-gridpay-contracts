//! Arbitrator staking, reputation, fees, tiers, and case lifecycle.

use crate::*;

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundEscalatedToArbitration {
    pub refund_id: u64,
    pub case_id: u64,
    pub fee_pool: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArbitrationVoteCast {
    pub case_id: u64,
    pub arbitrator: Address,
    pub vote_for_refund: bool,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArbitrationCaseDecided {
    pub case_id: u64,
    pub approved: bool,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArbitrationTimedOut {
    pub case_id: u64,
    pub default_outcome: bool,
    pub triggered_at: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArbitrationFeesDistributed {
    pub case_id: u64,
    pub per_arbitrator: i128,
    pub treasury_amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StakeDeposited {
    pub case_id: u64,
    pub staker: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StakeReturned {
    pub case_id: u64,
    pub winner: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StakeForfeited {
    pub case_id: u64,
    pub loser: Address,
    pub amount: i128,
}

#[derive(Clone)]
#[contracttype]
pub struct ArbitrationFeeConfig {
    pub arbitrator_share_bps: u32,
    pub treasury_share_bps: u32,
    pub treasury_address: Address,
    pub fee_token: Address,
    pub fee_per_case: i128,
}

#[derive(Clone)]
#[contracttype]
pub struct ArbitrationStakeConfig {
    pub token: Address,
    pub amount: i128,
    pub enabled: bool,
}

#[derive(Clone)]
#[contracttype]
pub struct ArbitrationStake {
    pub case_id: u64,
    pub staker: Address,
    pub amount: i128,
    pub deposited_at: u64,
    pub returned: bool,
}

#[derive(Clone)]
#[contracttype]
pub struct ArbitratorReputation {
    pub arbitrator: Address,
    pub total_cases: u64,
    pub majority_votes: u64,
    pub minority_votes: u64,
    pub avg_resolution_time: u64,
    pub score: i128,
    pub last_active: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArbitratorScoreUpdated {
    pub arbitrator: Address,
    pub old_score: i128,
    pub new_score: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArbitratorDeregistered {
    pub arbitrator: Address,
    pub reason: String,
}

// Issue #194: Tiered arbitration escalation
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub enum ArbitratorTier {
    Junior,
    Senior,
}

#[derive(Clone)]
#[contracttype]
pub struct ArbitrationTierConfig {
    pub junior_quorum: u32,
    pub senior_quorum: u32,
    pub escalation_timeout_seconds: u64,
}

#[derive(Clone)]
#[contracttype]
pub struct TieredArbitrator {
    pub address: Address,
    pub tier: ArbitratorTier,
    pub active: bool,
}

#[contracttype]
pub struct ArbitrationCase {
    pub case_id: u64,
    pub refund_id: u64,
    pub arbitrators: Vec<Address>,
    pub votes_for_refund: u32,
    pub votes_against_refund: u32,
    pub status: ArbitrationStatus,
    pub created_at: u64,
    pub deadline: u64,
    pub fee_pool: i128,
    pub timeout_at: u64,
    pub default_favor_customer: bool,
}

#[derive(Debug, Clone, PartialEq)]
#[contracttype]
pub enum ArbitrationStatus {
    Open,
    Decided,
    Appealed,
    Closed,
}

#[contracttype]
pub struct ArbitratorVote {
    pub arbitrator: Address,
    pub voted_for_refund: bool,
    pub reasoning_hash: BytesN<32>,
    pub voted_at: u64,
}

// Issue #198: Arbitrator auto-assignment
#[derive(Clone)]
#[contracttype]
pub struct ArbitratorAssignmentConfig {
    pub rotation_index: u32,
    pub panel_size: u32,
}

#[contractimpl]
impl RefundContract {
    /// Register a new arbitrator and initialize their reputation score.
    ///
    /// # Arguments
    /// * `admin` - The contract admin performing the registration.
    /// * `arbitrator` - The address of the arbitrator to register.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `Unauthorized` if the arbitrator is already registered.
    pub fn register_arbitrator(env: Env, admin: Address, arbitrator: Address) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }
        let mut list: Vec<Address> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitratorList)
            .unwrap_or(Vec::new(&env));
        if list.contains(&arbitrator) {
            return Err(Error::Core(CoreError::Unauthorized));
        }
        list.push_back(arbitrator.clone());
        env.storage()
            .instance()
            .set(&ArbitrationKey::ArbitratorList, &list);

        // Initialize reputation for new arbitrator
        let reputation = ArbitratorReputation {
            arbitrator: arbitrator.clone(),
            total_cases: 0,
            majority_votes: 0,
            minority_votes: 0,
            avg_resolution_time: 0,
            score: 100, // Starting score
            last_active: env.ledger().timestamp(),
        };
        env.storage().instance().set(
            &ArbitrationKey::ArbitratorReputation(arbitrator),
            &reputation,
        );

        Ok(())
    }

    /// Manually assign an arbitrator to an open arbitration case.
    ///
    /// # Arguments
    /// * `admin` - The contract admin performing the assignment.
    /// * `case_id` - The ID of the arbitration case.
    /// * `arbitrator` - The address of the arbitrator to assign.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `RefundNotFound` if the case does not exist.
    /// Returns `InvalidStatus` if the case is not in `Open` status.
    /// Returns `NotArbitrator` if the address is not a registered arbitrator.
    pub fn assign_arbitrator(
        env: Env,
        admin: Address,
        case_id: u64,
        arbitrator: Address,
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

        let mut case: ArbitrationCase = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationCase(case_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;
        if case.status != ArbitrationStatus::Open {
            return Err(Error::Core(CoreError::InvalidStatus));
        }

        let refund: Refund = env
            .storage()
            .instance()
            .get(&DataKey::Refund(case.refund_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;

        if let Some(denied_by) = refund.rejected_by.clone() {
            if arbitrator == denied_by {
                // denied_by cannot arbitrate their own denial decision
                return Err(Error::Core(CoreError::Unauthorized));
            }
        }

        let arbitrators: Vec<Address> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitratorList)
            .unwrap_or(Vec::new(&env));
        if !arbitrators.contains(&arbitrator) {
            return Err(Error::Core(CoreError::NotArbitrator));
        }

        if !case.arbitrators.contains(&arbitrator) {
            case.arbitrators.push_back(arbitrator);
            env.storage()
                .instance()
                .set(&ArbitrationKey::ArbitrationCase(case_id), &case);
        }

        Ok(())
    }

    /// Escalate a rejected refund to the arbitration panel for review.
    ///
    /// Transfers the arbitration fee from the caller and, if staking is enabled,
    /// also transfers the required stake. Creates an `Open` arbitration case
    /// assigned to all registered arbitrators.
    ///
    /// # Arguments
    /// * `caller` - The customer or party escalating the dispute.
    /// * `refund_id` - The ID of the rejected refund to escalate.
    /// * `fee_token` - The token address used to pay the arbitration fee.
    /// * `fee_amount` - The amount of the arbitration fee.
    ///
    /// # Returns
    /// The newly created arbitration case ID.
    ///
    /// # Errors
    /// Returns `InvalidStatus` if the refund is not in `Rejected` or `PendingAppeal` status.
    /// Returns `InvalidAmount` if `fee_amount` is not positive.
    /// Returns `QuorumNotReached` if fewer than 3 arbitrators are registered.
    pub fn escalate_to_arbitration(
        env: Env,
        caller: Address,
        refund_id: u64,
        fee_token: Address,
        fee_amount: i128,
    ) -> Result<u64, Error> {
        caller.require_auth();

        let refund: Refund = env
            .storage()
            .instance()
            .get(&DataKey::Refund(refund_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;
        // Matches `file_appeal`'s dual-status check: a rejection sits in
        // `PendingAppeal` during the appeal window before finalizing to
        // `Rejected`, and arbitration must remain reachable during that
        // window, not only after it closes.
        if refund.status != RefundStatus::Rejected && refund.status != RefundStatus::PendingAppeal {
            return Err(Error::Core(CoreError::InvalidStatus));
        }

        // Uniqueness guard: prevent the same refund from being escalated into
        // multiple parallel arbitration cases.  If a case already exists for
        // this refund_id, reject the duplicate attempt.
        if env
            .storage()
            .instance()
            .has(&ArbitrationKey::CaseByRefund(refund_id))
        {
            return Err(Error::Core(CoreError::AlreadyProcessed));
        }
        if fee_amount <= 0 {
            return Err(Error::Core(CoreError::InvalidAmount));
        }

        let fee_config: Option<ArbitrationFeeConfig> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationFeeConfig);
        if let Some(ref config) = fee_config {
            if config.fee_per_case > 0 && fee_amount < config.fee_per_case {
                return Err(Error::Core(CoreError::InvalidAmount));
            }
        }

        let counter: u64 = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationCounter)
            .unwrap_or(0);
        let case_id = counter + 1;

        let arbitrators = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitratorList)
            .unwrap_or(Vec::new(&env));
        if arbitrators.len() < 3 {
            return Err(Error::Core(CoreError::QuorumNotReached));
        }

        // Handle staking if enabled
        let stake_config: Option<ArbitrationStakeConfig> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationStakeConfig);

        if let Some(config) = stake_config {
            if config.enabled {
                if config.amount <= 0 {
                    return Err(Error::Core(CoreError::InvalidAmount));
                }

                // Transfer stake from caller to contract
                let stake_token_client = token::Client::new(&env, &config.token);
                stake_token_client.transfer(
                    &caller,
                    &env.current_contract_address(),
                    &config.amount,
                );

                // Record the stake
                let stake = ArbitrationStake {
                    case_id,
                    staker: caller.clone(),
                    amount: config.amount,
                    deposited_at: env.ledger().timestamp(),
                    returned: false,
                };
                env.storage()
                    .instance()
                    .set(&ArbitrationKey::ArbitrationStake(case_id), &stake);

                StakeDeposited {
                    case_id,
                    staker: caller.clone(),
                    amount: config.amount,
                }
                .publish(&env);
            }
        }

        env.storage()
            .instance()
            .set(&DataKey::PoolToken(case_id), &fee_token.clone());
        let token_client = token::Client::new(&env, &fee_token);
        token_client.transfer(&caller, &env.current_contract_address(), &fee_amount);

        let now = env.ledger().timestamp();
        let timeout_secs: u64 = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationTimeoutConfig)
            .unwrap_or(86400 * 14); // default 14 days
        let case = ArbitrationCase {
            case_id,
            refund_id,
            arbitrators: arbitrators.clone(),
            votes_for_refund: 0,
            votes_against_refund: 0,
            status: ArbitrationStatus::Open,
            created_at: now,
            deadline: now + timeout_secs,
            fee_pool: fee_amount,
            timeout_at: now + timeout_secs,
            default_favor_customer: true,
        };

        env.storage()
            .instance()
            .set(&ArbitrationKey::ArbitrationCase(case_id), &case);
        env.storage()
            .instance()
            .set(&ArbitrationKey::ArbitrationCounter, &case_id);
        // Record the reverse mapping so subsequent calls can detect the duplicate.
        env.storage()
            .instance()
            .set(&ArbitrationKey::CaseByRefund(refund_id), &case_id);

        RefundEscalatedToArbitration {
            refund_id,
            case_id,
            fee_pool: fee_amount,
        }
        .publish(&env);

        // Issue #144: Invoke notification hooks for Escalated event
        Self::invoke_hooks(&env, RefundEventType::Escalated, refund_id);

        Ok(case_id)
    }

    /// Cast a vote on an open arbitration case.
    ///
    /// # Arguments
    /// * `arbitrator` - The arbitrator casting the vote.
    /// * `case_id` - The ID of the arbitration case.
    /// * `vote_for_refund` - `true` to vote in favor of the refund, `false` to vote against.
    /// * `reasoning_hash` - SHA-256 hash of the arbitrator's reasoning document.
    ///
    /// # Errors
    /// Returns `InvalidStatus` if the case is not open or the deadline has passed.
    /// Returns `NotArbitrator` if the caller is not an assigned arbitrator for this case.
    /// Returns `AlreadyProcessed` if the arbitrator has already voted on this case.
    /// Returns `Unauthorized` if the arbitrator is the merchant or customer of the refund.
    pub fn cast_arbitration_vote(
        env: Env,
        arbitrator: Address,
        case_id: u64,
        vote_for_refund: bool,
        reasoning_hash: BytesN<32>,
    ) -> Result<(), Error> {
        arbitrator.require_auth();

        let mut case: ArbitrationCase = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationCase(case_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;
        if case.status != ArbitrationStatus::Open {
            return Err(Error::Core(CoreError::InvalidStatus));
        }
        if env.ledger().timestamp() > case.deadline {
            return Err(Error::Core(CoreError::InvalidStatus));
        }
        if !case.arbitrators.contains(&arbitrator) {
            return Err(Error::Core(CoreError::NotArbitrator));
        }
        if env
            .storage()
            .instance()
            .has(&ArbitrationKey::ArbitratorVote(case_id, arbitrator.clone()))
        {
            return Err(Error::Core(CoreError::AlreadyProcessed));
        }

        let refund: Refund = env
            .storage()
            .instance()
            .get(&DataKey::Refund(case.refund_id))
            .unwrap();
        if arbitrator == refund.merchant || arbitrator == refund.customer {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        let vote = ArbitratorVote {
            arbitrator: arbitrator.clone(),
            voted_for_refund: vote_for_refund,
            reasoning_hash,
            voted_at: env.ledger().timestamp(),
        };
        env.storage().instance().set(
            &ArbitrationKey::ArbitratorVote(case_id, arbitrator.clone()),
            &vote,
        );

        let mut voted: Vec<Address> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitratorsVoted(case_id))
            .unwrap_or_else(|| Vec::new(&env));
        if !voted.contains(&arbitrator) {
            voted.push_back(arbitrator.clone());
            env.storage()
                .instance()
                .set(&ArbitrationKey::ArbitratorsVoted(case_id), &voted);
        }

        if vote_for_refund {
            case.votes_for_refund += 1;
        } else {
            case.votes_against_refund += 1;
        }
        env.storage()
            .instance()
            .set(&ArbitrationKey::ArbitrationCase(case_id), &case);

        ArbitrationVoteCast {
            case_id,
            arbitrator,
            vote_for_refund,
        }
        .publish(&env);

        Ok(())
    }

    /// Close an arbitration case once quorum has been reached and tally votes.
    ///
    /// Requires at least 3 total votes. The refund is approved if the majority
    /// voted in favor, otherwise it is rejected. Also updates the arbitrator
    /// reputation scores based on majority/minority alignment.
    ///
    /// # Arguments
    /// * `case_id` - The ID of the arbitration case to close.
    ///
    /// # Errors
    /// Returns `RefundNotFound` if the case does not exist.
    /// Returns `InvalidStatus` if the case is not open or quorum (3 votes) has not been reached.
    pub fn close_arbitration_case(env: Env, case_id: u64) -> Result<(), Error> {
        let mut case: ArbitrationCase = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationCase(case_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;
        if case.status != ArbitrationStatus::Open {
            return Err(Error::Core(CoreError::InvalidStatus));
        }

        let total_votes = case.votes_for_refund + case.votes_against_refund;
        if total_votes < 3 {
            return Err(Error::Core(CoreError::InvalidStatus));
        } // quorum

        let approved = case.votes_for_refund > case.votes_against_refund;

        case.status = ArbitrationStatus::Decided;
        env.storage()
            .instance()
            .set(&ArbitrationKey::ArbitrationCase(case_id), &case);

        // Update refund status based on the arbitration outcome
        let mut refund: Refund = env
            .storage()
            .instance()
            .get(&DataKey::Refund(case.refund_id))
            .unwrap();
        if approved {
            refund.status = RefundStatus::Approved;
            env.storage()
                .instance()
                .set(&DataKey::Refund(case.refund_id), &refund);

            (RefundApproved {
                refund_id: case.refund_id,
                payment_id: refund.payment_id,
                amount: refund.amount,
                approved_by: env.current_contract_address(),
                approved_at: env.ledger().timestamp(),
            })
            .publish(&env);
            Self::invoke_hooks(&env, RefundEventType::Approved, case.refund_id);
        } else if refund.status == RefundStatus::PendingAppeal {
            // The arbitration panel upheld the rejection, so it's final now
            // — no need to wait out the rest of the appeal window.
            Self::remove_from_status_index(&env, RefundStatus::PendingAppeal, refund.id)?;
            refund.status = RefundStatus::Rejected;
            refund.rejected_at = Some(env.ledger().timestamp());
            env.storage()
                .instance()
                .set(&DataKey::Refund(case.refund_id), &refund);
            Self::add_to_status_index(&env, RefundStatus::Rejected, refund.id);
            Self::release_payment_refund_usage(&env, refund.payment_id, refund.amount);

            (RefundRejected {
                refund_id: case.refund_id,
                rejected_by: env.current_contract_address(),
                rejected_at: refund.rejected_at.unwrap(),
                rejection_reason: soroban_sdk::String::from_str(
                    &env,
                    "arbitration case decided against refund",
                ),
            })
            .publish(&env);
            Self::invoke_hooks(&env, RefundEventType::Rejected, case.refund_id);
        }

        // Distribute fees according to configuration
        let num_voters = total_votes as i128;

        // Get all arbitrators who voted (needed for both fee distribution and reputation updates)
        let all_voters: Vec<Address> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitratorsVoted(case_id))
            .unwrap_or_else(|| Vec::new(&env));

        if num_voters > 0 {
            let pool_token: Address = env
                .storage()
                .instance()
                .get(&DataKey::PoolToken(case_id))
                .unwrap();
            let token_client = token::Client::new(&env, &pool_token);

            // Get fee configuration
            let fee_config: Option<ArbitrationFeeConfig> = env
                .storage()
                .instance()
                .get(&ArbitrationKey::ArbitrationFeeConfig);

            let (arbitrator_share, treasury_share, treasury_address) = if let Some(ref config) =
                fee_config
            {
                // Calculate shares based on basis points
                let arbitrator_amount =
                    (case.fee_pool * config.arbitrator_share_bps as i128) / 10000;
                let treasury_amount = (case.fee_pool * config.treasury_share_bps as i128) / 10000;
                (
                    arbitrator_amount,
                    treasury_amount,
                    Some(config.treasury_address.clone()),
                )
            } else {
                // Default: 100% to arbitrators, 0% to treasury
                (case.fee_pool, 0, None)
            };

            // Filter to only majority voters
            let mut majority_voters = Vec::new(&env);
            for voter in all_voters.iter() {
                let vote: ArbitratorVote = env
                    .storage()
                    .instance()
                    .get(&ArbitrationKey::ArbitratorVote(case_id, voter.clone()))
                    .unwrap();

                // Check if this voter was in the majority
                let in_majority = if approved {
                    vote.voted_for_refund
                } else {
                    !vote.voted_for_refund
                };

                if in_majority {
                    majority_voters.push_back(voter.clone());
                }
            }

            // Distribute arbitrator share equally among majority voters
            let per_arbitrator = if majority_voters.len() > 0 {
                arbitrator_share / (majority_voters.len() as i128)
            } else {
                0
            };

            for arbitrator in majority_voters.iter() {
                token_client.transfer(&env.current_contract_address(), arbitrator, &per_arbitrator);
            }

            // Transfer treasury share if configured
            if treasury_share > 0 {
                if let Some(treasury_addr) = treasury_address {
                    token_client.transfer(
                        &env.current_contract_address(),
                        &treasury_addr,
                        &treasury_share,
                    );

                    // Accumulate treasury fees
                    let accumulated: i128 = env
                        .storage()
                        .instance()
                        .get(&ArbitrationKey::AccumulatedTreasuryFees)
                        .unwrap_or(0);
                    env.storage().instance().set(
                        &ArbitrationKey::AccumulatedTreasuryFees,
                        &(accumulated + treasury_share),
                    );
                }
            }

            ArbitrationFeesDistributed {
                case_id,
                per_arbitrator,
                treasury_amount: treasury_share,
            }
            .publish(&env);
        }

        // Handle stake return or forfeiture
        Self::settle_arbitration_stake(&env, case_id, approved);

        // Update arbitrator reputations
        let case_duration = env.ledger().timestamp() - case.created_at;
        let current_time = env.ledger().timestamp();

        for voter in all_voters.iter() {
            let vote: ArbitratorVote = env
                .storage()
                .instance()
                .get(&ArbitrationKey::ArbitratorVote(case_id, voter.clone()))
                .unwrap();

            // Check if this voter was in the majority
            let in_majority = if approved {
                vote.voted_for_refund
            } else {
                !vote.voted_for_refund
            };

            // Get current reputation
            let mut reputation: ArbitratorReputation = env
                .storage()
                .instance()
                .get(&ArbitrationKey::ArbitratorReputation(voter.clone()))
                .unwrap_or(ArbitratorReputation {
                    arbitrator: voter.clone(),
                    total_cases: 0,
                    majority_votes: 0,
                    minority_votes: 0,
                    avg_resolution_time: 0,
                    score: 100,
                    last_active: current_time,
                });

            let old_score = reputation.score;

            // Update vote counts
            reputation.total_cases += 1;
            if in_majority {
                reputation.majority_votes += 1;
                // Increase score for majority vote (e.g., +10 points)
                reputation.score += 10;
            } else {
                reputation.minority_votes += 1;
                // Decrease score for minority vote (e.g., -5 points)
                reputation.score -= 5;
            }

            // Update average resolution time
            if reputation.total_cases == 1 {
                reputation.avg_resolution_time = case_duration;
            } else {
                // Calculate weighted average
                let total_time = reputation.avg_resolution_time * (reputation.total_cases - 1);
                reputation.avg_resolution_time =
                    (total_time + case_duration) / reputation.total_cases;
            }

            // Update last active timestamp
            reputation.last_active = current_time;

            // Store updated reputation
            env.storage().instance().set(
                &ArbitrationKey::ArbitratorReputation(voter.clone()),
                &reputation,
            );

            // Emit score update event
            ArbitratorScoreUpdated {
                arbitrator: voter.clone(),
                old_score,
                new_score: reputation.score,
            }
            .publish(&env);
        }

        ArbitrationCaseDecided { case_id, approved }.publish(&env);

        Ok(())
    }

    /// Returns a case's stake to the staker if they won, or forfeits it to the
    /// treasury if they lost. Shared by both the quorum-vote resolution path
    /// (`close_arbitration_case`) and the timeout-default path
    /// (`trigger_arbitration_timeout`) so a staker can never permanently lose
    /// their stake just because a case was resolved by timeout instead of vote.
    ///
    /// * `approved` - whether the arbitration outcome favors the customer
    ///   (refund approved). The staker is whichever party escalated the case;
    ///   compare their role against the outcome to decide if they won.
    pub(crate) fn settle_arbitration_stake(env: &Env, case_id: u64, approved: bool) {
        let stake_opt: Option<ArbitrationStake> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationStake(case_id));

        let mut stake = match stake_opt {
            Some(s) if !s.returned => s,
            _ => return,
        };

        let case: ArbitrationCase = match env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationCase(case_id))
        {
            Some(c) => c,
            None => return,
        };

        let refund: Refund = match env
            .storage()
            .instance()
            .get(&DataKey::Refund(case.refund_id))
        {
            Some(r) => r,
            None => return,
        };

        let stake_config: Option<ArbitrationStakeConfig> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationStakeConfig);

        let stake_cfg = match stake_config {
            Some(cfg) => cfg,
            None => return,
        };

        let stake_token_client = token::Client::new(env, &stake_cfg.token);

        // Get treasury address from fee config
        let fee_config: Option<ArbitrationFeeConfig> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationFeeConfig);

        // Staker wins when the outcome aligns with their side of the dispute.
        let staker_won = (stake.staker == refund.customer && approved)
            || (stake.staker == refund.merchant && !approved);

        if staker_won {
            // Return stake to staker
            stake_token_client.transfer(
                &env.current_contract_address(),
                &stake.staker,
                &stake.amount,
            );

            StakeReturned {
                case_id,
                winner: stake.staker.clone(),
                amount: stake.amount,
            }
            .publish(env);
        } else {
            // Forfeit stake to treasury (use fee config treasury or staker as fallback)
            let treasury_addr = if let Some(fee_cfg) = fee_config {
                fee_cfg.treasury_address
            } else {
                // Fallback: return to staker if no treasury configured
                stake.staker.clone()
            };

            stake_token_client.transfer(
                &env.current_contract_address(),
                &treasury_addr,
                &stake.amount,
            );

            StakeForfeited {
                case_id,
                loser: stake.staker.clone(),
                amount: stake.amount,
            }
            .publish(env);
        }

        // Mark stake as returned
        stake.returned = true;
        env.storage()
            .instance()
            .set(&ArbitrationKey::ArbitrationStake(case_id), &stake);
    }

    /// Set the default timeout duration for arbitration cases.
    ///
    /// # Arguments
    /// * `admin` - The contract admin setting the timeout.
    /// * `timeout_seconds` - The timeout duration in seconds (default is 14 days).
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn set_arbitration_timeout(
        env: Env,
        admin: Address,
        timeout_seconds: u64,
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
            .set(&ArbitrationKey::ArbitrationTimeoutConfig, &timeout_seconds);
        Ok(())
    }

    /// Get the current arbitration timeout configuration in seconds.
    ///
    /// # Returns
    /// The timeout duration in seconds. Defaults to 1,209,600 (14 days) if not configured.
    pub fn get_arbitration_timeout_config(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationTimeoutConfig)
            .unwrap_or(86400 * 14)
    }

    /// Trigger a timeout on an arbitration case that has exceeded its deadline.
    ///
    /// If quorum has not been reached and the timeout has elapsed, the case is
    /// resolved with the default outcome (typically favoring the customer).
    ///
    /// # Arguments
    /// * `case_id` - The ID of the arbitration case to time out.
    ///
    /// # Errors
    /// Returns `RefundNotFound` if the case does not exist.
    /// Returns `InvalidStatus` if the case is not open or quorum was already reached.
    /// Returns `CaseNotTimedOut` if the timeout has not yet elapsed.
    pub fn trigger_arbitration_timeout(env: Env, case_id: u64) -> Result<(), Error> {
        let mut case: ArbitrationCase = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationCase(case_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;

        if case.status != ArbitrationStatus::Open {
            return Err(Error::Core(CoreError::InvalidStatus));
        }

        // Block if quorum already reached
        let total_votes = case.votes_for_refund + case.votes_against_refund;
        if total_votes >= 3 {
            return Err(Error::Core(CoreError::QuorumNotReached));
        }

        if env.ledger().timestamp() < case.timeout_at {
            return Err(Error::Core(CoreError::CaseNotTimedOut));
        }

        let approved = case.default_favor_customer;
        case.status = ArbitrationStatus::Decided;
        env.storage()
            .instance()
            .set(&ArbitrationKey::ArbitrationCase(case_id), &case);

        if approved {
            let mut refund: Refund = env
                .storage()
                .instance()
                .get(&DataKey::Refund(case.refund_id))
                .unwrap();
            refund.status = RefundStatus::Approved;
            env.storage()
                .instance()
                .set(&DataKey::Refund(case.refund_id), &refund);

            (RefundApproved {
                refund_id: case.refund_id,
                payment_id: refund.payment_id,
                amount: refund.amount,
                approved_by: env.current_contract_address(),
                approved_at: env.ledger().timestamp(),
            })
            .publish(&env);
            Self::invoke_hooks(&env, RefundEventType::Approved, case.refund_id);
        }

        // Handle stake return or forfeiture, same as the quorum-vote path —
        // a case resolved by timeout must not leave the staker's funds stuck.
        Self::settle_arbitration_stake(&env, case_id, approved);

        ArbitrationTimedOut {
            case_id,
            default_outcome: approved,
            triggered_at: env.ledger().timestamp(),
        }
        .publish(&env);

        ArbitrationCaseDecided { case_id, approved }.publish(&env);

        Ok(())
    }

    /// Get the reputation information for a specific arbitrator
    pub fn get_arbitrator_reputation(
        env: Env,
        arbitrator: Address,
    ) -> Option<ArbitratorReputation> {
        env.storage()
            .instance()
            .get(&ArbitrationKey::ArbitratorReputation(arbitrator))
    }

    /// Get the top arbitrators sorted by score (highest first)
    /// Returns up to `limit` arbitrators
    pub fn get_top_arbitrators(env: Env, limit: u32) -> Vec<ArbitratorReputation> {
        let mut results = Vec::new(&env);

        // Get all arbitrators from the arbitrator list
        let arbitrators: Vec<Address> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitratorList)
            .unwrap_or(Vec::new(&env));

        if arbitrators.len() == 0 {
            return results;
        }

        // Collect all reputations
        let mut reputations = Vec::new(&env);
        for arbitrator in arbitrators.iter() {
            if let Some(reputation) = env
                .storage()
                .instance()
                .get::<ArbitrationKey, ArbitratorReputation>(&ArbitrationKey::ArbitratorReputation(
                    arbitrator.clone(),
                ))
            {
                reputations.push_back(reputation);
            }
        }

        // Sort by score (descending) using bubble sort
        // Note: This is inefficient for large lists, but works for small arbitrator sets
        let len = reputations.len();
        for i in 0..len {
            for j in 0..(len - i - 1) {
                let rep_j = reputations.get(j).unwrap();
                let rep_j_plus_1 = reputations.get(j + 1).unwrap();

                if rep_j.score < rep_j_plus_1.score {
                    // Swap
                    let temp = rep_j_plus_1.clone();
                    reputations.set(j + 1, rep_j.clone());
                    reputations.set(j, temp);
                }
            }
        }

        // Return top `limit` arbitrators
        let count = core::cmp::min(limit as u32, reputations.len());
        for i in 0..count {
            results.push_back(reputations.get(i).unwrap());
        }

        results
    }

    /// Deregister all arbitrators with a score below the minimum threshold
    /// Requires admin authorization
    /// Returns the count of arbitrators removed
    pub fn deregister_low_performers(
        env: Env,
        admin: Address,
        min_score: i128,
    ) -> Result<u32, Error> {
        admin.require_auth();

        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        if min_score < 0 {
            return Err(Error::Ext(ExtError::InvalidScoreThreshold));
        }

        let mut arbitrators: Vec<Address> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitratorList)
            .unwrap_or(Vec::new(&env));

        let mut removed_count: u32 = 0;
        let mut new_arbitrators = Vec::new(&env);

        for arbitrator in arbitrators.iter() {
            let reputation: Option<ArbitratorReputation> = env
                .storage()
                .instance()
                .get(&ArbitrationKey::ArbitratorReputation(arbitrator.clone()));

            let should_remove = if let Some(rep) = reputation {
                rep.score < min_score
            } else {
                false
            };

            if should_remove {
                // Remove reputation data
                env.storage()
                    .instance()
                    .remove(&ArbitrationKey::ArbitratorReputation(arbitrator.clone()));

                // Emit deregistration event
                ArbitratorDeregistered {
                    arbitrator: arbitrator.clone(),
                    reason: String::from_str(&env, "Low performance score"),
                }
                .publish(&env);

                removed_count += 1;
            } else {
                // Keep this arbitrator
                new_arbitrators.push_back(arbitrator.clone());
            }
        }

        // Update the arbitrator list
        env.storage()
            .instance()
            .set(&ArbitrationKey::ArbitratorList, &new_arbitrators);

        Ok(removed_count)
    }

    /// Retrieve the details of an arbitration case by its ID.
    ///
    /// # Arguments
    /// * `case_id` - The ID of the arbitration case to retrieve.
    ///
    /// # Returns
    /// The `ArbitrationCase` details.
    ///
    /// # Errors
    /// Returns `RefundNotFound` if the case does not exist.
    pub fn get_arbitration_case(env: Env, case_id: u64) -> Result<ArbitrationCase, Error> {
        env.storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationCase(case_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))
    }

    /// Set the arbitration fee configuration
    /// Requires admin authorization
    /// arbitrator_share_bps + treasury_share_bps must equal 10000 (100%)
    pub fn set_arbitration_fee_config(
        env: Env,
        admin: Address,
        config: ArbitrationFeeConfig,
    ) -> Result<(), Error> {
        admin.require_auth();

        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        // Validate that shares add up to 10000 (100%)
        if config.arbitrator_share_bps + config.treasury_share_bps != 10000 {
            return Err(Error::Core(CoreError::InvalidFeeConfig));
        }

        env.storage()
            .instance()
            .set(&ArbitrationKey::ArbitrationFeeConfig, &config);

        Ok(())
    }

    /// Get the current arbitration fee configuration
    pub fn get_arbitration_fee_config(env: Env) -> Option<ArbitrationFeeConfig> {
        env.storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationFeeConfig)
    }

    /// Get the accumulated treasury fees from arbitration cases
    pub fn get_accumulated_arbitration_fees(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&ArbitrationKey::AccumulatedTreasuryFees)
            .unwrap_or(0)
    }

    /// Set the arbitration stake configuration
    /// Requires admin authorization
    pub fn set_arbitration_stake_config(
        env: Env,
        admin: Address,
        config: ArbitrationStakeConfig,
    ) -> Result<(), Error> {
        admin.require_auth();

        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        // Validate stake amount if enabled
        if config.enabled && config.amount <= 0 {
            return Err(Error::Core(CoreError::InvalidAmount));
        }

        env.storage()
            .instance()
            .set(&ArbitrationKey::ArbitrationStakeConfig, &config);

        Ok(())
    }

    /// Get the current arbitration stake configuration
    pub fn get_arbitration_stake_config(env: Env) -> Option<ArbitrationStakeConfig> {
        env.storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationStakeConfig)
    }

    /// Get the stake information for a specific arbitration case
    pub fn get_arbitration_stake(env: Env, case_id: u64) -> Option<ArbitrationStake> {
        env.storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationStake(case_id))
    }

    /// Configure the round-robin auto-assignment of arbitrators to cases.
    ///
    /// # Arguments
    /// * `admin` - The contract admin configuring the assignment.
    /// * `panel_size` - The number of arbitrators to assign per case.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `ArbitratorNotFound` if no arbitrators are registered or panel size exceeds the count.
    pub fn configure_auto_assignment(
        env: Env,
        admin: Address,
        panel_size: u32,
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

        let arbitrators: Vec<Address> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitratorList)
            .unwrap_or(Vec::new(&env));

        if arbitrators.is_empty() {
            return Err(Error::Ext(ExtError::ArbitratorNotFound));
        }

        if panel_size as u32 > arbitrators.len() {
            return Err(Error::Ext(ExtError::ArbitratorNotFound));
        }

        let config = ArbitratorAssignmentConfig {
            rotation_index: 0,
            panel_size,
        };
        env.storage()
            .instance()
            .set(&RefundExtKey::AssignmentConfig, &config);
        Ok(())
    }

    /// Automatically assign a panel of arbitrators to a case using round-robin rotation.
    ///
    /// Advances the rotation index after assignment to ensure even distribution of cases.
    ///
    /// # Arguments
    /// * `case_id` - The ID of the arbitration case (currently reserved for future use).
    ///
    /// # Returns
    /// A vector of assigned arbitrator addresses.
    ///
    /// # Errors
    /// Returns `PolicyNotFound` if auto-assignment has not been configured.
    /// Returns `ArbitratorNotFound` if no arbitrators are registered.
    pub fn auto_assign_arbitrators(env: Env, case_id: u64) -> Result<Vec<Address>, Error> {
        let mut config: ArbitratorAssignmentConfig = env
            .storage()
            .instance()
            .get(&RefundExtKey::AssignmentConfig)
            .ok_or(Error::Core(CoreError::PolicyNotFound))?;

        let arbitrators: Vec<Address> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitratorList)
            .unwrap_or(Vec::new(&env));

        if arbitrators.is_empty() {
            return Err(Error::Ext(ExtError::ArbitratorNotFound));
        }

        let total = arbitrators.len() as u32;
        if config.panel_size > total {
            return Err(Error::Ext(ExtError::ArbitratorNotFound));
        }

        let mut panel = Vec::new(&env);
        for i in 0..config.panel_size {
            let idx = ((config.rotation_index + i) % total) as u32;
            panel.push_back(arbitrators.get(idx).unwrap());
        }

        config.rotation_index = (config.rotation_index + config.panel_size) % total;
        env.storage()
            .instance()
            .set(&RefundExtKey::AssignmentConfig, &config);

        let mut case: ArbitrationCase = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationCase(case_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;
        case.arbitrators = panel.clone();
        env.storage()
            .instance()
            .set(&ArbitrationKey::ArbitrationCase(case_id), &case);

        Ok(panel)
    }

    /// Preview the next arbitrators that would be assigned using round-robin rotation.
    ///
    /// Does not advance the rotation index.
    ///
    /// # Arguments
    /// * `count` - The number of arbitrators to preview.
    ///
    /// # Returns
    /// A vector of the next arbitrator addresses in rotation order.
    pub fn get_next_arbitrators(env: Env, count: u32) -> Vec<Address> {
        let config: ArbitratorAssignmentConfig = match env
            .storage()
            .instance()
            .get(&RefundExtKey::AssignmentConfig)
        {
            Some(c) => c,
            None => return Vec::new(&env),
        };

        let arbitrators: Vec<Address> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitratorList)
            .unwrap_or(Vec::new(&env));

        let total = arbitrators.len() as u32;
        if total == 0 || count == 0 {
            return Vec::new(&env);
        }

        let n = if count > total { total } else { count };
        let mut result = Vec::new(&env);
        for i in 0..n {
            let idx = ((config.rotation_index + i) % total) as u32;
            result.push_back(arbitrators.get(idx).unwrap());
        }
        result
    }

    /// Reset the round-robin rotation index back to the beginning.
    ///
    /// # Arguments
    /// * `admin` - The contract admin resetting the index.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    /// Returns `PolicyNotFound` if auto-assignment has not been configured.
    pub fn reset_rotation_index(env: Env, admin: Address) -> Result<(), Error> {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::Core(CoreError::Unauthorized))?;
        if admin != stored_admin {
            return Err(Error::Core(CoreError::Unauthorized));
        }

        let mut config: ArbitratorAssignmentConfig = env
            .storage()
            .instance()
            .get(&RefundExtKey::AssignmentConfig)
            .ok_or(Error::Core(CoreError::PolicyNotFound))?;

        config.rotation_index = 0;
        env.storage()
            .instance()
            .set(&RefundExtKey::AssignmentConfig, &config);
        Ok(())
    }

    /// Add an arbitrator to the senior arbitrator list for tiered escalation.
    ///
    /// Senior arbitrators handle cases that have been escalated from the junior panel.
    ///
    /// # Arguments
    /// * `admin` - The contract admin adding the senior arbitrator.
    /// * `arbitrator` - The address of the arbitrator to add.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn add_senior_arbitrator(
        env: Env,
        admin: Address,
        arbitrator: Address,
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

        let mut list: Vec<Address> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::SeniorArbitratorList)
            .unwrap_or(Vec::new(&env));
        if !list.contains(&arbitrator) {
            list.push_back(arbitrator);
            env.storage()
                .instance()
                .set(&ArbitrationKey::SeniorArbitratorList, &list);
        }
        Ok(())
    }

    /// Set the configuration for tiered arbitration escalation.
    ///
    /// # Arguments
    /// * `admin` - The contract admin setting the configuration.
    /// * `config` - The `ArbitrationTierConfig` with escalation timeout settings.
    ///
    /// # Errors
    /// Returns `Unauthorized` if the caller is not the contract admin.
    pub fn set_arbitration_tier_config(
        env: Env,
        admin: Address,
        config: ArbitrationTierConfig,
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
            .set(&ArbitrationKey::ArbitrationTierConfig, &config);
        Ok(())
    }

    /// Escalate an arbitration case from the junior panel to the senior arbitrator panel.
    ///
    /// The case must be in `Open` status and must have exceeded the escalation timeout.
    /// Resets all votes and reassigns the case to senior arbitrators.
    ///
    /// # Arguments
    /// * `case_id` - The ID of the arbitration case to escalate.
    ///
    /// # Errors
    /// Returns `CaseAlreadyEscalated` if the case has already been escalated.
    /// Returns `RefundNotFound` if the case does not exist.
    /// Returns `InvalidStatus` if the case is not open.
    /// Returns `CaseNotTimedOut` if the escalation timeout has not elapsed.
    /// Returns `ArbitratorNotFound` if no senior arbitrators are registered.
    pub fn escalate_arbitration_case(env: Env, case_id: u64) -> Result<(), Error> {
        if env
            .storage()
            .instance()
            .has(&ArbitrationKey::CaseEscalated(case_id))
        {
            return Err(Error::Ext(ExtError::CaseAlreadyEscalated));
        }

        let mut case: ArbitrationCase = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationCase(case_id))
            .ok_or(Error::Core(CoreError::RefundNotFound))?;

        if case.status != ArbitrationStatus::Open {
            return Err(Error::Core(CoreError::InvalidStatus));
        }

        let config: ArbitrationTierConfig = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitrationTierConfig)
            .ok_or(Error::Core(CoreError::CaseNotTimedOut))?;

        if env.ledger().timestamp()
            < case
                .created_at
                .saturating_add(config.escalation_timeout_seconds)
        {
            return Err(Error::Core(CoreError::CaseNotTimedOut));
        }

        let senior_list: Vec<Address> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::SeniorArbitratorList)
            .unwrap_or(Vec::new(&env));

        if senior_list.len() == 0 {
            return Err(Error::Ext(ExtError::ArbitratorNotFound));
        }

        Self::clear_arbitration_votes(&env, case_id);

        case.arbitrators = senior_list;
        case.votes_for_refund = 0;
        case.votes_against_refund = 0;
        env.storage()
            .instance()
            .set(&ArbitrationKey::ArbitrationCase(case_id), &case);
        env.storage()
            .instance()
            .set(&ArbitrationKey::CaseEscalated(case_id), &true);

        Ok(())
    }

    /// Get the arbitration tier (Junior or Senior) for a given case.
    ///
    /// # Arguments
    /// * `case_id` - The ID of the arbitration case.
    ///
    /// # Returns
    /// `ArbitratorTier::Senior` if the case has been escalated, `ArbitratorTier::Junior` otherwise.
    pub fn get_arbitration_tier(env: Env, case_id: u64) -> ArbitratorTier {
        if env
            .storage()
            .instance()
            .has(&ArbitrationKey::CaseEscalated(case_id))
        {
            ArbitratorTier::Senior
        } else {
            ArbitratorTier::Junior
        }
    }

    pub(crate) fn clear_arbitration_votes(env: &Env, case_id: u64) {
        let voters: Vec<Address> = env
            .storage()
            .instance()
            .get(&ArbitrationKey::ArbitratorsVoted(case_id))
            .unwrap_or_else(|| Vec::new(env));

        for voter in voters.iter() {
            env.storage()
                .instance()
                .remove(&ArbitrationKey::ArbitratorVote(case_id, voter.clone()));
        }

        env.storage()
            .instance()
            .remove(&ArbitrationKey::ArbitratorsVoted(case_id));
    }
}
