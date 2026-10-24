// End-to-end buyer for the paid-oracle forager bee.
//
// Flow:
//   1. Enter as paid-oracle's buyer bee, through the thrum client.
//   2. Send chi:"tool-call" with name="quote" + args.pair.
//   3. Receive chi:"error" 402 with x402 terms (nonce + recipient
//      + atomic price + USDC contract).
//   4. Build USDC.transfer(recipient, amount) calldata, append the
//      nonce as ASCII bytes so the bee can bind the payment to
//      this challenge.
//   5. Send tx on the configured chain, wait for inclusion.
//   6. Resubmit chi:"tool-call" with paymentProof: {txHash, nonce}.
//   7. Receive chi:"tool-result" with the actual price.
//
// Run:
//   bun buy.ts ETH-USD
//
// Requires the BUYER wallet (from keygen.ts) funded with:
//   - Testnet ETH for gas (Base Sepolia faucet)
//   - Testnet USDC for the quote (Circle USDC faucet)
//
// Keys at $XDG_STATE_HOME/hum/hives/paid-oracle/keys.json.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";
import {
  createPublicClient,
  createWalletClient,
  http as viemHttp,
  encodeFunctionData,
  concatHex,
  type Hex,
} from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { baseSepolia } from "viem/chains";
import { Bee, type Outgoing } from "../../../thrum-clients/ts/index.ts";

const baseDir = process.env.XDG_STATE_HOME ?? join(homedir(), ".local", "state");
const keysPath = join(baseDir, "hum", "hives", "paid-oracle", "keys.json");
const keys = JSON.parse(readFileSync(keysPath, "utf-8")) as {
  receiver: { address: `0x${string}`; private_key: `0x${string}` };
  buyer: { address: `0x${string}`; private_key: `0x${string}` };
};

const PAIR = process.argv[2] ?? "ETH-USD";

interface X402Terms {
  chain: string;
  pay_to: `0x${string}`;
  asset: `0x${string}`;
  asset_kind: "erc20" | "native";
  decimals: number;
  price_atomic: string;
  nonce: string;
}

function log(...m: unknown[]): void { console.log("[buy]", ...m); }

