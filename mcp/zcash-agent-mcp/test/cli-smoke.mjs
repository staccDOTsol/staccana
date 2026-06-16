import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..");
const cli = path.join(repoRoot, "dist", "cli.js");
const temp = mkdtempSync(path.join(tmpdir(), "zam-cli-"));
const config = path.join(temp, "config.json");
const skillFile = path.join(temp, "SKILL.md");

writeFileSync(skillFile, "---\nname: demo\n---\n# Demo\n");

const setup = run(["setup", "--config", config]);
assert.match(setup.identity.agent_id, /^agent_zec_[0-9a-f]{32}$/);

const hash = run(["hash-skill", "--file", skillFile]);
assert.match(hash.hash, /^[0-9a-f]{64}$/);

const profile = run([
  "profile",
  "--config",
  config,
  "--display-name",
  "cli-agent",
  "--contact-ua",
  "u1contactqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq",
  "--directory-ua",
  "u1directoryqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq",
  "--skill",
  `demo,${hash.hash},1`,
  "--topic",
  "test",
  "--manifest-file",
  skillFile,
]);
assert.ok(profile.payment_uris.length >= 1);
assert.ok(profile.frames.length >= 1);
assert.ok(profile.frames.every((frame) => frame.bytes <= 512));

const directory = run(["directory", ...profile.frames.flatMap((frame) => ["--memo", frame.memo_base64url]), "--skill", "demo"]);
assert.equal(directory.results.length, 1);

const message = run(["message", "--config", config, "--to-ua", "u1recipientqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq", "--text", "hello"]);
const decoded = run(["decode", ...message.frames.flatMap((frame) => ["--memo", frame.memo_base64url])]);
assert.equal(decoded.envelopes[0].verification.ok, true);

console.log("cli smoke passed");

function run(args) {
  return JSON.parse(execFileSync(process.execPath, [cli, ...args], { encoding: "utf8" }));
}
