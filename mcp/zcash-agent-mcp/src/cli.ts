#!/usr/bin/env node

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import process from "node:process";

import { buildDirectory, searchDirectory } from "./directory.js";
import { createEnvelope, verifyEnvelope } from "./envelope.js";
import { generateIdentity, sha256Hex } from "./identity.js";
import { decodeMemoFrames, envelopeToMemoFrames, memoToBase64Url, memoToHex } from "./memo.js";
import type { AgentIdentity, AgentProfile, AgentSkill, EnvelopeKind } from "./types.js";
import { envelopeToPaymentUris } from "./zip321.js";

const DEFAULT_CONFIG = "~/.config/zcash-agent/config.json";

interface ParsedArgs {
  positional: string[];
  flags: Map<string, string[]>;
}

function main(): void {
  try {
    const [command, ...rest] = process.argv.slice(2);
    if (!command || command === "help" || command === "--help" || command === "-h") {
      printHelp();
      return;
    }

    const args = parseArgs(rest);
    switch (command) {
      case "identity":
        runIdentity(args);
        break;
      case "setup":
        runSetup(args);
        break;
      case "hash-skill":
        runHashSkill(args);
        break;
      case "profile":
        runProfile(args);
        break;
      case "message":
        runMessage(args);
        break;
      case "skill-request":
        runSkillRequest(args);
        break;
      case "decode":
        runDecode(args);
        break;
      case "directory":
        runDirectory(args);
        break;
      default:
        throw new Error(`unknown command ${command}`);
    }
  } catch (err) {
    console.error(err instanceof Error ? err.message : String(err));
    process.exit(1);
  }
}

function runIdentity(args: ParsedArgs): void {
  const subcommand = args.positional[0];
  if (subcommand !== "new") {
    throw new Error("usage: zam identity new [--out path] [--force]");
  }
  const identity = generateIdentity();
  const out = getFlag(args, "out");
  if (out) {
    writeJsonSecure(resolvePath(out), identity, hasFlag(args, "force"));
  }
  printJson(identity);
}

function runSetup(args: ParsedArgs): void {
  const configPath = resolvePath(getFlag(args, "config") ?? DEFAULT_CONFIG);
  const identity = generateIdentity();
  const config = {
    version: 1,
    created_at: Math.floor(Date.now() / 1000),
    identity,
    directory_ua: getFlag(args, "directory-ua") ?? "TODO_CREATE_DIRECTORY_UNIFIED_ADDRESS",
    directory_incoming_viewing_key: getFlag(args, "directory-ivk") ?? "TODO_EXPORT_DIRECTORY_INCOMING_VIEWING_KEY",
    contact_ua: getFlag(args, "contact-ua") ?? "TODO_CREATE_AGENT_INBOX_UNIFIED_ADDRESS",
  };
  writeJsonSecure(configPath, config, hasFlag(args, "force"));
  printJson({ config_path: configPath, ...config });
}

function runHashSkill(args: ParsedArgs): void {
  const text = readTextArg(args, "text", "file");
  printJson({ hash: sha256Hex(Buffer.from(text, "utf8")) });
}

function runProfile(args: ParsedArgs): void {
  const identity = readIdentity(args);
  const manifestHash = getFlag(args, "manifest-hash") ?? maybeHashFile(getFlag(args, "manifest-file"));
  const payload: AgentProfile = {
    display_name: requireFlag(args, "display-name"),
    contact_ua: requireFlag(args, "contact-ua"),
    skills: getFlags(args, "skill").map(parseSkill),
    topics: getFlags(args, "topic"),
    manifest_hash: manifestHash,
    homepage: getFlag(args, "homepage"),
    stake_zat: maybeNumber(getFlag(args, "stake-zat")),
    expires_at: maybeNumber(getFlag(args, "expires-at")),
  };
  emitEnvelope("profile", identity, payload, getFlag(args, "directory-ua"), maybeNumber(getFlag(args, "amount-zat")) ?? 1, "agent profile");
}

function runMessage(args: ParsedArgs): void {
  const payload = {
    to: getFlag(args, "to-agent"),
    subject: getFlag(args, "subject"),
    text: readTextArg(args, "text", "file"),
    reply_to_ua: getFlag(args, "reply-to-ua"),
  };
  emitEnvelope(
    "message",
    readIdentity(args),
    payload,
    requireFlag(args, "to-ua"),
    maybeNumber(getFlag(args, "amount-zat")) ?? 1,
    "agent message"
  );
}

function runSkillRequest(args: ParsedArgs): void {
  const inputRaw = readTextArg(args, "input-json", "input-file");
  const payload = {
    to: getFlag(args, "to-agent"),
    skill: requireFlag(args, "skill"),
    input: JSON.parse(inputRaw),
    reply_to_ua: getFlag(args, "reply-to-ua"),
    max_price_zat: maybeNumber(getFlag(args, "max-price-zat")),
  };
  emitEnvelope(
    "skill_request",
    readIdentity(args),
    payload,
    requireFlag(args, "to-ua"),
    maybeNumber(getFlag(args, "amount-zat")) ?? 1,
    "agent skill request"
  );
}

function runDecode(args: ParsedArgs): void {
  const memos = readMemoArgs(args);
  const decoded = decodeMemoFrames(memos);
  printJson({
    frames: decoded.frames,
    envelopes: decoded.envelopes.map((envelope) => ({
      envelope,
      verification: verifyEnvelope(envelope),
    })),
  });
}

