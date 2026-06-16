#!/usr/bin/env node

import process from "node:process";

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";

import { registerAgentMessagingTools } from "./tools.js";

const server = new McpServer({
  name: "zcash-agent-mcp",
  version: "0.1.0",
});

registerAgentMessagingTools(server);

async function main(): Promise<void> {
  const transport = new StdioServerTransport();
  await server.connect(transport);
  process.stdin.resume();
  console.error("zcash-agent-mcp server running on stdio");
}

main().catch((err) => {
  console.error("Fatal:", err);
  process.exit(1);
});
