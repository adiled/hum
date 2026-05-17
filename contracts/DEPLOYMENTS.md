# DEPLOYMENTS

Known deployments of `IHumdRegistry`-shaped contracts, by chain and
subnet.

**There are no canonical hum-maintainer deployments.** This file
exists so subnets can publish their addresses for the humds and
agents that participate in them. Adding your subnet is a PR adding
one row.

## Schema

Each row:

| field | what |
|---|---|
| chain | EVM chain label (`arc-testnet`, `base-mainnet`, `private-l2-xyz`) |
| subnet | a short, distinctive name for the deployment (your team, your project) |
| address | 0x-prefixed contract address |
| implementation | which `IHumdRegistry` impl (e.g. `HumdRegistry`, or a richer fork) |
| operator | rough pointer to who runs / owns this subnet (URL or handle) |
| notes | optional — allowlist policy, fees, anything a participant should know |

## Registry of registries

| chain | subnet | address | implementation | operator | notes |
|---|---|---|---|---|---|
| _(empty — no subnets have published yet)_ | | | | | |

## How to add your subnet

1. Deploy `HumdRegistry.sol` (or your own `IHumdRegistry` impl) to
   your target chain.
2. Open a PR adding one row above. Use `_unverified_` in the notes
   column if your deployment is brand new and you haven't operated
   it under load yet.
3. Tell your humds the address via `HUMD_REGISTRY_ADDR` /
   `HUMD_REGISTRY_RPC` env vars.

## Why not a canonical mainnet deployment

A single canonical address would imply hum-maintainer governance:
who pays gas? who handles abuse? who hard-forks if it ever needs
upgrading? None of those questions need answers today, and pushing
deployment down to subnets sidesteps them entirely. If a canonical
deployment ever makes sense, it will be additive — subnet
deployments stay valid forever regardless.
