import { verifyEnvelope } from "./envelope.js";
import type { AgentEnvelope, AgentProfile, DirectoryBuildResult, DirectoryEntry } from "./types.js";

export function buildDirectory(envelopes: AgentEnvelope[]): DirectoryBuildResult {
  const byAgent = new Map<string, DirectoryEntry>();
  const rejected: DirectoryBuildResult["rejected"] = [];

  envelopes.forEach((envelope, index) => {
    const verification = verifyEnvelope(envelope);
    if (!verification.ok) {
      rejected.push({ index, reason: verification.error });
      return;
    }

    if (envelope.k !== "profile") {
      return;
    }

    const profile = envelope.b as Partial<AgentProfile>;
    const reason = validateProfile(profile);
    if (reason) {
      rejected.push({ index, reason });
      return;
    }

    const entry: DirectoryEntry = {
      ...(profile as AgentProfile),
      agent_id: envelope.s,
      public_key: envelope.pk,
      signature: envelope.sig,
      updated_at: envelope.t,
    };
    const existing = byAgent.get(envelope.s);
    if (!existing || entry.updated_at >= existing.updated_at) {
      byAgent.set(envelope.s, entry);
    }
  });

  return {
    entries: [...byAgent.values()].sort((a, b) => b.updated_at - a.updated_at),
    rejected,
  };
}

export function searchDirectory(
  directory: DirectoryBuildResult,
  query: { skill?: string; topic?: string; text?: string }
): DirectoryEntry[] {
  const skill = query.skill?.toLowerCase();
  const topic = query.topic?.toLowerCase();
  const text = query.text?.toLowerCase();

  return directory.entries.filter((entry) => {
    if (skill && !entry.skills.some((item) => item.name.toLowerCase() === skill || item.hash?.toLowerCase() === skill)) {
      return false;
    }
    if (topic && !entry.topics.some((item) => item.toLowerCase() === topic)) {
      return false;
    }
    if (text) {
      const haystack = [
        entry.agent_id,
        entry.display_name,
        entry.contact_ua,
        entry.manifest_hash,
        entry.homepage,
        ...entry.topics,
        ...entry.skills.flatMap((item) => [item.name, item.hash ?? ""]),
      ]
        .join(" ")
        .toLowerCase();
      return haystack.includes(text);
    }
    return true;
  });
}

function validateProfile(profile: Partial<AgentProfile>): string | null {
  if (!profile || typeof profile !== "object") {
    return "profile payload must be an object";
  }
  if (!profile.display_name || typeof profile.display_name !== "string") {
    return "profile display_name is required";
  }
  if (!profile.contact_ua || typeof profile.contact_ua !== "string") {
    return "profile contact_ua is required";
  }
  if (!Array.isArray(profile.skills)) {
    return "profile skills must be an array";
  }
  if (!Array.isArray(profile.topics)) {
    return "profile topics must be an array";
  }
  return null;
}
