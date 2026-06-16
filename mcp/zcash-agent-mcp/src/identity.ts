import {
  createHash,
  createPrivateKey,
  createPublicKey,
  generateKeyPairSync,
  randomBytes,
  sign,
  verify,
} from "node:crypto";

import { base64UrlDecode, base64UrlEncode } from "./base64url.js";
import { canonicalJson } from "./canonical.js";
import type { AgentIdentity } from "./types.js";

export function generateIdentity(): AgentIdentity {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const publicDer = publicKey.export({ format: "der", type: "spki" });
  const privateDer = privateKey.export({ format: "der", type: "pkcs8" });
  const public_key = base64UrlEncode(publicDer);
  return {
    agent_id: deriveAgentId(public_key),
    public_key,
    private_key: base64UrlEncode(privateDer),
  };
}

export function deriveAgentId(publicKeyBase64Url: string): string {
  const digest = createHash("sha256").update(base64UrlDecode(publicKeyBase64Url)).digest("hex");
  return `agent_zec_${digest.slice(0, 32)}`;
}

export function randomNonce(): string {
  return base64UrlEncode(randomBytes(16));
}

export function sha256Hex(input: string | Buffer | Uint8Array): string {
  return createHash("sha256").update(input).digest("hex");
}

export function signCanonical(privateKeyBase64Url: string, value: unknown): string {
  const privateKey = createPrivateKey({
    key: base64UrlDecode(privateKeyBase64Url),
    format: "der",
    type: "pkcs8",
  });
  return base64UrlEncode(sign(null, Buffer.from(canonicalJson(value)), privateKey));
}

export function verifyCanonical(publicKeyBase64Url: string, value: unknown, signature: string): boolean {
  const publicKey = createPublicKey({
    key: base64UrlDecode(publicKeyBase64Url),
    format: "der",
    type: "spki",
  });
  return verify(null, Buffer.from(canonicalJson(value)), publicKey, base64UrlDecode(signature));
}
