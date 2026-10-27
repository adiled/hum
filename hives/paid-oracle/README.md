---
title: "paid-oracle"
description: "an attendant that sells a price for USDC, x402-style: terms on the first reach, the price on a reach with proof of payment"
---

# paid-oracle

> _an attendant that sells a price for USDC: a reach without payment is answered with terms, a reach with proof of payment with the price_

Its one bee is entered as an attendant, `main`, and serves one tool,
`quote`. A reach of `quote` with no payment is answered with the terms:
a `chi:"tendril-result"` whose `isError` is true and whose `result.code`
is `"402"`. The forager pays on chain and reaches again with the
transaction's hash; the oracle reads the transaction and its receipt from
its RPC, and when they pay the terms, answers with the price.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| stateless | lean | `x402/tendril` | everything but `hello`, `tendril-reach` and `tendril-result` |

Its hello lists those three chis, and carries its terms in
`ext["paid-oracle"].x402`: `chain`, `pay_to`, `usdc_contract`,
`decimals`, `pay_kind`, `price_atomic_usdc` and `confirmations`.

## What it sells

One number: the price at `PAID_ORACLE_PRICE_URL`, by default CoinGecko's
price of ETH in USD. The tool takes no pair, and its description names
the URL. The URL is the price source, and its contract is CoinGecko's
simple price: a `GET` answered with a 2xx and the JSON
`{ "<id>": { "<currency>": <number> } }`, one id holding one currency,
whose number is finite and above zero. Anything else is a failure: an
error status, a body that is not JSON, a second id or currency, a number
that is zero or a string.

The price is fetched once a payment verifies and before it is redeemed,
so a failure refuses the reach with `503` and spends nothing.

A quote costs 5 cents of USDC (`QUOTE_PRICE_CENTS` in `src/main.rs`), in
the chain's atomic units: 50000 with 6 decimals, 50000000000000000 with 18.

## Wire

```
forager ── chi:"tendril-reach" { callId, name: "quote", args: {} } ──► humd ──► paid-oracle

forager ◄── chi:"tendril-result" { callId, isError: true, output: "payment required: …",
                                   result: { code: "402", message: "payment required",
                                             x402: { chain, pay_to, asset, asset_kind, decimals,
                                                     price_atomic, confirmations, nonce } } }

        the forager pays pay_to at least price_atomic of USDC,
        with the nonce's bytes in the transaction's input

forager ── chi:"tendril-reach" { callId, name: "quote",
                                 args: { paymentProof: { txHash: "0x…", nonce } } } ──►

forager ◄── chi:"tendril-result" { callId, output, result: { px, source, expires_at_ms } }
```

Every answer is a `chi:"tendril-result"` under the reach's `callId` and
`sid`. Its `result` is JSON for a forager, and its `output` says the same
in words for a cell.

**The challenge** carries a fresh `nonce` for each reach without
`paymentProof`: 64 hex digits, 16 random bytes and the first 16 bytes
of their HMAC-SHA256 under the oracle's key, so the oracle holds no
challenge and knows its own nonces after a restart. `asset` is the USDC contract (the zero address
when USDC is native), `asset_kind` is `native` or `erc20`,
`price_atomic` is a string, and `confirmations` is how deep the payment
must be before it is taken.

**Paying.** The transaction's input must contain the nonce's UTF-8 bytes,
the whole string. Paying native USDC is a transaction to `pay_to` whose
value is at least `price_atomic` and whose input is those bytes. Paying
ERC-20 USDC is a call of `transfer(pay_to, amount)` on `asset`, with the
bytes appended to its calldata, or any call whose input carries the bytes
and in which `asset` moves the amount to `pay_to`.

**Verifying.** The oracle reads the transaction
(`eth_getTransactionByHash`), its receipt (`eth_getTransactionReceipt`)
and the chain's head (`eth_blockNumber`) from `PAID_ORACLE_RPC`, and
takes the payment when:

