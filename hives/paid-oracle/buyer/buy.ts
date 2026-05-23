// End-to-end buyer for the paid-oracle forager bee.
//
// Flow:
//   1. Connect to humd's thrum.sock as a synthetic bee.
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
// Keys at $XDG_CONFIG_HOME/hum/paid-oracle/keys.json.

import { connect } from "node:net";
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

const baseDir = process.env.XDG_CONFIG_HOME ?? join(homedir(), ".config");
const keysPath = join(baseDir, "hum", "paid-oracle", "keys.json");
const keys = JSON.parse(readFileSync(keysPath, "utf-8")) as {
  receiver: { address: `0x${string}`; private_key: `0x${string}` };
  buyer: { address: `0x${string}`; private_key: `0x${string}` };
};

const PAIR = process.argv[2] ?? "ETH-USD";
const SOCK_PATH = process.env.HUM_THRUM_SOCK
  ?? `${process.env.XDG_RUNTIME_DIR ?? `/run/user/${process.getuid?.() ?? 0}`}/hum/thrum.sock`;

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

// One thrum-socket session: hello → tool-call (quote) → consume
// frames until either chi:"error" (402, expected first time) or
// chi:"tool-result". Resolves with the parsed body.
function thrumCall(args: Record<string, unknown>): Promise<{ kind: "error"; body: any } | { kind: "result"; body: any }> {
  return new Promise((resolve, reject) => {
    const sock = connect(SOCK_PATH);
    let buf = "";
    const callId = `call-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
    const sid = `paid-oracle-buyer-${Date.now().toString(36)}`;
    sock.on("connect", () => {
      const hello = {
        chi: "hello",
        rid: `hello-${Date.now().toString(36)}`,
        from: "paid-oracle-buyer",
        bee: ["asker"],
        hive: "paid-oracle-buyer",
        version: "0.0.1",
        protoVersion: "0.7.0",
        chis: ["hello", "tool-call", "tool-result", "error"],
      };
      sock.write(JSON.stringify(hello) + "\n");
      const tone = {
        chi: "tool-call",
        rid: `r-${Date.now().toString(36)}`,
        sid,
        callId,
        name: "quote",
        ...args,
      };
      sock.write(JSON.stringify(tone) + "\n");
    });
    sock.on("data", (chunk: Buffer) => {
      buf += chunk.toString();
      for (;;) {
        const nl = buf.indexOf("\n");
        if (nl < 0) break;
        const line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        if (!line.trim()) continue;
        let msg: any;
        try { msg = JSON.parse(line); } catch { continue; }
        if (msg.chi === "error" && msg.code === 402) {
          sock.end();
          resolve({ kind: "error", body: msg });
          return;
        }
        if (msg.chi === "tool-result") {
          sock.end();
          resolve({ kind: "result", body: msg });
          return;
        }
      }
    });
    sock.on("error", reject);
    sock.setTimeout(60_000, () => { sock.destroy(new Error("thrum timeout")); });
  });
}

// ERC-20 transfer(address,uint256). Standard 4-byte selector + 32-byte
// recipient + 32-byte amount. paid-oracle's verifier also expects the
// nonce ASCII bytes appended after the standard calldata so the
// payment is bound to a specific challenge.
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
  log(`socket=${SOCK_PATH}`);

  // Step 1: ask for the quote without payment. Expect 402.
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

  // Step 2: send USDC.transfer + nonce. Hardcoded to Base Sepolia
  // for now; trivial to extend by switching on terms.chain.
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

  // Pre-flight balances so we fail loudly with a faucet hint.
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

  // Step 3: resubmit the quote with paymentProof.
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
