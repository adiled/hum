# gsm-modem

> _hum-on-a-burner-phone — USB GSM modem nestling driven by AT commands_

A nestling that turns a cheap USB GSM dongle into a hum agent. SMS in
→ `chi:"prompt"` to humd → daemon's reply sent back as SMS via
`AT+CMGS`. No carrier API, no webhook, no public IP — the modem talks
to the cellular network directly.

Same conversation continues across messages from the same number
(sid = sha256("gsm-modem:From")[..16]).

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| stateful (per-phone sid) | lean | GSM AT-command serial (`/dev/ttyUSB*`) | tools, system prompts, perf, drone, breath |

## Wire

```
SIM card phone ──SMS──► cellular tower ──► your USB modem
                                                │
                                                ▼ +CMT URC over serial
                                          gsm-modem nestling
                                                │
                                                │ chi:"prompt"
                                                ▼
                                                humd
                                                │
                                                │ chi:"chunk" → "finish"
                                                ▼
                                          gsm-modem nestling
                                                │
                                                ▼ AT+CMGS over serial
                                            USB modem ──► tower ──► phone
```

## Tested modems

The AT command set used here is the GSM 07.05 SMS subset. Anything
that speaks SMS text mode works:

| modem | port style |
|---|---|
| Huawei E303 / E353 / E3531 | `/dev/ttyUSB0` (the AT port — others are NMEA/diag) |
| SIMCom SIM800 / SIM900 / SIM7600 | `/dev/ttyUSB0` (HAT-style boards via USB-UART) |
| Quectel EC25 / EG91 | `/dev/ttyUSB2` (AT port; check vendor docs) |

A SIM card with an active SMS plan is required. Prepaid burner SIMs
work fine.

## Configure

| env | default | what |
|---|---|---|
| `HUM_GSM_DEVICE` | `/dev/ttyUSB0` | serial path the modem appears on |
| `HUM_GSM_BAUD` | `115200` | baud rate |
| `HUM_GSM_MODEL` | `claude-haiku-4.5` | model humd spawns |
| `HUM_GSM_SYSTEM` | terse SMS-friendly system prompt | system instruction |
| `HUM_GSM_REPLY_LIMIT` | `1500` | hard cap on reply length |
| `HUM_THRUM_SOCK` | `$XDG_RUNTIME_DIR/hum/thrum.sock` | humd's NDJSON socket |

The process needs read+write access to the serial device. On Debian /
Ubuntu add yourself to `dialout`:

```bash
sudo usermod -aG dialout $USER
# log out + back in
```

## Install

```bash
npm install
npm run build
npm start
```

`serialport` is a native module — needs build tools on systems
without prebuilts. On Debian: `sudo apt-get install build-essential
python3`.

## Test without a SIM

You can pipe AT commands at the nestling via a fake serial pair using
`socat`:

```bash
sudo apt-get install -y socat
socat -d -d pty,raw,echo=0 pty,raw,echo=0
# socat prints two /dev/pts/N paths — point HUM_GSM_DEVICE at one,
# then write +CMT URCs to the other to simulate inbound SMS.
```

## What flows where

| modem URC / AT | hum chi |
|---|---|
| `+CMT: "<from>",...\n<body>` | `chi:"prompt"` (sid keyed off `<from>`) |
| daemon's `chi:"chunk"` text parts | collected into one SMS body |
| daemon's `chi:"finish"` | `AT+CMGS="<from>"` + body + Ctrl-Z |

## What it doesn't do

- **No PDU mode.** Text-mode (`AT+CMGF=1`) only — fine for plain ASCII
  / GSM-7. Emoji and non-Latin characters need `AT+CSCS="UCS2"` +
  PDU-encoded payload, which this nestling does not implement.
- **No long-SMS concatenation.** Replies > 160 chars are clipped at
  `HUM_GSM_REPLY_LIMIT`. A future revision can split into multi-part
  SMS via UDH headers.
- **No call handling.** Voice (`RING` URC, `ATA` / `ATH`) is not wired.
  Easy to add — answer with `ATA`, hang up with `ATH`, but the audio
  path requires a separate codec stack.
- **No SIM PIN unlock.** If your SIM requires a PIN, send `AT+CPIN=<pin>`
  manually before starting the nestling (or extend `init()`).

## Sibling: DECT

DECT (cordless home phones) is a related radio standard but
practical programmatic access is poor — most DECT base stations
expose proprietary HTTP/RPC and a custom firmware is needed to drive
the radio directly. If you have a Gigaset SL series base station or
an open-DECT hardware project (rare), the wire mapping would be
similar to this nestling but the transport is HTTP, not serial.

## See also

- [`twilio-sms`](../twilio-sms) — same idea but via Twilio webhook
  instead of a physical modem.
- [`openai-server`](../openai-server), [`anthropic-server`](../anthropic-server) — text-based HTTP surfaces.
- [WIRE.md](../../thrum/WIRE.md) — the language-neutral protocol spec.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.
