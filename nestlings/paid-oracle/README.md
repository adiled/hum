# paid-oracle

> _x402-style paid oracle nestling — sells one price per USDC payment, verified on-chain_

A minimal **paid nestling** for hum. Listens for `chi:"tool-call"`
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
| `PAID_ORACLE_RPC` | no | `https://mainnet.base.org` | JSON-RPC endpoint for verify |
| `PAID_ORACLE_CHAIN` | no | `base-mainnet` | human label, echoed in challenges |
| `PAID_ORACLE_USDC` | no | `0x8335…2913` (Base mainnet USDC) | ERC-20 contract |
| `PAID_ORACLE_PRICE_URL` | no | CoinGecko ETH/USD | where to fetch the underlying |
| `HUM_THRUM_SOCK` | no | `$XDG_RUNTIME_DIR/hum/thrum.sock` | humd's NDJSON socket |

Price is hardcoded at $0.05 atomic USDC. Edit `QUOTE_PRICE_ATOMIC` in
`src/main.rs` to change.

## Run

```bash
# From the workspace root.
cargo run -p paid-oracle

# Set your address (required):
PAID_ORACLE_PAY_TO=0xYourAddr cargo run -p paid-oracle

# Point at Arc instead of Base mainnet:
PAID_ORACLE_PAY_TO=0xYourAddr \
PAID_ORACLE_RPC=https://rpc.arc.network \
PAID_ORACLE_CHAIN=arc-mainnet \
PAID_ORACLE_USDC=0x...USDCOnArc \
  cargo run -p paid-oracle
```

The nestling self-announces to the mesh as soon as it handshakes.
Other humds in the ensemble discover it via:

```rust
let mut found = ensemble.nestling_discover("paid-oracle");
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
A nestling that already knew how to do `chi:"tool-call"` can become
paid by adding one branch in its handler.

## Status

Reference implementation. Bug reports + PRs welcome.

## See also

- [`thrum-core`](../../thrum-core) — the wire contract this imports.
- [`ensemble/README.md`](../../ensemble/README.md) — full discover +
  trade flow including Kademlia lookup of unknown HumdAddrs.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.
