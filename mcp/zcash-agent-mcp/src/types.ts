export const PROTOCOL = "zam/1";

export type EnvelopeKind =
  | "profile"
  | "message"
  | "skill_request"
  | "skill_result"
  | "receipt"
  | "tombstone";

export interface AgentIdentity {
  agent_id: string;
  public_key: string;
  private_key: string;
}

export interface UnsignedEnvelope {
  p: typeof PROTOCOL;
  k: EnvelopeKind;
  s: string;
  pk: string;
  t: number;
  n: string;
  b: unknown;
}

export interface AgentEnvelope extends UnsignedEnvelope {
  sig: string;
}

export interface AgentSkill {
  name: string;
  hash?: string;
  price_zat?: number;
}

export interface AgentProfile {
  display_name: string;
  contact_ua: string;
  skills: AgentSkill[];
  topics: string[];
  manifest_hash?: string;
  homepage?: string;
  stake_zat?: number;
  expires_at?: number;
}

export interface DirectoryEntry extends AgentProfile {
  agent_id: string;
  public_key: string;
  signature: string;
  updated_at: number;
}

export interface DirectoryBuildResult {
  entries: DirectoryEntry[];
  rejected: Array<{ index: number; reason: string }>;
}

export type MemoFrame =
  | {
      v: 1;
      c: 0;
      e: AgentEnvelope;
    }
  | {
      v: 1;
      c: 1;
      id: string;
      i: number;
      n: number;
      d: string;
    };

export interface PaymentUri {
  index: number;
  total: number;
  memo_base64url: string;
  memo_hex: string;
  uri: string;
}
