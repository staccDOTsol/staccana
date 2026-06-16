/**
 * Cloudflare Worker — agave-3.x → legacy RPC compatibility shim.
 *
 * Drop this on the rpc.mp.fun route in the Cloudflare dashboard (Workers
 * & Pages → Create Worker → paste this). The cloudflared tunnel from val-1
 * stays as the origin; the Worker sits in front and translates a few
 * deprecated JSON-RPC method names that older wallets (Backpack as of late
 * 2025) still call. Without this, those calls return -32601 "Method not
 * found" → wallet ends up with undefined blockhash → tx simulation fails
 * with empty logs + units_consumed=0 + the infamous "Cannot destructure
 * property 'err' of 'r' as it is undefined" Backpack popup.
 *
 * Translated methods:
 *   getRecentBlockhash    → getLatestBlockhash + synthesized feeCalculator
 *   getFees               → getLatestBlockhash + synthesized feeCalculator + lastValidSlot
 *   getMinimumLedgerSlot  → getFirstAvailableBlock
 *
 * Everything else passes through unchanged.
 *
 * Deploy steps (CF dashboard, ~2 min):
 *   1. Workers & Pages → Create Worker → name `rpc-mp-fun-compat`
 *   2. Paste this file as the worker code
 *   3. Save + Deploy
 *   4. Workers Routes → Add route: pattern `rpc.mp.fun/*`, worker `rpc-mp-fun-compat`
 *   5. Verify: `curl -X POST https://rpc.mp.fun/ -d '{"jsonrpc":"2.0","id":1,"method":"getRecentBlockhash"}' -H content-type:application/json`
 *      should now return `{result: {context: ..., value: {blockhash, feeCalculator: {lamportsPerSignature:5000}}}}` instead of -32601.
 *
 * Or via wrangler CLI:
 *   wrangler deploy rpc-compat-worker.js --route 'rpc.mp.fun/*' --name rpc-mp-fun-compat
 */

// The cloudflared tunnel publishes the validator's RPC at rpc.mp.fun. The
// Worker's `fetch(request)` proxies straight through — Cloudflare resolves
// the same hostname to the tunnel origin behind the Worker.

async function callUpstream(request, body) {
  return fetch(request.url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

async function translateOne(request, item) {
  const id = item.id;
  if (item.method === "getRecentBlockhash") {
    const upstreamRes = await callUpstream(request, {
      jsonrpc: "2.0",
      id,
      method: "getLatestBlockhash",
      params: item.params ?? [],
    });
    const upstream = await upstreamRes.json();
    if (upstream.error || !upstream.result) return upstream;
    return {
      jsonrpc: "2.0",
      id,
      result: {
        context: upstream.result.context,
        value: {
          blockhash: upstream.result.value.blockhash,
          feeCalculator: { lamportsPerSignature: 5000 },
        },
      },
    };
  }
  if (item.method === "getFees") {
    const upstreamRes = await callUpstream(request, {
      jsonrpc: "2.0",
      id,
      method: "getLatestBlockhash",
      params: item.params ?? [],
    });
    const upstream = await upstreamRes.json();
    if (upstream.error || !upstream.result) return upstream;
    return {
      jsonrpc: "2.0",
      id,
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
  if (item.method === "getMinimumLedgerSlot") {
    const upstreamRes = await callUpstream(request, {
      jsonrpc: "2.0",
      id,
      method: "getFirstAvailableBlock",
      params: item.params ?? [],
    });
    return upstreamRes.json();
  }
  // Pass-through.
  const upstreamRes = await callUpstream(request, item);
  return upstreamRes.json();
}

export default {
  async fetch(request, env, ctx) {
    // WebSocket upgrade — pass straight through to the cloudflared origin
    // (which now routes via nginx on val-1 that demuxes by `Upgrade` header
    // to agave's pubsub on :8900). The Worker mustn't intercept these or
    // web3.js's `confirmTransaction` hangs with `Unexpected server response:
    // 200` because the GET branch below returns a health JSON instead of
    // upgrading to 101.
    if ((request.headers.get("upgrade") || "").toLowerCase() === "websocket") {
      return fetch(request);
    }
    if (request.method === "GET") {
      // Useful health-check.
      return new Response(
        JSON.stringify({
          name: "rpc.mp.fun compat proxy",
          translatedMethods: ["getRecentBlockhash", "getFees", "getMinimumLedgerSlot"],
          upstream: "agave 3.1.14 via cloudflared tunnel from val-1 (nginx demux: HTTP→:8899, WS→:8900)",
        }),
        { headers: { "content-type": "application/json", "access-control-allow-origin": "*" } },
      );
    }
    if (request.method !== "POST") {
      // Anything other than POST — pass straight through.
      return fetch(request);
    }

    let raw;
    try {
      raw = await request.text();
    } catch {
      return new Response("bad request", { status: 400 });
    }

    let body;
    try {
      body = JSON.parse(raw);
    } catch {
      // Not JSON — pass through unchanged.
      return fetch(new Request(request.url, { method: "POST", body: raw, headers: request.headers }));
    }

    // Fast path: only translate when the request actually carries one of the
    // legacy method names. This keeps the hot RPC path (every tx flow + the
    // explorer + every page-load) at one origin RTT, no extra parsing cost.
    const isBatch = Array.isArray(body);
    const items = isBatch ? body : [body];
    const needsTranslation = items.some((it) =>
      it && (it.method === "getRecentBlockhash" || it.method === "getFees" || it.method === "getMinimumLedgerSlot"),
    );
    if (!needsTranslation) {
      return fetch(new Request(request.url, { method: "POST", body: raw, headers: request.headers }));
    }

    const out = await Promise.all(items.map((item) => translateOne(request, item)));
    const resp = isBatch ? out : out[0];
    return new Response(JSON.stringify(resp), {
      headers: {
        "content-type": "application/json",
        "access-control-allow-origin": "*",
      },
    });
  },
};
