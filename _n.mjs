import { PublicKey } from "@solana/web3.js";
const BRIDGE = new PublicKey("Bridge1111111111111111111111111111111111111");
const assetIdLe = new Uint8Array(4);
new DataView(assetIdLe.buffer).setUint32(0, 3, true);
for (let n = 0; n < 6; n++) {
  const nLe = new Uint8Array(8);
  new DataView(nLe.buffer).setBigUint64(0, BigInt(n), true);
  const [pda] = PublicKey.findProgramAddressSync([Buffer.from("nonce_in"), Buffer.from(assetIdLe), Buffer.from(nLe)], BRIDGE);
  console.log("nonce", n, pda.toBase58());
}