- the receipt names a block and its `status` is 1;
- that block is `confirmations` deep, the block itself counting as one;
- the transaction's input contains the nonce's bytes;
- native: its `to` is `pay_to` and its `value` is at least the price;
- ERC-20: the receipt's logs from `asset` hold `Transfer`s to `pay_to`,
  from anyone but `pay_to`, that sum to at least the price. The
  transaction's own `to` and calldata are not read.

**Redeeming.** A transaction redeems one call, however many nonces its
input carries. The oracle files its hash, in lower case, as
`redeemed/<txHash>` in the hive's state directory,
`$XDG_STATE_HOME/hum/hives/paid-oracle/`, after the price is fetched and
before it is answered; the file holds the nonce. The key the nonces are
signed under is `nonce.key` beside it, minted on the first start. A
restart forgets neither. Removing `nonce.key` voids every nonce issued
under it, and removing a file under `redeemed/` lets its transaction buy
again.

**The answer** is `{ px, source, expires_at_ms }`: `px` the price as a
string, `source` the URL it came from, and `expires_at_ms` 30 seconds
after it was fetched. `output` is the same JSON as text.

**Refusals** are a `chi:"tendril-result"` with `isError`, its `result`
`{ code, message }` and its `output` the message.

| `code` | when |
|---|---|
| `400` | `paymentProof` lacks a `txHash` of `0x` and 64 hex digits, or a `nonce` |
| `402` | the transaction does not pay the terms: it reverted, its input lacks the nonce, or it pays `pay_to` less than the price |
| `404` | the reach is for a tool other than `quote` |
| `409` | the transaction has already redeemed its call |
| `410` | this oracle never issued the nonce |
| `425` | the transaction is not mined, or not yet `confirmations` deep; reach again with the same proof |
| `503` | the RPC could not be read, the price could not be had, or the redemption could not be filed; nothing is spent, so reach again with the same proof |

## Who can buy

Every answer is a `tendril-result`, which a nurse hears as a forager
does, so a cell that reaches `quote` gets the terms as its tool's output
and goes on. A cell holds no wallet, so the paid flow is a
forager's: one that reaches `quote` itself, as [`buyer/`](buyer) does.
humd hands `quote` to every bloom on its humd, as it does every
attendant's tools.

humd announces the bee to its peers once it nestles
([ensemble](../../ensemble/README.md)). A forager on a peer humd reaches
it the same way, when that humd trusts remote attendants
(`humd.trustRemoteAttendants` in `hum.json`, or
`HUM_TRUST_REMOTE_ATTENDANTS`) and has this hive installed.

## Configure

| env | default | what |
|---|---|---|
| `PAID_ORACLE_PAY_TO` | required | the EVM address USDC must reach, `0x` and 40 hex digits; without it the bee does not start |
| `PAID_ORACLE_RPC` | `https://rpc.testnet.arc.network` | the JSON-RPC endpoint transactions are read from |
| `PAID_ORACLE_CHAIN` | `arc-testnet` | a label, echoed in the terms |
| `PAID_ORACLE_PAY_KIND` | `native` | `native`, where USDC is the gas token (Arc), or `erc20` |
| `PAID_ORACLE_USDC` | the zero address (native); `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`, Base mainnet's USDC (erc20) | the USDC contract whose logs are read, for `erc20` only |
| `PAID_ORACLE_DECIMALS` | `18` (native); `6` (erc20) | USDC's decimals on this chain, from 2 to 37 |
| `PAID_ORACLE_CONFIRMATIONS` | `1` | how deep a payment must be, at least 1 |
| `PAID_ORACLE_PRICE_URL` | CoinGecko's ETH in USD | the price source (above) |
| `HUM_THRUM_SOCK` | humd's socket | where the bee finds humd |
| `RUST_LOG` | `info` | what it logs |

An empty variable is unset, and keeps the default. A value that does not
parse, or an address that is not one, stops the bee at start.

The defaults are for Arc's testnet, where USDC is the native gas token
with 18 decimals, and a payment is the transaction's value.

## Run

