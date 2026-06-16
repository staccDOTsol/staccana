//! Write the four megadrop output files.
//!
//! 1. `allocations.json` — `[{holder, weight_breakdown, allocation_lamports}]`
//!    (human-readable; the frontend consumes this).
//! 2. `merkle-root.hex` — 32-byte root in hex (for the on-chain `init_megadrop` ix).
//! 3. `init-megadrop-args.json` — ready-to-paste args for the `init_megadrop` ix.
//! 4. `proofs.json` — per-holder Merkle inclusion proofs (the frontend serves these
//!    to claimers).
//!
//! Merkle root math goes through `staccana_genesis::merkle::MerkleTree::build` so the
//! root is byte-for-byte identical to what the on-chain verifier in
//! `programs/megadrop/src/merkle.rs` would compute.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use solana_program::hash::{hashv, Hash};
use solana_sdk::pubkey::Pubkey;
use staccana_genesis::merkle::{ClaimableLeaf, MerkleTree, NODE_DOMAIN};
#[cfg(test)]
use staccana_genesis::merkle::LEAF_DOMAIN;

use crate::allocate::HolderAllocation;

/// Root + leaf count.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MerkleSummary {
    pub root_hex: String,
    pub leaf_count: usize,
}

/// One row in `allocations.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AllocationRow {
    pub holder: String,
    pub based_stacc_0_count: u64,
    pub proofv3_balance: u64,
    pub based_weight: u128,
    pub proofv3_weight: u128,
    pub total_weight: u128,
    pub allocation_lamports: u64,
}

/// Top-level `init-megadrop-args.json` shape.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InitMegadropArgsOut {
    pub claimable_root_hex: String,
    pub genesis_month: u32,
    pub total_allocation_lamports: u64,
    /// The treasury authority Pubkey (base58). Set to the well-known PDA derived from
    /// the megadrop program's `["megadrop_treasury"]` seed once the program is
    /// deployed; the snapshot tool emits the placeholder for off-chain inspection.
    pub treasury_authority_placeholder: String,
}

/// One row in `proofs.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProofRow {
    pub holder: String,
    pub total_allocation: u64,
    /// Sibling hashes, hex-encoded, leaf-level upward.
    pub proof_hex: Vec<String>,
    /// Packed sibling-side bit flags. Bit `i` controls level `i`: 0 ⇒ sibling on left.
    pub proof_flags: Vec<u8>,
}

/// Serialize all four output files into `output_dir`.
///
/// Returns the [`MerkleSummary`] for callers that want to log it.
pub fn write_outputs(
    allocations: &[HolderAllocation],
    output_dir: &Path,
    genesis_month: u32,
    total_megadrop_lamports: u64,
) -> Result<MerkleSummary> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!("create output directory {}", output_dir.display())
    })?;

    // 1. allocations.json (human-readable).
    let alloc_rows: Vec<AllocationRow> = allocations
        .iter()
        .map(|h| AllocationRow {
            holder: h.holder.to_string(),
            based_stacc_0_count: h.based_stacc_0_count,
            proofv3_balance: h.proofv3_balance,
            based_weight: h.based_weight,
            proofv3_weight: h.proofv3_weight,
            total_weight: h.total_weight,
            allocation_lamports: h.allocation_lamports,
        })
        .collect();
    let path = output_dir.join("allocations.json");
    let pretty = serde_json::to_string_pretty(&alloc_rows)
        .context("serialize allocations.json")?;
    fs::write(&path, pretty).with_context(|| format!("write {}", path.display()))?;

    // 2 + 3. Merkle root via staccana-genesis (byte-for-byte identical to the
    // on-chain verifier).
    let leaves: Vec<ClaimableLeaf> = allocations
        .iter()
        .filter(|h| h.allocation_lamports > 0)
        .map(|h| ClaimableLeaf {
            pubkey: h.holder,
            lamports: h.allocation_lamports,
        })
        .collect();
    let tree = MerkleTree::build(leaves.clone());
    let root_hex = hex_encode(tree.root.0.as_ref());

    let path = output_dir.join("merkle-root.hex");
    fs::write(&path, &root_hex).with_context(|| format!("write {}", path.display()))?;

    let init_args = InitMegadropArgsOut {
        claimable_root_hex: root_hex.clone(),
        genesis_month,
        total_allocation_lamports: total_megadrop_lamports,
        treasury_authority_placeholder: "<DERIVE_FROM_MEGADROP_PROGRAM_AT_DEPLOY>"
            .to_string(),
    };
    let path = output_dir.join("init-megadrop-args.json");
    let pretty = serde_json::to_string_pretty(&init_args)
        .context("serialize init-megadrop-args.json")?;
    fs::write(&path, pretty).with_context(|| format!("write {}", path.display()))?;

    // 4. proofs.json — one row per holder with allocation > 0.
    let proof_rows = build_proof_rows(&leaves);
    let path = output_dir.join("proofs.json");
    let pretty =
        serde_json::to_string_pretty(&proof_rows).context("serialize proofs.json")?;
    fs::write(&path, pretty).with_context(|| format!("write {}", path.display()))?;

    Ok(MerkleSummary {
        root_hex,
        leaf_count: tree.leaf_count,
    })
}

