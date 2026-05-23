// One-shot keypair generator. Writes JSON to
// `$XDG_CONFIG_HOME/hum/paid-oracle/keys.json` (or
// `~/.config/hum/paid-oracle/keys.json`).
//
// Receiver = where paid-oracle expects USDC to land. Address only;
// private key is included for completeness but never used.
// Buyer    = the asker's wallet. Needs testnet ETH (gas) + USDC
// (price). Fund manually at https://faucet.circle.com (Base Sepolia
// USDC) and a Base Sepolia ETH faucet.

import { generatePrivateKey, privateKeyToAccount } from "viem/accounts";
import { mkdirSync, writeFileSync, existsSync, readFileSync, chmodSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

const baseDir = process.env.XDG_CONFIG_HOME ?? join(homedir(), ".config");
const dir = join(baseDir, "hum", "paid-oracle");
const path = join(dir, "keys.json");

if (existsSync(path) && !process.argv.includes("--force")) {
  const existing = JSON.parse(readFileSync(path, "utf-8"));
  console.log(`Keys already exist at ${path}`);
  console.log(`  receiver: ${existing.receiver?.address}`);
  console.log(`  buyer:    ${existing.buyer?.address}`);
  console.log(`(pass --force to regenerate)`);
  process.exit(0);
}

const receiverKey = generatePrivateKey();
const buyerKey = generatePrivateKey();
const receiver = privateKeyToAccount(receiverKey);
const buyer = privateKeyToAccount(buyerKey);

mkdirSync(dir, { recursive: true });
const out = {
  generated_at: new Date().toISOString(),
  chain: "base-sepolia",
  receiver: { address: receiver.address, private_key: receiverKey },
  buyer:    { address: buyer.address,    private_key: buyerKey },
};
writeFileSync(path, JSON.stringify(out, null, 2));
chmodSync(path, 0o600);

console.log("Generated fresh testnet keypairs at:", path);
console.log("");
console.log("  RECEIVER (paid-oracle's pay_to)");
console.log(`    address: ${receiver.address}`);
console.log("");
console.log("  BUYER (your asker's wallet)");
console.log(`    address: ${buyer.address}`);
console.log("");
console.log("Next:");
console.log("  1. Fund BUYER on Base Sepolia:");
console.log("       - testnet ETH:  any Base Sepolia ETH faucet");
console.log("       - testnet USDC: https://faucet.circle.com (Base Sepolia)");
console.log("  2. Boot paid-oracle with PAID_ORACLE_PAY_TO set to RECEIVER above.");
console.log("  3. Run `bun buy.ts ETH-USD` to play the 402 dance.");
