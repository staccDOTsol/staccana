import { base64UrlDecode, base64UrlEncode } from "./base64url.js";
import { canonicalJson } from "./canonical.js";
import { sha256Hex } from "./identity.js";
import type { AgentEnvelope, MemoFrame } from "./types.js";

export const ZIP302_PRIVATE_PREFIX = 0xff;
export const APP_MAGIC = "ZAM1";
export const MAX_ZCASH_MEMO_BYTES = 512;
const CHUNK_BYTES = 260;

export function envelopeToMemoFrames(envelope: AgentEnvelope): Buffer[] {
  const unchunked: MemoFrame = { v: 1, c: 0, e: envelope };
  const unchunkedMemo = encodeFrame(unchunked, false);
  if (unchunkedMemo.length <= MAX_ZCASH_MEMO_BYTES) {
    return [unchunkedMemo];
  }

  const envelopeBytes = Buffer.from(canonicalJson(envelope), "utf8");
  const id = sha256Hex(envelopeBytes).slice(0, 24);
  const chunks: Buffer[] = [];
  const total = Math.ceil(envelopeBytes.length / CHUNK_BYTES);

  for (let i = 0; i < total; i += 1) {
    const frame: MemoFrame = {
      v: 1,
      c: 1,
      id,
      i,
      n: total,
      d: base64UrlEncode(envelopeBytes.subarray(i * CHUNK_BYTES, (i + 1) * CHUNK_BYTES)),
    };
    chunks.push(encodeFrame(frame, true));
  }

  return chunks;
}

export function memoToBase64Url(memo: Buffer): string {
  return base64UrlEncode(memo);
}

export function memoToHex(memo: Buffer): string {
  return memo.toString("hex");
}

export function decodeMemoFrame(memo: string): MemoFrame {
  const bytes = decodeMemoBytes(memo);
  const trimmed = stripTrailingZeroes(bytes);
  if (trimmed.length < 5 || trimmed[0] !== ZIP302_PRIVATE_PREFIX) {
    throw new Error("not a ZAM private ZIP-302 memo");
  }

  const magic = trimmed.subarray(1, 5).toString("ascii");
  if (magic !== APP_MAGIC) {
    throw new Error(`bad ZAM memo magic ${magic}`);
  }

  return JSON.parse(trimmed.subarray(5).toString("utf8")) as MemoFrame;
}

export function decodeMemoFrames(memos: string[]): { frames: MemoFrame[]; envelopes: AgentEnvelope[] } {
  const frames = memos.map(decodeMemoFrame);
  return {
    frames,
    envelopes: reassembleEnvelopes(frames),
  };
}

export function reassembleEnvelopes(frames: MemoFrame[]): AgentEnvelope[] {
  const envelopes: AgentEnvelope[] = [];
  const chunkGroups = new Map<string, Extract<MemoFrame, { c: 1 }>[]>();

  for (const frame of frames) {
    if (frame.c === 0) {
      envelopes.push(frame.e);
    } else {
      const group = chunkGroups.get(frame.id) ?? [];
      group.push(frame);
      chunkGroups.set(frame.id, group);
    }
  }

  for (const [id, group] of chunkGroups) {
    const total = group[0]?.n ?? 0;
    if (total === 0 || group.length !== total) {
      throw new Error(`incomplete chunk set ${id}: have ${group.length}, need ${total}`);
    }

    group.sort((a, b) => a.i - b.i);
    for (let i = 0; i < group.length; i += 1) {
      if (group[i]?.i !== i || group[i]?.n !== total) {
        throw new Error(`bad chunk ordering for ${id}`);
      }
    }

    const bytes = Buffer.concat(group.map((chunk) => base64UrlDecode(chunk.d)));
    envelopes.push(JSON.parse(bytes.toString("utf8")) as AgentEnvelope);
  }

  return envelopes;
}

function encodeFrame(frame: MemoFrame, enforceLimit: boolean): Buffer {
  const payload = Buffer.from(canonicalJson(frame), "utf8");
  const memo = Buffer.concat([Buffer.from([ZIP302_PRIVATE_PREFIX]), Buffer.from(APP_MAGIC, "ascii"), payload]);
  if (enforceLimit && memo.length > MAX_ZCASH_MEMO_BYTES) {
    throw new Error(`memo frame is ${memo.length} bytes, max ${MAX_ZCASH_MEMO_BYTES}`);
  }
  return memo;
}

function decodeMemoBytes(memo: string): Buffer {
  if (/^[0-9a-fA-F]+$/.test(memo) && memo.length % 2 === 0) {
    return Buffer.from(memo, "hex");
  }
  return base64UrlDecode(memo);
}

function stripTrailingZeroes(bytes: Buffer): Buffer {
  let end = bytes.length;
  while (end > 0 && bytes[end - 1] === 0) {
    end -= 1;
  }
  return bytes.subarray(0, end);
}
