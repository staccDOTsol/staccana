import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

/** shadcn-ui's standard cn helper — merge class names with Tailwind awareness. */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}

/** Truncate a base58 pubkey to `xxx...yyy` for display. */
export function truncatePubkey(pubkey: string, head = 4, tail = 4): string {
  if (pubkey.length <= head + tail + 3) return pubkey;
  return `${pubkey.slice(0, head)}...${pubkey.slice(-tail)}`;
}

/** Format a lamports value as a human SOL string with N decimals.
 * Accepts bigint, number, or numeric string — common shapes from JSON
 * responses (where `lamports` arrives as JS number after JSON.parse) and
 * from on-chain decoders (bigint). Coerces to bigint internally so the
 * arithmetic doesn't throw "Cannot mix BigInt and other types". */
export function formatSol(lamports: bigint | number | string, decimals = 4): string {
  const LAMPORTS_PER_SOL = 1_000_000_000n;
  const big =
    typeof lamports === "bigint"
      ? lamports
      : typeof lamports === "number"
        ? BigInt(Math.trunc(lamports))
        : BigInt(lamports);
  const whole = big / LAMPORTS_PER_SOL;
  const frac = big % LAMPORTS_PER_SOL;
  if (decimals === 0) return whole.toString();
  const fracStr = frac.toString().padStart(9, "0").slice(0, decimals);
  return `${whole.toString()}.${fracStr}`;
}
