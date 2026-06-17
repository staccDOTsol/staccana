//! Register Staccana's programs as upgradeable BPF programs at slot 0.
//!
//! The standard Solana upgradeable-loader layout is two accounts per program:
//!
//! 1. **Program account** at the program ID (e.g. `LAZY_CLAIM_PROGRAM_ID`). Owner:
//!    `bpf_loader_upgradeable`. Executable. Data: bincode-serialized
//!    `UpgradeableLoaderState::Program { programdata_address }` (36 bytes).
//! 2. **ProgramData account** at the address returned by
//!    `get_program_data_address(&program_id)`. Owner: `bpf_loader_upgradeable`.
//!    Data: bincode-serialized `UpgradeableLoaderState::ProgramData { slot,
//!    upgrade_authority_address }` (45 bytes) followed by the raw `.so` bytes.
//!
//! Both accounts are rent-exempt. The Program account points at the ProgramData
//! account by address; the ProgramData account holds the actual byte payload that the
//! BPF loader reads at execution time.
//!
//! ## Upgrade authority
//!
//! For mainnet-sigma launch night we set the upgrade authority to `None`, which makes
//! the programs effectively immutable from genesis. This matches the security posture
//! of "freeze the code at slot 0" — upgrades can be enabled later by re-deploying with
//! a fresh upgrade authority via the standard `solana program deploy` flow if/when
//! governance decides to. Setting it to `Some(authority)` for a phased upgrade window
//! is a one-line change in [`build_program_pair_with_authority`].
//!
//! ## ZK ElGamal Proof builtin
//!
//! The fifth program registration here is the runtime's `solana_zk_elgamal_proof_program`
//! (sdk-id `ZkE1Gama1Proof11...`), required for the CTE feature gates to function. It is
//! a *native* program, not a BPF program, so it gets registered via
//! [`solana_genesis_config::GenesisConfig::native_instruction_processors`] rather than
//! as a Program/ProgramData pair — see [`zk_elgamal_proof_native_processor`]. The
//! caller is responsible for `add_native_instruction_processor`-ing the result.

use std::path::Path;

use anyhow::{Context, Result};
use solana_account::{AccountSharedData, WritableAccount};
use solana_loader_v3_interface::{get_program_data_address, state::UpgradeableLoaderState};
use solana_pubkey::Pubkey;
use solana_rent::Rent;
use solana_sdk_ids::{
    address_lookup_table, bpf_loader_upgradeable, zk_elgamal_proof_program,
};

use crate::pdas::{
    LAZY_CLAIM_PROGRAM_ID, MEGADROP_PROGRAM_ID, SECRET_PUMP_PROGRAM_ID,
    SPL_ASSOCIATED_TOKEN_PROGRAM_ID, SPL_MEMO_PROGRAM_ID, SPL_TOKEN_2022_PROGRAM_ID,
    SPL_TOKEN_PROGRAM_ID,
};

/// The staccana program slots (in canonical order: lazy-claim, secret-pump, megadrop).
/// Pairs each program ID with its human-readable name (used in the bake-summary log) and
/// the matching `.so` path from [`crate::BakeInputs`]. The bridge and validator-subsidy
/// programs were removed (no bridge, no yield engine — see `docs/AUDIT_SCOPE.md`).
pub struct ProgramSlot<'a> {
    pub program_id: Pubkey,
    pub name: &'static str,
    pub so_path: Option<&'a Path>,
}

/// One Program + ProgramData pair, ready to be inserted into the genesis accounts
/// map.
pub struct ProgramPair {
    pub program_id: Pubkey,
    pub program_account: AccountSharedData,
    pub program_data_address: Pubkey,
    pub program_data_account: AccountSharedData,
    /// Size of the `.so` byte payload, useful for the bake summary.
    pub elf_bytes: usize,
}

