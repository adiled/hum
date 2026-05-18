---
title: "eggs-on-the-hum"
description: "four devices, four bee roles, one hum that finishes while the human is buying eggs"
---

# eggs-on-the-hum

> _four devices, four bee roles, one hum that finishes while the
> human is buying eggs_

See `sim/tests/eggs_on_the_hum.rs` for the executable form.

## The setup

One operator. Four humds in the same ensemble, each running a
different bee role:

| device | humd | bee | hive | what it does |
|---|---|---|---|---|
| **laptop** | humd-L | nestler (the asker) | — | TUI / openai-server forager; emits the prompt and observes the bloom |
| **server** | humd-S | worker | `claude-cli` | runs the LLM; emits `chi:"chunk"` / `"tool-call"` / `"finish"` |
| **workstation** | humd-W | forager | `humfs` | owns the work disk; handles `chi:"tool-call"` for `humfs_*` tools |
| **phone** | humd-P | forager | `twilio-sms` | observes `chi:"finish"` on the sid; sends an SMS |

Trust tier **T1** — all four humds share the operator's pubkey,
ensemble-signed peer link between every pair (full mesh keeps the
scenario simple; partial mesh works too via relay).

The operator types one prompt on the laptop ("refactor the auth
middleware to drop the legacy session-cookie path"), then closes
the laptop and walks to the corner store. The bloom doesn't pause
— it just keeps running across three other humds while the laptop
sleeps.

## The happy path

```
laptop (humd-L)          server (humd-S)          workstation (humd-W)      phone (humd-P)
─────────────────        ─────────────────        ──────────────────────    ────────────────
 1. chi:"prompt"  ───►   …                         …                        …
    to: humd-S
    sid: hum-eggs

 2. …                    chi:"prompt"             …                         …
                         delivered to claude-cli
                         worker; claude reasons

 3. …                    chi:"tool-call"  ───►    …                         …
                         toolName: humfs_read     humfs forager dispatches
                         callId: c-1              against /Users/op/auth/...

 4. …                    chi:"tool-result"  ◄───  …                         …
                         routed back to worker
                         by humd-S via call_id

 5. (operator attaches phone, hearOnly, then walks away)

 6. …                    chi:"chunk" ×N   ───►    …                        chi:"chunk" mirror
                         (text_delta + tool_use                              (laptop tab too,
                          for additional reads /                              until laptop sleeps)
                          edits as the model
                          works)

 7. …                    chi:"tool-call"  ───►    humfs_do_code             …
                         toolName: humfs_do_code  rewrites auth.ts inside
                         args:{symbol:"...",      the workstation's fs.roots
                              new_source:"..."}

 8. …                    chi:"tool-result"  ◄───  …                         …

 9. …                    chi:"finish"     ───►    …                        chi:"finish" arrives
                                                                            twilio-sms forager
                                                                            ✉  +1-…: "auth
                                                                               middleware
                                                                               refactor done"

 10. (operator at the egg counter, phone buzzes)
```

## What's load-bearing in each step

- **Step 1**: laptop's local hum routing sends `to: humd-S` over
  ensemble; `humd-S` accepts the prompt as if it had been delivered
  via local thrum.
- **Step 3**: server's worker hello declared `bee: ["worker"]`,
  workstation's hello declared `bee: ["forager"], tools: [humfs_*]`.
  humd-S's chi:tool-call interception (P8) routes the tone by
  `toolName` → workstation's client_id → ensemble pump → humd-W →
  humfs-forager. `tool_routes` records `callId → worker client_id`
  on humd-S so step 4's tool-result returns to the right place.
- **Step 4**: workstation's humfs runs the read against its own
  `fs.roots`, emits `chi:"tool-result"` with the same callId.
  humd-W routes the tone back to humd-S via ensemble; humd-S looks
  up `tool_routes[callId]` and forwards to the worker.
- **Step 5**: phone's nestler emits `chi:"attach" {sid:"hum-eggs",
  to: humd-S, hearOnly: true}`. humd-S adds humd-P to
  `observers["hum-eggs"]`. The laptop stays attached too, hearOnly
  or otherwise — the bloom doesn't care.
- **Step 6–8**: worker's chunks fan out to every observer. humd-S
  iterates `observers["hum-eggs"]`, stamps `to: <peer>` per copy,
  routes via ensemble. The workstation never sees the chunks (no
  attach); the phone does.
- **Step 9**: same observer fan-out for `chi:"finish"`. humd-P's
  twilio-sms forager handles it locally — its hello declared
  `chis: [..., "finish"]` and its dispatcher matches on `chi`.
  Forager dispatches the SMS over the Twilio HTTPS API.

## The failure modes

- **No humfs registered**. Server's worker emits chi:"tool-call"
  for `humfs_read`; humd-S finds no forager hive in its manifests
  carrying that tool name. Falls through to the sigil broadcast,
  which lands on the laptop's openai-server forager — wrong place.
  Test asserts that when only workstation registers `humfs`, the
  tool-call lands at humd-W, never at humd-L.
- **Phone attaches AFTER finish**. Without wane catch-up, the
  phone's twilio forager misses the finish and no SMS fires. Test
  asserts late-attach still gets the finish (replay against wane
  cursor on the laptop / workstation cache).
- **Workstation drops mid-tool-call**. callId is in `tool_routes`
  but the forager went away. Server's worker should surface an
  error tone after the timeout; humd-S clears the route; phone
  observes `chi:"error"` not silence.
- **Cross-humd auth fail**. Phone's HumdId is forged; humd-S must
  refuse the attach and never fan out a chunk to it.

## The success criteria

- After step 1, humd-S receives `chi:"prompt" sid=hum-eggs` within
  `RTT + 50ms` of laptop's send, with `from: humd-L`.
- Step 3's `chi:"tool-call"` reaches humd-W within `RTT + 50ms`
  carrying `toolName: humfs_read` and a non-empty `callId`.
- Step 4's `chi:"tool-result"` reaches the worker on humd-S within
  `tool_call_RTT + 50ms` and carries the original `callId`.
- After step 5, humd-S's `observers["hum-eggs"]` list contains
  humd-P (and humd-L unless the laptop explicitly detaches).
- Step 9's `chi:"finish"` reaches humd-P's twilio forager within
  `RTT + 50ms` of the worker's emission.
- The phone's SMS-emitted assertion fires inside the test's
  deadline; the captured SMS body references the sid.

## What this scenario validates

- **Forager hives are ensemble-routable.** humfs on a different
  humd than the worker reachable by `toolName`. No assumption that
  fs lives with compute.
- **Tool-call return path closes the loop.** `tool_routes` on
  humd-S maps callId back to the originating worker; humd-W's
  result arrives via ensemble, gets handed to the right worker,
  no broadcast guessing.
- **Multi-observer fan-out works.** Two humds (laptop, phone)
  observe the same sid; both see the bloom; the worker doesn't
  duplicate compute.
- **The forager catalogue scales.** Adding twilio-sms as a third
  forager (alongside humfs and openai-server) doesn't require
  changes to humd; the hello mechanism + observer fan-out covers
  it.
- **Compute and fs separate cleanly.** Worker has no fs; the
  workstation's humfs has no compute. The split exists because the
  wire makes it cheap.

## Why it's "eggs"

Operator can leave. The bloom keeps running on iron the operator
doesn't have to babysit. The phone in their pocket is the thinnest
wire between the running bloom and the body buying groceries.
That's the test — not whether one of the four humds works, but
whether the wire is honest enough that you can walk away.
