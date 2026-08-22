---
title: "wifi-p2p meetup"
description: "three phones, one picnic table, no cloud — the hum spreads by proximity"
ensemble:
  - alice-phone
  - bob-phone
  - carla-phone
  - the-table
---

# wifi-p2p meetup

> _three phones, one picnic table, no cloud — the hum spreads by proximity_

## Cast

| device | role | what it runs |
|---|---|---|
| **Alice's Pixel 9** | Group Owner + worker | humd (Termux), wifi-p2p bee, claude-cli worker |
| **Bob's Galaxy S25** | Client + worker | humd (Termux), wifi-p2p bee, claude-cli worker |
| **Carla's iPhone 17** | Client + forager | humd (iSH or relay), wifi-p2p bee, openai-server forager |
| **the picnic table** | the place | a cardboard sign that says "FREE HUMS" in sharpie |

## The scene

Sunday afternoon. A park in Bushwick. Three humans who have never met,
each nursing a phone and a half-empty thermos, sit at the same
weathered picnic table. No router. No cellular plan that matters. No
intention to cooperate — just three people who showed up for the sun.

Alice's phone buzzes. She has a humd running in Termux — a side effect
of being the kind of person who reads install scripts before running
them. Her wifi-p2p bee has been scanning for thirty seconds and found
two peers: `galaxy-s25` and `iphone-17`. The bee forms a group. Alice
is the Group Owner because her phone has the better battery.

Bob's phone joins. Carla's phone joins. Three phones on a P2P TCP mesh
under a single tree, no packets leaving the park.

Alice doesn't notice any of this. She's reading a zine about
fermentation.

## The first bloom

Bob has been stuck on a recipe for sourdough starter that calls for
"one cup of chaos agent." He opens his hum CLI and types:

```
hum ask alice-phone "what does 'one cup of chaos agent' mean in a
sourdough context? also the starter is runny, should i add flour"
```

The prompt leaves Bob's phone as a thrum tone, hits his local humd,
which checks the mesh: Bob's phone has no worker that serves
sourdough expertise. But Alice's phone advertised `claude-cli` as a
worker hive in its hello. The ensemble routes the prompt to Alice's
humd over the P2P TCP link.

Alice's humd spawns a cell. The cell runs Claude. Claude replies:

```
"Chaos agent" in sourdough means the wild yeast you're cultivating —
it's unpredictable by design. If your starter is runny, add 10% more
flour by weight. You're fine.
```

Chunks stream back across the picnic table, over the P2P link, into
Bob's phone. The finish tone lands. Bob adds flour. His starter
thickens. He smiles.

Alice never knew any of this happened. Her phone thought it was idle.

## Carla needs a lift

Carla's iPhone runs humd via iSH (iOS terminal emulator) with a
wifi-p2p bee that connects as a client. Her phone has no local LLM —
she's using the openai-server forager pointed at her OpenAI API key.
But the API key is rate-limited and she's already hit the tier-1 cap.

Her forager advertises `["openai-server"]` in its provides. Bob's humd
sees this and marks Carla as a route for OpenAI-shaped prompts. When
Alice's phone needs an embedding for a retrieval task, the prompt
routes: Alice's humd → P2P TCP → Carla's phone → OpenAI API → reply
bundle → P2P TCP → Alice. Carla's phone acts as a paid-oracle relay
without Carla noticing. Her battery drains 2% faster. She thinks it's
the TikTok tab.

## The table becomes a nest

By hour two, the three phones have gossiped enough state that the
mesh acts as a single distributed humd. A prompt typed on any phone
finds compute on any other phone, or reaches OpenAI through Carla's
forager, or forks a tool-call to Bob's phone for filesystem access.

The picnic table has no power and no internet. It is a wooden surface
with birdshit on it. But the hum on top of it is a three-node ensemble
with routing, capability discovery, and session continuity.

When a fourth person sits down — a girl with a Nokia 3310 and no humd
at all — Alice's phone scans, finds nothing, and keeps scanning. The
mesh doesn't extend to her. She eats her sandwich and leaves. The hum
doesn't notice.

## Dispersal

At 4pm, Carla stands up and walks toward the subway. Her phone
exits the P2P group gracefully — the GO detects the link drop,
emits `chi:"error"` with `lost` reason, and the remaining two phones
re-form the group without her. Bob's starter question is already
finished; no session is orphaned.

Alice's phone has been the GO for two hours. Its battery is at 18%.
When she finally gets up and walks home, the P2P group dissolves. No
state is lost — each humd has its own nest, and the ensemble gossip
washed every tone across all three phones. Alice's phone holds a
complete copy of Bob's sourdough conversation, Bob's phone holds a
copy of Carla's OpenAI relay logs, and none of them ever talked to a
server.

## What this setup looks like

On each phone, the hum setup is:

```json
{
  "hum": {
    "hives": ["claude-cli", "wifi-p2p"],
    "ensemble": {
      "discovery": "wifi-p2p",
      "transport": "p2p-tcp"
    }
  }
}
```

Three binaries running:

```
Termux:
  humd                     — the daemon, one per phone
  claude-cli-worker        — local inference (Alice, Bob)
  openai-server            — cloud relay (Carla)

Android foreground service:
  wifi-p2p-bee             — P2P discovery + group + TCP bridge
                          — connects to local humd via Unix socket
                          — translates P2P NDJSON ⇄ thrum NDJSON
```

No config. No `peers.json`. No DNS. The phones find each other by
WiFi Direct service scan, form a group, and the ensemble layer
discovers capabilities by gossip. The bee is the radio bridge; the
rest is stock hum.

## Enjoyment

Bob's starter works. Alice's battery is low but her zine is good.
Carla's API key survives the afternoon. None of them configured
anything. The hum found the mesh by walking around.

This is the use case: **three strangers at one table, no cloud, no
setup, no intentional cooperation, and the conversation still routes
through the best compute the mesh can find.** The bees do the
foraging; the humans just sit in the sun.