/// Iterate the canonical slot list, paired with the supplied paths from BakeInputs.
/// Order is fixed (lazy-claim first, megadrop last) — same order the bake-summary
/// renders.
#[allow(clippy::too_many_arguments)]
pub fn canonical_slots<'a>(
    lazy_claim_so: Option<&'a Path>,
    secret_pump_so: Option<&'a Path>,
    megadrop_so: Option<&'a Path>,
    spl_token_so: Option<&'a Path>,
    spl_token_2022_so: Option<&'a Path>,
    spl_associated_token_so: Option<&'a Path>,
    spl_memo_so: Option<&'a Path>,
    address_lookup_table_so: Option<&'a Path>,
) -> Vec<ProgramSlot<'a>> {
    vec![
        ProgramSlot {
            program_id: LAZY_CLAIM_PROGRAM_ID,
            name: "staccana_lazy_claim",
            so_path: lazy_claim_so,
        },
        ProgramSlot {
            program_id: SECRET_PUMP_PROGRAM_ID,
            name: "staccana_secret_pump",
            so_path: secret_pump_so,
        },
        ProgramSlot {
            program_id: MEGADROP_PROGRAM_ID,
            name: "staccana_megadrop",
            so_path: megadrop_so,
        },
        ProgramSlot {
            program_id: SPL_TOKEN_PROGRAM_ID,
            name: "spl_token (v3)",
            so_path: spl_token_so,
        },
        ProgramSlot {
            program_id: SPL_TOKEN_2022_PROGRAM_ID,
            name: "spl_token_2022 (v8)",
            so_path: spl_token_2022_so,
        },
        ProgramSlot {
            program_id: SPL_ASSOCIATED_TOKEN_PROGRAM_ID,
            name: "spl_associated_token_account",
            so_path: spl_associated_token_so,
        },
        ProgramSlot {
            program_id: SPL_MEMO_PROGRAM_ID,
            name: "spl_memo (v3)",
            so_path: spl_memo_so,
        },
        ProgramSlot {
            program_id: address_lookup_table_program_id(),
            name: "address_lookup_table (core-bpf v3)",
            so_path: address_lookup_table_so,
        },
    ]
}

/// Address Lookup Table program ID — `AddressLookupTab1e1111111111111111111111111`.
/// In agave 2.3+ this is no longer a native builtin; it's a regular BPF
/// program deployed at the canonical address. Source `.so` ships with
/// `solana-program-test`'s programs directory as
/// `core_bpf_address_lookup_table-3.0.0.so`.
pub fn address_lookup_table_program_id() -> Pubkey {
    address_lookup_table::id()
}

