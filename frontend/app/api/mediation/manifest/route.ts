import { NextResponse } from "next/server";

import { MEDIATION_MANIFEST } from "@/lib/mediation";

export async function GET(): Promise<NextResponse> {
  return NextResponse.json(MEDIATION_MANIFEST, {
    headers: {
      "Cache-Control": "public, max-age=300, s-maxage=300",
    },
  });
}
