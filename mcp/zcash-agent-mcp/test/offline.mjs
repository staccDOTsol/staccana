import assert from "node:assert/strict";
import { URL } from "node:url";

import { buildDirectory, searchDirectory } from "../dist/directory.js";
import { createEnvelope, verifyEnvelope } from "../dist/envelope.js";
import { generateIdentity, sha256Hex } from "../dist/identity.js";
import { decodeMemoFrames, envelopeToMemoFrames, memoToBase64Url } from "../dist/memo.js";
import { envelopeToPaymentUris, formatZecAmount } from "../dist/zip321.js";

const identity = generateIdentity();
assert.match(identity.agent_id, /^agent_zec_[0-9a-f]{32}$/);

const profile = createEnvelope({
  identity,
  kind: "profile",
  payload: {
    display_name: "swap-sage",
    contact_ua: "u1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq",
    skills: [{ name: "quote-swap", hash: sha256Hex("quote-swap skill"), price_zat: 25 }],
    topics: ["zcash", "private-amm"],
    manifest_hash: sha256Hex("SKILL.md"),
    stake_zat: 1000,
  },
});

assert.deepEqual(verifyEnvelope(profile), { ok: true });

const directoryUris = envelopeToPaymentUris({
  address: "u1directoryqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq",
  envelope: profile,
  amountZat: 1,
  label: "profile",
});
assert.ok(directoryUris.length >= 1);
assert.ok(directoryUris.every((item) => item.uri.startsWith("zcash:u1directory")));
assert.ok(directoryUris.every((item) => item.memo_base64url.length > 0));

const decodedProfile = decodeMemoFrames(directoryUris.map((item) => new URL(item.uri).searchParams.get("memo")));
assert.equal(decodedProfile.envelopes.length, 1);
assert.deepEqual(verifyEnvelope(decodedProfile.envelopes[0]), { ok: true });

const directory = buildDirectory(decodedProfile.envelopes);
assert.equal(directory.rejected.length, 0);
assert.equal(directory.entries.length, 1);
assert.equal(searchDirectory(directory, { skill: "quote-swap" }).length, 1);
assert.equal(searchDirectory(directory, { topic: "private-amm" }).length, 1);
assert.equal(searchDirectory(directory, { text: "missing" }).length, 0);

const longMessage = createEnvelope({
  identity,
  kind: "message",
  payload: {
    to: "agent_zec_recipient",
    text: "hello ".repeat(220),
    reply_to_ua: "u1replyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq",
  },
});
const longFrames = envelopeToMemoFrames(longMessage);
assert.ok(longFrames.length > 1);
assert.ok(longFrames.every((frame) => frame.length <= 512));
const decodedLong = decodeMemoFrames(longFrames.map(memoToBase64Url));
assert.equal(decodedLong.envelopes.length, 1);
assert.deepEqual(verifyEnvelope(decodedLong.envelopes[0]), { ok: true });

const tampered = structuredClone(profile);
tampered.b.display_name = "evil";
assert.equal(verifyEnvelope(tampered).ok, false);

assert.equal(formatZecAmount(1), "0.00000001");
assert.equal(formatZecAmount(100000000), "1");
assert.equal(formatZecAmount(123456789), "1.23456789");

console.log("offline tests passed");
