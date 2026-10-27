---
title: "gsm-modem"
description: "hum on a burner phone: a forager that answers SMS through a USB GSM modem, driven by AT commands"
---

# gsm-modem

> _hum on a burner phone: a forager that answers SMS through a USB GSM modem, driven by AT commands_

A Rust forager that reads texts from a cheap USB GSM dongle over its
serial port and answers them through it. No carrier API, no webhook, no
public IP: the modem talks to the cellular network itself.

Its one bee is entered as a forager, and links to humd once, when it
starts. For each text that arrives it prompts humd with the whole text,
gathers the text petals the bloom unfurls, and sends them back with
`AT+CMGS` when the bloom wilts. A text that arrives while humd is away
waits for it.

Every text from one number shares a sid, the first 8 bytes of
sha256(`gsm-modem:<number>`) in hex, so a nurse that keeps a cell per
sigil grows each text on from the last. The bee sends no graft. A bloom
is keyed by its sid, so the texts of one number bloom one at a time, in
the order they came, and each gets its own reply; texts from different
numbers bloom at once. A bloom lost before it closes, to humd going away
or to the peer growing it leaving the ensemble (`nurse-gone`), is grown
again, up to three tries in all; past that, the text gets what the last
try grew.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| stateful (a sid per phone number) | lean | `gsm/at-cmd-sms`, AT commands over serial | tools, system prompts, perf, drone, breath |

It speaks `hello`, `prompt`, `unfurl`, `wilt` and `error`.

## Wire

```
SIM card phone ──SMS──► cellular tower ──► your USB modem
                                                │
                                                ▼ +CMT over serial
                                          gsm-modem bee
                                                │
                                                │ chi:"prompt"
                                                ▼
                                                humd
                                                │
                                                │ chi:"unfurl" → "wilt"
                                                ▼
                                          gsm-modem bee
                                                │
                                                ▼ AT+CMGS over serial
                                            USB modem ──► tower ──► phone
```

The bee opens the serial device once, for exclusive use, and splits it:
one task reads what the modem pushes, and each reply writes through the
other half. At start it sends `AT`, `AT+CMGF=1` (text mode),
`AT+CSCS="GSM"` (the GSM 7-bit charset) and `AT+CNMI=2,2,0,0,0` (push
each new SMS as a `+CMT` line), 150 ms apart, and reads nothing the
modem answers. A text arrives as `+CMT: "<number>",...` and its body,
which runs to the CR LF the modem ends the line with, so a line break the
sender typed (LF) stays in the text. The reply goes out as
`AT+CMGS="<number>"`, a 200 ms pause, the body, and Ctrl-Z, and the bee
holds the port from the first byte to the last, so two replies never
land inside each other.

## The GSM charset

The modem's charset is GSM 03.38's 7-bit default alphabet, one byte a
character, and the bee reads and writes in it: a body is decoded from it,
its extension table (`€`, `[`, `{` and the rest) included, and a reply is
encoded into it. A character the alphabet lacks goes as its nearest: `…`
as `...`, curly quotes as straight ones, dashes as `-`, `€` as `EUR`,
brackets and braces as parentheses, a few accented letters without
their accent, and anything else as `?`. The extension table's characters
go that way in a reply too, since reaching them takes ESC, which in text
mode abandons the text, and so does `Ξ`, whose code is Ctrl-Z.

## Modems

The commands are the GSM 07.05 SMS text-mode subset, which these modems
speak:

| modem | AT port |
|---|---|
| Huawei E303 / E353 / E3531 | `/dev/ttyUSB0` (the others are NMEA or diagnostics) |
| SIMCom SIM800 / SIM900 / SIM7600 | `/dev/ttyUSB0` (HAT-style boards via USB-UART) |
| Quectel EC25 / EG91 | `/dev/ttyUSB2` (check the vendor docs) |

A SIM card with an active SMS plan is required, and one without a PIN
(the bee sends no `AT+CPIN`). Prepaid burner SIMs work.

## Configure

