/**
 * Token-22 mint initialization for the staccana launchpad.
 *
 * Builds the prelude instructions that the launchpad runs BEFORE
 * `secret_pump::create` to stand up a fresh Token-22 mint with:
 *
 *   1. ConfidentialTransfer mint extension (matches the staccana confidentiality
 *      guarantees — token amounts encrypted on transfer, plaintext only inside
 *      the bonding curve vault).
 *   2. MetadataPointer extension pointing back at the mint itself.
 *   3. Base mint init (decimals = 9, mint_authority initially = payer so the
 *      payer can sign the metadata-init ix below; freeze_authority = None).
 *   4. TokenMetadata extension `Initialize` (mint authority = payer signs).
 *   5. SetAuthority transfer of mint_authority from payer → curve PDA, so only
 *      `secret_pump` can mint after this point. The curve PDA mints the
 *      VIRTUAL_TOKENS allocation into the vault inside `secret_pump::create`.
 *
 * Rent is computed exactly via `getMintLen` + `pack(metadata).length` — no fixed
 * 200-byte cap. Once the mint is rent-funded, the caller follows up with
 * `secret_pump::create` to wire up the bonding curve PDA + vault on top.
 *
 * The on-chain `secret_pump::create` handler validates that mint_authority ==
 * curve PDA, decimals == 9, and supply == 0 before allocating the curve PDA.
 */

import {
  AuthorityType,
  ExtensionType,
  LENGTH_SIZE,
  TOKEN_2022_PROGRAM_ID,
  TYPE_SIZE,
  createInitializeMetadataPointerInstruction,
  createInitializeMintInstruction,
  createSetAuthorityInstruction,
  getMintLen,
} from "@solana/spl-token";
import {
  createInitializeInstruction,
  createUpdateFieldInstruction,
  pack,
  type TokenMetadata,
} from "@solana/spl-token-metadata";
import {
  Connection,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
} from "@solana/web3.js";

import { bondingCurvePda } from "./pump";

/**
 * Off-chain metadata fields that survive the round trip into the Token-22
 * TokenMetadata extension. `name` / `symbol` / `uri` are the canonical fields;
 * everything else lands in `additionalMetadata` as `[key, value]` pairs (which
 * is exactly how Token-22 stores them — the spec guarantees the order is
 * preserved on read).
 */
export interface MintMetadataFields {
  name: string;
  symbol: string;
  /** Off-chain JSON URI (typically a Vercel Blob URL pointing at a metadata.json). */
  uri: string;
  /** Optional flat key/value extras; e.g. `[["twitter", "https://x.com/foo"], ...]`. */
  additionalMetadata?: Array<[string, string]>;
}

/**
 * Inputs for `buildMintInitInstructions`.
 */
export interface BuildMintInitArgs {
  connection: Connection;
  /** Pays rent for the mint account. Same wallet as the curve creator. */
  payer: PublicKey;
  /** The fresh mint keypair's pubkey. Must sign the tx (createAccount needs the seed signer). */
  mint: PublicKey;
  /** Token-22 metadata fields (name/symbol/uri/additional). */
  metadata: MintMetadataFields;
  /** Mint decimals. Must match the on-chain curve invariant (9). */
  decimals?: number;
}

/**
 * Manually build the InitializeConfidentialTransferMint instruction. The 0.4.x
 * line of `@solana/spl-token` does not expose a typed builder for the
 * Confidential Transfer extension, so we encode the wire format directly per
 * `spl_token_2022/extension/confidential_transfer/instruction.rs`:
 *
 * - byte 0:           TokenInstruction::ConfidentialTransferExtension (= 27)
 * - byte 1:           ConfidentialTransferInstruction::InitializeMint (= 0)
 * - bytes 2..34:      authority (32) — Pubkey, all-zeros = None
 * - byte  34:         auto_approve_new_accounts (bool, 1 = true)
 * - bytes 35..67:     auditor_elgamal_pubkey (32) — all-zeros = None
 *
 * Defaults match the staccana program's mint-creation path:
 * `auto_approve_new_accounts = true`, no protocol decryption back-door, no
 * authority (immutable).
 */
function buildInitializeConfidentialTransferMintIx(mint: PublicKey): TransactionInstruction {
  const data = new Uint8Array(1 + 1 + 32 + 1 + 32);
  data[0] = 27; // TokenInstruction::ConfidentialTransferExtension
  data[1] = 0; // ConfidentialTransferInstruction::InitializeMint
  // authority: zero-filled = None
  data[34] = 1; // auto_approve_new_accounts = true
  // auditor pubkey: zero-filled = None
  return new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [{ pubkey: mint, isWritable: true, isSigner: false }],
    data: Buffer.from(data),
  });
}