`hum hive paid-oracle install` builds `target/release/paid-oracle` and
keeps it running under orchd. Each setting above but the last two is an
`ARG` of the Orchfile that the service is handed, so set any of them in
the environment of the install; one left out keeps its default.
Installed without `PAID_ORACLE_PAY_TO`, the bee stops at once, and orchd
starts it again.

```bash
# Arc testnet, native USDC: only the address to pay
PAID_ORACLE_PAY_TO=0xYourAddr hum hive paid-oracle install

# Base mainnet, ERC-20 USDC (the erc20 defaults are Base mainnet's)
PAID_ORACLE_PAY_TO=0xYourAddr \
PAID_ORACLE_RPC=https://mainnet.base.org \
PAID_ORACLE_CHAIN=base-mainnet \
PAID_ORACLE_PAY_KIND=erc20 \
  hum hive paid-oracle install
```

The bee runs by hand the same way, `PAID_ORACLE_PAY_TO=0xYourAddr
target/release/paid-oracle` in this directory, once the hive is
installed. It is not a member of the root cargo workspace; `./build`
here, or `cargo build --release`, builds it. The bee enters itself as
`main`, an attendant of the hive `paid-oracle`, and exits if humd
refuses it.

## Test

```bash
cd hives/paid-oracle && cargo test
```

`src/tests.rs` holds the oracle to a chain and a price source served on
loopback (`tests/chain`): settings, verification, redemption and the
price. `tests/reach.rs` runs the bee as orchd does against a real humd,
with each Orchfile setting it is not given handed to it empty: a nurse
and a forager reach `quote`, a forager buys a quote, and a restart
forgets no redemption and no nonce.

## The buyer

[`buyer/`](buyer) buys one quote end to end. It is a forager bee of this
hive, entered as `buyer`, and knows only Base Sepolia and ERC-20 USDC. It
spends testnet funds.

```bash
cd hives/paid-oracle/buyer
bun install
bun keygen.ts      # a receiver wallet and a buyer wallet, in $XDG_STATE_HOME/hum/hives/paid-oracle/keys.json
```

Fund the buyer's address with Base Sepolia ETH, for gas, and USDC
(`https://faucet.circle.com`). Run the oracle for Base Sepolia, paying the
receiver:

```bash
PAID_ORACLE_PAY_TO=<receiver address> \
PAID_ORACLE_RPC=https://sepolia.base.org \
PAID_ORACLE_CHAIN=base-sepolia \
PAID_ORACLE_PAY_KIND=erc20 \
PAID_ORACLE_USDC=0x036CbD53842c5426634e7929541eC2318f3dCF7e \
  ../target/release/paid-oracle
```

Then `bun buy.ts`. It reaches for a quote, sends the nonce-bound
`transfer`, waits for the confirmations the terms ask for, reaches again
with the proof, and prints the price, or the refusal and its code. It
reads `PAID_ORACLE_RPC` too (by default `https://sepolia.base.org`), and
takes the `tendril-result` under each reach's `callId` as its answer,
waiting 60 seconds for it.

## What it does not do

- **Bind a payment to its payer.** A proof is a bearer's: the nonce and
  the hash are public once the transaction is, and whoever presents
  them first gets the quote.
- **Follow a reorg.** A payment taken at `confirmations` deep stays
  redeemed if its block is later dropped.
- **See value moved inside a call.** A native payment is the
  transaction's own value to `pay_to`; value a contract forwards to it
  is not counted.
- **Write on chain, or call a facilitator.** It reads one transaction,
  its receipt, the chain's head and one URL. It takes x402's semantics,
  a 402 with terms and a retry with proof, and none of its wire: no HTTP
  headers and no signed authorization.
- **Ask for anything but payment.** The trust boundary is that the RPC
  showed a payment to `pay_to`.

## See also

- [THRUM.md, The tendril](../../THRUM.md#the-tendril): how humd routes a
  reach to an attendant and its answer back.
- [`hum-thrum`](../../hum-thrum): the Rust client this bee links with.
- [`ensemble/README.md`](../../ensemble/README.md): discovery across humds.
- [x402](https://github.com/coinbase/x402): the HTTP protocol whose
  semantics it borrows.
