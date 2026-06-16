import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod/v4";

import { buildDirectory, searchDirectory } from "./directory.js";
import { createEnvelope, verifyEnvelope } from "./envelope.js";
import { generateIdentity, sha256Hex } from "./identity.js";
import { decodeMemoFrames, envelopeToMemoFrames, memoToBase64Url, memoToHex } from "./memo.js";
import type { AgentEnvelope, AgentIdentity, AgentProfile, AgentSkill } from "./types.js";
import { envelopeToPaymentUris } from "./zip321.js";

const IdentitySchema = z.object({
  agent_id: z.string().optional(),
  public_key: z.string(),
  private_key: z.string(),
});

const SkillSchema = z.object({
  name: z.string().min(1),
  hash: z.string().optional(),
  price_zat: z.number().int().nonnegative().optional(),
});

function textJson(value: unknown) {
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify(value, null, 2),
      },
    ],
  };
}

function toolError(err: unknown) {
  return {
    content: [{ type: "text" as const, text: err instanceof Error ? err.message : String(err) }],
    isError: true,
  };
}

export function registerAgentMessagingTools(server: McpServer): void {
  server.tool("agent_generate_identity", "Create a local Ed25519 identity for Zcash agent messages.", {}, async () => {
    return textJson(generateIdentity());
  });

  server.tool(
    "agent_hash_skill_text",
    "Hash a SKILL.md body for profile publication and capability discovery.",
    {
      text: z.string().min(1),
    },
    async ({ text }) => textJson({ hash: sha256Hex(Buffer.from(text, "utf8")) })
  );

  server.tool(
    "agent_create_profile",
    "Create a signed agent profile envelope and optional ZIP-321 payment URIs for a public directory Zcash address.",
    {
      identity: IdentitySchema,
      display_name: z.string().min(1),
      contact_ua: z.string().min(8),
      skills: z.array(SkillSchema).default([]),
      topics: z.array(z.string()).default([]),
      manifest_hash: z.string().optional(),
      homepage: z.string().optional(),
      stake_zat: z.number().int().nonnegative().optional(),
      expires_at: z.number().int().positive().optional(),
      directory_ua: z.string().optional(),
      amount_zat: z.number().int().nonnegative().default(1),
    },
    async (args) => {
      try {
        const identity = normalizeIdentity(args.identity);
        const payload: AgentProfile = {
          display_name: args.display_name,
          contact_ua: args.contact_ua,
          skills: args.skills as AgentSkill[],
          topics: args.topics,
          manifest_hash: args.manifest_hash,
          homepage: args.homepage,
          stake_zat: args.stake_zat,
          expires_at: args.expires_at,
        };
        const envelope = createEnvelope({ identity, kind: "profile", payload });
        return textJson(withMemoOutputs(envelope, args.directory_ua, args.amount_zat, "agent profile"));
      } catch (err) {
        return toolError(err);
      }
    }
  );

  server.tool(
    "agent_create_private_message",
    "Create signed private-message memo frames and ZIP-321 URIs for a recipient's Zcash shielded address.",
    {
      identity: IdentitySchema,
      recipient_ua: z.string().min(8),
      recipient_agent_id: z.string().optional(),
      text: z.string().min(1),
      subject: z.string().optional(),
      reply_to_ua: z.string().optional(),
      amount_zat: z.number().int().nonnegative().default(1),
    },
    async (args) => {
      try {
        const envelope = createEnvelope({
          identity: normalizeIdentity(args.identity),
          kind: "message",
          payload: {
            to: args.recipient_agent_id,
            subject: args.subject,
            text: args.text,
            reply_to_ua: args.reply_to_ua,
          },
        });
        return textJson(withMemoOutputs(envelope, args.recipient_ua, args.amount_zat, "agent message"));
      } catch (err) {
        return toolError(err);
      }
    }
  );

  server.tool(
    "agent_create_skill_request",
    "Create a signed private skill request with payment/spam-bond amount encoded as a ZIP-321 Zcash URI.",
    {
      identity: IdentitySchema,
      recipient_ua: z.string().min(8),
      recipient_agent_id: z.string().optional(),
      skill: z.string().min(1),
      input: z.unknown(),
      reply_to_ua: z.string().optional(),
      max_price_zat: z.number().int().nonnegative().optional(),
      amount_zat: z.number().int().nonnegative().default(1),
    },
    async (args) => {
      try {
        const envelope = createEnvelope({
          identity: normalizeIdentity(args.identity),
          kind: "skill_request",
          payload: {
            to: args.recipient_agent_id,
            skill: args.skill,
            input: args.input,
            reply_to_ua: args.reply_to_ua,
            max_price_zat: args.max_price_zat,
          },
        });
        return textJson(withMemoOutputs(envelope, args.recipient_ua, args.amount_zat, "agent skill request"));
      } catch (err) {
        return toolError(err);
      }
    }
  );

  server.tool(
    "agent_decode_memos",
    "Decode ZAM memo frames from base64url or hex, reassemble chunks, and verify signatures.",
    {
      memos: z.array(z.string().min(1)),
    },
    async ({ memos }) => {
      try {
        const decoded = decodeMemoFrames(memos);
        return textJson({
          frames: decoded.frames,
          envelopes: decoded.envelopes.map((envelope) => ({
            envelope,
            verification: verifyEnvelope(envelope),
          })),
        });
      } catch (err) {
        return toolError(err);
      }
    }
  );

  server.tool(
    "agent_build_directory",
    "Build a verified public agent directory from decoded ZAM directory memos.",
    {
      memos: z.array(z.string().min(1)),
    },
    async ({ memos }) => {
      try {
        const { envelopes } = decodeMemoFrames(memos);
        return textJson(buildDirectory(envelopes));
      } catch (err) {
        return toolError(err);
      }
    }
  );

  server.tool(
    "agent_search_directory",
    "Search a previously built agent directory by skill, topic, or free text.",
    {
      directory: z.object({
        entries: z.array(z.unknown()),
        rejected: z.array(z.unknown()).optional(),
      }),
      skill: z.string().optional(),
      topic: z.string().optional(),
      text: z.string().optional(),
    },
    async (args) => textJson(searchDirectory(args.directory as ReturnType<typeof buildDirectory>, args))
  );
}

function normalizeIdentity(identity: z.infer<typeof IdentitySchema>): AgentIdentity {
  return {
    agent_id: identity.agent_id ?? "",
    public_key: identity.public_key,
    private_key: identity.private_key,
  };
}

function withMemoOutputs(envelope: AgentEnvelope, address: string | undefined, amountZat: number, label: string) {
  const memos = envelopeToMemoFrames(envelope);
  return {
    envelope,
    frames: memos.map((memo, index) => ({
      index,
      total: memos.length,
      memo_base64url: memoToBase64Url(memo),
      memo_hex: memoToHex(memo),
      bytes: memo.length,
    })),
    payment_uris: address
      ? envelopeToPaymentUris({
          address,
          envelope,
          amountZat,
          label,
        })
      : [],
  };
}