| env | default | what |
|---|---|---|
| `HUM_GSM_DEVICE` | `/dev/ttyUSB0` | the modem's AT port |
| `HUM_GSM_BAUD` | `115200` | baud rate |
| `HUM_GSM_MODEL` | none | the `modelId` on every prompt; with none, a prompt names no model |
| `HUM_GSM_SYSTEM` | `You are a concise assistant. Keep replies under 1000 characters since they're sent as SMS.` | the `systemPrompt` on every prompt |
| `HUM_GSM_REPLY_LIMIT` | `1500` | the most characters a reply keeps, in the GSM charset; a longer one is cut and ends in `...`, and a limit of 3 or less only cuts |
| `HUM_THRUM_SOCK` | `$XDG_STATE_HOME/hum/thrum.sock` | humd's socket |

A prompt with no `modelId` goes to any nurse with room, which grows it
with a model of its own. One that names a model goes only to a nurse
that claims that id exactly: the bundled Claude nurses claim
`claude-haiku-4-5`, and a model no nurse claims is refused with
`no-nurse`, so the text is answered `(no reply)`.

Every setting but `HUM_THRUM_SOCK` is an `ARG` of the hive's Orchfile:
set it in the environment of `hum hive gsm-modem install`, which hands
it to the bee's service, and an empty one keeps its default.
`HUM_THRUM_SOCK` is not among them, since hum's client reads an empty one
as a path; an installed bee finds humd's socket the way every bee does.

The process needs read and write access to the serial device. On Debian
or Ubuntu, add yourself to `dialout`:

```bash
sudo usermod -aG dialout $USER
# log out and back in
```

## Run

```bash
hum hive gsm-modem install                              # build it (cargo build --release), and keep it running under orchd
HUM_GSM_DEVICE=/dev/ttyUSB2 hum hive gsm-modem install  # a Quectel modem
```

By hand, on a humd with the hive installed:

```bash
cd hives/gsm-modem
HUM_GSM_DEVICE=/dev/ttyUSB2 cargo run --release
```

The hive is its own cargo workspace, not a member of the repo's, so build
it from its own directory.

## Test without a SIM

On Linux, a pair of linked ptys stands in for the modem. macOS refuses
to set a pty's baud rate, so the bee cannot open one there.

```bash
socat -d -d pty,raw,echo=0 pty,raw,echo=0
# socat prints two /dev/pts/N paths. Point HUM_GSM_DEVICE at one; on the
# other, write a +CMT line and a body, each ending in \r\n, such as
#   +CMT: "+14155551234","","25/05/17,12:00:00+00"
#   ping
# and read the AT+CMGS="+14155551234" and reply the bee writes back.
```

`cargo test` needs no modem: its tests run the bee against an in-memory
port or a pty pair, and a humd of their own.

## What flows where

| modem / AT | thrum |
|---|---|
| `+CMT: "<number>",...` | the prompt's sid, and `ext["gsm-modem"].from` |
| the body after it, up to its CR LF | the prompt's `text`, decoded from the GSM charset |
| the bloom's `chi:"unfurl"` text petals | gathered, trimmed, into one SMS body in the GSM charset |
| the bloom's `chi:"wilt"` or `chi:"error"` | `AT+CMGS="<number>"`, the body, Ctrl-Z; `(no reply)` when the bloom grew no text |

## What it doesn't do

- **No PDU mode.** Text mode (`AT+CMGF=1`) in the GSM charset only, so a
  text sent in UCS2 (emoji, most non-Latin scripts) arrives as the hex
  the modem shows it in, and a reply carries only what the GSM charset
  holds. A text whose own line breaks are CR LF is cut at the first.
- **No long-SMS concatenation.** The reply goes out as one `AT+CMGS`,
  never split. One text-mode SMS holds 160 GSM 7-bit characters and the
  default limit is 1500; what the modem does with a longer body is the
  modem's own, and the bee reads no answer, so a reply the modem refuses
  is lost without a word.
- **No call handling.** A `RING` is ignored.
- **No SIM PIN unlock.** Send `AT+CPIN=<pin>` yourself before the bee
  starts, or use a SIM without a PIN.

## See also

- [`twilio-sms`](../twilio-sms): the same idea through a Twilio webhook instead of a physical modem.
- [`openai-server`](../openai-server), [`anthropic-server`](../anthropic-server): text-based HTTP surfaces.
- [THRUM.md](../../THRUM.md): the thrum.
- [adiled.github.io/hum](https://adiled.github.io/hum/): the docs site.
