//! Activate the four CTE feature gates at slot 0.
//!
//! Solana's feature mechanism uses one account per feature, owned by the
//! `Feature111...` program (`solana_sdk_ids::feature::id()`), with bincode-serialized
//! `Feature { activated_at: Option<u64> }` data. Setting `activated_at = Some(0)` at
//! genesis is what tells the runtime "this feature is live from the start; never
//! schedule it for activation, never wait for the next epoch."
//!
//! The four gates we activate are listed in
//! [`staccana_genesis::CTE_FEATURE_GATES_AT_GENESIS`]; each is a `(base58 pubkey,
//! human-readable description)` pair. They are the ZK ElGamal Proof + confidential
//! transfer extension gates that ship OFF on mainnet/devnet/testnet today — turning
//! them ON at staccana's slot 0 is what lets Token-22's Confidential Transfer
//! Extension work for any program from boot.
//!
//! The composed-genesis JSON also carries an `active_feature_gates` field with the
//! same set as owned `String`s — we cross-validate against the constant set from
//! `staccana-genesis` to catch JSON drift. Mismatches abort the bake (we'd rather fail
//! loudly than silently activate the wrong gates).

use anyhow::{anyhow, Context, Result};
use solana_account::AccountSharedData;
use solana_feature_gate_interface::{create_account, Feature};
use solana_pubkey::Pubkey;
use solana_rent::Rent;

use staccana_genesis::CTE_FEATURE_GATES_AT_GENESIS;
use staccana_genesis_emit::ActiveFeatureGate;

/// Build the `(Pubkey, AccountSharedData)` pair that activates a single feature gate
/// at slot 0.
///
/// Internally calls `solana_feature_gate_interface::create_account` with the rent-
/// exempt minimum balance for `Feature::size_of()` (9 bytes). The account is owned by
/// the `Feature` program ID and contains a bincode-encoded `Feature { activated_at:
/// Some(0) }` payload.
pub fn build_feature_account(gate_pubkey: Pubkey) -> (Pubkey, AccountSharedData) {
    let feature = Feature {
        activated_at: Some(0),
    };
    let lamports = Rent::default().minimum_balance(Feature::size_of());
    let account = create_account(&feature, lamports);
    (gate_pubkey, account)
}

/// Build the full set of feature accounts to activate at slot 0.
///
/// Two layers:
///
///   1. The CTE feature gates from `composed-genesis.json` (cross-validated
///      against the compile-time `CTE_FEATURE_GATES_AT_GENESIS` constant).
///      These are the load-bearing CTE / ZK-proof gates the staccana programs
///      depend on at boot.
///
///   2. EVERY feature in `agave_feature_set::FEATURE_NAMES` — the full runtime
///      feature set that mainnet has accumulated over years. Without this,
///      core-BPF programs built against current mainnet (AddressLookupTable,
///      future loader_v4 migrations, etc.) hit `unsupported BPF instruction`
///      and similar runtime errors because they emit SBPFv3 / use syscalls
///      gated behind features that haven't been flipped on for our cluster.
///
///      `solana-test-validator` activates the same set for the same reason —
///      it's what makes a fresh dev cluster behave like mainnet from slot 0.
///      If a future agave SBPFv4 lands and our chain is left behind, we just
///      bump the agave-feature-set dep here and rebake.
pub fn build_all_feature_accounts(
    composed_gates: &[ActiveFeatureGate],
) -> Result<Vec<(Pubkey, AccountSharedData)>> {
    cross_validate_against_constant(composed_gates)?;

    use std::collections::BTreeMap;
    let mut out: BTreeMap<Pubkey, AccountSharedData> = BTreeMap::new();

    // Layer 1: CTE gates from composed-genesis.json.
    for gate in composed_gates {
        let pk = parse_b58_pubkey(&gate.pubkey_b58)
            .with_context(|| format!("parsing feature gate pubkey {}", gate.pubkey_b58))?;
        let (k, v) = build_feature_account(pk);
        out.insert(k, v);
    }

    // Layer 2: every feature gate the runtime knows about.
    for pk in agave_feature_set::FEATURE_NAMES.keys() {
        // BTreeMap: layer 2 inserts only if not already present (so layer 1
        // wins for any overlap — same activated_at=0 either way, doesn't
        // matter, but consistent).
        out.entry(*pk)
            .or_insert_with(|| build_feature_account(*pk).1);
    }

    Ok(out.into_iter().collect())
}

