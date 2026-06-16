import { NextResponse } from "next/server";

/**
 * Backwards-compatible JSON-RPC proxy in front of staccana's RPC.
 *
 * Why this exists: agave 3.1.14 dropped three RPC methods that older wallets
 * (Backpack as of late 2025) still call internally to fetch blockhash + fee
 * info for transaction simulation:
 *   - getRecentBlockhash → use getLatestBlockhash
 *   - getFees           → use getFeeForMessage / getLatestBlockhash
 *   - getMinimumLedgerSlot → use getFirstAvailableBlock
 *
 * If the wallet is configured with the raw rpc.mp.fun URL, the deprecated
 * call returns -32601 "Method not found", the wallet ends up with an
 * undefined blockhash, signs a malformed tx, simulation rejects with empty
 * logs + units_consumed=0 and a "Cannot destructure 'err'" wallet error.
 *
 * This proxy intercepts the legacy method names, calls the modern equivalent
 * upstream, and reshapes the response to the legacy schema the wallet expects.
 *
 * Wallets should be configured with `https://app.mp.fun/api/rpc` instead of
 * `https://rpc.mp.fun/` directly.
 */

export const runtime = "edge";

const UPSTREAM = process.env.STACCANA_UPSTREAM_RPC ?? "https://rpc.mp.fun/";

interface JsonRpcReq {
  jsonrpc: "2.0";
  id: number | string;
  method: string;
  params?: unknown;
}

async function call(method: string, params: unknown, id: number | string): Promise<unknown> {
  const r = await fetch(UPSTREAM, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
  });
  return r.json();
}

export async function POST(request: Request): Promise<NextResponse> {
  const raw = await request.text();
  let body: JsonRpcReq | JsonRpcReq[];
  try {
    body = JSON.parse(raw) as JsonRpcReq | JsonRpcReq[];
  } catch (e) {
    return NextResponse.json(
      { jsonrpc: "2.0", id: null, error: { code: -32700, message: "parse error", data: String(e) } },
      { status: 400 },
    );
  }

  // JSON-RPC supports batch requests (array). Translate each item, preserve
  // array-vs-object shape on response.
  const isBatch = Array.isArray(body);
  const items = isBatch ? body : [body];

  const out = await Promise.all(items.map((item) => translateOne(item)));
  const resp = isBatch ? out : out[0];
  return NextResponse.json(resp);
}

async function translateOne(req: JsonRpcReq): Promise<unknown> {
  // === getRecentBlockhash → getLatestBlockhash + fake feeCalculator ===
  if (req.method === "getRecentBlockhash") {
    const upstream = (await call("getLatestBlockhash", req.params ?? [], req.id)) as {
      result?: { context: unknown; value: { blockhash: string; lastValidBlockHeight: number } };
      error?: unknown;
    };
    if (upstream.error || !upstream.result) return upstream;
    return {
      jsonrpc: "2.0",
      id: req.id,
      result: {
        context: upstream.result.context,
        value: {
          blockhash: upstream.result.value.blockhash,
          // Legacy shape: every tx had a 5000 lamport per-sig base fee.
          feeCalculator: { lamportsPerSignature: 5000 },
        },
      },
    };
  }

  // === getFees → getLatestBlockhash + fake feeCalculator ===
  if (req.method === "getFees") {
    const upstream = (await call("getLatestBlockhash", req.params ?? [], req.id)) as {
      result?: { context: unknown; value: { blockhash: string; lastValidBlockHeight: number } };
      error?: unknown;
    };
    if (upstream.error || !upstream.result) return upstream;
    return {
      jsonrpc: "2.0",
      id: req.id,
      result: {
        context: upstream.result.context,
        value: {
          blockhash: upstream.result.value.blockhash,
          feeCalculator: { lamportsPerSignature: 5000 },
          lastValidSlot: upstream.result.value.lastValidBlockHeight,
          lastValidBlockHeight: upstream.result.value.lastValidBlockHeight,
        },
      },
    };
  }

  // === getMinimumLedgerSlot → getFirstAvailableBlock ===
  if (req.method === "getMinimumLedgerSlot") {
    return await call("getFirstAvailableBlock", req.params ?? [], req.id);
  }

  // Pass-through everything else.
  return await call(req.method, req.params ?? [], req.id);
}

export async function GET(): Promise<NextResponse> {
  return NextResponse.json({
    name: "staccana JSON-RPC compat proxy",
    upstream: UPSTREAM,
    legacyMethodsTranslated: ["getRecentBlockhash", "getFees", "getMinimumLedgerSlot"],
    wallet: "Set your wallet's custom RPC to https://app.mp.fun/api/rpc",
  });
}
