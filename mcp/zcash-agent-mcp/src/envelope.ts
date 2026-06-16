import { deriveAgentId, randomNonce, signCanonical, verifyCanonical } from "./identity.js";
import { PROTOCOL, type AgentEnvelope, type AgentIdentity, type EnvelopeKind, type UnsignedEnvelope } from "./types.js";

export interface CreateEnvelopeArgs {
  identity: AgentIdentity;
  kind: EnvelopeKind;
  payload: unknown;
  createdAt?: number;
  nonce?: string;
}

export function createEnvelope(args: CreateEnvelopeArgs): AgentEnvelope {
  const sender = args.identity.agent_id || deriveAgentId(args.identity.public_key);
  const unsigned: UnsignedEnvelope = {
    p: PROTOCOL,
    k: args.kind,
    s: sender,
    pk: args.identity.public_key,
    t: args.createdAt ?? Math.floor(Date.now() / 1000),
    n: args.nonce ?? randomNonce(),
    b: args.payload,
  };
  return {
    ...unsigned,
    sig: signCanonical(args.identity.private_key, unsigned),
  };
}

export function unsignedEnvelope(envelope: AgentEnvelope): UnsignedEnvelope {
  return {
    p: envelope.p,
    k: envelope.k,
    s: envelope.s,
    pk: envelope.pk,
    t: envelope.t,
    n: envelope.n,
    b: envelope.b,
  };
}

export function verifyEnvelope(envelope: AgentEnvelope): { ok: true } | { ok: false; error: string } {
  if (envelope.p !== PROTOCOL) {
    return { ok: false, error: `unsupported protocol ${String(envelope.p)}` };
  }

  const derived = deriveAgentId(envelope.pk);
  if (envelope.s !== derived) {
    return { ok: false, error: `sender ${envelope.s} does not match public key ${derived}` };
  }

  if (!verifyCanonical(envelope.pk, unsignedEnvelope(envelope), envelope.sig)) {
    return { ok: false, error: "signature verification failed" };
  }

  return { ok: true };
}
