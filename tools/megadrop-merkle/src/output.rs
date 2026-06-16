//! Write the five megadrop output files into the `--output-dir`.
//!
//! - `allocations.json` — `[{owner, lamports, nft_count, token_balance, contributions}]`
//! - `merkle-leaves.json` — `[{owner, lamports, leaf_hash}]` (canonical order)
//! - `merkle-tree.json` — full tree with per-leaf proofs (consumed by the frontend
//!   to serve inclusion proofs)
//! - `merkle-root.hex` — single-line hex of the 32-byte root, ready to pass to
//!   `tools/megadrop-init --root <hex>`
//! - `summary.json` — total holders, total allocated lamports, Gini coefficient,
//!   top 10 by allocation
//!
//! All files use pretty-printed JSON for human review; the on-chain commitments only
//! depend on the bytes inside the leaves, not the JSON shape.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::allocate::{HolderAllocation, HolderContributions};
use crate::tree::BuiltTree;

/// One row in `allocations.json`. Matches the operator-friendly layout requested in
/// the task brief.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AllocationRow {
    pub owner: String,
    pub lamports: u64,
    pub nft_count: u64,
    pub token_balance: u64,
    pub contributions: HolderContributions,
}

/// One row in `merkle-leaves.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MerkleLeafRow {
    pub owner: String,
    pub lamports: u64,
    pub leaf_hash: String,
}

/// One row in `merkle-tree.json`'s `proofs` array. Sibling hashes are hex-encoded
/// leaf-level upward; `proof_flags` is the packed bit field consumed by the on-chain
/// verifier.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProofRow {
    pub owner: String,
    pub lamports: u64,
    pub leaf_hash: String,
    pub proof: Vec<String>,
    pub proof_flags: Vec<u8>,
}

/// `merkle-tree.json` top-level shape.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FullTreeOut {
    pub root_hex: String,
    pub leaf_count: usize,
    pub proofs: Vec<ProofRow>,
}

/// `summary.json` shape — the operator's at-a-glance audit row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub total_holders: usize,
    pub holders_with_nonzero_allocation: usize,
    pub total_allocation_lamports: u128,
    pub target_total_lamports: u128,
    pub merkle_root_hex: String,
    /// Lorenz-curve Gini coefficient over per-holder lamports. 0 = perfect
    /// equality, 1 = one holder gets everything. Useful for sanity-checking the
    /// allocation policy.
    pub gini_coefficient: f64,
    pub top_10: Vec<TopHolderRow>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopHolderRow {
    pub owner: String,
    pub lamports: u64,
    pub nft_count: u64,
    pub token_balance: u64,
}

/// In-memory carrier of everything `write_outputs` produces. Returned to the binary
/// for logging.
#[derive(Clone, Debug)]
pub struct RunOutputs {
    pub root_hex: String,
    pub leaf_count: usize,
    pub total_allocation_lamports: u128,
    pub gini: f64,
}