/// Build per-holder proofs. Sorted-by-pubkey leaf ordering is enforced inside
/// `MerkleTree::build`; this function re-sorts here so the proof builder sees the same
/// canonical order.
fn build_proof_rows(leaves: &[ClaimableLeaf]) -> Vec<ProofRow> {
    let mut sorted: Vec<ClaimableLeaf> = leaves.to_vec();
    sorted.sort_by(|a, b| a.pubkey.cmp(&b.pubkey));
    let total_allocation_by_pubkey: BTreeMap<Pubkey, u64> = sorted
        .iter()
        .map(|l| (l.pubkey, l.lamports))
        .collect();

    let mut rows: Vec<ProofRow> = Vec::with_capacity(sorted.len());
    for leaf in &sorted {
        let (proof, packed) = build_proof(&sorted, leaf.pubkey);
        let proof_hex: Vec<String> = proof.iter().map(|h| hex_encode(h.as_ref())).collect();
        let total = total_allocation_by_pubkey
            .get(&leaf.pubkey)
            .copied()
            .unwrap_or(0);
        rows.push(ProofRow {
            holder: leaf.pubkey.to_string(),
            total_allocation: total,
            proof_hex,
            proof_flags: packed,
        });
    }
    rows
}

/// Build a Merkle proof for `target` against the sorted `leaves`. Mirrors
/// `staccana_genesis::merkle` walk semantics so the proof bytes match what the
/// on-chain verifier expects.
fn build_proof(leaves: &[ClaimableLeaf], target: Pubkey) -> (Vec<Hash>, Vec<u8>) {
    let target_idx = leaves
        .iter()
        .position(|l| l.pubkey == target)
        .expect("target must exist in leaves");

    let mut layer: Vec<Hash> = leaves.iter().map(ClaimableLeaf::hash).collect();
    let mut idx = target_idx;
    let mut proof: Vec<Hash> = Vec::new();
    let mut flag_bits: Vec<u8> = Vec::new();

    while layer.len() > 1 {
        let pair_idx = idx ^ 1;
        let (sibling, sibling_on_right) = if pair_idx < layer.len() {
            (layer[pair_idx], idx % 2 == 0)
        } else {
            (layer[idx], true)
        };
        proof.push(sibling);
        flag_bits.push(if sibling_on_right { 1 } else { 0 });

        let mut next = Vec::with_capacity(layer.len() / 2 + 1);
        for chunk in layer.chunks(2) {
            let parent = if chunk.len() == 2 {
                hashv(&[&[NODE_DOMAIN], chunk[0].as_ref(), chunk[1].as_ref()])
            } else {
                hashv(&[&[NODE_DOMAIN], chunk[0].as_ref(), chunk[0].as_ref()])
            };
            next.push(parent);
        }
        idx /= 2;
        layer = next;
    }

    let mut packed = vec![0u8; (flag_bits.len() + 7) / 8];
    for (i, b) in flag_bits.iter().enumerate() {
        packed[i / 8] |= b << (i % 8);
    }
    (proof, packed)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocate::compute_allocations;
    use crate::snapshot::HolderEntry;
    use crate::AllocationModel;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn entry(holder: Pubkey, based: u64, proof: u64) -> HolderEntry {
        HolderEntry {
            holder,
            based_stacc_0_count: based,
            proofv3_balance: proof,
        }
    }

    #[test]
    fn write_outputs_emits_all_four_files() {
        let tmp = tempfile::tempdir().unwrap();
        let holders = vec![
            entry(pk(1), 1, 0),
            entry(pk(2), 1, 0),
            entry(pk(3), 1, 0),
        ];
        let allocs = compute_allocations(
            &holders,
            AllocationModel::Linear,
            100,
            0,
            3_000,
        );
        let summary = write_outputs(&allocs, tmp.path(), 202605, 3_000).unwrap();

        for f in [
            "allocations.json",
            "merkle-root.hex",
            "init-megadrop-args.json",
            "proofs.json",
        ] {
            let p = tmp.path().join(f);
            assert!(p.exists(), "missing {}", f);
        }
        // 64 hex chars = 32 bytes.
        assert_eq!(summary.root_hex.len(), 64);
        assert_eq!(summary.leaf_count, 3);
    }

    #[test]
    fn merkle_root_matches_genesis_crate() {
        let tmp = tempfile::tempdir().unwrap();
        let holders = vec![entry(pk(1), 1, 0), entry(pk(2), 2, 0)];
        let allocs = compute_allocations(
            &holders,
            AllocationModel::Linear,
            100,
            0,
            3_000,
        );
        let summary = write_outputs(&allocs, tmp.path(), 202605, 3_000).unwrap();

        // Recompute via genesis directly and compare.
        let leaves: Vec<ClaimableLeaf> = allocs
            .iter()
            .filter(|h| h.allocation_lamports > 0)
            .map(|h| ClaimableLeaf {
                pubkey: h.holder,
                lamports: h.allocation_lamports,
            })
            .collect();
        let direct = MerkleTree::build(leaves);
        assert_eq!(summary.root_hex, hex_encode(direct.root.0.as_ref()));
    }

    #[test]
    fn proofs_round_trip_via_inclusion_check() {
        // Build a tree of three holders, then verify each proof recomputes the root.
        let tmp = tempfile::tempdir().unwrap();
        let holders = vec![
            entry(pk(1), 1, 0),
            entry(pk(2), 1, 0),
            entry(pk(3), 1, 0),
        ];
        let allocs = compute_allocations(
            &holders,
            AllocationModel::Linear,
            100,
            0,
            3_000,
        );
        let summary = write_outputs(&allocs, tmp.path(), 202605, 3_000).unwrap();

        let raw = fs::read_to_string(tmp.path().join("proofs.json")).unwrap();
        let rows: Vec<ProofRow> = serde_json::from_str(&raw).unwrap();
        assert_eq!(rows.len(), 3);

        // For each row, recompute the root from the leaf + proof and compare against
        // the stored summary root.
        for row in rows {
            let pubkey: Pubkey = row.holder.parse().unwrap();
            let leaf = hashv(&[
                &[LEAF_DOMAIN],
                pubkey.as_ref(),
                &row.total_allocation.to_le_bytes(),
            ]);
            let proof: Vec<Hash> = row
                .proof_hex
                .iter()
                .map(|h| {
                    let bytes = hex_decode_32(h);
                    Hash::new_from_array(bytes)
                })
                .collect();
            let mut running = leaf;
            for (i, sibling) in proof.iter().enumerate() {
                let bit = (row.proof_flags[i / 8] >> (i % 8)) & 1;
                running = if bit == 0 {
                    hashv(&[&[NODE_DOMAIN], sibling.as_ref(), running.as_ref()])
                } else {
                    hashv(&[&[NODE_DOMAIN], running.as_ref(), sibling.as_ref()])
                };
            }
            assert_eq!(hex_encode(running.as_ref()), summary.root_hex);
        }
    }

    fn hex_decode_32(s: &str) -> [u8; 32] {
        assert_eq!(s.len(), 64, "expected 32-byte hex");
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] =
                u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("valid hex");
        }
        out
    }

    #[test]
    fn empty_allocations_produces_empty_proofs() {
        let tmp = tempfile::tempdir().unwrap();
        let summary =
            write_outputs(&[], tmp.path(), 202605, 0).unwrap();
        assert_eq!(summary.leaf_count, 0);
        let raw = fs::read_to_string(tmp.path().join("proofs.json")).unwrap();
        let rows: Vec<ProofRow> = serde_json::from_str(&raw).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn zero_allocation_holders_excluded_from_tree() {
        // A holder with allocation 0 shouldn't appear in the Merkle tree (no point
        // in giving them a proof for a zero-pay leaf — it'd just bloat the tree).
        let tmp = tempfile::tempdir().unwrap();
        let holders = vec![
            entry(pk(1), 1, 0),
            entry(pk(2), 0, 0), // empty cohort, allocation will be 0
        ];
        let allocs = compute_allocations(
            &holders,
            AllocationModel::Linear,
            100,
            0,
            1_000,
        );
        let summary = write_outputs(&allocs, tmp.path(), 202605, 1_000).unwrap();
        assert_eq!(summary.leaf_count, 1);
    }
}
