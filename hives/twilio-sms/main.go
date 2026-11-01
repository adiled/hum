// twilio-sms: hum by text message, a forager behind a Twilio Messaging
// webhook.
//
// Twilio POSTs each text that reaches the number as a form. The bee
// prompts humd with it on the sender's sid, gathers the text petals the
// bloom unfurls, and answers the webhook with TwiML when the bloom wilts,
// so Twilio sends the reply as an SMS. Twilio waits about 15 s for that
// answer, so a bloom still growing at 14 s is cancelled.
//
// A bloom is keyed by its sid, so the texts of one number bloom one at a
// time, each queued behind the last. A bloom whose grower left the
// ensemble (nurse-gone) is grown again while there is time.

package main

import (
	"context"
	"crypto/hmac"
	"crypto/sha1"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/url"
	"os"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode/utf8"

	thrum "github.com/adiled/hum/thrum-clients/go/thrum"
)

const beeVersion = "0.0.0"

const (
	replyNone = "(no reply)"
	replyAway = "(internal error)"
	replyLate = "(that took too long to answer, so it was stopped; try again)"
	replyBusy = "(still answering your last text; send this one again in a moment)"
)

type config struct {
	listen     string
	model      string
	system     string
	replyLimit int
	authToken  string
	url        string
	deadline   time.Duration
	grace      time.Duration
}

func loadConfig() config {
	limit, _ := strconv.Atoi(env("HUM_TWILIO_REPLY_LIMIT", "1500"))
	if limit <= 0 {
		limit = 1500
	}
	return config{
		listen:     net.JoinHostPort(env("HUM_TWILIO_HOST", "0.0.0.0"), env("HUM_TWILIO_PORT", "14623")),
		model:      env("HUM_TWILIO_MODEL", ""),
		system:     env("HUM_TWILIO_SYSTEM", "You are a concise assistant. Keep replies under 1000 characters since they're sent as SMS."),
		replyLimit: limit,
		authToken:  env("TWILIO_AUTH_TOKEN", ""),
		url:        env("HUM_TWILIO_URL", ""),
		deadline:   14 * time.Second,
		grace:      5 * time.Second,
	}
}