// One call is one bee session: the buyer enters as paid-oracle's buyer,
// asks, and leaves. The thrum client does the hello and the socket.
async function thrumCall(args: Record<string, unknown>): Promise<{ kind: "error"; body: any } | { kind: "result"; body: any }> {
  const bee = new Bee({
    hive: "paid-oracle",
    as: "buyer",
    hello: { roles: ["asker"], version: "0.0.1", chis: ["hello", "tool-call", "tool-result", "error"] },
  });
  const callId = `call-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
  const sid = `paid-oracle-buyer-${Date.now().toString(36)}`;
  const answer = new Promise<{ kind: "error"; body: any } | { kind: "result"; body: any }>((resolve) => {
    bee.onAny((msg) => {
      if (msg.chi === "error" && msg.code === 402) resolve({ kind: "error", body: msg });
      else if (msg.chi === "tool-result") resolve({ kind: "result", body: msg });
    });
  });
  await bee.start();
  bee.send({ chi: "tool-call", sid, callId, name: "quote", ...args } as Outgoing);
  const timeout = new Promise<never>((_, reject) => setTimeout(() => reject(new Error("thrum timeout")), 60_000));
  try {
    return await Promise.race([answer, timeout]);
  } finally {
    await bee.close();
  }
}

function buildTransferCalldata(recipient: `0x${string}`, amountAtomic: bigint, nonce: string): Hex {
  const base = encodeFunctionData({
    abi: [{
      name: "transfer",
      type: "function",
      stateMutability: "nonpayable",
      inputs: [
        { name: "to", type: "address" },
        { name: "amount", type: "uint256" },
      ],
      outputs: [{ name: "ok", type: "bool" }],
    }],
    functionName: "transfer",
    args: [recipient, amountAtomic],
  });
  const nonceHex = ("0x" + Buffer.from(nonce, "utf-8").toString("hex")) as Hex;
  return concatHex([base, nonceHex]);
}

async function main(): Promise<void> {
  log(`buyer=${keys.buyer.address}`);
  log(`pair=${PAIR}`);

  log("step 1: thrum tool-call quote (no payment) →");
  const first = await thrumCall({ args: { pair: PAIR } });
  if (first.kind !== "error" || first.body.code !== 402) {
    log("unexpected first response:", JSON.stringify(first.body, null, 2));
    process.exit(2);
  }
  const terms = first.body.x402 as X402Terms;
  const priceAtomic = BigInt(terms.price_atomic);
  log("  ↳ 402 challenge:");
  log(`    chain     = ${terms.chain}`);
  log(`    pay_to    = ${terms.pay_to}`);
  log(`    asset     = ${terms.asset} (${terms.asset_kind}, ${terms.decimals} dp)`);
  log(`    price     = ${priceAtomic.toString()} atomic = $${Number(priceAtomic) / 10 ** terms.decimals}`);
  log(`    nonce     = ${terms.nonce}`);

  if (terms.chain !== "base-sepolia") {
    log(`buyer.ts only knows base-sepolia today; got ${terms.chain}`);
    process.exit(3);
  }
  if (terms.asset_kind !== "erc20") {
    log("only erc20 supported in this buyer");
    process.exit(4);
  }

  const account = privateKeyToAccount(keys.buyer.private_key);
  const transport = viemHttp(process.env.PAID_ORACLE_RPC ?? "https://sepolia.base.org");
  const publicClient = createPublicClient({ chain: baseSepolia, transport });
  const walletClient = createWalletClient({ chain: baseSepolia, transport, account });

  const usdcBal = await publicClient.readContract({
    address: terms.asset,
    abi: [{ name: "balanceOf", type: "function", stateMutability: "view", inputs: [{ name: "a", type: "address" }], outputs: [{ name: "b", type: "uint256" }] }],
    functionName: "balanceOf",
    args: [keys.buyer.address],
  }) as bigint;
  const ethBal = await publicClient.getBalance({ address: keys.buyer.address });
  log(`  buyer balances: eth=${ethBal} wei, usdc=${usdcBal} atomic`);
  if (usdcBal < priceAtomic) {
    log(`  buyer underfunded: needs at least ${priceAtomic} atomic USDC.`);
    log("  faucet: https://faucet.circle.com (Base Sepolia)");
    log(`  buyer address: ${keys.buyer.address}`);
    process.exit(5);
  }
  if (ethBal === 0n) {
    log("  buyer has no ETH for gas. Use a Base Sepolia ETH faucet.");
    log(`  buyer address: ${keys.buyer.address}`);
    process.exit(5);
  }

  const data = buildTransferCalldata(terms.pay_to, priceAtomic, terms.nonce);
  log("step 2: sending USDC.transfer with nonce-bound calldata →");
  const txHash = await walletClient.sendTransaction({
    to: terms.asset,
    data,
    value: 0n,
  });
  log(`  txHash=${txHash}`);
  log("  waiting for inclusion…");
  const receipt = await publicClient.waitForTransactionReceipt({ hash: txHash });
  log(`  block=${receipt.blockNumber} status=${receipt.status}`);
  if (receipt.status !== "success") { log("tx failed on-chain"); process.exit(6); }

  log("step 3: resubmitting with paymentProof →");
  const second = await thrumCall({
    args: { pair: PAIR },
    paymentProof: { txHash, nonce: terms.nonce },
  });
  if (second.kind !== "result") {
    log("unexpected second response:", JSON.stringify(second.body, null, 2));
    process.exit(7);
  }
  log("  ↳ chi:tool-result");
  log(JSON.stringify(second.body.result, null, 2));
}

main().catch((err) => {
  console.error("[buy] fatal:", err);
  process.exit(1);
});