/**
 * Build the prelude instructions that initialize a Token-22 mint with the
 * MetadataPointer + TokenMetadata + ConfidentialTransfer extensions.
 *
 * Returns the full set of ixs plus the computed metadata account size — the
 * latter is exposed for UI display (so the launchpad can show users the
 * actual on-chain footprint instead of the old hardcoded 200-byte cap).
 */
export async function buildMintInitInstructions(args: BuildMintInitArgs): Promise<{
  instructions: TransactionInstruction[];
  mintLen: number;
  metadataLen: number;
  totalLamports: number;
}> {
  const decimals = args.decimals ?? 9;
  const curvePda = bondingCurvePda(args.mint);

  // Token-22 TokenMetadata is a *variable-length* extension; we don't include
  // it in `getMintLen` — the runtime allocates it on first `Initialize` call
  // via realloc. We size the account for the fixed-length extensions
  // (MetadataPointer + ConfidentialTransfer) and pre-pay rent for the metadata
  // blob via the lamports we send to `createAccount`.
  const fixedExtensions = [
    ExtensionType.MetadataPointer,
    ExtensionType.ConfidentialTransferMint,
  ];
  const mintLen = getMintLen(fixedExtensions);

  const metadata: TokenMetadata = {
    mint: args.mint,
    name: args.metadata.name,
    symbol: args.metadata.symbol,
    uri: args.metadata.uri,
    additionalMetadata: args.metadata.additionalMetadata ?? [],
  };
  // The metadata extension's TLV record is `type (2) | length (2) | pack(metadata)`.
  const metadataLen = TYPE_SIZE + LENGTH_SIZE + pack(metadata).length;

  // Rent must cover mint base + fixed extensions + the metadata extension that
  // gets realloc'd in by `createInitializeInstruction`.
  const totalLamports = await args.connection.getMinimumBalanceForRentExemption(
    mintLen + metadataLen,
  );

  const instructions: TransactionInstruction[] = [
    SystemProgram.createAccount({
      fromPubkey: args.payer,
      newAccountPubkey: args.mint,
      space: mintLen,
      lamports: totalLamports,
      programId: TOKEN_2022_PROGRAM_ID,
    }),
    // MetadataPointer must be initialized BEFORE InitializeMint.
    createInitializeMetadataPointerInstruction(
      args.mint,
      args.payer, // pointer-update authority — payer can repoint later
      args.mint, // metadataAddress — points at the mint itself
      TOKEN_2022_PROGRAM_ID,
    ),
    // ConfidentialTransfer also goes before InitializeMint.
    buildInitializeConfidentialTransferMintIx(args.mint),
    // Now the base mint. We TEMPORARILY set `mint_authority = payer` so the
    // payer can sign the TokenMetadata `Initialize` ix below (which requires
    // the *mint authority* as a signer). We hand mint authority over to the
    // curve PDA in the SetAuthority ix that follows the metadata init.
    createInitializeMintInstruction(
      args.mint,
      decimals,
      args.payer,
      null, // freeze authority intentionally None — no rug
      TOKEN_2022_PROGRAM_ID,
    ),
    // TokenMetadata `Initialize`. Realloc-allocates the metadata TLV record on
    // the mint and writes name/symbol/uri/additionalMetadata. The payer signs
    // as both `mintAuthority` (currently held) and `updateAuthority` (which
    // they retain post-launch so they can edit social links etc.).
    createInitializeInstruction({
      programId: TOKEN_2022_PROGRAM_ID,
      mint: args.mint,
      metadata: args.mint,
      name: metadata.name,
      symbol: metadata.symbol,
      uri: metadata.uri,
      mintAuthority: args.payer,
      updateAuthority: args.payer,
    }),
    // Hand mint authority over to the curve PDA. Once this ix lands, only
    // `secret_pump::create` (running with curve-PDA signer seeds) can mint
    // new tokens. It does so exactly once, to seed the vault with VIRTUAL_TOKENS.
    createSetAuthorityInstruction(
      args.mint,
      args.payer,
      AuthorityType.MintTokens,
      curvePda,
      [],
      TOKEN_2022_PROGRAM_ID,
    ),
  ];

  // If the caller passed additional metadata key/value pairs, the
  // `createInitializeInstruction` above does NOT include them — only `name`,
  // `symbol`, and `uri` are part of the initialize payload. Append a
  // `createUpdateFieldInstruction` per extra key. We import that lazily so the
  // common path (no socials) doesn't pay the import cost; here we just defer
  // to the same module and append.
  // updateAuthority is still the payer at this point (SetAuthority above
  // only touched MintTokens, not the metadata update authority).
  for (const [field, value] of metadata.additionalMetadata) {
    instructions.push(
      createUpdateFieldInstruction({
        programId: TOKEN_2022_PROGRAM_ID,
        metadata: args.mint,
        updateAuthority: args.payer,
        field,
        value,
      }),
    );
  }

  return { instructions, mintLen, metadataLen, totalLamports };
}
