---
title: "twilio-sms (Go)"
description: "hum by text message — Twilio SMS webhook bee"
---

# twilio-sms (Go)

> _hum by text message — Twilio SMS webhook bee_

Built in Go with the stdlib `net/http`. Imports the
[`clients/go/thrum`](../../clients/go) reference client. The
canonical Go bee in this repo.

A bee that turns a Twilio phone number into a hum agent. Send an
SMS to your Twilio number → Twilio POSTs to this bee → the
message becomes a `chi:"prompt"` to humd → humd's reply comes back as
TwiML so Twilio answers inline.

Same conversation continues across messages from the same phone
number (sid = sha256("twilio-sms:From")[..16] — stable per remote).

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| stateful (per-phone sid) | lean | Twilio Messaging webhook | tools, system prompts, perf, drone, breath |

## Wire

```
phone user ──SMS──► Twilio ──POST /sms──► twilio-sms ──chi:prompt──► humd
                                                                         │
phone user ◄──SMS── Twilio ◄──TwiML <Response>── twilio-sms ◄──chi:finish─┘
```

## Configure Twilio

In your Twilio console:

1. Buy / select a phone number with SMS capability.
2. Under **Messaging → Configuration**, set:
   - **A MESSAGE COMES IN**: `Webhook`
   - **URL**: `https://<your-public-host>/sms` (must be reachable; for
     dev, point a tunnel like `cloudflared` or `ngrok` at your laptop)
   - **HTTP**: `POST`

You also need humd reachable from the same machine that runs this
bee (its thrum socket is local-only by default — that's fine,
the bee itself terminates the SMS, hum stays private).

## Configure (this bee)

| env | default | what |
|---|---|---|
| `HUM_TWILIO_PORT` | `14623` | HTTP listen port |
| `HUM_TWILIO_HOST` | `0.0.0.0` | HTTP listen host |
| `HUM_TWILIO_MODEL` | `claude-haiku-4.5` | model the daemon spawns |
| `HUM_TWILIO_SYSTEM` | terse SMS-friendly system prompt | system instruction prepended to every turn |
| `HUM_TWILIO_REPLY_LIMIT` | `1500` | hard cap on reply length (Twilio caps long-form SMS) |
| `TWILIO_AUTH_TOKEN` | _(unset)_ | reserved — signature verification not yet implemented |
| `HUM_THRUM_SOCK` | `$XDG_RUNTIME_DIR/hum/thrum.sock` | humd's NDJSON socket |

## Run

```bash
cd hives/twilio-sms
go run .

# Custom port:
HUM_TWILIO_PORT=8080 go run .
```

Smoke test without Twilio:

```bash
curl -X POST http://localhost:14623/sms \
  -d "From=%2B14155551234" \
  -d "To=%2B18005550000" \
  -d "Body=ping"
```

You'll get a TwiML response back — same shape Twilio gets.

## What flows where

| Twilio webhook field | hum chi field |
|---|---|
| `From` (phone number) | `prompt.ext["twilio-sms"].from` + sid seed |
| `To` (your number) | `prompt.ext["twilio-sms"].to` |
| `Body` (the text) | `prompt.text` |
| `MessageSid` | `rid` |
| daemon's `chi:"chunk"` text parts (collected) | TwiML `<Message>` body |
| daemon's `chi:"finish"` | response sent |

## What it doesn't do

- **No signature verification yet.** Reserved env (`TWILIO_AUTH_TOKEN`)
  will be wired to HMAC-SHA1 verification per
  [Twilio docs](https://www.twilio.com/docs/usage/webhooks/webhooks-security).
  Today: trust the network path. Don't expose to the open internet
  without verification.
- **No async / long-reply path.** Twilio's inline webhook deadline is
  ~15s; replies that take longer than that timeout will still complete
  on humd's side but won't reach the user. A future revision can send
  via Twilio REST API (Account SID + Auth Token required) for async.
- **No MMS / images.** Inbound media URLs are ignored. Future revision
  could pass them through as `petal-cell` attachments.
- **No outbound-initiated chat.** This bee only answers; humd
  doesn't push unsolicited messages to phone numbers.

## See also

- [`openai-server`](../openai-server), [`anthropic-server`](../anthropic-server) — sibling HTTP surfaces.
- [`gsm-modem`](../gsm-modem) — same idea but talks directly to a USB
  GSM modem instead of going through Twilio.
- [WIRE.md](../../WIRE.md) — the language-neutral protocol spec.
- [adiled.github.io/hum](https://adiled.github.io/hum/) — docs site.
