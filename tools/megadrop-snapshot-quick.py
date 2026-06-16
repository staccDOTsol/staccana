#!/usr/bin/env python3
"""
megadrop-snapshot-quick.py — single-file holder snapshot for the megadrop.

Captures, at the moment of execution:
  * All holders of the `based_stacc_0` Metaplex NFT collection
  * All holders of the `proofv3` Token-22 SPL fungible mint

Writes three files into --output-dir:
  * based_stacc_0_holders.json   one line per NFT: {owner, mint}
  * proofv3_holders.json         one line per token account: {owner, balance, mint}
  * snapshot-meta.json           timestamp + slot + collection/mint addresses

Usage:
  HELIUS_KEY=xxxxx python3 megadrop-snapshot-quick.py --output-dir ~/megadrop-snapshot
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from urllib import request
from urllib.error import HTTPError

BASED_STACC_0_COLLECTION = "Ej1jbbw7QKgC9XMmWPxKFipMLJY5oVNd3rdbE1TzjNdz"
PROOFV3_MINT = "CLWeikxiw8pC9JEtZt14fqDzYfXF7uVwLuvnJPkrE7av"
TOKEN_2022_PROGRAM = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"

PAGE_SIZE = 1000


def rpc(url: str, body: dict, timeout: int = 60) -> dict:
    req = request.Request(
        url,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    with request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def fetch_collection_nfts(rpc_url: str, collection: str) -> list[dict]:
    """Page through getAssetsByGroup; return [{owner, mint}]."""
    out: list[dict] = []
    page = 1
    while True:
        body = {
            "jsonrpc": "2.0",
            "id": page,
            "method": "getAssetsByGroup",
            "params": {
                "groupKey": "collection",
                "groupValue": collection,
                "page": page,
                "limit": PAGE_SIZE,
                "displayOptions": {"showCollectionMetadata": False},
            },
        }
        for attempt in range(3):
            try:
                resp = rpc(rpc_url, body)
                break
            except HTTPError as e:
                print(f"  page {page} attempt {attempt + 1}: HTTP {e.code}, retrying", file=sys.stderr)
                time.sleep(2)
        else:
            raise RuntimeError(f"page {page} failed 3 times")

        items = resp.get("result", {}).get("items", [])
        if not items:
            break
        for item in items:
            owner = item.get("ownership", {}).get("owner")
            mint = item.get("id")
            if owner and mint:
                out.append({"owner": owner, "mint": mint})
        print(f"  collection page {page}: +{len(items)} (total {len(out)})", file=sys.stderr)
        if len(items) < PAGE_SIZE:
            break
        page += 1
    return out


def fetch_token_22_holders(rpc_url: str, mint: str) -> list[dict]:
    """getProgramAccounts against Token-2022 filtered by mint; return [{owner, balance, mint, ata}]."""
    body = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getProgramAccounts",
        "params": [
            TOKEN_2022_PROGRAM,
            {
                "filters": [
                    {"memcmp": {"offset": 0, "bytes": mint}},
                ],
                "encoding": "jsonParsed",
            },
        ],
    }
    resp = rpc(rpc_url, body, timeout=120)
    raw = resp.get("result", [])
    out: list[dict] = []
    for entry in raw:
        try:
            info = entry["account"]["data"]["parsed"]["info"]
            owner = info["owner"]
            amount = info["tokenAmount"]["amount"]
            ata = entry["pubkey"]
            if int(amount) > 0:
                out.append({"owner": owner, "balance": amount, "mint": mint, "ata": ata})
        except (KeyError, ValueError):
            continue
    print(f"  proofv3 token accounts: {len(raw)} raw, {len(out)} with non-zero balance", file=sys.stderr)
    return out


def fetch_slot(rpc_url: str) -> int:
    resp = rpc(rpc_url, {"jsonrpc": "2.0", "id": 1, "method": "getSlot"})
    return int(resp.get("result", 0))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--output-dir", required=True, help="dir for output files")
    ap.add_argument(
        "--rpc-url",
        default=None,
        help="JSON-RPC url; defaults to https://mainnet.helius-rpc.com/?api-key=$HELIUS_KEY",
    )
    args = ap.parse_args()

    rpc_url = args.rpc_url
    if not rpc_url:
        key = os.environ.get("HELIUS_KEY")
        if not key:
            print("FATAL: set HELIUS_KEY env var or pass --rpc-url", file=sys.stderr)
            sys.exit(1)
        rpc_url = f"https://mainnet.helius-rpc.com/?api-key={key}"

    out_dir = Path(args.output_dir).expanduser()
    out_dir.mkdir(parents=True, exist_ok=True)

    started = datetime.now(timezone.utc).isoformat()
    slot_at_start = fetch_slot(rpc_url)
    print(f"[snapshot] started at {started} (slot {slot_at_start})", file=sys.stderr)

    # 1. based_stacc_0 NFT collection holders
    print(f"[snapshot] fetching based_stacc_0 collection {BASED_STACC_0_COLLECTION}", file=sys.stderr)
    nfts = fetch_collection_nfts(rpc_url, BASED_STACC_0_COLLECTION)
    based_path = out_dir / "based_stacc_0_holders.json"
    with based_path.open("w") as f:
        for entry in nfts:
            f.write(json.dumps(entry) + "\n")
    print(f"[snapshot] wrote {based_path} ({len(nfts)} NFTs)", file=sys.stderr)

    # 2. proofv3 Token-22 holders
    print(f"[snapshot] fetching proofv3 mint {PROOFV3_MINT}", file=sys.stderr)
    accounts = fetch_token_22_holders(rpc_url, PROOFV3_MINT)
    proofv3_path = out_dir / "proofv3_holders.json"
    with proofv3_path.open("w") as f:
        for entry in accounts:
            f.write(json.dumps(entry) + "\n")
    print(f"[snapshot] wrote {proofv3_path} ({len(accounts)} accounts)", file=sys.stderr)

    finished = datetime.now(timezone.utc).isoformat()
    slot_at_end = fetch_slot(rpc_url)

    # Group by holder for sanity
    nft_holders = len({n["owner"] for n in nfts})
    token_holders = len({a["owner"] for a in accounts})

    meta = {
        "started_utc": started,
        "finished_utc": finished,
        "slot_at_start": slot_at_start,
        "slot_at_end": slot_at_end,
        "based_stacc_0_collection": BASED_STACC_0_COLLECTION,
        "proofv3_mint": PROOFV3_MINT,
        "token_2022_program": TOKEN_2022_PROGRAM,
        "based_stacc_0_nft_count": len(nfts),
        "based_stacc_0_unique_holders": nft_holders,
        "proofv3_account_count": len(accounts),
        "proofv3_unique_holders": token_holders,
        "rpc_url_redacted": rpc_url.split("?")[0] if "?" in rpc_url else rpc_url,
    }
    meta_path = out_dir / "snapshot-meta.json"
    with meta_path.open("w") as f:
        json.dump(meta, f, indent=2)
    print(f"[snapshot] wrote {meta_path}", file=sys.stderr)
    print(json.dumps(meta, indent=2))


if __name__ == "__main__":
    main()
