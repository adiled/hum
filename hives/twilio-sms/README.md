---
title: "twilio-sms (Go)"
description: "hum by text message: a forager behind a Twilio Messaging webhook"
---

# twilio-sms (Go)

> _hum by text message: a forager behind a Twilio Messaging webhook_

Built in Go with the stdlib `net/http`, on the Go
[thrum client](../../thrum-clients/go). It is the repo's Go hive.

Its one bee is entered as a forager. Twilio POSTs each text that reaches
your number to the bee's webhook; the bee prompts humd with it, gathers
the text petals the bloom unfurls, and answers the webhook with TwiML
when the bloom wilts, so Twilio sends the reply back as an SMS. The bloom
grows on whichever nurse humd hands it to, on this humd or on a trusted
peer.

Every text from one number shares a sid, the first 8 bytes of
sha256(`twilio-sms:<From>`) in hex, so a nurse that keeps a cell per
sigil grows each text on from the last. The bee sends no graft, so a
nurse that keeps no cell sees each text alone. A bloom is keyed by its
sid, so the texts of one number bloom one at a time: a text that arrives
while the last is still growing waits its turn, and each gets its own
reply. Texts from different numbers bloom at once.

## Propensity

| statefulness | richness | wire shape | hides |
|---|---|---|---|
| stateful (a sid per phone number) | lean | `twilio/sms-webhook` | tools, system prompts, perf, drone, breath |

It speaks `hello`, `prompt`, `cancel`, `unfurl`, `wilt` and `error`.

## Wire

```
phone user ──SMS──► Twilio ──POST /sms──► twilio-sms ──chi:prompt──► humd
                                                                         │
phone user ◄──SMS── Twilio ◄──TwiML <Response>── twilio-sms ◄──chi:wilt───┘
```

## Configure Twilio

In your Twilio console:

1. Buy or select a phone number with SMS capability.
2. Under **Messaging → Configuration**, set:
   - **A MESSAGE COMES IN**: `Webhook`
   - **URL**: `https://<your-public-host>/sms` (`/webhook` is the same
     handler). It must be reachable from Twilio; for dev, point a tunnel
     like `cloudflared` or `ngrok` at the bee's port.
   - **HTTP**: `POST`
3. Give the bee your account's **Auth Token** as `TWILIO_AUTH_TOKEN`,
   and the URL from step 2 as `HUM_TWILIO_URL`.

The bee runs beside humd and reaches it through `hum bee link`, on
humd's Unix socket; only the webhook faces Twilio.

## Signatures

