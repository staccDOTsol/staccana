import { envelopeToMemoFrames, memoToBase64Url, memoToHex } from "./memo.js";
import type { AgentEnvelope, PaymentUri } from "./types.js";

const ZATOSHIS_PER_ZEC = 100_000_000n;

export function envelopeToPaymentUris(args: {
  address: string;
  envelope: AgentEnvelope;
  amountZat?: number;
  label?: string;
}): PaymentUri[] {
  const memos = envelopeToMemoFrames(args.envelope);
  return memos.map((memo, index) => ({
    index,
    total: memos.length,
    memo_base64url: memoToBase64Url(memo),
    memo_hex: memoToHex(memo),
    uri: createPaymentUri({
      address: args.address,
      amountZat: args.amountZat ?? 1,
      memo,
      label: memos.length > 1 ? `${args.label ?? "ZAM"} ${index + 1}/${memos.length}` : args.label,
    }),
  }));
}

export function createPaymentUri(args: {
  address: string;
  amountZat: number;
  memo: Buffer;
  label?: string;
}): string {
  validateZcashAddress(args.address);
  if (!Number.isInteger(args.amountZat) || args.amountZat < 0) {
    throw new Error("amountZat must be a non-negative integer");
  }

  const params = new URLSearchParams();
  params.set("amount", formatZecAmount(args.amountZat));
  params.set("memo", memoToBase64Url(args.memo));
  if (args.label) {
    params.set("label", args.label);
  }
  return `zcash:${args.address}?${params.toString()}`;
}

export function formatZecAmount(zat: number): string {
  const value = BigInt(zat);
  const whole = value / ZATOSHIS_PER_ZEC;
  const fraction = value % ZATOSHIS_PER_ZEC;
  if (fraction === 0n) {
    return whole.toString();
  }
  return `${whole}.${fraction.toString().padStart(8, "0").replace(/0+$/, "")}`;
}

function validateZcashAddress(address: string): void {
  if (!/^(u1|utest|zs1|ztestsapling|t1|t3|tm)[A-Za-z0-9]+$/.test(address)) {
    throw new Error("address does not look like a Zcash address");
  }
}
