---
title: "android-humd"
description: "humd for Android — the native Rust daemon, cross-compiled and hosted by a foreground service, with WiFi Direct as the radio underlay"
---

# android-humd

> _humd for Android — the actual daemon, not a thin client._

This is **humd itself compiled for Android**: the full Rust workspace
(`humd` + `ensemble` + `config` + `thrum-core` + `hum-paths` + …)
cross-compiled to an `aarch64-linux-android` PIE ELF and hosted by a
native foreground service. Rust was chosen because it runs everywhere —
so the portability problem is *build* engineering, not a rewrite: the
daemon boots unchanged, hum-paths resolves everything from XDG env
vars, and the host points those at app-private storage.

No Termux, no root, no JNI. The binary ships as an asset and is
`exec`'d by the service.

## Why this exists

The ensemble is the mesh of cooperating humds. To bring a phone into
it without Termux, the phone must *run* humd, not just dial one. This
module is that port. Two layers stack:

- **Overlay — the ensemble mesh**: iroh (QUIC + Noise + relay
  hole-punching) reaches the machine's humd over the internet / relay,
  no WiFi Direct needed. This is the "thrum is the end of TCP" path.
- **Underlay — WiFi Direct**: `WifiP2pManager` gives phones an ad-hoc
  L2 link (p2p0) without a router or internet — the phone↔phone
  meetup case. The native daemon's iroh runs on p2p0 when a group forms.

## Layout

```
android/
  scripts/build-humd.sh   NDK cross-compile of the workspace humd → assets
  app/                     Android app (foreground service host)
    src/main/java/hum/daemon/
      HumdService.kt       spawns + supervises the native humd, writes config
      WifiDirectManager.kt radio underlay (WifiP2pManager discovery + groups)
```

## Build

Prereqs: `rustup target add aarch64-linux-android`, an Android NDK
(`brew install --cask android-commandlinetools`, then
`sdkmanager "ndk;27d"`, or set `ANDROID_NDK_HOME`).

```sh
# cross-compile humd and bundle it into the app
./android/scripts/build-humd.sh

# then assemble the APK from android/
(cd android && ./gradlew :app:assembleDebug)
```

## Runtime

`HumdService` (a foreground `dataSync` service) extracts the bundled
binary, writes a minimal `hum.json` (all sections default) plus an
optional `peers.json` from `-Dhum.peerHint=humd_id,iroh:nodeid` so the
phone daemon dials the machine on boot, points XDG_* at app-private
storage, and `exec`s the daemon — supervising it and logging to
`filesDir/humd.log`.

The phone then participates in the ensemble exactly as any humd: signed
hello, peer registry, kad, gossip — and routes prompts to whatever
worker bees register (e.g. the machine's `ollama-worker`).

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| convention-stateful | lean | ensemble (iroh QUIC) + thrum (Unix socket) | tools, drone, breath |

## See also

- `hives/wifi-p2p` — the radio-underlay prototype (WiFi Direct forager bee)
- `ensemble/` — the mesh layer this daemon runs
- `hum-paths/` — XDG resolution that makes the port a build problem, not a code problem