// env reads a setting, an empty one as unset.
func env(key, fallback string) string {
	if v := os.Getenv(key); strings.TrimSpace(v) != "" {
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

// fit cuts a reply to at most limit bytes, on a character boundary, ending
// it in "..." when the limit leaves room for more than that.
func fit(reply string, limit int) string {
	const more = "..."
	if len(reply) <= limit {
		return reply
	}
	if limit <= len(more) {
		return cut(reply, limit)
	}
	return cut(reply, limit-len(more)) + more
}

func cut(s string, n int) string {
	for n > 0 && !utf8.RuneStart(s[n]) {
		n--
	}
	return s[:n]
}

// A bloom gathers the text petals one prompt unfurls, until it closes.
type bloom struct {
	mu     sync.Mutex
	text   strings.Builder
	away   bool
	gone   bool
	closed chan struct{}
	once   sync.Once
}

func (bl *bloom) hear(tone thrum.Tone) {
	chi, _ := tone["chi"].(string)
	switch thrum.Chi(chi) {
	case thrum.ChiUnfurl:
		if kind, _ := tone["kind"].(string); kind != string(thrum.PetalKindText) {
			return
		}
		if text, ok := tone["text"].(string); ok {
			bl.mu.Lock()
			bl.text.WriteString(text)
			bl.mu.Unlock()
		}
	case thrum.ChiError:
		if code, _ := tone["code"].(string); code == "nurse-gone" {
			bl.mu.Lock()
			bl.gone = true
			bl.mu.Unlock()
		}
		bl.end(false)
	case thrum.ChiWilt:
		bl.end(false)
	}
}

func (bl *bloom) lost() bool {
	bl.mu.Lock()
	defer bl.mu.Unlock()
	return bl.gone
}

func (bl *bloom) end(away bool) {
	bl.once.Do(func() {
		bl.mu.Lock()
		bl.away = away
		bl.mu.Unlock()
		close(bl.closed)
	})
}

func (bl *bloom) reply(limit int) string {
	bl.mu.Lock()
	defer bl.mu.Unlock()
	if bl.away {
		return replyAway
	}
	reply := strings.TrimSpace(bl.text.String())
	if reply == "" {
		return replyNone
	}
	return fit(reply, limit)
}

type smsBee struct {
	*thrum.Bee
	cfg config
	up  atomic.Bool

	mu    sync.Mutex
	lines map[string]chan struct{}
	open  map[string]*bloom
}

func newSmsBee(cfg config) *smsBee {
	b := &smsBee{
		Bee: thrum.NewBee(thrum.Options{
			Hive: "twilio-sms",
			As:   "main",
			Role: thrum.RoleForager,
			Hello: map[string]any{
				"version": beeVersion,
				"propensity": map[string]any{
					"statefulness": "stateful",
					"richness":     "lean",
					"wire":         "twilio/sms-webhook",
				},
				"chis":   []string{"hello", "prompt", "cancel", "unfurl", "wilt", "error"},
				"source": "https://github.com/adiled/hum/tree/main/hives/twilio-sms",
			},
		}),
		cfg:   cfg,
		lines: map[string]chan struct{}{},
		open:  map[string]*bloom{},
	}
	b.OnNestled(func() { b.up.Store(true) })
	b.OnAstray(func() {
		b.up.Store(false)
		b.mu.Lock()
		defer b.mu.Unlock()
		for _, bl := range b.open {
			bl.end(true)
		}
	})
	return b
}

// queue puts a bloom on its sid's line. turn closes once every bloom
// queued there before it has closed, and done closes this one; done is
// called once, after turn.
func (b *smsBee) queue(sid string) (turn <-chan struct{}, done func()) {
	mine := make(chan struct{})
	b.mu.Lock()
	prev, ok := b.lines[sid]
	b.lines[sid] = mine
	b.mu.Unlock()
	if !ok {
		prev = make(chan struct{})
		close(prev)
	}
	return prev, func() {
		b.mu.Lock()
		if b.lines[sid] == mine {
			delete(b.lines, sid)
		}
		b.mu.Unlock()
		close(mine)
	}
}

// answer grows a bloom for one text and returns its reply by the
// deadline. A bloom still growing then is cancelled, and holds its line
// until it closes or the grace runs out.
func (b *smsBee) answer(ctx context.Context, from, body, messageSid string) string {
	ctx, stop := context.WithTimeout(ctx, b.cfg.deadline)
	defer stop()
	sid := sidFor(from)
	turn, done := b.queue(sid)
	select {
	case <-turn:
	case <-ctx.Done():
		go func() { <-turn; done() }()
		return replyBusy
	}
	for {
		bl, err := b.prompt(sid, from, body, messageSid)
		if err != nil {
			log.Printf("twilio-sms: from=%s: %v", from, err)
			done()
			return replyAway
		}
		select {
		case <-bl.closed:
			b.forget(sid)
			if bl.lost() && ctx.Err() == nil {
				log.Printf("twilio-sms: from=%s: the nurse growing it is gone, so it is grown again", from)
				continue
			}
			done()
			return bl.reply(b.cfg.replyLimit)
		case <-ctx.Done():
			_ = b.Send(thrum.Tone{"chi": thrum.ChiCancel, "sid": sid})
			log.Printf("twilio-sms: from=%s: the bloom had not wilted by the deadline, so it was cancelled", from)
			go func() {
				select {
				case <-bl.closed:
				case <-time.After(b.cfg.grace):
				}
				b.forget(sid)
				done()
			}()
			return replyLate
		}
	}
}

func (b *smsBee) prompt(sid, from, body, messageSid string) (*bloom, error) {
	if !b.up.Load() {
		return nil, errors.New("humd is away")
	}
	rid := "sms-" + messageSid
	if messageSid == "" {
		rid = "sms-" + strconv.FormatInt(time.Now().UnixMilli(), 10)
	}
	tone := thrum.Tone{
		"chi":          thrum.ChiPrompt,
		"rid":          rid,
		"sid":          sid,
		"text":         body,
		"systemPrompt": b.cfg.system,
		"ext":          map[string]any{"twilio-sms": map[string]any{"from": from}},
	}
	if b.cfg.model != "" {
		tone["modelId"] = b.cfg.model
	}
	bl := &bloom{closed: make(chan struct{})}
	b.mu.Lock()
	b.open[sid] = bl
	b.mu.Unlock()
	b.OnSid(sid, bl.hear)
	if err := b.Send(tone); err != nil {
		b.forget(sid)
		return nil, fmt.Errorf("thrum.prompt: %w", err)
	}
	return bl, nil
}

func (b *smsBee) forget(sid string) {
	b.OffSid(sid)
	b.mu.Lock()
	delete(b.open, sid)
	b.mu.Unlock()
}

// signed says whether a request carries Twilio's signature: HMAC-SHA1,
// keyed by the auth token, over the URL Twilio posted to followed by each
// POST param's name and value, sorted by name, in base64. Twilio's own
// validators take the URL with its scheme's default port and without it.
func signed(token, at string, form url.Values, signature string) bool {
	if signature == "" {
		return false
	}
	for _, u := range portVariants(at) {
		if hmac.Equal([]byte(signature), []byte(sign(token, u, form))) {
			return true
		}
	}
	return false
}

func sign(token, at string, form url.Values) string {
	names := make([]string, 0, len(form))
	for name := range form {
		names = append(names, name)
	}
	sort.Strings(names)
	mac := hmac.New(sha1.New, []byte(token))
	_, _ = io.WriteString(mac, at)
	for _, name := range names {
		values := append([]string(nil), form[name]...)
		sort.Strings(values)
		for _, v := range values {
			_, _ = io.WriteString(mac, name+v)
		}
	}
	return base64.StdEncoding.EncodeToString(mac.Sum(nil))
}

func portVariants(at string) []string {
	i := strings.Index(at, "://")
	if i < 0 {
		return []string{at}
	}
	port := map[string]string{"http": ":80", "https": ":443"}[strings.ToLower(at[:i])]
	rest := at[i+3:]
	end := strings.IndexAny(rest, "/?#")
	if end < 0 {
		end = len(rest)
	}
	host := rest[:end]
	switch {
	case port == "":
		return []string{at}
	case strings.HasSuffix(host, port):
		host = strings.TrimSuffix(host, port)
	case strings.LastIndex(host, ":") > strings.LastIndex(host, "]"):
		return []string{at}
	default:
		host += port
	}
	return []string{at, at[:i+3] + host + rest[end:]}
}

// webhookURL is the URL Twilio posted to: HUM_TWILIO_URL when it is set,
// else the request's own, with the scheme a proxy in front reports.
func webhookURL(cfg config, r *http.Request) string {
	if cfg.url != "" {
		return cfg.url
	}
	scheme := "http"
	if p := r.Header.Get("X-Forwarded-Proto"); p != "" {
		scheme = strings.TrimSpace(strings.Split(p, ",")[0])
	}
	return scheme + "://" + r.Host + r.RequestURI
}

func handleSms(b *smsBee) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if err := r.ParseForm(); err != nil {
			http.Error(w, "bad form", http.StatusBadRequest)
			return
		}
		if b.cfg.authToken != "" {
			at := webhookURL(b.cfg, r)
			if !signed(b.cfg.authToken, at, r.PostForm, r.Header.Get("X-Twilio-Signature")) {
				log.Printf("twilio-sms: refused a request to %s that Twilio did not sign", at)
				http.Error(w, "not signed by Twilio", http.StatusForbidden)
				return
			}
		}
		from := r.PostForm.Get("From")
		body := r.PostForm.Get("Body")
		if from == "" || body == "" {
			http.Error(w, "missing From/Body", http.StatusBadRequest)
			return
		}
		reply := b.answer(r.Context(), from, body, r.PostForm.Get("MessageSid"))
		w.Header().Set("Content-Type", "application/xml")
		_, _ = w.Write(twimlReply(reply))
	}
}

func main() {
	cfg := loadConfig()
	bee := newSmsBee(cfg)
	if err := bee.Start(context.Background()); err != nil {
		log.Fatalf("twilio-sms: %v", err)
	}
	go func() {
		if err := bee.Wait(); err != nil {
			log.Fatalf("twilio-sms: the bee ended: %v", err)
		}
	}()
	if cfg.authToken == "" {
		log.Printf("twilio-sms: TWILIO_AUTH_TOKEN is unset, so a request is answered without checking that Twilio sent it")
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/sms", handleSms(bee))
	mux.HandleFunc("/webhook", handleSms(bee))
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/" {
			http.NotFound(w, r)
			return
		}
		w.Header().Set("Content-Type", "text/plain")
		_, _ = w.Write([]byte("twilio-sms bee: POST to /sms with Twilio webhook form data\n"))
	})
	log.Printf("twilio-sms listening on http://%s/sms", cfg.listen)
	if err := http.ListenAndServe(cfg.listen, mux); err != nil {
		log.Fatal(err)
	}
}