/// Build a single Program + ProgramData pair for a program whose `.so` byte payload
/// is supplied directly.
///
/// Use [`build_program_pair_from_path`] for the common case where the `.so` lives on
/// disk; this lower-level entrypoint exists for tests that don't want to touch the
/// filesystem.
pub fn build_program_pair_with_authority(
    program_id: Pubkey,
    elf: Vec<u8>,
    upgrade_authority: Option<Pubkey>,
) -> Result<ProgramPair> {
    let program_data_address = get_program_data_address(&program_id);
    let elf_bytes = elf.len();

    // Program account: 36-byte serialized header pointing at the ProgramData account.
    let program_state = UpgradeableLoaderState::Program {
        programdata_address: program_data_address,
    };
    let program_account_data = bincode::serialize(&program_state)
        .context("serializing UpgradeableLoaderState::Program")?;
    debug_assert_eq!(
        program_account_data.len(),
        UpgradeableLoaderState::size_of_program(),
        "Program-state encoding length does not match the canonical size_of_program()"
    );

    let program_lamports = Rent::default().minimum_balance(program_account_data.len());
    let mut program_account = AccountSharedData::new(
        program_lamports,
        program_account_data.len(),
        &bpf_loader_upgradeable::id(),
    );
    program_account.set_data_from_slice(&program_account_data);
    program_account.set_executable(true);

    // ProgramData account: 45-byte canonical header + ELF payload.
    //
    // The runtime hard-codes ELF-data-offset == `size_of_programdata_metadata()` (45)
    // regardless of what the upgrade authority field actually contains; the BPF loader
    // does `programdata_account.get_data().get(45..)` to find the ELF
    // (see `solana-bpf-loader-program::lib.rs`). So our header MUST be exactly 45
    // bytes. When `upgrade_authority == Some(_)`, bincode produces 45 naturally
    // (1 variant + 8 slot + 1 Option-Some + 32 pubkey + 3 padding-from-Option-tag
    // alignment math; total 45). When `upgrade_authority == None`, bincode produces
    // 13 bytes (the 32-byte pubkey is omitted) — we zero-pad to 45 so the offset
    // arithmetic still lands on the ELF correctly. Zero-padding is safe because the
    // runtime reads the header via `bincode::deserialize` from the front, and bincode
    // stops at the Option discriminant when it sees `None` — the trailing zeros are
    // ignored.
    let program_data_header = UpgradeableLoaderState::ProgramData {
        slot: 0,
        upgrade_authority_address: upgrade_authority,
    };
    let mut program_data_header_bytes = bincode::serialize(&program_data_header)
        .context("serializing UpgradeableLoaderState::ProgramData header")?;
    let canonical_metadata_size = UpgradeableLoaderState::size_of_programdata_metadata();
    if program_data_header_bytes.len() < canonical_metadata_size {
        program_data_header_bytes.resize(canonical_metadata_size, 0u8);
    }
    debug_assert_eq!(
        program_data_header_bytes.len(),
        canonical_metadata_size,
        "ProgramData-header encoding length does not match the canonical metadata size after padding"
    );
    let mut program_data_data = program_data_header_bytes;
    program_data_data.extend_from_slice(&elf);

    let program_data_lamports = Rent::default().minimum_balance(program_data_data.len());
    let mut program_data_account = AccountSharedData::new(
        program_data_lamports,
        program_data_data.len(),
        &bpf_loader_upgradeable::id(),
    );
    program_data_account.set_data_from_slice(&program_data_data);

    Ok(ProgramPair {
        program_id,
        program_account,
        program_data_address,
        program_data_account,
        elf_bytes,
    })
}

/// Read a `.so` file from disk and build the Program + ProgramData pair.
///
/// Convenience wrapper over [`build_program_pair_with_authority`]. For mainnet-sigma
/// launch every program is built with `upgrade_authority = None` — see module docs.
pub fn build_program_pair_from_path(
    program_id: Pubkey,
    so_path: impl AsRef<Path>,
) -> Result<ProgramPair> {
    let path = so_path.as_ref();
    let elf = std::fs::read(path).with_context(|| format!("reading .so at {}", path.display()))?;
    build_program_pair_with_authority(program_id, elf, None)
}

/// Build the `(name, pubkey)` entry for the ZK ElGamal Proof native program.
///
/// This is a *native* program (the runtime's compiled-in implementation, not a BPF
/// program), required by the CTE feature gates to function. It must be registered via
/// `GenesisConfig::add_native_instruction_processor` rather than as a Program +
/// ProgramData pair.
///
/// The (name, id) pair returned here is what the runtime's BankBuilder consults
/// during slot-0 bank construction.
pub fn zk_elgamal_proof_native_processor() -> (String, Pubkey) {
    (
        "solana_zk_elgamal_proof_program".to_string(),
        zk_elgamal_proof_program::id(),
    )
}

// AddressLookupTable was a native builtin in agave <2.3 and is now a
// core-BPF program at `AddressLookupTab1e1111111111111111111111111` shipped
// as `core_bpf_address_lookup_table-3.0.0.so`. It enters the genesis via
// `canonical_slots()` like any other BPF program — see
// `address_lookup_table_program_id` above.

