---
title: "paid-oracle"
description: "x402-style paid oracle bee — sells one price per USDC payment, verified on-chain"
---

# paid-oracle

> _x402-style paid oracle bee — sells one price per USDC payment, verified on-chain_

A minimal **paid bee** for hum. Listens for `chi:"tool-call"`
with `name: "quote"`, replies with an HTTP-402-style payment
challenge, accepts retries carrying a transaction hash, verifies the
payment on-chain, then returns the price.

Reference for hackathon participants (e.g. Agora / Arc / Canteen) who
want to ship the **simplest possible monetized agent** running on hum.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| stateless (per-call) | lean | x402-over-tool-call | everything except hello/tool-call/tool-result/error |

## What it sells

Default: ETH-USD spot price from CoinGecko, repriced per call.
Configurable via `PAID_ORACLE_PRICE_URL`. Anything you can fetch over
HTTP with a single GET works.

## Wire

```
counterparty  ──── chi:tool-call { name:"quote", args:{ pair:"ETH/USDC" } } ───►  paid-oracle

              ◄─── chi:error    { code:402, x402:{ chain, pay_to, asset,
                                                    asset_kind:"erc20",
                                                    price_atomic, nonce, memo } }

  (counterparty pays USDC on Base / Arc / configured chain. The transfer's
   input data MUST contain the nonce bytes, so we can bind the payment
   to this specific quote — single payment can't claim two quotes.)

              ──── chi:tool-call { name:"quote",
                                   args:{ pair:"ETH/USDC" },
                                   paymentProof:{ txHash:"0x…", nonce } } ───►

              ◄─── chi:tool-result { result:{ pair, px, source, expires_at_ms } }
```

## Configure

| env | required | default | what |
|---|---|---|---|
| `PAID_ORACLE_PAY_TO` | yes | — | your EVM address — where USDC must land |
| `PAID_ORACLE_RPC` | no | `https://rpc.testnet.arc.network` | JSON-RPC endpoint |
| `PAID_ORACLE_CHAIN` | no | `arc-testnet` | human label echoed in challenges |
| `PAID_ORACLE_PAY_KIND` | no | `native` | `native` for Arc (USDC = gas token); `erc20` for Base / Eth mainnet |
| `PAID_ORACLE_USDC` | no | `0x000…` (native) / `0x833…2913` (erc20) | ERC-20 contract address (only used when `pay_kind=erc20`) |
| `PAID_ORACLE_DECIMALS` | no | `18` (native) / `6` (erc20) | USDC decimals on this chain |
| `PAID_ORACLE_PRICE_URL` | no | CoinGecko ETH/USD | where to fetch the underlying |
| `HUM_THRUM_SOCK` | no | `$XDG_RUNTIME_DIR/hum/thrum.sock` | humd's NDJSON socket |

Quote is hardcoded at $0.05 (5 cents). The atomic amount scales with
`PAID_ORACLE_DECIMALS` automatically — same $0.05 quote works whether
USDC has 6 decimals (ERC-20 chains) or 18 (Arc native). Edit
`QUOTE_PRICE_CENTS` in `src/main.rs` to change the price.

### Arc note

USDC is **the native gas token** on Arc (and has 18 decimals there,
not the 6 you'd expect from Ethereum/Base). Payment is a plain
`tx.value` transfer; the ERC-20 contract address `0x3600…0000` exists
for compatibility but day-to-day transfers go native. Default config
matches this — no flags needed.

## Run

```bash
# From the workspace root. Defaults to Arc testnet — just set your address:
PAID_ORACLE_PAY_TO=0xYourAddr cargo run -p paid-oracle

# Switch to Base mainnet (ERC-20 USDC):
PAID_ORACLE_PAY_TO=0xYourAddr \
PAID_ORACLE_RPC=https://mainnet.base.org \
PAID_ORACLE_CHAIN=base-mainnet \
PAID_ORACLE_PAY_KIND=erc20 \
PAID_ORACLE_USDC=0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
PAID_ORACLE_DECIMALS=6 \
  cargo run -p paid-oracle
```

The bee self-announces to the mesh as soon as it handshakes.
Other humds in the ensemble discover it via:

```rust
let mut found = ensemble.hive_discover("paid-oracle");
while let Some((humd_id, manifest)) = found.recv().await {
    // dial them, ask for a quote, pay, settle
}
```

## What it doesn't do

- **No on-chain writes.** Read-only RPC for `eth_getTransactionByHash`.
- **No LLM.** Pure data resale. Plug in `chi:"prompt"` instead of
  `chi:"tool-call"` and you have `paid-prompt` — inference-for-pay.
- **No facilitator.** Verifies the tx itself. Drop in
  [x402 facilitator](https://github.com/coinbase/x402) verification by
  POSTing the proof to a facilitator URL and trusting its signature.
- **No persistence.** Restart loses the `spent` nonce set. A real
  deploy writes it to disk so payments can't be replayed.
- **No KYC, no rate-limit, no auth beyond payment.** The trust seam
  is "the chain confirmed payment to my address." Layer your own
  policy on top.

## Why x402 over hum tones

x402 was designed for HTTP. We re-use the *semantics* (402 challenge,
retry with proof) but transport them over thrum's `chi:"error"` /
`chi:"tool-call"` pair. Same idea, no new protocol element required.
A bee that already knew how to do `chi:"tool-call"` can become
paid by adding one branch in its handler.

## Status

Reference implementation. Bug reports + PRs welcome.

## See also

- [`thrum-core`](../../thrum-core) — the wire contract this imports.
- [`ensemble/README.md`](../../ensemble/README.md) — full discover +
  trade flow including Kademlia lookup of unknown HumdAddrs.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.
