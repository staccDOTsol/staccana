/**
 * Known bridged-token metadata fallbacks.
 *
 * The staccana-side bridge mirror mints (Token-22 with mint authority pinned to
 * the bridge program's per-asset PDA) do NOT have the on-chain `TokenMetadata`
 * Token-22 extension populated — they're created bare so the bridge can mint
 * to them with no metadata-update CPI overhead. That makes the picker / wallet
 * UI render them as "Unknown Token" which is a poor UX for "for the culture"
 * tokens that DO have well-known names/symbols on the source chain.
 *
 * This file holds a hand-maintained map of `mirror_mint_b58 → metadata` that
 * the UI consults as a fallback when `readTokenMetadataSymbol` returns null.
 * Long-term the right answer is to read the source-chain metadata directly via
 * a mainnet RPC fetch keyed on `mainnet_mint`, but that's a network round-trip
 * per render and we only have one bridged asset at v1 launch — a static map is
 * cheaper and more reliable.
 *
 * Add a new entry here whenever a new asset is registered on the bridge.
 */

export type BridgedTokenMeta = {
  /** Display symbol (max ~12 chars). What appears in the picker dropdown. */
  symbol: string;
  /** Full display name. Used in tooltips / detail screens. */
  name: string;
  /** Source-chain mint address (mainnet) — informational, used in tooltips
   *  + by future fetchers that pull live metadata. */
  mainnetMint: string;
  /** Optional logo URL — the source-chain asset's icon. Empty string ⇒ no logo. */
  logoURI?: string;
};

/**
 * Map keyed on the **staccana mirror mint** base58 string. Lookup is exact-match
 * — we never want to misattribute a stranger token's metadata to a different
 * mirror mint.
 */
export const BRIDGED_TOKEN_METADATA: Record<string, BridgedTokenMeta> = {
  // asset_id=3 — "Solana Fork Staccana" community token, the only bridged asset
  // at v1 launch. Mainnet mint is the pump.fun token; "for the culture, not an
  // endorsement" per the disclaimer banner on /bridge.
  DbjnN9ZeSZRy3U2shsdJtfEu5cLsgQeg8hBXBU28Zxqi: {
    symbol: "Staccana",
    name: "Solana Fork Staccana (bridged)",
    mainnetMint: "73edX6xoGY4v5y2hzuKdrUbJXLntqgmo74au1Ki1pump",
    logoURI: "",
  },
};

/**
 * Look up bridged-token metadata by staccana mirror mint. Returns `null` when
 * the mint isn't a known bridge mirror (i.e. it's a normal staccana-native
 * Token-22 that should fall back to its on-chain TokenMetadata extension).
 */
export function lookupBridgedTokenMetadata(
  mirrorMintB58: string,
): BridgedTokenMeta | null {
  return BRIDGED_TOKEN_METADATA[mirrorMintB58] ?? null;
}
