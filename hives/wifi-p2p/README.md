---
title: "wifi-p2p"
description: "hum-over-WiFi-Direct — peer-to-peer agent mesh on Android's WifiP2p radio"
---

# wifi-p2p

> _hum-over-WiFi-Direct — peer-to-peer agent mesh on Android's WifiP2p radio_

A **forager** bee that bridges Android WiFi Direct (WPA2-PSK P2P groups)
into thrum. Two phones in the same room discover each other by service
scan, form a P2P group, and exchange hum tones over TCP — no router,
no cellular, no internet.

Each phone runs its own local humd (in Termux or on a companion
machine). The bee translates incoming P2P messages into `chi:"prompt"`
to humd and routes `chunk`/`finish` replies back over the P2P link.
Conversations are continuous per peer — the sid is keyed off the
peer's Ed25519 identity (its hid), stable across link drops and
reconnects.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| convention-stateful (per-peer sid) | lean | WiFi Direct P2P TCP (`p2p0` interface) | tools, system prompts, perf, drone, breath |

## Wire

```
┌─ Phone A (Group Owner) ─────────────────┐
│                                          │
│  Peer ── TCP :4377 ───► wifi-p2p bee     │
│  (Phone B)     p2p0      │               │
│                          │ chi:"prompt"   │
│                          ▼               │
│                        humd              │
│                          │               │
│                          │ chunk/finish  │
│                          ▼               │
│  Peer ◄── TCP :4377 ──── wifi-p2p bee    │
│  (Phone B)     p2p0      │               │
└──────────────────────────────────────────┘
```

## How it works

1. **Service discovery**. Phone A registers `_hum._tcp` on WiFi Direct
   and starts scanning. Phone B's bee sees the service and initiates a
   connection. The TXT record carries the phone's humd capabilities.

2. **Group formation**. One phone becomes the Group Owner (GO); the
   other connects as a client. The GO opens a TCP server on
   `p2p0:4377`; the client connects to the GO's P2P IP.

3. **Tone exchange**. Both sides speak NDJSON over the P2P TCP
   socket — the same framing as thrum. Each tone carries a `sid`
   keyed off the peer's hid, so the conversation survives
   disconnect-reconnect cycles.

4. **Local humd bridge**. Each bee connects to its local humd via
   Unix socket (Termux) or TCP bridge and translates inbound P2P
   tones into thrum prompts. Chunks are collected and the final
   reply is sent back over the P2P link.

## Configure

| env (Android, via `setprop` or config) | default | what |
|---|---|---|
| `HUM_P2P_PORT` | `4377` | TCP port on `p2p0` interface |
| `HUM_P2P_SERVICE_TYPE` | `_hum._tcp` | Bonjour-style service type for discovery |
| `HUM_P2P_MODEL` | `claude-haiku-4.5` | model humd spawns |
| `HUM_P2P_SYSTEM` | default system prompt | system instruction |
| `HUM_P2P_REPLY_LIMIT` | `4096` | hard cap on reply length |
| `HUM_THRUM_SOCK` | Unix socket or TCP bridge | thrum connection to humd |
| `HUM_P2P_GO_INTENT` | `8` | group owner intent (higher = more likely to be GO) |

## Permissions

Android manifest requires:

```xml
<uses-permission android:name="android.permission.NEARBY_WIFI_PEERS" />
<uses-permission android:name="android.permission.ACCESS_FINE_LOCATION" />
<uses-permission android:name="android.permission.ACCESS_COARSE_LOCATION" />
<uses-permission android:name="android.permission.FOREGROUND_SERVICE" />
<uses-permission android:name="android.permission.FOREGROUND_SERVICE_DATA_SYNC" />
<uses-permission android:name="android.permission.POST_NOTIFICATIONS" />
```

## Build

```bash
cd hives/wifi-p2p
gradle build
# produces app/build/outputs/apk/debug/app-debug.apk
```

## Install (via hum managed service)

If humd and orchd are running on the Android device (Termux):

```bash
hum hive install ./hives/wifi-p2p
```

Or sideload the APK and start the service manually:

```bash
adb install app/build/outputs/apk/debug/app-debug.apk
adb shell am start-foreground-service \
  -n hum.hive.wifip2p/.WifiP2pBeeService \
  -a start
```

## What flows where

| P2P tone | hum chi |
|---|---|
| `{"chi":"prompt","sid":"<peer-hid>","text":"..."}` | `chi:"prompt"` to humd (sid keyed off peer hid) |
| humd's `chi:"chunk"` text parts | collected into one reply buffer |
| humd's `chi:"finish"` | `{"chi":"finish","sid":"<peer-hid>","reply":"..."}` over P2P TCP |

The P2P wire uses the same NDJSON framing as thrum, so a peer that
also runs humd can forward tones directly. The bee is the translator
between the P2P radio and the local thrum socket.

## What it doesn't do

- **No mesh routing.** WiFi Direct groups are star topologies (one GO,
  multiple clients). Cross-group routing requires a humd with multiple
  P2P interfaces or an ensemble gossip layer over an alternative
  transport.
- **No background scanning.** Android 13+ restricts background WiFi
  scanning; the bee must be a foreground service with a notification.
- **No encryption beyond WPA2.** The P2P link is WPA2-PSK protected;
  no application-layer encryption. For production, pair with the
  ensemble's Ed25519 handshake.
- **No STA concurrency.** Many phones can't do P2P + STA (normal WiFi)
  simultaneously. The bee detects this and falls back gracefully.
- **No cross-device group persistence.** Groups are ephemeral; the bee
  re-forms on each discovery cycle.

## See also

- [`gsm-modem`](../gsm-modem) — same forager pattern over GSM AT-command serial
- [`bp7`](../bp7) — same forager pattern over Bundle Protocol v7 (DTN)
- [`twilio-sms`](../twilio-sms) — same forager pattern over Twilio webhook
- [WIRE.md](../../WIRE.md) — the thrum protocol spec
- [Android WifiP2pManager docs](https://developer.android.com/guide/topics/connectivity/wifip2p)
