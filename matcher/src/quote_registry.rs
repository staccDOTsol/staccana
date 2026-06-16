//! Registry of mints treated as the "quote" side of swaps.
//!
//! Quote mints (USDC, USDT, native SOL, etc.) are not given their own batch queue — they
//! participate as the counterparty side of base-mint batches. This is the "longtail
//! weighting" property: the matcher cares about clearing the longtail mint cleanly; the
//! quote is just the medium.

use solana_program::pubkey::Pubkey;
use std::collections::BTreeSet;

/// Set of mints to treat as quotes.
///
/// Backed by `BTreeSet` so iteration is deterministic — useful for hashing the registry
/// state into the genesis or a slashing fingerprint later.
#[derive(Clone, Debug, Default)]
pub struct QuoteRegistry {
    quotes: BTreeSet<Pubkey>,
}

impl QuoteRegistry {
    pub fn new(quotes: impl IntoIterator<Item = Pubkey>) -> Self {
        Self {
            quotes: quotes.into_iter().collect(),
        }
    }

    pub fn is_quote(&self, mint: &Pubkey) -> bool {
        self.quotes.contains(mint)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Pubkey> {
        self.quotes.iter()
    }

    pub fn len(&self) -> usize {
        self.quotes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.quotes.is_empty()
    }
}
