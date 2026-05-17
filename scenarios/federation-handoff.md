---
title: "federation-handoff"
description: "two orgs shake hands, one hum crosses the seam under bounded power"
---

# federation-handoff

> _two orgs shake hands, one hum crosses the seam under bounded power_

See `sim/tests/federation_handoff.rs` for the executable form.

## The setup

Trust tier **T3** — federated. Two organisations, each with its own
root signing key:

- **org-A** runs **humd-A1**. Its operator holds a hum mid-task. The
  hum is the artefact to be exposed.
- **org-B** runs **humd-B1**. Its operator wants to attach a nestler
  to org-A's hum to assist on a defined slice of work.

Each org publishes a signing cert chain: `root → humd`. humd-A1 carries
a cert signed by org-A's root; humd-B1 carries one signed by org-B's
root. The two orgs have previously exchanged root pubkeys out of band
and pinned them in their respective config (`config::federation.peers`).

The handshake produces a **scoped capability grant**: a signed token
naming the sigil to be shared, the chi subset humd-B1 may emit
(`prompt`, `cancel` allowed; `permission-response` denied), an expiry
timestamp, and a revocation id. The token is presented on every tone
B1 sends and verified on every tone A1 receives.

## The happy path

1. Operator-A flags a hum for hand-off. humd-A1 mints a grant token
   signed by org-A's root, scoped to (sigil, allowed-chi-set, expiry,
   revocation-id), and ships it to humd-B1 over the federation link.
2. humd-B1 verifies the chain (org-A root → humd-A1 → grant), checks
   expiry, and stashes the token under the sigil.
3. humd-B1's nestler attaches to the hum. Every outbound tone carries
   the token. humd-A1 verifies the token on each tone before admitting
   it to the brood.
4. humd-B1 emits a `chi:"prompt"`. Token allows it; humd-A1 admits the
   prompt. Bloom proceeds; petals tee to both humds (per `co-pilot`).
5. humd-B1's nestler tries to answer a `chi:"permission-ask"`. Token
   denies that chi; humd-A1 drops the response and emits a
   `chi:"error"` qualified `federation.scope.denied` back to humd-B1.
   The hold stays open until Operator-A answers locally.
6. Operator-A clicks revoke. humd-A1 broadcasts a revocation tone
   naming the revocation-id; humd-B1 immediately tears down its attach
   and surfaces `chi:"error"` qualified `federation.revoked` to its
   nestler. Subsequent tones from humd-B1 fail handshake at humd-A1.
7. Token expiry fires on a separate sigil; humd-A1 silently stops
   admitting tones for that grant once the timestamp passes, even
   without an explicit revoke.

## The failure modes

- **Forged grant.** humd-B1 presents a token signed by the wrong root
  or with a tampered scope. humd-A1 must refuse on first tone and
  never emit a petal toward humd-B1. Test asserts zero leak.
- **Scope creep.** humd-B1 emits a chi outside the granted set
  (`permission-response`, `tool-result` for a tool it didn't own).
  humd-A1 must drop and surface `federation.scope.denied`; the hum's
  on-disk transcript must show no record of the rejected tone.
- **Stale token.** humd-B1 keeps using a token past expiry. humd-A1
  rejects with `federation.expired` *without* needing a revoke
  broadcast — the expiry check is local.
- **Slow revoke.** humd-A1 fires revoke; humd-B1's link is briefly
  partitioned. When the link heals, the queued tones must still be
  rejected — the revocation is sigil-scoped, not connection-scoped.
- **Cross-talk.** humd-B1 attempts to attach to a sigil it was not
  granted. humd-A1 returns `UnknownPeer`-shaped rejection; the
  presence of the federation link does not imply blanket access.

## The success criteria

- After step 3, humd-B1's tap receives `chi:"hello"` for the sigil
  within `RTT + 100ms`. Hello payload includes the token's
  capability-set so the nestler knows what it may emit.
- Every petal humd-A1's local tap sees, humd-B1's tap also sees,
  byte-identical modulo framing, within `RTT + 100ms`.
- Every forbidden tone humd-B1 emits produces a `chi:"error"` on
  humd-B1's tap with `qualifier:"federation.scope.denied"` and the
  rejected `rid`. None of those tones appear in humd-A1's hum log.
- After step 6, the next tone from humd-B1 (any chi) produces
  `federation.revoked` on humd-B1's tap and a `RouteError`-equivalent
  on humd-A1's side. `Ensemble::peers()` on humd-A1 either drops
  humd-B1 entirely or marks it `caps.scope=empty` for the sigil.
- The terminal `chi:"finish"` on the non-revoked sigil arrives on
  both taps with `usage.output_tokens > 0` and identical token
  counts.
- All admitted tones from humd-B1 carry a verifiable signature chain;
  the test asserts cryptographic verification, not just presence.

## What this scenario validates

- **Cert chain handshake.** Root → humd → grant verifies end-to-end.
  A real `Transport` impl (T3) is exercised by mocking the verifier
  inside `InMemoryEndpoint`'s middleware.
- **Revocation.** Pull a capability mid-flight; the hum survives, the
  peer is detached, no in-flight tones sneak through. Same primitive
  whether the revoke is operator-driven or expiry-driven.
- **Scoped capability.** The grant names a chi subset; the brood
  enforces it without consulting policy code on every tone.
- **Federation surface.** Identity crosses an org boundary while
  routing, replication, and wane continue to behave as in T1/T2 —
  trust changes, mechanics don't.
