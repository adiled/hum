---
title: "eggs-on-the-hum"
description: "four devices, four bees, one bloom that finishes while the human is buying eggs"
---

# eggs-on-the-hum

> _four devices, four bees, one bloom that finishes while the
> human is buying eggs_

See `sim/tests/eggs_on_the_hum.rs` for the executable form.

## The setup

One operator. Four humds in the same ensemble, each lending the bloom
one thing:

| device | humd | role | what it does |
|---|---|---|---|
| **laptop** | humd-L | forager | opens the bloom, and says where it builds: `cwd: hum://humd-W/work` |
| **server** | humd-S | nurse | grows the bloom in its cell; reaches `bash` when it needs the disk |
| **workstation** | humd-W | builder | owns the work disk; runs each `bash` in the sigil's shell, in `/work` |
| **phone** | humd-P | forager | observes the bloom's `chi:"wilt"`; sends an SMS |

Trust tier **T1** — all four humds share the operator's pubkey,
ensemble-signed peer link between every pair (full mesh keeps the
scenario simple; partial mesh works too via relay).

The operator types one prompt on the laptop ("refactor the auth
middleware to drop the legacy session-cookie path"), then closes the
laptop and walks to the corner store. The bloom doesn't pause: it
keeps growing on the server and building on the workstation while the
laptop sleeps.

## The happy path

```
laptop (humd-L)          server (humd-S)          workstation (humd-W)      phone (humd-P)
─────────────────        ─────────────────        ──────────────────────    ────────────────
 1. chi:"prompt"  ───►   …                         …                        …
    to: humd-S
    sid: hum-eggs
    cwd: hum://humd-W/work

 2. …                    the bloom builds on
                         humd-W; the nurse's
                         cell is handed bash

 3. …                    chi:"tendril-reach" ───► the builder runs it       …
                         name: bash               in hum-eggs's shell,
                         args: {command:          started in /work
                           "cat auth.ts"}

 4. …                    chi:"tendril-result" ◄── …                         …
                         back to the nurse
                         by callId

 5. (operator attaches phone, hearOnly, then walks away)

 6. …                    chi:"unfurl" ×N  ───►    …                        chi:"unfurl" mirror
                         as the model works,
                         reaching bash again
                         to edit auth.ts

 7. …                    chi:"wilt"       ───►    …                        chi:"wilt" arrives
                                                                            ✉  +1-…: "auth
                                                                               middleware
                                                                               refactor done"

 8. (operator at the egg counter, phone buzzes)
```

## What's load-bearing in each step

- **Step 1**: humd-L resolves the `cwd` to humd-W's full hid before the
  prompt leaves, and routes the prompt `to: humd-S` over the ensemble.
- **Step 2**: humd-S takes the `cwd` as where the bloom builds and pins
  the sigil there. Because the bloom builds somewhere, the prompt the
  nurse receives carries the thrum's `bash` in its `tools`.
- **Step 3**: the nurse's reach for `bash` goes to the builder where the
  bloom builds, not by name to anything nearby. humd-S records the
  callId against the nurse and routes the reach to humd-W, adding the
  shell's starting directory, `/work`.
- **Step 4**: the builder answers with the command's output and exit
  code. humd-W routes the result back to humd-S, which hands it to the
  nurse by its callId.
- **Step 5**: the phone emits `chi:"attach" {sid:"hum-eggs", to: humd-S,
  hearOnly: true}`, and humd-S adds humd-P to `observers["hum-eggs"]`.
- **Step 6–7**: the nurse's unfurls and its wilt fan out to every
  observer. The workstation never sees them; the phone does, and its
  forager sends the SMS when the bloom wilts.

## The failure modes

- **No builder on the workstation**. humd-W answers the reach itself,
  `isError` with "no builder", so the cell hears why instead of waiting.
- **The prompt names no cwd**. The bloom builds nowhere unless its
  forager's hello says `builds`; the nurse is handed no `bash`, and a
  reach for it is answered unserved.
- **Phone attaches after the wilt**. Without wane catch-up the phone
  misses it and no SMS fires; late-attach should still replay it.
- **Cross-humd auth fail**. A forged HumdId on the phone: humd-S must
  refuse the attach and never fan out an unfurl to it.

## The success criteria

- The workstation's builder runs exactly the nurse's command, in
  `/work`.
- The nurse hears what it grasped, by its callId.
- The phone observes the bloom's `chi:"wilt"` across the mesh.

## What this scenario validates

- **A bloom builds where its forager says.** The nurse has no host of
  its own; the workstation lends its shell, and nothing else does.
- **The reach's return path closes the loop.** humd-S maps the callId
  back to the nurse; no broadcast guessing.
- **Multi-observer fan-out works.** Two humds observe the same sid;
  both see the bloom; the nurse doesn't grow it twice.
- **Inference and the disk separate cleanly.** The nurse makes words;
  the builder changes the world. The split exists because the wire
  makes it cheap.

## Why it's "eggs"

Operator can leave. The bloom keeps running on iron the operator
doesn't have to babysit. The phone in their pocket is the thinnest
wire between the running bloom and the body buying groceries.
That's the test — not whether one of the four humds works, but
whether the wire is honest enough that you can walk away.
