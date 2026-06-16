import assert from "node:assert/strict";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..");

const transport = new StdioClientTransport({
  command: process.execPath,
  args: [path.join(repoRoot, "dist", "index.js")],
  cwd: repoRoot,
  env: { ...process.env },
  stderr: "pipe",
});

let stderr = "";
if (transport.stderr) {
  transport.stderr.setEncoding("utf8");
  transport.stderr.on("data", (chunk) => {
    stderr += chunk.toString();
  });
}

const client = new Client({
  name: "zcash-agent-mcp-smoke",
  version: "0.1.0",
});

try {
  await client.connect(transport);
  const version = client.getServerVersion();
  assert.equal(version?.name, "zcash-agent-mcp");

  const tools = await client.listTools();
  const names = new Set(tools.tools.map((tool) => tool.name));
  for (const expected of [
    "agent_generate_identity",
    "agent_create_profile",
    "agent_create_private_message",
    "agent_decode_memos",
    "agent_build_directory",
    "agent_search_directory",
  ]) {
    assert.ok(names.has(expected), `missing tool ${expected}`);
  }

  const identityResult = await client.callTool({
    name: "agent_generate_identity",
    arguments: {},
  });
  assert.equal(identityResult.isError, undefined);
  const identity = JSON.parse(identityResult.content[0].text);
  assert.match(identity.agent_id, /^agent_zec_[0-9a-f]{32}$/);

  const profileResult = await client.callTool({
    name: "agent_create_profile",
    arguments: {
      identity,
      display_name: "smoke-agent",
      contact_ua: "u1smokeqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq",
      skills: [{ name: "smoke-test" }],
      topics: ["test"],
      directory_ua: "u1directorysmokeqqqqqqqqqqqqqqqqqqqqqqq",
      amount_zat: 1,
    },
  });
  assert.equal(profileResult.isError, undefined);
  const profile = JSON.parse(profileResult.content[0].text);
  assert.ok(profile.payment_uris.length >= 1);

  await client.ping();
} catch (error) {
  const suffix = stderr.trim() ? `\nserver stderr:\n${stderr.trim()}` : "";
  throw new Error(`${error instanceof Error ? error.message : String(error)}${suffix}`);
} finally {
  await transport.close().catch(() => {});
}

console.log("mcp smoke passed");