With `TWILIO_AUTH_TOKEN` set, the bee answers only a request Twilio
signed, and every other one `403`. Twilio signs each webhook in its
`X-Twilio-Signature` header
([Twilio docs](https://www.twilio.com/docs/usage/webhooks/webhooks-security)):
HMAC-SHA1, keyed by the auth token, over the URL it posted to followed
by each POST param's name and value, sorted by name, in base64. The bee
computes the same and compares, taking the URL with its scheme's default
port and without it, as Twilio's own validators do.

The URL is `HUM_TWILIO_URL` when it is set, exactly as Twilio's console
has it. Otherwise it is the request's own, `<scheme>://<Host><path>`,
whose scheme is the one `X-Forwarded-Proto` names when a proxy in front
sets it, and `http` when none does. A proxy or tunnel that keeps the
host and path but ends TLS needs no setting if it sets
`X-Forwarded-Proto`; one that changes either needs `HUM_TWILIO_URL`.

Without `TWILIO_AUTH_TOKEN` the bee checks nothing, says so when it
starts, and listens on every interface by default, so anyone who can
reach the port can prompt humd.

## Configure (this bee)

| env | default | what |
|---|---|---|
| `HUM_TWILIO_PORT` | `14623` | HTTP listen port |
| `HUM_TWILIO_HOST` | `0.0.0.0` | HTTP listen host |
| `HUM_TWILIO_MODEL` | none | the `modelId` on every prompt; with none, a prompt names no model |
| `HUM_TWILIO_SYSTEM` | `You are a concise assistant. Keep replies under 1000 characters since they're sent as SMS.` | the `systemPrompt` on every prompt |
| `HUM_TWILIO_REPLY_LIMIT` | `1500` | the most bytes a reply keeps; a longer one is cut on a character boundary and ends in `...`, and a limit of 3 or less only cuts (Twilio takes a body of up to 1600 characters) |
| `TWILIO_AUTH_TOKEN` | none | your Twilio auth token; with it, a request Twilio did not sign is refused |
| `HUM_TWILIO_URL` | the request's own URL | the URL Twilio posts to, which its signature covers |
| `HUM_CLI` | `hum` on `PATH`, else `~/.local/bin/hum` | the hum CLI the bee links through |
| `HUM_THRUM_SOCK` | `$XDG_STATE_HOME/hum/thrum.sock` | humd's socket, for `hum bee link` |

A prompt with no `modelId` goes to any nurse with room, which grows it
with a model of its own. One that names a model goes only to a nurse
that claims that id exactly: the bundled Claude nurses claim
`claude-haiku-4-5`, and a model no nurse claims is refused with
`no-nurse`, so the text is answered `(no reply)`.

Every setting but `HUM_THRUM_SOCK` is an `ARG` of the hive's Orchfile:
set it in the environment of `hum hive twilio-sms install`, which hands
it to the bee's service, and an empty one keeps its default.
`HUM_THRUM_SOCK` is not among them, since hum's client reads an empty one
as a path; an installed bee finds humd's socket the way every bee does.

## Run

```bash
hum hive twilio-sms install    # build it (go build), and keep it running under orchd
TWILIO_AUTH_TOKEN=... HUM_TWILIO_URL=https://hum.example.com/sms hum hive twilio-sms install
```

By hand, on a humd with the hive installed:

```bash
cd hives/twilio-sms
HUM_TWILIO_PORT=8080 go run .
```

The bee enters itself (`--as main`) when it starts, and the webhook
listens once humd has admitted it.

Smoke test without Twilio, on a bee with no `TWILIO_AUTH_TOKEN`:

```bash
curl -X POST http://localhost:14623/sms \
  -d "From=%2B14155551234" \
  -d "To=%2B18005550000" \
  -d "Body=ping"
```

The answer is TwiML, the same Twilio gets. With a token set, the same
request is answered `403`.

## What flows where

| Twilio webhook field | thrum |
|---|---|
| `X-Twilio-Signature` (header) | checked against `TWILIO_AUTH_TOKEN`, when it is set |
| `From` (phone number) | the prompt's sid, and `ext["twilio-sms"].from` |
| `Body` (the text) | the prompt's `text` |
| `MessageSid` | the prompt's `rid`, `sms-<MessageSid>` |
| the bloom's `chi:"unfurl"` text petals | gathered, trimmed, into the TwiML `<Message>` |
| the bloom's `chi:"wilt"` or `chi:"error"` | the webhook's answer |

A webhook without `From` or `Body` is answered `400`. Every other signed
one, or every one when no token is set, is answered with TwiML within 14
seconds, whatever happened:

| the texter gets | when |
|---|---|
| the bloom's text | it wilted |
| `(no reply)` | it grew no text, as when no nurse grows the model |
| `(internal error)` | humd is away, or goes away while the bloom grows |
| `(that took too long to answer, so it was stopped; try again)` | it has not wilted at 14 seconds, and the bee cancels it |
| `(still answering your last text; send this one again in a moment)` | the number's last text still holds the line at 14 seconds, so this one never prompted |

A cancelled bloom holds its number's line until it closes, or for 5
seconds at most. A bloom that breaks off with `nurse-gone`, because the
peer growing it left the ensemble, is prompted again while there is time.

## What it doesn't do

- **No async / long-reply path.** Twilio waits about 15 seconds for a
  webhook, so a bloom that needs longer is cancelled at 14. Replying
  later would take Twilio's REST API.
- **No MMS / images.** Inbound media URLs are ignored.
- **No outbound-initiated chat.** The bee only answers; nothing sends a
  text to a number that has not written.

## See also

- [`openai-server`](../openai-server), [`anthropic-server`](../anthropic-server): sibling HTTP surfaces.
- [`gsm-modem`](../gsm-modem): the same idea through a USB GSM modem instead of Twilio.
- [THRUM.md](../../THRUM.md): the thrum.
- [adiled.github.io/hum](https://adiled.github.io/hum/): the docs site.
