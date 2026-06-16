# staccana validator image — runs a follower validator on a laptop or server.
#
# Lineage: this is the v2 incarnation of jrsdunn/solana-classic-validator. The
# v1 image (last pushed 2025-05-27, "20250527" tag, 332 organic pulls over a
# year of dormancy) carried the Solana-Classic single-laptop fork. v2 rebases
# onto staccana — same brand, same image slug, same audience, but with the
# full anti-MEV / Token-22 CTE / bridge architecture documented in
# docs/ARCHITECTURE.md.
#
# What this image is:
#   - agave-validator 2.3.13 built from staccana fork
#   - staccana genesis.bin (slot 0, with all 9 feature gates active and the
#     5 staccana programs registered as upgradeable BPF builtins)
#   - rocksdb blockstore seeded with slot-0 ticks
#   - solana CLI for interacting with the running validator
#   - staccana-run.sh entrypoint that auto-generates identity/vote keys on
#     first boot and joins the staccana cluster via val-1's gossip endpoint
#
# What this image is NOT:
#   - Not a bootstrap validator (no --no-snapshot-fetch; followers fetch a
#     snapshot from the cluster on boot rather than booting raw from genesis)
#   - Not val-1's identity (every container generates its own keypair)
#   - Not a full archive node by default (--limit-ledger-size 200000000 to
#     stay laptop-friendly; override with -e STACCANA_LIMIT_LEDGER_SIZE=)
#
# Build context: this Dockerfile expects to be built from /Users/stacc/staccana
# with the genesis ledger pre-baked at /var/lib/staccana/ledger/. The
# build-and-push.sh helper in this dir handles that.
#
# GLIBC CONSTRAINT — IMPORTANT.
# The agave-validator binary in `bin/agave-validator` was compiled on val-1
# (the build host). The base image below MUST provide a glibc version >= the
# glibc on val-1, or you'll see at container start:
#   "agave-validator: /lib/.../libc.so.6: version `GLIBC_2.38' not found"
# Reference table: Ubuntu 22.04 = glibc 2.35, Ubuntu 24.04 = 2.39, Debian 12
# bookworm = 2.36, Debian 13 trixie = 2.41. If val-1 is upgraded past this
# image's glibc, EITHER bump the base image here OR switch to the multi-stage
# Dockerfile.multiarch (which builds the validator in-image so glibc auto-
# matches by construction). Last verified: val-1 on Ubuntu 24.04 (glibc 2.39),
# matching the runtime base below.
FROM ubuntu:24.04

# All staccana validators expose the same Solana port topology.
# RPC JSON / RPC pubsub
EXPOSE 8899/tcp 8900/tcp
# gossip
EXPOSE 8001/tcp 8001/udp
# tvu / tvu_quic / tpu / tpu_forwards / retransmit / repair / serve_repair / broadcast / tpu_vote
EXPOSE 8000/udp 8002/udp 8003/udp 8004/udp 8005/udp 8006/udp 8007/udp 8008/udp 8009/udp
# dynamic port range (matches --dynamic-port-range 8002-8020)
EXPOSE 8010-8020/udp

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        bzip2 libssl-dev ca-certificates curl jq tini && \
    rm -rf /var/lib/apt/lists/* && \
    useradd -r -m -d /var/lib/staccana -s /bin/bash staccana && \
    mkdir -p /var/lib/staccana/ledger /var/lib/staccana/accounts \
             /var/log/staccana /etc/staccana/keys /var/staccana/seed-ledger && \
    chown -R staccana:staccana /var/lib/staccana /var/log/staccana \
                                /etc/staccana /var/staccana

# Agave validator + tooling (built from the staccana fork = agave 2.3.13).
COPY --chmod=0755 bin/agave-validator   /usr/local/bin/agave-validator
COPY --chmod=0755 bin/agave-ledger-tool /usr/local/bin/agave-ledger-tool
COPY --chmod=0755 bin/solana            /usr/local/bin/solana
COPY --chmod=0755 bin/solana-keygen     /usr/local/bin/solana-keygen

# Staccana genesis: slot-0 ledger seed (genesis.bin + rocksdb with first ticks).
# The container copies this to /var/lib/staccana/ledger/ on first boot iff
# the runtime ledger dir is empty — re-runs preserve replay state.
COPY --chown=staccana:staccana ledger/   /var/staccana/seed-ledger/

# Embedded program-ids.json so the run script can hint at deployed program addresses.
COPY --chown=staccana:staccana program-ids.json /var/staccana/program-ids.json

COPY --chmod=0755 staccana-run.sh /usr/local/bin/staccana-run.sh

USER staccana
WORKDIR /var/lib/staccana

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/staccana-run.sh"]
CMD []
