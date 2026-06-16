/**
 * GET /api/launch/[mint]/trades?limit=50
 *
 * Recent indexed trades for a mint. lamports come back as decimal strings —
 * caller bigints them.
 */
import { NextResponse } from "next/server";

import { ensureSchema, getRecentTrades } from "@/lib/db";

export const runtime = "edge";
export const dynamic = "force-dynamic";

let schemaReady: Promise<void> | null = null;
function readySchema(): Promise<void> {
  return (schemaReady ??= ensureSchema());
}

export async function GET(
  req: Request,
  ctx: { params: Promise<{ mint: string }> },
): Promise<Response> {
  await readySchema();
  const { mint } = await ctx.params;
  const url = new URL(req.url);
  const limitRaw = Number(url.searchParams.get("limit") ?? "50");
  const limit = Number.isFinite(limitRaw) ? Math.min(Math.max(1, limitRaw | 0), 200) : 50;
  const trades = await getRecentTrades(mint, limit);
  return NextResponse.json(
    { mint, trades },
    {
      headers: {
        "cache-control": "public, max-age=5",
      },
    },
  );
}
