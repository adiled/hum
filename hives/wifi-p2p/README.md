---
title: "wifi-p2p"
description: "hum over WiFi Direct: an Android forager that carries prompts from the phones in its P2P group to a humd, and each bloom back to the phone that asked"
---

# wifi-p2p

> _hum over WiFi Direct: an Android forager that carries prompts from the phones in its P2P group to a humd, and each bloom back to the phone that asked._

The hive's bee is an Android foreground service, `WifiP2pBeeService` in
the app `hum.hive.wifip2p`, entered as a forager on the humd it reaches.
It advertises `_hum._tcp` over WiFi Direct, forms a group with a phone
that advertises it too, and takes tones from the phones in its group over
TCP: a `prompt` opens a bloom on humd, and when the bloom wilts its text
goes back to the phone that asked, as one petal and a wilt.

The half of the app that speaks the thrum and forages runs on the JVM,
and is tested there against a real humd ([Tests](#tests)). The half that
speaks WiFi Direct compiles and has not run on a phone ([What is not
yet](#what-is-not-yet)).

## The hive

The Orchfile has no units. Installing the hive teaches a humd the kind,
and nothing runs on that humd's machine; the bee is entered there and its
key goes on the phone:

```bash
hum hive wifi-p2p install
id=$(hum bee enter --hive wifi-p2p --role forager --as phone)
key=${XDG_STATE_HOME:-$HOME/.local/state}/hum/bees/$id.key
```

The service reads the bee, and its settings, from the app's files
directory:

| file | holds |
|---|---|
| `/data/data/hum.hive.wifip2p/files/hum/bee` | the id, `bee_<hex>` |
| `/data/data/hum.hive.wifip2p/files/hum/bees/<id>.key` | the key, the 32-byte seed `hum bee enter` filed |
| `/data/data/hum.hive.wifip2p/files/hum/wifi-p2p.json` | its settings ([Configure](#configure)), optional |

On a debug build, `adb shell run-as hum.hive.wifip2p` reaches that
directory:

```bash
adb shell run-as hum.hive.wifip2p mkdir -p files/hum/bees
echo "$id" | adb shell run-as hum.hive.wifip2p sh -c "'cat > files/hum/bee'"
adb exec-in run-as hum.hive.wifip2p sh -c "'cat > files/hum/bees/$id.key'" < "$key"
```

On every connect the service checks that the key hashes to the id and
signs its hello with it, as hum-identity does: ed25519 over
`hum-bee-socket-v1:<hid>:<signedAt>`. The app keeps its files out of
cloud backup and device transfer, so the key stays on the phone it was
put on.

## Reaching humd

humd admits bees only on its Unix socket; its TCP listener,
`humd.tcpListen`, is for peer humds. The service dials the `humd`
address in its settings, as hum's client does: it sends its signed hello,
takes humd's echo and then a breath as admission and an echo with `ok:
false` as a refusal, and dials again when the connection closes, 250 ms
doubling to 30 s with a fifth of jitter either way. A refusal is final:
the service stops.

| `humd` | what the service dials |
|---|---|
| `/<path>` | a Unix socket, with `LocalSocket` |
| `<host>:<port>` | a TCP stream that carries the thrum to humd's socket |

For humd in Termux on the same phone, the socket is no use to the app as
it is: Termux's files are private to Termux, and Android gives two apps
no shared place for a socket file. Something in Termux carries a loopback
port to the socket instead, and the default address, `127.0.0.1:4378`,
is that port:

```bash
# in Termux, beside humd
pkg install socat
socat TCP-LISTEN:4378,bind=127.0.0.1,reuseaddr,fork \
  UNIX-CONNECT:${XDG_STATE_HOME:-$HOME/.local/state}/hum/thrum.sock
```

humd sees each connection as one on its socket, and admits the bee by
its signed hello as it admits any other; nothing in humd changes. The
port is open to every app on the phone, though, where the socket was
open only to Termux. humd refuses a hello that is not signed by a key
entered for its hive and role, but it does not wait for a hello before it
hears a connection's other tones: a connection that never says hello can
still open a bloom. Through the forwarder, every app on the phone can do
that. For the port to be no more open than the socket, humd would hear
nothing but a hello from a connection until a breath admits it.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| convention-stateful (a line of blooms per peer and sid) | lean | `wifi-p2p/tcp`, the stream over WiFi Direct TCP | tendrils, every petal but text, the unfurls as they grow, the graft |

Its hello declares `hello`, `prompt`, `cancel`, `cleanup`, `unfurl`,
`wilt` and `error`.

## The group

The bee advertises itself as a WiFi Direct DNS-SD service, `_hum._tcp`,
instance `hum`, and looks for it once a minute while it is in no group.
It asks to connect to the first device it finds advertising it, with its
group-owner intent, and Android forms the group. Once a group forms, the
group owner listens on TCP port 4377 at the group address, and a client
dials the owner there, again whenever the link drops, for as long as the
group lasts. A link is known by its address and port, `<ip>:<port>`,
never by a key.

A link is the same both ways: a prompt that arrives on one is asked of
this bee's humd. A phone in the group with no bee asks the owner's bee
by connecting to the owner's group address, `192.168.49.1:4377` on
Android:

```
$ nc 192.168.49.1 4377
{"chi":"hello","rid":"h1"}
{"chi":"breath","rid":"h1","sigils":{},"models":[{"id":"claude-haiku-4-5","hive":"claude-cli"}]}
{"chi":"prompt","rid":"a1","sid":"sourdough","text":"what does one cup of chaos agent mean?"}
{"chi":"unfurl","rid":"a1","sid":"sourdough","petal":0,"kind":"text","text":"…","closed":true}
{"chi":"wilt","rid":"a1","sid":"sourdough","reason":"stop"}
```

## The stream on a link

Each line is one tone. A line over 256 KiB is dropped whole, and so is a
tone past its `dusk`.

| P2P tone | what the bee does |
|---|---|
| `{"chi":"prompt","rid":"…","sid":"…","text":"…"}` | a `chi:"prompt"` to humd with that text, and the tone's own `modelId`, `systemPrompt` and `dusk` when it has them, the bee's [settings](#configure) otherwise |
| `cancel`, `cleanup` | the same chi to humd, under that sid |
| `hello` | answered with a `breath` whose `models` are what humd's last breath said this forager can reach |
| anything else | nothing |

The sid on humd is `wifi-p2p-` and the first 16 hex digits of
sha256(`<peer ip> <sid>`), so each sid a peer names is a line of blooms
of its own, and no peer grows on another's. A peer that names no sid has
one line.

When the bloom wilts, its text petals, gathered and trimmed (or `(no
reply)`), go to the peer that asked, under its own rid and sid:

```
{"chi":"unfurl","rid":"<its rid>","sid":"<its sid>","petal":0,"kind":"text","text":"…","closed":true}
{"chi":"wilt","rid":"<its rid>","sid":"<its sid>","reason":"<the bloom's reason>"}
```

A bloom that breaks off, or a prompt that cannot open one, sends the
peer `{"chi":"error","rid":"…","sid":"…","code":"…","message":"…"}`
instead:

| `code` | when |
|---|---|
| humd's own (`no-nurse`, `empty-prompt`, …) | the bloom broke off on humd |
| `refused` | humd refused the prompt |
| `away` | humd was away, and the prompt was not sent |
| `growing` | a bloom is already open on that sid |
| `astray` | humd went away mid-bloom |

A peer that leaves cancels the blooms still growing for it.

## Configure

`files/hum/wifi-p2p.json`, read when the service starts. Every key is
optional, and a file that does not parse is the defaults.

| key | default | what |
|---|---|---|
| `humd` | `127.0.0.1:4378` | where humd is ([Reaching humd](#reaching-humd)) |
| `port` | `4377` | TCP port on the group address |
| `goIntent` | `8` | group-owner intent on a connect, 0 to 15, higher is likelier to own |
| `serviceType` | `_hum._tcp` | the DNS-SD service the bee advertises and looks for |
| `modelId` | none | the `modelId` on a prompt whose peer names none; without one, humd hands the prompt to any nurse with room |
| `systemPrompt` | `You are a concise assistant for P2P mesh agents.` | the `systemPrompt` on a prompt whose peer gives none; `null` for none |
| `replyLimit` | `4096` | the most characters a reply keeps; a longer one is cut and ends in `...` |

## Permissions

The manifest declares `ACCESS_WIFI_STATE`, `CHANGE_WIFI_STATE`,
`NEARBY_WIFI_DEVICES` (`neverForLocation`), `ACCESS_FINE_LOCATION` and
`ACCESS_COARSE_LOCATION` up to Android 12, `INTERNET`,
`FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_CONNECTED_DEVICE` and
`POST_NOTIFICATIONS`. The app has no activity and asks for nothing at run
time, so the run-time ones are granted by hand:

```bash
adb shell pm grant hum.hive.wifip2p android.permission.NEARBY_WIFI_DEVICES   # Android 13 and later
adb shell pm grant hum.hive.wifip2p android.permission.ACCESS_FINE_LOCATION  # Android 10 to 12, with location on
adb shell pm grant hum.hive.wifip2p android.permission.POST_NOTIFICATIONS    # to see its notification
```

Without the first that applies, the service stops as it starts, and logs
why.

## Build

The app targets SDK 34 (from 29) with Java 17. The wrapper pins Gradle
8.10.2, and the build pins the Android plugin at 8.7.3 and Kotlin at
2.1.0. It needs a JDK 17, and the Android SDK's platform 34 and
build-tools 34.0.0, found through `local.properties` (`sdk.dir=…`) or
`ANDROID_HOME`:

```bash
./gradlew assembleDebug   # app/build/outputs/apk/debug/app-debug.apk
./gradlew test            # the JVM tests
```

## Tests

The JVM tests hold the app to hum:

- `ThrumTest` reads `hum.thrum` at the repo root: `THRUM_VERSION` and the
  range in `Tone.kt` are its own, the hello carries every key it requires,
  and nothing the bee sounds, to humd or to a peer, carries a key it does
  not declare.
- `IdentityTest` signs a hello with a fixed seed and gets the hid,
  pubkey and signature hum-identity's `BeeKey` gets for it, and loads a
  bee from where `hum bee enter` files one.
- `ThrumClientTest` attunes against a humd on a loopback port: admission,
  refusal, no verdict, humd going away and coming back.
- `ForageTest` and `StreamTest` cover a link: prompts, replies, errors, the
  sids on humd, and the framing.
- `HumdTest` runs the bee's thrum client and forage against a real humd,
  and is skipped without one:

```bash
./gradlew testDebugUnitTest --tests '*HumdTest*' \
  -Phumd=127.0.0.1:4378 -Pbee=<a directory holding bee and bees/<id>.key> \
  -Pexpect='echo: one cup of chaos agent'
```

Against a humd with the Python echo nurse (`thrum-clients/python/examples`)
nestled and `socat` carrying a port to its socket, the bee nestles, and a
peer's prompt with no `modelId` comes back as `echo: one cup of chaos
agent` and a wilt. A key entered for another role is refused, and the bee
says so.

## Start

```bash
adb install app/build/outputs/apk/debug/app-debug.apk
adb shell am start-foreground-service -n hum.hive.wifip2p/.WifiP2pBeeService
adb logcat -s HumP2P
```

The service shows an ongoing notification that says whether it is
nestled. It is exported, so any app on the phone can start it, and it
reads where humd is and which bee it is only from its own files.

## What is not yet

- **It has not run on a phone.** Discovery, the group, the links, the
  `LocalSocket` dial, starting the service, the grants and the `adb`
  steps above are compiled and untested. Android may ask the user of an
  invited phone to accept the connection.
- **The forwarder opens humd to the phone's apps**
  ([Reaching humd](#reaching-humd)).
- **Prompts come from outside the app.** Nothing in it prompts; a phone
  asks by writing tones to a link.
- **A group is a star.** It has one owner and many clients. Every phone
  in the group reaches the owner's bee, and a client's bee hears only what
  the owner sends on their link. The bee forwards only to its own humd;
  the ensemble does not travel over the group.
- **No authentication on a link.** The group's WPA2 is the only lock: a
  link carries tones as they are, any device in the group can prompt the
  humd, and a peer's lines of blooms are drawn from an address another
  device in the group could take.

## See also

- [scenarios/wifi-p2p-meetup.md](../../scenarios/wifi-p2p-meetup.md): the meetup this hive is drawn toward
- [`gsm-modem`](../gsm-modem): a forager over GSM AT-command serial
- [`bp7`](../bp7): a forager over Bundle Protocol v7 (DTN)
- [`twilio-sms`](../twilio-sms): a forager over a Twilio webhook
- [THRUM.md](../../THRUM.md): the thrum, and speaking its wire yourself
- [Android WifiP2pManager docs](https://developer.android.com/guide/topics/connectivity/wifip2p)
