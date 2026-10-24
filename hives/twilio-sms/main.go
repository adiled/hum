// twilio-sms — Twilio webhook bee. Hum-by-text-message.
//
// Twilio's "Messaging webhook URL" delivers each inbound SMS as a
// form-urlencoded POST. We:
//   1. Parse From/To/Body
//   2. Pick a stable sid per remote phone number (sha256[..16])
//      so successive texts from the same phone continue the conversation
//   3. Send chi:"prompt" to humd
//   4. Collect chi:"chunk" text into one buffer
//   5. On chi:"finish", emit TwiML <Response><Message>...</Message></Response>
//      so Twilio replies inline
//
// Inline TwiML deadline is ~15s. For longer replies, swap in the
// async REST path (Twilio Account SID + Auth Token required). Not
// wired today.

package main

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	thrum "github.com/adiled/hum/thrum-clients/go/thrum"
)

const beeVersion = "0.0.0"

type config struct {
	listen     string
	model      string
	system     string
	replyLimit int
}

func loadConfig() config {
	port := envOr("HUM_TWILIO_PORT", "14623")
	host := envOr("HUM_TWILIO_HOST", "0.0.0.0")
	limit, _ := strconv.Atoi(envOr("HUM_TWILIO_REPLY_LIMIT", "1500"))
	if limit <= 0 {
		limit = 1500
	}
	return config{
		listen:     net.JoinHostPort(host, port),
		model:      envOr("HUM_TWILIO_MODEL", "claude-haiku-4.5"),
		system:     envOr("HUM_TWILIO_SYSTEM", "You are a concise assistant. Keep replies under 1000 characters since they're sent as SMS."),
		replyLimit: limit,
	}
}

func envOr(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

func sidFor(phone string) string {
	h := sha256.Sum256([]byte("twilio-sms:" + phone))
	return hex.EncodeToString(h[:8])
}

type twimlMessage struct {
	XMLName xml.Name `xml:"Message"`
	Body    string   `xml:",chardata"`
}

type twimlResponse struct {
	XMLName xml.Name     `xml:"Response"`
	Message twimlMessage `xml:"Message"`
}

func twimlReply(body string) []byte {
	resp := twimlResponse{Message: twimlMessage{Body: body}}
	out, _ := xml.Marshal(resp)
	return append([]byte(xml.Header), out...)
}

// smsBee is the hive's one bee. Every SMS asks through it under the
// sender's session, followed by sid; humd being away turns an SMS away
// at once rather than at Twilio's deadline.
type smsBee struct {
	*thrum.Bee
	up atomic.Bool
}

func newSmsBee() *smsBee {
	b := &smsBee{Bee: thrum.NewBee(thrum.Options{
		Hive: "twilio-sms",
		As:   "main",
		Hello: map[string]any{
			"roles":   []string{"forager"},
			"version": beeVersion,
			"propensity": map[string]any{
				"statefulness": "stateful",
				"richness":     "lean",
				"wire":         "twilio/sms-webhook",
			},
			"chis":   []string{"hello", "prompt", "chunk", "finish", "error"},
			"source": "https://github.com/adiled/hum/tree/main/hives/twilio-sms",
		},
	})}
	b.OnNestled(func() { b.up.Store(true) })
	b.OnLost(func() { b.up.Store(false) })
	return b
}

func runPrompt(ctx context.Context, bee *smsBee, cfg config, from, body, messageSid string) (string, error) {
	if !bee.up.Load() {
		return "", errors.New("humd is away")
	}
	sid := sidFor(from)
	rid := "sms-" + messageSid
	if rid == "sms-" {
		rid = "sms-" + strconv.FormatInt(time.Now().UnixMilli(), 10)
	}

	var (
		buf  strings.Builder
		mu   sync.Mutex
		done = make(chan struct{}, 1)
	)
	bee.OnSid(sid, func(tone thrum.Tone) {
		chi, _ := tone["chi"].(string)
		switch chi {
		case "chunk":
			part, ok := tone["part"].(map[string]any)
			if !ok {
				return
			}
			if t, _ := part["type"].(string); t != "text" {
				return
			}
			if text, ok := part["text"].(string); ok {
				mu.Lock()
				buf.WriteString(text)
				mu.Unlock()
			}
		case "finish", "error":
			select {
			case done <- struct{}{}:
			default:
			}
		}
	})

	defer bee.OffSid(sid)
	if err := bee.Send(thrum.Tone{
		"chi":          thrum.ChiPrompt,
		"rid":          rid,
		"sid":          sid,
		"text":         body,
		"modelId":      cfg.model,
		"systemPrompt": cfg.system,
		"ext":          map[string]any{"twilio-sms": map[string]any{"from": from}},
	}); err != nil {
		return "", fmt.Errorf("thrum.prompt: %w", err)
	}

	runCtx, cancel := context.WithTimeout(ctx, 14*time.Second)
	defer cancel()
	select {
	case <-done:
	case <-runCtx.Done():
		return "", errors.New("twilio inline deadline (~15s) reached before chi:finish")
	}

	mu.Lock()
	reply := strings.TrimSpace(buf.String())
	mu.Unlock()
	if reply == "" {
		reply = "(no reply)"
	}
	if len(reply) > cfg.replyLimit {
		reply = reply[:cfg.replyLimit-3] + "..."
	}
	return reply, nil
}

func handleSms(cfg config, bee *smsBee) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		raw, err := io.ReadAll(r.Body)
		if err != nil {
			http.Error(w, "read body", http.StatusBadRequest)
			return
		}
		_ = r.Body.Close()

		if err := r.ParseForm(); err != nil {
			form, perr := parseForm(string(raw))
			if perr != nil {
				http.Error(w, "bad form", http.StatusBadRequest)
				return
			}
			r.PostForm = form
		}
		if r.PostForm == nil {
			form, perr := parseForm(string(raw))
			if perr != nil {
				http.Error(w, "bad form", http.StatusBadRequest)
				return
			}
			r.PostForm = form
		}

		from := r.PostForm.Get("From")
		body := r.PostForm.Get("Body")
		messageSid := r.PostForm.Get("MessageSid")
		if from == "" || body == "" {
			http.Error(w, "missing From/Body", http.StatusBadRequest)
			return
		}

		reply, err := runPrompt(r.Context(), bee, cfg, from, body, messageSid)
		if err != nil {
			log.Printf("twilio-sms: runPrompt error from=%s err=%v", from, err)
			reply = "(internal error)"
		}

		w.Header().Set("Content-Type", "application/xml")
		_, _ = w.Write(twimlReply(reply))
	}
}

