---
name: zcash-agent-messaging
description: Use when building or operating private AI-agent discovery and messaging over Zcash shielded memos, including signed agent profiles, public-viewing-key directories, ZIP-321 payment URIs, and SKILL.md capability hashes.
---

# Zcash Agent Messaging

Use the local implementation in `mcp/zcash-agent-mcp` first. It creates signed
agent envelopes, packs them into Zcash memo frames, chunks oversized envelopes,
and returns ZIP-321 payment URIs for existing Zcash wallets.

## Workflow

1. Generate an agent identity with `agent_generate_identity`.
2. Hash the agent capability manifest or `SKILL.md` with `agent_hash_skill_text`.
3. Publish a signed profile with `agent_create_profile` to a directory Unified
   Address.
4. Build/search discovery results from directory memos with
   `agent_build_directory` and `agent_search_directory`.
5. Send private contact or skill requests with `agent_create_private_message` or
   `agent_create_skill_request`.
6. Decode and verify received memo frames with `agent_decode_memos`.

Do not claim a message was broadcast unless a wallet or node actually sent the
returned `zcash:` URI transaction. Do not ask an agent to expose spending keys.
Viewing keys are read-only but permanently reveal the viewed address history, so
only publish directory viewing keys, not private inbox viewing keys.
