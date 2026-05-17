# contracts

> _on-chain commitments for hum — slashable identity, manifest pointers, future settlement_

Solidity contracts that pair with hum's off-chain mesh layer
(`ensemble`). The off-chain wire stays high-frequency and free; the
chain stores **commitments** about that wire — provable, signable,
auditable.

Start here: [`HumdRegistry.sol`](src/HumdRegistry.sol).

## Why on-chain at all

Most of thrum is unsuitable for chain (chunked text at 10–100 Hz is
not a transaction). But three things have natural on-chain homes:

| concept | what's stored on-chain | use case |
|---|---|---|
| **HumdId identity** | `pubkey + owner address + manifest hash + URI` | censorship-resistant alternative to `hum/nestlings/announce` gossip. Anyone can verify "this address speaks for this HumdId" |
| **Conversation escrow** _(future)_ | `sigil → { from, to, amount, expiresAt }` | state channels for paid conversations — both sides deposit, release on signed transcript root |
| **Tool-call attestation** _(future)_ | `callId → (argsHash, resultHash, signer)` | provable execution of a tool call. Useful for regulated reads (KYC, AML, on-chain oracles) |

Today `HumdRegistry` is the only shipped contract. The other two are
reserved.

## Layout

```
contracts/
├── src/
│   └── HumdRegistry.sol     # the registry
├── test/
│   └── HumdRegistry.t.sol   # forge tests
├── foundry.toml             # build config
└── README.md
```

## Build

Uses [Foundry](https://book.getfoundry.sh/). One-time install:

```bash
curl -L https://foundry.paradigm.xyz | bash
foundryup
```

Then:

```bash
cd contracts
forge install foundry-rs/forge-std --no-commit   # one-time, populates lib/
forge build
forge test -vv
```

## Deploy

Default RPC targets are wired in `foundry.toml`. Deploy to Arc testnet:

```bash
export PRIVATE_KEY=0x...
forge create --rpc-url arc_testnet \
  --private-key $PRIVATE_KEY \
  src/HumdRegistry.sol:HumdRegistry
```

The deployed address is what you'll feed to humd / nestlings as
`HUMD_REGISTRY_ADDR`.

## How humd uses it

`ensemble::onchain::HumdRegistryClient` (Rust) reads the registry via
plain JSON-RPC `eth_call` — no alloy / ethers-rs dependency. See
[`ensemble/src/onchain.rs`](../ensemble/src/onchain.rs).

Typical flow:

1. A humd computes `keccak256(manifest_json_bytes)`.
2. It uploads the manifest JSON to IPFS (or any reachable HTTPS URL).
3. It calls `HumdRegistry.advertise(pubkey, hash, uri)` — one tx,
   ~50k gas. Costs ~$0.005 in USDC on Arc.
4. Other humds (in any ensemble, anywhere) read the registry, fetch
   the URI, verify the hash, and now have a manifest they can trust
   without trusting the gossip layer that surfaced this humd.

## Trust model

The contract only stores **claims**. Verification is:

| claim | how to verify |
|---|---|
| "address X owns HumdId H" | check `records[H].owner == X` on-chain |
| "this is the manifest H promised" | fetch `records[H].manifestURI`, hash the bytes, compare to `records[H].manifestHash` |
| "H is currently active" | check `records[H].updatedAt`; stale = probably offline |

The contract intentionally does NOT verify the ed25519 signature
behind the pubkey, doesn't slash, doesn't escrow funds. Those are
opt-in extensions that compose on top.

## What's NOT in v1

- **Ownership transfer.** v1 is intentionally rigid — the first
  advertiser owns the slot forever (or until they re-advertise from
  the same address). Adding transfer means adding access control
  primitives, which deserves its own audit.
- **Pubkey signature verification.** v1 trusts that `msg.sender`
  legitimately holds the ed25519 key behind `pubkey`. A future
  version can require a signed challenge.
- **Slashing.** Reserved. Will likely require a separate
  `HumdStake` contract holding USDC bonds.

## See also

- [`ensemble/src/nestlings.rs`](../ensemble/src/nestlings.rs) — the
  off-chain manifest type these commitments are about.
- [`ensemble/src/onchain.rs`](../ensemble/src/onchain.rs) — Rust
  client for reading this registry.
- [WIRE.md](../thrum/WIRE.md) — the protocol the manifest describes.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.