func parseForm(raw string) (map[string][]string, error) {
	out := make(map[string][]string)
	for _, pair := range strings.Split(raw, "&") {
		if pair == "" {
			continue
		}
		eq := strings.IndexByte(pair, '=')
		if eq < 0 {
			continue
		}
		key, err := pctDecode(pair[:eq])
		if err != nil {
			return nil, err
		}
		val, err := pctDecode(pair[eq+1:])
		if err != nil {
			return nil, err
		}
		out[key] = append(out[key], val)
	}
	return out, nil
}

func pctDecode(s string) (string, error) {
	s = strings.ReplaceAll(s, "+", " ")
	out := make([]byte, 0, len(s))
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c != '%' {
			out = append(out, c)
			continue
		}
		if i+2 >= len(s) {
			return "", errors.New("bad percent escape")
		}
		v, err := strconv.ParseUint(s[i+1:i+3], 16, 8)
		if err != nil {
			return "", err
		}
		out = append(out, byte(v))
		i += 2
	}
	return string(out), nil
}

func main() {
	cfg := loadConfig()
	bee := newSmsBee()
	if err := bee.Start(context.Background()); err != nil {
		log.Fatalf("twilio-sms: %v", err)
	}
	go func() {
		if err := bee.Wait(); err != nil {
			log.Fatalf("twilio-sms: the bee ended: %v", err)
		}
	}()
	mux := http.NewServeMux()
	mux.HandleFunc("/sms", handleSms(cfg, bee))
	mux.HandleFunc("/webhook", handleSms(cfg, bee))
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/" {
			http.NotFound(w, r)
			return
		}
		w.Header().Set("Content-Type", "text/plain")
		_, _ = w.Write([]byte("twilio-sms bee — POST to /sms with Twilio webhook form data\n"))
	})
	log.Printf("twilio-sms listening on http://%s/sms", cfg.listen)
	if err := http.ListenAndServe(cfg.listen, mux); err != nil {
		log.Fatal(err)
	}
}