/// Confirm the JSON-supplied gate set matches the compile-time constant set from
/// `staccana_genesis::CTE_FEATURE_GATES_AT_GENESIS` exactly (same elements, no extras
/// in either direction). Order-insensitive — we compare set membership.
fn cross_validate_against_constant(composed_gates: &[ActiveFeatureGate]) -> Result<()> {
    use std::collections::BTreeSet;

    let constant_set: BTreeSet<&str> = CTE_FEATURE_GATES_AT_GENESIS
        .iter()
        .map(|(pk, _desc)| *pk)
        .collect();
    let composed_set: BTreeSet<&str> = composed_gates
        .iter()
        .map(|g| g.pubkey_b58.as_str())
        .collect();

    if constant_set != composed_set {
        let only_in_constant: Vec<_> = constant_set.difference(&composed_set).collect();
        let only_in_composed: Vec<_> = composed_set.difference(&constant_set).collect();
        return Err(anyhow!(
            "CTE feature gate mismatch: only-in-constant={:?}, only-in-composed-json={:?}",
            only_in_constant,
            only_in_composed
        ));
    }
    Ok(())
}

fn parse_b58_pubkey(s: &str) -> Result<Pubkey> {
    let bytes = bs58::decode(s)
        .into_vec()
        .with_context(|| format!("base58 decoding {}", s))?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("base58 pubkey {} did not decode to 32 bytes", s))?;
    Ok(Pubkey::new_from_array(arr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_account::ReadableAccount;
    use solana_sdk_ids::feature as feature_program;

    fn cte_gates_as_active_feature_gates() -> Vec<ActiveFeatureGate> {
        CTE_FEATURE_GATES_AT_GENESIS
            .iter()
            .map(|(pk, desc)| ActiveFeatureGate {
                pubkey_b58: (*pk).to_string(),
                description: (*desc).to_string(),
            })
            .collect()
    }

    #[test]
    fn build_feature_account_owner_is_feature_program() {
        let pk = Pubkey::new_unique();
        let (key, acct) = build_feature_account(pk);
        assert_eq!(key, pk);
        assert_eq!(*acct.owner(), feature_program::id());
    }

    #[test]
    fn build_feature_account_data_decodes_to_activated_at_zero() {
        let pk = Pubkey::new_unique();
        let (_key, acct) = build_feature_account(pk);
        let feature: Feature = bincode::deserialize(acct.data())
            .expect("baked Feature account data must bincode-deserialize");
        assert_eq!(feature.activated_at, Some(0));
    }

    #[test]
    fn build_feature_account_is_rent_exempt() {
        let pk = Pubkey::new_unique();
        let (_key, acct) = build_feature_account(pk);
        let floor = Rent::default().minimum_balance(Feature::size_of());
        assert_eq!(acct.lamports(), floor);
    }

    #[test]
    fn build_all_feature_accounts_yields_one_per_gate() {
        let gates = cte_gates_as_active_feature_gates();
        let accts = build_all_feature_accounts(&gates).expect("build");
        assert_eq!(accts.len(), CTE_FEATURE_GATES_AT_GENESIS.len());
        // All distinct pubkeys (defense against accidental duplication in the gate
        // list).
        for i in 0..accts.len() {
            for j in (i + 1)..accts.len() {
                assert_ne!(accts[i].0, accts[j].0);
            }
        }
    }

    #[test]
    fn build_all_feature_accounts_rejects_missing_gate() {
        let mut gates = cte_gates_as_active_feature_gates();
        gates.pop(); // drop the last gate; constant set still has 4.
        let err = build_all_feature_accounts(&gates).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("only-in-constant"),
            "expected mismatch error, got: {msg}"
        );
    }

    #[test]
    fn build_all_feature_accounts_rejects_extra_gate() {
        let mut gates = cte_gates_as_active_feature_gates();
        // Inject an unknown gate — `Pubkey::new_unique` produces something never in
        // the static set.
        gates.push(ActiveFeatureGate {
            pubkey_b58: bs58::encode(Pubkey::new_unique().to_bytes()).into_string(),
            description: "synthetic gate not in the constant set".to_string(),
        });
        let err = build_all_feature_accounts(&gates).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("only-in-composed-json"),
            "expected mismatch error, got: {msg}"
        );
    }

    #[test]
    fn parse_b58_pubkey_round_trips() {
        let original = Pubkey::new_unique();
        let s = bs58::encode(original.to_bytes()).into_string();
        let parsed = parse_b58_pubkey(&s).unwrap();
        assert_eq!(parsed, original);
    }
}
