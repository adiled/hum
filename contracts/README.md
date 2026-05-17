---
title: "contracts"
description: "on-chain commitments for hum — slashable identity, manifest pointers, future settlement"
---

# contracts

> _on-chain commitments for hum — slashable identity, manifest pointers, future settlement_

Solidity contracts that pair with hum's off-chain mesh layer
(`ensemble`). The wire stays high-frequency and free; the chain
stores **commitments** about that wire — provable, signable,
auditable.

## Interface is the standard

The interface — [`IHumdRegistry.sol`](src/IHumdRegistry.sol) — is
what hum considers canonical. Anyone can deploy a contract that
implements it. Off-chain clients (the Rust `ensemble::onchain`
module, future TS/Python/Go clients) read against the interface, not
against a specific deployment.

That means:

- **No canonical hum-maintainer deployment.** There is no single
  registry address everyone shares. The repo ships
  [`HumdRegistry.sol`](src/HumdRegistry.sol) — the vanilla reference
  implementation — but it's just *one possible* implementation.
- **Subnets are first-class.** A hackathon team, a single org, or a
  free agent deploys *their own* address on whatever chain they
  like. Their humds read/write only that address. Other subnets are
  isolated by address.
- **Policy lives in the implementation.** Want allowlisted advertise?
  Stake-on-advertise? Name aliases? Write a contract implementing
  `IHumdRegistry` with those rules. All humds keep reading you the
  same way they read the vanilla.
- **No hum-side governance required.** None of the contracts in this
  repo today have owner functions, multisigs, or upgrade paths.
  Deploying one is a one-time act.

See [`DEPLOYMENTS.md`](DEPLOYMENTS.md) for known subnet addresses
(empty by default — your subnet, your entry).

## What's on-chain vs what isn't

Most of thrum is unsuitable for chain — text streamed at 10–100 Hz
is not a transaction. Three things have natural on-chain homes:

| concept | what's stored on-chain | use case |
|---|---|---|
| **HumdId identity** | `pubkey + owner address + manifest hash + URI` | censorship-resistant alternative to `hum/nestlings/announce` gossip. Anyone can verify "this address speaks for this HumdId" |
| **Conversation escrow** _(future)_ | `sigil → { from, to, amount, expiresAt }` | state channels for paid conversations — both sides deposit, release on signed transcript root |
| **Tool-call attestation** _(future)_ | `callId → (argsHash, resultHash, signer)` | provable execution of a tool call. Useful for regulated reads (KYC, AML, on-chain oracles) |

Today the repo ships HumdRegistry. The other two follow the same
interface-as-standard pattern when they land.

## Layout

```
contracts/
├── src/
│   ├── IHumdRegistry.sol    # the interface — the standard
│   └── HumdRegistry.sol     # vanilla implementation
├── test/
│   └── HumdRegistry.t.sol   # forge tests against the vanilla impl
├── DEPLOYMENTS.md           # known subnet addresses (your entry, not ours)
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

## Deploy your own subnet

Default RPC targets are wired in `foundry.toml`. Deploy the vanilla
implementation to Arc testnet:

```bash
export PRIVATE_KEY=0x...
forge create --rpc-url arc_testnet \
  --private-key $PRIVATE_KEY \
  src/HumdRegistry.sol:HumdRegistry
```

`forge create` prints the deployed address. Hand it to your humds and
nestlings:

```bash
export HUMD_REGISTRY_ADDR=0xthe-address-forge-just-printed
export HUMD_REGISTRY_RPC=https://rpc.testnet.arc.network
```

Add your address to [`DEPLOYMENTS.md`](DEPLOYMENTS.md) so other
people running your subnet can find it.

## How humd uses it

`ensemble::onchain::HumdRegistryClient` (Rust) reads any
`IHumdRegistry`-shaped contract via plain JSON-RPC `eth_call` — no
alloy / ethers-rs dependency. See
[`ensemble/src/onchain.rs`](../ensemble/src/onchain.rs).

Typical flow:

1. A humd computes `keccak256(manifest_json_bytes)`.
2. It uploads the manifest JSON to IPFS (or any reachable HTTPS URL).
3. It calls `advertise(pubkey, hash, uri)` on its subnet's registry
   — one tx, ~50k gas.
4. Other humds in the same subnet read the registry, fetch the URI,
   verify the hash, and now have a manifest they can trust without
   trusting the off-chain gossip layer.

## Trust model

The contract only stores **claims**. Verification is:

| claim | how to verify |
|---|---|
| "address X owns HumdId H" | check `records[H].owner == X` on-chain |
| "this is the manifest H promised" | fetch `records[H].manifestURI`, hash the bytes, compare to `records[H].manifestHash` |
| "H is currently active" | check `records[H].updatedAt`; stale = probably offline |

The vanilla implementation makes no claims beyond that. Slashing,
ed25519-signature verification, name resolution — all opt-in by
swapping in a richer implementation of the same interface.

## See also

- [`ensemble/src/nestlings.rs`](../ensemble/src/nestlings.rs) — the
  off-chain manifest type these commitments are about.
- [`ensemble/src/onchain.rs`](../ensemble/src/onchain.rs) — Rust
  client. Reads against the interface, works with any implementation.
- [WIRE.md](../thrum/WIRE.md) — the protocol the manifest describes.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.