function runDirectory(args: ParsedArgs): void {
  const { envelopes } = decodeMemoFrames(readMemoArgs(args));
  const directory = buildDirectory(envelopes);
  const skill = getFlag(args, "skill");
  const topic = getFlag(args, "topic");
  const text = getFlag(args, "text");
  printJson({
    directory,
    results: skill || topic || text ? searchDirectory(directory, { skill, topic, text }) : directory.entries,
  });
}

function emitEnvelope(kind: EnvelopeKind, identity: AgentIdentity, payload: unknown, address: string | undefined, amountZat: number, label: string): void {
  const envelope = createEnvelope({ identity, kind, payload });
  const memos = envelopeToMemoFrames(envelope);
  printJson({
    envelope,
    frames: memos.map((memo, index) => ({
      index,
      total: memos.length,
      memo_base64url: memoToBase64Url(memo),
      memo_hex: memoToHex(memo),
      bytes: memo.length,
    })),
    payment_uris: address ? envelopeToPaymentUris({ address, envelope, amountZat, label }) : [],
  });
}

function readIdentity(args: ParsedArgs): AgentIdentity {
  const identityPath = getFlag(args, "identity");
  if (identityPath) {
    return JSON.parse(readFileSync(resolvePath(identityPath), "utf8")) as AgentIdentity;
  }

  const configPath = resolvePath(getFlag(args, "config") ?? DEFAULT_CONFIG);
  const config = JSON.parse(readFileSync(configPath, "utf8")) as { identity?: AgentIdentity };
  if (!config.identity) {
    throw new Error(`config ${configPath} does not contain identity`);
  }
  return config.identity;
}

function readMemoArgs(args: ParsedArgs): string[] {
  const memos = getFlags(args, "memo");
  const file = getFlag(args, "file");
  if (file) {
    const parsed = JSON.parse(readFileSync(resolvePath(file), "utf8")) as string[];
    memos.push(...parsed);
  }
  if (memos.length === 0) {
    throw new Error("provide at least one --memo or --file");
  }
  return memos;
}

function parseSkill(value: string): AgentSkill {
  const [name, hash, price] = value.split(",");
  if (!name) {
    throw new Error("--skill must start with a name");
  }
  return {
    name,
    hash: hash || undefined,
    price_zat: price ? Number.parseInt(price, 10) : undefined,
  };
}

function readTextArg(args: ParsedArgs, textFlag: string, fileFlag: string): string {
  const text = getFlag(args, textFlag);
  const file = getFlag(args, fileFlag);
  if (text && file) {
    throw new Error(`use --${textFlag} or --${fileFlag}, not both`);
  }
  if (file) {
    return readFileSync(resolvePath(file), "utf8");
  }
  if (text !== undefined) {
    return text;
  }
  throw new Error(`missing --${textFlag} or --${fileFlag}`);
}

function maybeHashFile(file: string | undefined): string | undefined {
  return file ? sha256Hex(readFileSync(resolvePath(file))) : undefined;
}

function maybeNumber(value: string | undefined): number | undefined {
  if (value === undefined) {
    return undefined;
  }
  const number = Number.parseInt(value, 10);
  if (!Number.isFinite(number)) {
    throw new Error(`bad integer ${value}`);
  }
  return number;
}

function parseArgs(argv: string[]): ParsedArgs {
  const positional: string[] = [];
  const flags = new Map<string, string[]>();
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i]!;
    if (!arg.startsWith("--")) {
      positional.push(arg);
      continue;
    }
    const key = arg.slice(2);
    const next = argv[i + 1];
    if (!next || next.startsWith("--")) {
      flags.set(key, [...(flags.get(key) ?? []), "true"]);
      continue;
    }
    flags.set(key, [...(flags.get(key) ?? []), next]);
    i += 1;
  }
  return { positional, flags };
}

function hasFlag(args: ParsedArgs, key: string): boolean {
  return args.flags.has(key);
}

function getFlag(args: ParsedArgs, key: string): string | undefined {
  const values = args.flags.get(key);
  return values?.[values.length - 1];
}

function getFlags(args: ParsedArgs, key: string): string[] {
  return args.flags.get(key) ?? [];
}

function requireFlag(args: ParsedArgs, key: string): string {
  const value = getFlag(args, key);
  if (!value) {
    throw new Error(`missing --${key}`);
  }
  return value;
}

function resolvePath(value: string): string {
  if (value === "~") {
    return homedir();
  }
  if (value.startsWith("~/")) {
    return path.join(homedir(), value.slice(2));
  }
  return path.resolve(value);
}

function writeJsonSecure(filePath: string, value: unknown, force: boolean): void {
  mkdirSync(path.dirname(filePath), { recursive: true });
  if (!force) {
    try {
      readFileSync(filePath);
      throw new Error(`${filePath} already exists; pass --force to overwrite`);
    } catch (err) {
      if (err instanceof Error && "code" in err && err.code === "ENOENT") {
        // ok
      } else if (err instanceof Error && err.message.includes("already exists")) {
        throw err;
      }
    }
  }
  writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
}

function printJson(value: unknown): void {
  console.log(JSON.stringify(value, null, 2));
}

function printHelp(): void {
  console.log(`zam - Zcash Agent Messaging CLI

Commands:
  zam setup [--config path] [--directory-ua u1...] [--contact-ua u1...] [--force]
  zam identity new [--out path] [--force]
  zam hash-skill --file SKILL.md
  zam profile --config path --display-name NAME --contact-ua UA --directory-ua UA --skill name[,hash[,price_zat]] --topic topic
  zam message --config path --to-ua UA --text "hello"
  zam skill-request --config path --to-ua UA --skill name --input-json '{}'
  zam decode --memo BASE64URL_OR_HEX
  zam directory --memo BASE64URL_OR_HEX [--skill name] [--topic topic] [--text query]
`);
}

main();
