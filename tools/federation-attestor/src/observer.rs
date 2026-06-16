//! Event observer — watches mainnet vault `Deposit` events and staccana `Burn` events.
//!
//! ## v0 status
//!
//! **STUB.** This module currently exposes the data shapes ([`DepositEvent`],
//! [`BurnEvent`]) and a no-op [`Observer`] that the daemon main loop can talk to. It does
//! not actually subscribe to either chain. The daemon will simply not emit attestations
//! until this is wired up.
//!
//! ## v1 implementation plan (TODO)
//!
//! The real implementation needs two `solana-pubsub-client` `WebSocketClient` connections:
//!
//! 1. **Mainnet vault subscription**
//!    - `pubsubClient.account_subscribe` on each registered mainnet vault PDA, OR
//!    - `pubsubClient.logs_subscribe` filtered by `mainnet_vault_program` for the per-asset
//!      vault programs and parse `Deposit { asset, user, value_after_fee, dest, nonce }`
//!      from the program log line emitted by the on-chain program (`anchor_lang::emit!`
//!      writes a base64-encoded discriminator+payload prefixed with `Program data:`).
//!    - On reconnect, query getSignaturesForAddress to backfill any deposits missed
//!      during the disconnect window. Track the last-seen signature in persistent state.
//!
//! 2. **Staccana bridge subscription**
//!    - Same shape, but `logs_subscribe` filtered by the `BRIDGE_PROGRAM_ID` and parsing
//!      `Burn { asset_id, user, release_amount, mainnet_dest, nonce_out, chain_id=mainnet }`.
//!
//! Both subscriptions feed an `mpsc::UnboundedSender<ObservedEvent>` that the
//! attestation-construction loop drains. The `Observer` trait below is the seam — swap the
//! `StubObserver` for a `WebsocketObserver` that backs onto those subscriptions.
//!
//! ### Re-org / finality
//!
//! Deposits and burns must wait for **finalized** commitment before being attested.
//! Federation members that sign a not-yet-finalized event risk attesting to a forked block
//! that's later abandoned, which would let an attacker replay the deposit.
//! `pubsubClient` accepts a `CommitmentConfig::finalized()` parameter — use it.
//!
//! ## What this stub provides
//!
//! - The on-the-wire event shapes ([`DepositEvent`], [`BurnEvent`]) with field names
//!   matching SPEC §5.4 / §5.5.
//! - A trait [`Observer`] the daemon programs against.
//! - A [`StubObserver`] that returns "no events" so the daemon main loop compiles and
//!   runs end-to-end without subscriptions.

use solana_sdk::pubkey::Pubkey;

/// Errors the observer can surface. Kept open-ended (`Other`) so the v1 implementation
/// can wrap `solana_pubsub_client::PubsubClientError` without breaking this stub's API.
#[derive(Debug, thiserror::Error)]
pub enum ObserverError {
    #[error("websocket subscription not yet implemented (v0 stub)")]
    NotImplemented,

    #[error("rpc / websocket error: {0}")]
    Other(String),
}

/// `Deposit` event observed on the mainnet vault for some asset. SPEC §5.4 step 3.
///
/// Federation members aggregate vault state at `slot` and use `(value_after_fee, ...)` to
/// build the per-asset attestation tuple. Note: the *attestation* signs `vault_value` and
/// `mint_supply`, not `value_after_fee` directly — the deposit is a trigger, not a
/// payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DepositEvent {
    pub asset_id: u32,
    /// Mainnet payer. Informational.
    pub user: Pubkey,
    /// Underlying tokens credited to the vault, net of mainnet vault fee.
    pub value_after_fee: u64,
    /// Recipient pubkey on staccana (the user's destination ATA owner).
    pub dest: Pubkey,
    /// Per-(asset, direction) nonce minted by the vault.
    pub nonce: u64,
    /// Mainnet slot at which the event was emitted.
    pub slot: u64,
}

/// `Burn` event observed on the staccana bridge for some asset. SPEC §5.5 step 5.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BurnEvent {
    pub asset_id: u32,
    /// Staccana ATA authority that initiated the burn.
    pub user: Pubkey,
    /// Underlying owed to the user on mainnet (post-burn-fee).
    pub release_amount: u64,
    /// Mainnet destination address for the release.
    pub mainnet_dest: Pubkey,
    /// Per-(asset, direction) nonce from the on-chain `nonce_out` counter.
    pub nonce_out: u64,
    /// Staccana slot at which the burn was processed.
    pub slot: u64,
}

/// Daemon-facing observer trait. The main loop polls these in a `tokio::select!`.
///
/// The trait is `async`-free in v0 (returns `Result<Option<_>>`) so that the stub can
/// implement it trivially. v1 will likely change the signature to
/// `async fn next_deposit(&mut self) -> Result<DepositEvent, ObserverError>` backed by
/// an mpsc receiver — this is a stub-API stability tradeoff worth flagging now.
pub trait Observer {
    /// Best-effort poll for the next deposit. `Ok(None)` means "no event ready, try again
    /// later." Errors are operational (RPC failure, dropped subscription); the daemon
    /// should log and back off.
    fn poll_deposit(&mut self) -> Result<Option<DepositEvent>, ObserverError>;

    /// Same shape, for burns on staccana.
    fn poll_burn(&mut self) -> Result<Option<BurnEvent>, ObserverError>;
}

/// v0 no-op observer. Returns `Ok(None)` for both polls; useful for keeping the daemon
/// loop compiling and exercising the rest of the pipeline (config loading, signing) end-
/// to-end without a live RPC.
#[derive(Debug, Default)]
pub struct StubObserver;

impl StubObserver {
    pub fn new() -> Self {
        Self
    }
}

impl Observer for StubObserver {
    fn poll_deposit(&mut self) -> Result<Option<DepositEvent>, ObserverError> {
        // TODO(v1): replace with an mpsc::Receiver fed by a websocket logs_subscribe on
        // the mainnet vault programs. See module-level doc comment for the full plan.
        Ok(None)
    }

    fn poll_burn(&mut self) -> Result<Option<BurnEvent>, ObserverError> {
        // TODO(v1): replace with an mpsc::Receiver fed by a websocket logs_subscribe on
        // the staccana BRIDGE_PROGRAM_ID, filtering for the Burn event discriminator.
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_observer_yields_no_events() {
        // Pin the v0 contract so a future "stub now actually does something" change has to
        // explicitly break this test.
        let mut o = StubObserver::new();
        assert_eq!(o.poll_deposit().unwrap(), None);
        assert_eq!(o.poll_burn().unwrap(), None);
    }

    #[test]
    fn deposit_event_round_trips_through_clone() {
        // Trivial sanity for the value type — catches accidental field-removal during
        // refactors.
        let d = DepositEvent {
            asset_id: 7,
            user: Pubkey::new_unique(),
            value_after_fee: 1_000_000,
            dest: Pubkey::new_unique(),
            nonce: 42,
            slot: 12345,
        };
        assert_eq!(d.clone(), d);
    }

    #[test]
    fn burn_event_round_trips_through_clone() {
        let b = BurnEvent {
            asset_id: 7,
            user: Pubkey::new_unique(),
            release_amount: 999_999,
            mainnet_dest: Pubkey::new_unique(),
            nonce_out: 99,
            slot: 67890,
        };
        assert_eq!(b.clone(), b);
    }
}