/// Build the executable account that lives at a native program's address.
///
/// agave 3.x demands BOTH `add_native_instruction_processor` (registers the
/// program with the runtime's BankBuilder) AND `add_account` (puts an
/// executable, NativeLoader-owned account at the program ID so
/// `getAccountInfo` resolves and tx loaders can find it). Without the
/// account, txs that touch the program preflight-reject with
/// `ProgramAccountNotFound` and zero logs / zero CU consumed — exactly
/// what we hit on launch/create, /validators init_subsidy, and any
/// confidential transfer chain referencing a LUT.
///
/// Pattern matches `solana_sdk::native_loader::create_loadable_account_for_test`
/// — executable account, owner = NativeLoader, data = name bytes,
/// rent-exempt minimum lamports for the data length.
pub fn native_program_account(name: &str) -> AccountSharedData {
    use solana_sdk_ids::native_loader;
    let data = name.as_bytes().to_vec();
    let lamports = Rent::default().minimum_balance(data.len()).max(1);
    let mut account = AccountSharedData::new(lamports, data.len(), &native_loader::id());
    account.set_data_from_slice(&data);
    account.set_executable(true);
    account
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_account::ReadableAccount;

    fn dummy_elf(byte: u8, len: usize) -> Vec<u8> {
        vec![byte; len]
    }

    #[test]
    fn build_program_pair_with_authority_yields_executable_program_account() {
        let prog_id = Pubkey::new_unique();
        let pair = build_program_pair_with_authority(prog_id, dummy_elf(0xAA, 1024), None)
            .expect("build");
        assert_eq!(pair.program_id, prog_id);
        assert!(pair.program_account.executable());
        assert_eq!(*pair.program_account.owner(), bpf_loader_upgradeable::id());
        assert_eq!(
            pair.program_account.data().len(),
            UpgradeableLoaderState::size_of_program()
        );
    }

    #[test]
    fn program_account_points_at_program_data_address() {
        let prog_id = Pubkey::new_unique();
        let pair =
            build_program_pair_with_authority(prog_id, dummy_elf(0xBB, 16), None).expect("build");

        // Decode the Program-state header and verify it points at the ProgramData
        // address that `get_program_data_address` produces — this is the round-trip
        // invariant the BPF loader relies on at execution.
        let decoded: UpgradeableLoaderState = bincode::deserialize(pair.program_account.data())
            .expect("Program account data must deserialize");
        match decoded {
            UpgradeableLoaderState::Program { programdata_address } => {
                assert_eq!(programdata_address, pair.program_data_address);
                assert_eq!(programdata_address, get_program_data_address(&prog_id));
            }
            other => panic!("expected Program state, got {other:?}"),
        }
    }

    #[test]
    fn program_data_account_carries_header_then_elf_payload() {
        let prog_id = Pubkey::new_unique();
        let elf = dummy_elf(0xCC, 256);
        let pair =
            build_program_pair_with_authority(prog_id, elf.clone(), None).expect("build");

        assert_eq!(*pair.program_data_account.owner(), bpf_loader_upgradeable::id());
        // Header + ELF length total.
        assert_eq!(
            pair.program_data_account.data().len(),
            UpgradeableLoaderState::size_of_programdata_metadata() + elf.len()
        );
        // The ELF payload must appear verbatim immediately after the header.
        let header_size = UpgradeableLoaderState::size_of_programdata_metadata();
        assert_eq!(&pair.program_data_account.data()[header_size..], &elf[..]);
        assert_eq!(pair.elf_bytes, elf.len());
    }

    #[test]
    fn program_data_header_records_authority() {
        let prog_id = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let pair = build_program_pair_with_authority(
            prog_id,
            dummy_elf(0xDD, 8),
            Some(authority),
        )
        .expect("build");

        // Slice off the header (the first 45 bytes) for separate decoding — bincode
        // doesn't have a "decode and stop" mode, so we deserialize from a slice that
        // ends exactly at the metadata boundary.
        let header_size = UpgradeableLoaderState::size_of_programdata_metadata();
        let header_slice = &pair.program_data_account.data()[..header_size];
        let decoded: UpgradeableLoaderState =
            bincode::deserialize(header_slice).expect("header must deserialize");
        match decoded {
            UpgradeableLoaderState::ProgramData {
                slot,
                upgrade_authority_address,
            } => {
                assert_eq!(slot, 0);
                assert_eq!(upgrade_authority_address, Some(authority));
            }
            other => panic!("expected ProgramData state, got {other:?}"),
        }
    }

    #[test]
    fn program_data_header_omits_authority_when_immutable() {
        let prog_id = Pubkey::new_unique();
        let pair = build_program_pair_with_authority(prog_id, dummy_elf(0xEE, 8), None)
            .expect("build");
        let header_size = UpgradeableLoaderState::size_of_programdata_metadata();
        let header_slice = &pair.program_data_account.data()[..header_size];
        let decoded: UpgradeableLoaderState =
            bincode::deserialize(header_slice).expect("header must deserialize");
        match decoded {
            UpgradeableLoaderState::ProgramData {
                upgrade_authority_address,
                ..
            } => assert_eq!(upgrade_authority_address, None),
            other => panic!("expected ProgramData state, got {other:?}"),
        }
    }

    #[test]
    fn canonical_slots_yields_eight_in_canonical_order() {
        let slots = canonical_slots(None, None, None, None, None, None, None, None);
        assert_eq!(slots.len(), 8);
        assert_eq!(slots[0].program_id, LAZY_CLAIM_PROGRAM_ID);
        assert_eq!(slots[1].program_id, SECRET_PUMP_PROGRAM_ID);
        assert_eq!(slots[2].program_id, MEGADROP_PROGRAM_ID);
        assert_eq!(slots[3].program_id, SPL_TOKEN_PROGRAM_ID);
        assert_eq!(slots[4].program_id, SPL_TOKEN_2022_PROGRAM_ID);
        assert_eq!(slots[5].program_id, SPL_ASSOCIATED_TOKEN_PROGRAM_ID);
        assert_eq!(slots[6].program_id, SPL_MEMO_PROGRAM_ID);
        assert_eq!(slots[7].program_id, address_lookup_table_program_id());
    }

    #[test]
    fn build_program_pair_from_path_reads_file() {
        // Write a tiny synthetic ELF to a temp file and confirm we can read it back
        // through the path-taking entrypoint. We don't validate that the bytes are a
        // real BPF program — that's the runtime's job at execution; here we only care
        // that the wiring round-trips bytes correctly.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("synthetic.so");
        let payload: Vec<u8> = (0..200u8).collect();
        std::fs::write(&path, &payload).expect("write");

        let prog_id = Pubkey::new_unique();
        let pair = build_program_pair_from_path(prog_id, &path).expect("build");
        let header_size = UpgradeableLoaderState::size_of_programdata_metadata();
        assert_eq!(&pair.program_data_account.data()[header_size..], &payload[..]);
    }

    #[test]
    fn zk_elgamal_proof_native_processor_returns_expected_id() {
        let (name, id) = zk_elgamal_proof_native_processor();
        assert_eq!(name, "solana_zk_elgamal_proof_program");
        assert_eq!(id, zk_elgamal_proof_program::id());
        // Sanity-check the well-known base58 form.
        assert_eq!(
            bs58::encode(id.to_bytes()).into_string(),
            "ZkE1Gama1Proof11111111111111111111111111111"
        );
    }

    #[test]
    fn program_accounts_are_rent_exempt() {
        let prog_id = Pubkey::new_unique();
        let pair = build_program_pair_with_authority(prog_id, dummy_elf(0x01, 100), None)
            .expect("build");
        let prog_floor = Rent::default().minimum_balance(pair.program_account.data().len());
        assert_eq!(pair.program_account.lamports(), prog_floor);
        let data_floor =
            Rent::default().minimum_balance(pair.program_data_account.data().len());
        assert_eq!(pair.program_data_account.lamports(), data_floor);
    }
}
