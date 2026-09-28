#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger},
    token, Address, Env, Event,
};

struct Setup {
    env: Env,
    contract_id: Address,
    client: EscrowContractClient<'static>,
    admin: Address,
    customer: Address,
    merchant: Address,
    token: Address,
}

fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let customer = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(admin.clone()).address();
    token::StellarAssetClient::new(&env, &token).mint(&customer, &1_000_000);
    Setup {
        env,
        contract_id,
        client,
        admin,
        customer,
        merchant,
        token,
    }
}

fn create_escrow(s: &Setup, amount: i128, release_timestamp: u64) -> u64 {
    s.client.create_escrow(
        &s.customer,
        &s.merchant,
        &amount,
        &s.token,
        &release_timestamp,
        &0_u64,
        &0_u64,
        &false,
    )
}

/// Asserts that the most recent invocation published `event` from the escrow contract.
fn assert_published(s: &Setup, event: &impl Event) {
    let expected = (
        s.contract_id.clone(),
        event.topics(&s.env),
        event.data(&s.env),
    );
    assert!(
        s.env.events().all().contains(expected),
        "expected escrow event was not published"
    );
}

#[test]
fn test_create_escrow_publishes_escrow_created_event() {
    let s = setup();
    s.env.ledger().set_timestamp(1_000);

    let escrow_id = create_escrow(&s, 5_000, 2_000);

    assert_published(
        &s,
        &EscrowCreatedEvent {
            escrow_id,
            customer: s.customer.clone(),
            merchant: s.merchant.clone(),
            amount: 5_000,
            token: s.token.clone(),
            release_timestamp: 2_000,
            created_at: 1_000,
        },
    );
}

#[test]
fn test_release_escrow_publishes_escrow_released_event() {
    let s = setup();
    s.env.ledger().set_timestamp(1_000);
    let escrow_id = create_escrow(&s, 5_000, 2_000);

    s.env.ledger().set_timestamp(2_500);
    s.client.release_escrow(&s.admin, &escrow_id, &false);

    assert_published(
        &s,
        &EscrowReleasedEvent {
            escrow_id,
            customer: s.customer.clone(),
            merchant: s.merchant.clone(),
            recipient: s.merchant.clone(),
            amount: 5_000,
            net_amount: 5_000,
            token: s.token.clone(),
            released_at: 2_500,
        },
    );
}

#[test]
fn test_dispute_escrow_publishes_dispute_opened_event() {
    let s = setup();
    s.env.ledger().set_timestamp(1_000);
    let escrow_id = create_escrow(&s, 5_000, 10_000);

    s.env.ledger().set_timestamp(1_500);
    s.client.dispute_escrow(&s.customer, &escrow_id);

    assert_published(
        &s,
        &DisputeOpenedEvent {
            escrow_id,
            disputed_by: s.customer.clone(),
            customer: s.customer.clone(),
            merchant: s.merchant.clone(),
            amount: 5_000,
            token: s.token.clone(),
            opened_at: 1_500,
        },
    );
}

#[test]
fn test_resolve_dispute_publishes_dispute_resolved_event() {
    let s = setup();
    s.env.ledger().set_timestamp(1_000);
    let escrow_id = create_escrow(&s, 5_000, 10_000);
    s.client.dispute_escrow(&s.customer, &escrow_id);

    s.env.ledger().set_timestamp(3_000);
    s.client.resolve_dispute(&s.admin, &escrow_id, &false);

    assert_published(
        &s,
        &DisputeResolvedEvent {
            escrow_id,
            customer: s.customer.clone(),
            merchant: s.merchant.clone(),
            winner: s.customer.clone(),
            released_to_merchant: false,
            amount: 5_000,
            token: s.token.clone(),
            resolved_at: 3_000,
        },
    );
}
