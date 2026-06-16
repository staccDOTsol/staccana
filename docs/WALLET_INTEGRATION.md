# Wallet Integration Guide

How users connect existing Solana wallets to staccana. None of the major wallets (Phantom, Solflare, Backpack) ship a built-in "staccana" cluster — users add it as a custom RPC endpoint.

## Phantom

1. Open Phantom → Settings → Developer Settings → Change Network → "Add custom RPC"
2. Network name: `Staccana`
3. RPC URL: `https://rpc.mp.fun/` (replace with the live endpoint when announced)
4. Save → switch active network to Staccana
5. The wallet now shows your staccana balances + lets you sign staccana transactions

**Caveat**: Phantom does not yet have native UI for the Token-22 Confidential Transfer Extension. CTE-active mints (stSOL, ssUSDC) appear as standard SPL tokens; the encrypted balance is invisible to the wallet UI. To interact with confidential balances, use the staccana web frontend (`https://app.mp.fun/`) which has CTE-aware encryption / decryption flows.

## Solflare

1. Open Solflare → Settings → Network → Custom Network
2. Network name: `Staccana`
3. RPC URL: `https://rpc.mp.fun/`
4. Save and select

Same CTE caveat as Phantom — use the staccana frontend for confidential balance ops.

## Backpack

1. Open Backpack → Settings → Solana → Network → Custom
2. URL: `https://rpc.mp.fun/`
3. Save

Backpack has the most progressive Token-22 support of the three but still lacks CTE UI. Same workaround.

## CLI / SDK users

```bash
solana config set --url https://rpc.mp.fun/
```

For programmatic access, the staccana RPC speaks the standard Solana JSON-RPC API. Drop-in compatible with `@solana/web3.js`, `solana-py`, `solana-sdk` (Rust), and any other Solana client library.

```typescript
import { Connection, PublicKey } from '@solana/web3.js';
const connection = new Connection('https://rpc.mp.fun/', 'confirmed');
const balance = await connection.getBalance(new PublicKey('...'));
```

## Genesis hash + chain ID

Staccana has a distinct genesis hash from Solana mainnet (no cross-chain double-signing). Wallets that key off the genesis hash will treat staccana as a separate chain from mainnet. The staccana genesis hash will be published at launch via the staccana docs site and pinned in the staccana repo's release notes.

There is no EVM-style "chainId"; Solana uses genesis hash for chain identity.

## Claiming your mainnet SOL on staccana

If you held SOL on Solana mainnet at the snapshot slot, you have a claimable balance on staccana. To claim:

**Web flow** (recommended):
1. Visit `https://app.mp.fun/claim`
2. Connect your wallet (the one that holds your mainnet SOL)
3. Sign the claim message (`STACCANA_CLAIM_V1` — see SPEC §4.2)
4. Submit; balance materializes on staccana within a few slots

**CLI flow**:
```bash
git clone https://github.com/staccDOTsol/solana-classic
cd solana-classic
cargo run --release -p staccana-claim-cli -- \
  --keypair ~/.config/solana/id.json \
  --snapshot https://snapshot.mp.fun/genesis-output.json \
  --rpc https://rpc.mp.fun/
```

The claim instruction is fee-exempt (SPEC §4.4), so you don't need any staccana SOL to claim — the lazy-claim program covers the fee from the treasury.

## Bridging in (deposit SOL on mainnet, mint stSOL on staccana)

Users who want more staccana exposure than their claim grants — or who didn't hold SOL on mainnet at the snapshot slot — can bridge in.

**Web flow**:
1. Visit `https://app.mp.fun/bridge`
2. Connect mainnet wallet → choose deposit asset (SOL → pSYRUP → stSOL, or USDC → ssUSDC)
3. Approve mainnet vault deposit
4. Wait for federation attestation (~1 minute, 5-of-9 sigs)
5. Receive stSOL/ssUSDC on staccana

**CLI flow**:
```bash
cargo run --release -p staccana-bridge-cli -- deposit \
  --asset stSOL \
  --amount 1.5 \
  --mainnet-keypair ~/.config/solana/id.json \
  --staccana-dest <YOUR_STACCANA_PUBKEY> \
  --mainnet-rpc https://api.mainnet-beta.solana.com
```

## Confidential transfers

Once you hold a CTE-active token (stSOL, ssUSDC, or any secret-pump token), you can opt your token account into the Confidential Transfer Extension and start sending private balances.

The staccana frontend handles this end-to-end: it generates your ElGamal decryption key (separate from your signing key), configures the token account, and runs the CTE-aware send / receive flow.

**WARNING**: your ElGamal decryption key is separate from your wallet signing key. Lose it and you cannot decrypt your own confidential balance. The staccana frontend stores it encrypted-at-rest in browser localStorage; export and back up via Settings → Export ElGamal Key.

CLI flow for confidential transfers is more involved and lives in the documentation site (`https://docs.mp.fun/cte`) rather than this short guide.

## Block explorer

`https://explorer.mp.fun/` — forked solana-explorer pointed at the staccana RPC. Same UI, same features, different chain.
