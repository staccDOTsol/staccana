import { handleUpload, type HandleUploadBody } from "@vercel/blob/client";
import { NextResponse } from "next/server";

/**
 * Vercel Blob upload route — issues a one-time client-upload token and finalizes.
 *
 * Two-hop pattern for the /launch/create flow:
 *   1. User picks an image → client uploads it directly to Vercel Blob via this
 *      route (presigned token returned by `handleUpload`). Returned URL goes
 *      into a metadata JSON: `{name, symbol, image: <blob URL>, ...}`.
 *   2. Client uploads the metadata JSON via this same route.
 *   3. The JSON's blob URL is what we pass as the `uri` field to the
 *      on-chain `create_curve` ix. Final URL is ~80–100 bytes (well under the
 *      200-byte cap in `programs/secret-pump/src/instructions/create.rs`).
 *
 * Requires the `BLOB_READ_WRITE_TOKEN` env var to be set in the Vercel project
 * (it is — see `vercel env add BLOB_READ_WRITE_TOKEN production`).
 *
 * Allowed content types are restricted to the formats the launchpad UI emits;
 * tighten further if abuse becomes a concern.
 */
export async function POST(request: Request): Promise<NextResponse> {
  const body = (await request.json()) as HandleUploadBody;

  try {
    const jsonResponse = await handleUpload({
      body,
      request,
      onBeforeGenerateToken: async (_pathname /*, clientPayload */) => ({
        allowedContentTypes: [
          "image/jpeg",
          "image/png",
          "image/gif",
          "image/webp",
          "image/svg+xml",
          "application/json",
        ],
        // Random suffix is added by the client so multiple uploads of the
        // same filename don't collide.
        addRandomSuffix: true,
        tokenPayload: JSON.stringify({}),
      }),
      onUploadCompleted: async ({ blob }) => {
        // No-op for now; could log to analytics here.
        console.log("[blob-upload] uploaded", blob.url, blob.pathname);
      },
    });
    return NextResponse.json(jsonResponse);
  } catch (error) {
    return NextResponse.json(
      { error: (error as Error).message },
      { status: 400 },
    );
  }
}