/// Serialize all five output files into `output_dir`. Creates the dir if it doesn't
/// exist.
pub fn write_outputs(
    allocations: &[HolderAllocation],
    tree: &BuiltTree,
    target_total_lamports: u128,
    output_dir: &Path,
) -> Result<RunOutputs> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create output dir {}", output_dir.display()))?;

    let root_hex = hex_encode(tree.root.as_ref());

    // 1. allocations.json — sorted by owner ascending (canonical order).
    let mut sorted_allocs: Vec<HolderAllocation> = allocations.to_vec();
    sorted_allocs.sort_by(|a, b| a.owner.cmp(&b.owner));
    let alloc_rows: Vec<AllocationRow> = sorted_allocs
        .iter()
        .map(|a| AllocationRow {
            owner: a.owner.to_string(),
            lamports: a.lamports,
            nft_count: a.contributions.nft_count,
            token_balance: a.contributions.token_balance,
            contributions: a.contributions,
        })
        .collect();
    write_pretty_json(&output_dir.join("allocations.json"), &alloc_rows)?;

    // 2. merkle-leaves.json — same canonical order, includes the leaf hash so the
    //    operator can audit the preimage independently.
    let leaf_rows: Vec<MerkleLeafRow> = tree
        .leaves
        .iter()
        .map(|(owner, lamports, h)| MerkleLeafRow {
            owner: owner.to_string(),
            lamports: *lamports,
            leaf_hash: hex_encode(h.as_ref()),
        })
        .collect();
    write_pretty_json(&output_dir.join("merkle-leaves.json"), &leaf_rows)?;

    // 3. merkle-tree.json — root + per-leaf proof material for the frontend.
    let proof_rows: Vec<ProofRow> = tree
        .leaves
        .iter()
        .zip(tree.proofs.iter().zip(tree.proof_flags.iter()))
        .map(|((owner, lamports, h), (proof, flags))| ProofRow {
            owner: owner.to_string(),
            lamports: *lamports,
            leaf_hash: hex_encode(h.as_ref()),
            proof: proof.iter().map(|s| hex_encode(s.as_ref())).collect(),
            proof_flags: flags.clone(),
        })
        .collect();
    let full_tree = FullTreeOut {
        root_hex: root_hex.clone(),
        leaf_count: tree.leaves.len(),
        proofs: proof_rows,
    };
    write_pretty_json(&output_dir.join("merkle-tree.json"), &full_tree)?;

    // 4. merkle-root.hex — single-line hex.
    fs::write(output_dir.join("merkle-root.hex"), &root_hex)
        .with_context(|| format!("write merkle-root.hex"))?;

    // 5. summary.json — audit dashboard.
    let total_alloc: u128 = sorted_allocs
        .iter()
        .map(|a| a.lamports as u128)
        .sum();
    let nonzero = sorted_allocs.iter().filter(|a| a.lamports > 0).count();
    let gini = compute_gini(&sorted_allocs);
    let mut top = sorted_allocs.clone();
    top.sort_by(|a, b| b.lamports.cmp(&a.lamports));
    let top_10: Vec<TopHolderRow> = top
        .iter()
        .take(10)
        .map(|a| TopHolderRow {
            owner: a.owner.to_string(),
            lamports: a.lamports,
            nft_count: a.contributions.nft_count,
            token_balance: a.contributions.token_balance,
        })
        .collect();
    let summary = Summary {
        total_holders: sorted_allocs.len(),
        holders_with_nonzero_allocation: nonzero,
        total_allocation_lamports: total_alloc,
        target_total_lamports,
        merkle_root_hex: root_hex.clone(),
        gini_coefficient: gini,
        top_10,
    };
    write_pretty_json(&output_dir.join("summary.json"), &summary)?;

    Ok(RunOutputs {
        root_hex,
        leaf_count: tree.leaves.len(),
        total_allocation_lamports: total_alloc,
        gini,
    })
}

fn write_pretty_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let body = serde_json::to_string_pretty(value)
        .with_context(|| format!("serialize {}", path.display()))?;
    fs::write(path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Lorenz-curve Gini coefficient. Pure function, doesn't allocate beyond the sort.
///
/// Formula: `G = (2 * sum_i(i * x_i) - (n+1) * sum(x)) / (n * sum(x))` where `x_i`
/// is the sorted-ascending allocation series. Edge cases: empty input or all-zero
/// allocation → returns 0.0.
fn compute_gini(allocs: &[HolderAllocation]) -> f64 {
    if allocs.is_empty() {
        return 0.0;
    }
    let mut values: Vec<u128> = allocs.iter().map(|a| a.lamports as u128).collect();
    values.sort();
    let n = values.len() as f64;
    let sum: u128 = values.iter().copied().sum();
    if sum == 0 {
        return 0.0;
    }
    let mut weighted: f64 = 0.0;
    for (i, v) in values.iter().enumerate() {
        // Index is 1-based in the standard formulation.
        weighted += (i as f64 + 1.0) * (*v as f64);
    }
    let g = (2.0 * weighted - (n + 1.0) * (sum as f64)) / (n * (sum as f64));
    // Numerical noise can push G slightly negative or above 1; clamp.
    g.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocate::{compute_allocations, AllocationParams, HolderContributions};
    use crate::tree::{build_tree, verify_proof};
    use solana_program::hash::Hash;
    use solana_sdk::pubkey::Pubkey;
    use std::collections::BTreeMap;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn alloc(owner: Pubkey, lamports: u64) -> HolderAllocation {
        HolderAllocation {
            owner,
            lamports,
            contributions: HolderContributions {
                nft_count: 1,
                token_balance: 0,
            },
        }
    }

    #[test]
    fn write_outputs_emits_all_five_files() {
        let tmp = tempfile::tempdir().unwrap();
        let allocs = vec![alloc(pk(1), 1_000), alloc(pk(2), 2_000), alloc(pk(3), 3_000)];
        let tree = build_tree(&allocs);
        let target = 6_000u128;
        let r = write_outputs(&allocs, &tree, target, tmp.path()).unwrap();

        for f in [
            "allocations.json",
            "merkle-leaves.json",
            "merkle-tree.json",
            "merkle-root.hex",
            "summary.json",
        ] {
            let p = tmp.path().join(f);
            assert!(p.exists(), "missing {f}");
        }
        assert_eq!(r.root_hex.len(), 64); // 32 bytes hex
        assert_eq!(r.leaf_count, 3);
        assert_eq!(r.total_allocation_lamports, 6_000);
    }

    #[test]
    fn merkle_root_hex_matches_built_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let allocs = vec![alloc(pk(1), 100), alloc(pk(2), 200)];
        let tree = build_tree(&allocs);
        write_outputs(&allocs, &tree, 300, tmp.path()).unwrap();
        let on_disk = fs::read_to_string(tmp.path().join("merkle-root.hex")).unwrap();
        assert_eq!(on_disk, hex_encode(tree.root.as_ref()));
    }

    #[test]
    fn full_tree_proofs_round_trip_to_root() {
        // Build outputs, parse `merkle-tree.json`, verify each proof recomputes the
        // root. Same shape the frontend will follow.
        let tmp = tempfile::tempdir().unwrap();
        let allocs = vec![
            alloc(pk(1), 100),
            alloc(pk(2), 200),
            alloc(pk(3), 300),
            alloc(pk(4), 400),
        ];
        let tree = build_tree(&allocs);
        write_outputs(&allocs, &tree, 1_000, tmp.path()).unwrap();

        let raw = fs::read_to_string(tmp.path().join("merkle-tree.json")).unwrap();
        let parsed: FullTreeOut = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.leaf_count, 4);
        assert_eq!(parsed.root_hex, hex_encode(tree.root.as_ref()));

        for row in &parsed.proofs {
            let leaf = Hash::new_from_array(hex_decode_32(&row.leaf_hash));
            let proof: Vec<Hash> = row
                .proof
                .iter()
                .map(|s| Hash::new_from_array(hex_decode_32(s)))
                .collect();
            assert!(verify_proof(leaf, &proof, &row.proof_flags, &tree.root));
        }
    }

    fn hex_decode_32(s: &str) -> [u8; 32] {
        assert_eq!(s.len(), 64);
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn summary_top_10_is_sorted_descending() {
        let tmp = tempfile::tempdir().unwrap();
        let allocs: Vec<HolderAllocation> =
            (1u8..=15).map(|i| alloc(pk(i), (i as u64) * 100)).collect();
        let tree = build_tree(&allocs);
        write_outputs(&allocs, &tree, 100 * (1 + 15) * 15 / 2, tmp.path()).unwrap();

        let raw = fs::read_to_string(tmp.path().join("summary.json")).unwrap();
        let summary: Summary = serde_json::from_str(&raw).unwrap();
        assert_eq!(summary.top_10.len(), 10);
        for w in summary.top_10.windows(2) {
            assert!(w[0].lamports >= w[1].lamports);
        }
        assert_eq!(summary.top_10[0].lamports, 1_500); // pk(15)
        assert_eq!(summary.total_holders, 15);
    }

    #[test]
    fn summary_gini_zero_for_perfect_equality() {
        let allocs: Vec<HolderAllocation> = (1u8..=10).map(|i| alloc(pk(i), 1_000)).collect();
        let tree = build_tree(&allocs);
        let tmp = tempfile::tempdir().unwrap();
        write_outputs(&allocs, &tree, 10_000, tmp.path()).unwrap();
        let summary: Summary =
            serde_json::from_str(&fs::read_to_string(tmp.path().join("summary.json")).unwrap())
                .unwrap();
        assert!(summary.gini_coefficient.abs() < 1e-9);
    }

    #[test]
    fn summary_gini_positive_for_unequal_distribution() {
        // 9 holders with 1 lamport, 1 holder with 1000 → high Gini.
        let mut allocs: Vec<HolderAllocation> = (1u8..=9).map(|i| alloc(pk(i), 1)).collect();
        allocs.push(alloc(pk(99), 1_000));
        let tree = build_tree(&allocs);
        let tmp = tempfile::tempdir().unwrap();
        write_outputs(&allocs, &tree, 1_009, tmp.path()).unwrap();
        let summary: Summary =
            serde_json::from_str(&fs::read_to_string(tmp.path().join("summary.json")).unwrap())
                .unwrap();
        assert!(summary.gini_coefficient > 0.5);
    }

    /// End-to-end integration: build allocations from synthetic snapshot inputs,
    /// build the tree, write the outputs, then re-verify a random proof from the
    /// emitted `merkle-tree.json` against `merkle-root.hex`. Same shape the
    /// frontend + on-chain claim path will follow.
    #[test]
    fn end_to_end_synthetic_round_trip() {
        let mut nfts = BTreeMap::new();
        let mut tokens = BTreeMap::new();
        for i in 1u8..=20 {
            nfts.insert(pk(i), (i as u64) % 5);
            tokens.insert(pk(i), (i as u64) * 1_000_000);
        }
        let p = AllocationParams {
            total_allocation_sol: 30_000_000,
            base_allocation_sol: 10,
            per_nft_bonus_sol: 100,
            per_token_bonus_sol_per_million: 5,
        };
        let allocs = compute_allocations(&nfts, &tokens, p);
        let tree = build_tree(&allocs);
        let tmp = tempfile::tempdir().unwrap();
        let r = write_outputs(
            &allocs,
            &tree,
            (p.total_allocation_sol as u128) * 1_000_000_000,
            tmp.path(),
        )
        .unwrap();

        // Sum of leaves must equal target.
        assert_eq!(
            r.total_allocation_lamports,
            (p.total_allocation_sol as u128) * 1_000_000_000
        );

        let parsed: FullTreeOut = serde_json::from_str(
            &fs::read_to_string(tmp.path().join("merkle-tree.json")).unwrap(),
        )
        .unwrap();
        let root_hex_on_disk =
            fs::read_to_string(tmp.path().join("merkle-root.hex")).unwrap();
        assert_eq!(parsed.root_hex, root_hex_on_disk);

        // Pick an arbitrary proof and verify.
        let row = &parsed.proofs[7];
        let leaf = Hash::new_from_array(hex_decode_32(&row.leaf_hash));
        let proof: Vec<Hash> = row
            .proof
            .iter()
            .map(|s| Hash::new_from_array(hex_decode_32(s)))
            .collect();
        assert!(verify_proof(leaf, &proof, &row.proof_flags, &tree.root));
    }
}
