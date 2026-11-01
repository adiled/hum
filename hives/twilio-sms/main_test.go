package main

import (
	"bufio"
	"context"
	"encoding/json"
	"encoding/xml"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

const fakeLinkEnv = "TWILIO_SMS_FAKE_LINK"

func TestMain(m *testing.M) {
	if log := os.Getenv(fakeLinkEnv); log != "" {
		fakeLink(log)
		return
	}
	os.Exit(m.Run())
}

// fakeLink is `hum bee link` with humd and a nurse behind it. It admits
// the bee, writes every tone it hears to the file at log, and answers a
// prompt "echo: <text>", or by the text's first word: `slow <ms> <text>`
// echoes after ms, `hang` wilts only when cancelled, `deaf` never wilts,
// `away` has humd go away and come back, and `gone` breaks off as
// nurse-gone, after a petal, the first time its sid prompts it.
func fakeLink(log string) {
	var mu sync.Mutex
	out := json.NewEncoder(os.Stdout)
	say := func(v any) {
		mu.Lock()
		defer mu.Unlock()
		_ = out.Encode(v)
	}
	logf, err := os.OpenFile(log, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		os.Exit(1)
	}
	say(map[string]any{"link": "ready", "linkVersion": 2, "bee": "bee_fake"})
	cancels := map[string]chan struct{}{}
	prompted := map[string]int{}
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 1<<20)
	for in.Scan() {
		var msg map[string]any
		if json.Unmarshal(in.Bytes(), &msg) != nil {
			continue
		}
		if _, ok := msg["hello"]; ok {
			say(map[string]any{"link": "nestled"})
			continue
		}
		mu.Lock()
		_, _ = logf.Write(append(append([]byte{}, in.Bytes()...), '\n'))
		mu.Unlock()
		sid, _ := msg["sid"].(string)
		switch msg["chi"] {
		case "prompt":
			stop := make(chan struct{})
			cancels[sid] = stop
			text, _ := msg["text"].(string)
			prompted[sid]++
			go grow(say, sid, text, stop, prompted[sid])
		case "cancel":
			if stop, ok := cancels[sid]; ok {
				close(stop)
				delete(cancels, sid)
			}
		}
	}
}

func grow(say func(any), sid, text string, stop chan struct{}, nth int) {
	wilt := func(reason string) { say(map[string]any{"chi": "wilt", "sid": sid, "reason": reason}) }
	words := strings.Fields(text)
	first := ""
	if len(words) > 0 {
		first = words[0]
	}
	switch {
	case first == "deaf":
		return
	case first == "gone" && nth == 1:
		say(map[string]any{"chi": "unfurl", "sid": sid, "petal": 0, "kind": "text", "text": "half"})
		say(map[string]any{"chi": "error", "sid": sid, "code": "nurse-gone", "message": "the peer growing it left the ensemble"})
		return
	case first == "away":
		say(map[string]any{"link": "astray"})
		time.Sleep(100 * time.Millisecond)
		say(map[string]any{"link": "nestled"})
		return
	case first == "hang":
		<-stop
		wilt("cancel")
		return
	case first == "slow" && len(words) > 2:
		ms, _ := strconv.Atoi(words[1])
		select {
		case <-time.After(time.Duration(ms) * time.Millisecond):
		case <-stop:
			wilt("cancel")
			return
		}
		text = strings.Join(words[2:], " ")
	}
	say(map[string]any{"chi": "unfurl", "sid": sid, "petal": 0, "kind": "text", "text": "echo: " + text})
	wilt("stop")
}

type world struct {
	t   *testing.T
	srv *httptest.Server
	log string
}

func start(t *testing.T, tune func(*config)) *world {
	t.Helper()
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	log := filepath.Join(t.TempDir(), "heard.ndjson")
	t.Setenv("HUM_CLI", exe)
	t.Setenv(fakeLinkEnv, log)
	cfg := loadConfig()
	if tune != nil {
		tune(&cfg)
	}
	bee := newSmsBee(cfg)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := bee.Start(ctx); err != nil {
		t.Fatalf("the bee did not start: %v", err)
	}
	t.Cleanup(func() { _ = bee.Close() })
	srv := httptest.NewServer(handleSms(bee))
	t.Cleanup(srv.Close)
	return &world{t: t, srv: srv, log: log}
}

func (w *world) post(path string, form url.Values, header http.Header) (int, string) {
	w.t.Helper()
	req, _ := http.NewRequest(http.MethodPost, w.srv.URL+path, strings.NewReader(form.Encode()))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	for k, vs := range header {
		for _, v := range vs {
			req.Header.Add(k, v)
		}
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		w.t.Errorf("POST %s: %v", path, err)
		return 0, ""
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusOK {
		return resp.StatusCode, string(raw)
	}
	var tw twimlResponse
	if err := xml.Unmarshal(raw, &tw); err != nil {
		w.t.Errorf("not TwiML: %q", raw)
	}
	return resp.StatusCode, tw.Message.Body
}

func (w *world) text(from, body string) string {
	w.t.Helper()
	code, reply := w.post("/sms", url.Values{"From": {from}, "To": {"+18005550000"}, "Body": {body}}, nil)
	if code != http.StatusOK {
		w.t.Errorf("answered %d: %s", code, reply)
	}
	return reply
}

func (w *world) heard(chi string) []map[string]any {
	w.t.Helper()
	raw, _ := os.ReadFile(w.log)
	var tones []map[string]any
	for _, line := range strings.Split(strings.TrimSpace(string(raw)), "\n") {
		var tone map[string]any
		if json.Unmarshal([]byte(line), &tone) == nil && tone["chi"] == chi {
			tones = append(tones, tone)
		}
	}
	return tones
}

const number = "+14155551234"

func TestAPromptNamesNoModelUnlessOneIsSet(t *testing.T) {
	t.Setenv("HUM_TWILIO_MODEL", "")
	w := start(t, nil)
	if got := w.text(number, "ping"); got != "echo: ping" {
		t.Fatalf("reply %q", got)
	}
	prompts := w.heard("prompt")
	if len(prompts) != 1 {
		t.Fatalf("%d prompts", len(prompts))
	}
	if m, ok := prompts[0]["modelId"]; ok {
		t.Fatalf("the prompt names %v, which a nurse may not claim", m)
	}

	t.Setenv("HUM_TWILIO_MODEL", "claude-haiku-4-5")
	w = start(t, nil)
	w.text(number, "ping")
	if m := w.heard("prompt")[0]["modelId"]; m != "claude-haiku-4-5" {
		t.Fatalf("modelId %v", m)
	}
}

func TestAnEmptySettingKeepsItsDefault(t *testing.T) {
	for _, name := range settings(t) {
		t.Setenv(name, "")
	}
	want := config{
		listen:     "0.0.0.0:14623",
		system:     "You are a concise assistant. Keep replies under 1000 characters since they're sent as SMS.",
		replyLimit: 1500,
		deadline:   14 * time.Second,
		grace:      5 * time.Second,
	}
	if got := loadConfig(); got != want {
		t.Fatalf("%+v, want %+v", got, want)
	}
}

// settings are the names the bee reads with env.
func settings(t *testing.T) []string {
	src, err := os.ReadFile("main.go")
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, m := range regexp.MustCompile(`env\("([A-Z_]+)"`).FindAllStringSubmatch(string(src), -1) {
		names = append(names, m[1])
	}
	if len(names) < 7 {
		t.Fatalf("found only %v", names)
	}
	return names
}

func TestTheOrchfileHandsEverySettingToTheBee(t *testing.T) {
	orch, err := os.ReadFile("Orchfile")
	if err != nil {
		t.Fatal(err)
	}
	lines := map[string]bool{}
	for _, line := range strings.Split(string(orch), "\n") {
		lines[strings.TrimSpace(line)] = true
	}
	for _, name := range append(settings(t), "HUM_CLI") {
		for _, want := range []string{"ARG " + name + "=", "ENV " + name + "=${" + name + "}"} {
			if !lines[want] {
				t.Errorf("the Orchfile has no %q", want)
			}
		}
	}
}

func TestFitCutsOnACharacterBoundary(t *testing.T) {
	for _, c := range []struct {
		reply string
		limit int
		want  string
	}{
		{"hello", 5, "hello"},
		{"hello", 1, "h"},
		{"hello", 2, "he"},
		{"hello", 3, "hel"},
		{"hello", 4, "h..."},
		{"héllo", 2, "h"},
		{"héllo", 3, "hé"},
		{"héllo", 5, "h..."},
		{"日本語です", 7, "日..."},
		{"日本語です", 2, ""},
	} {
		if got := fit(c.reply, c.limit); got != c.want {
			t.Errorf("fit(%q, %d) = %q, want %q", c.reply, c.limit, got, c.want)
		}
	}
}

func TestALowReplyLimitCutsTheReply(t *testing.T) {
	for limit, want := range map[string]string{"1": "e", "2": "ec", "3": "ech", "4": "e...", "9": "echo: ..."} {
		t.Setenv("HUM_TWILIO_REPLY_LIMIT", limit)
		w := start(t, nil)
		if got := w.text(number, "a long enough text"); got != want {
			t.Errorf("limit %s: %q, want %q", limit, got, want)
		}
	}
}

func TestTwoTextsFromOneNumberEachGetTheirOwnReply(t *testing.T) {
	w := start(t, nil)
	replies := make([]string, 3)
	var wg sync.WaitGroup
	began := time.Now()
	var other time.Duration
	for i, text := range []struct{ from, body string }{
		{number, "slow 400 one"},
		{number, "two"},
		{"+14155550000", "three"},
	} {
		i, text := i, text
		wg.Add(1)
		go func() {
			defer wg.Done()
			replies[i] = w.text(text.from, text.body)
			if i == 2 {
				other = time.Since(began)
			}
		}()
		time.Sleep(100 * time.Millisecond)
	}
	wg.Wait()
	if replies[0] != "echo: one" || replies[1] != "echo: two" || replies[2] != "echo: three" {
		t.Fatalf("replies %q", replies)
	}
	if other > 350*time.Millisecond {
		t.Errorf("another number's text waited %v behind the first", other)
	}
}

func TestALateBloomIsCancelled(t *testing.T) {
	w := start(t, func(c *config) { c.deadline = 300 * time.Millisecond })
	if got := w.text(number, "hang"); got != replyLate {
		t.Fatalf("the texter got %q", got)
	}
	cancels := w.heard("cancel")
	if len(cancels) != 1 || cancels[0]["sid"] != sidFor(number) {
		t.Fatalf("cancels %v", cancels)
	}
	if got := w.text(number, "two"); got != "echo: two" {
		t.Fatalf("the next text got %q", got)
	}
}

func TestATextQueuedPastTheDeadlineIsToldSo(t *testing.T) {
	w := start(t, func(c *config) {
		c.deadline = 300 * time.Millisecond
		c.grace = time.Second
	})
	replies := make([]string, 2)
	var wg sync.WaitGroup
	for i, body := range []string{"deaf", "two"} {
		i, body := i, body
		wg.Add(1)
		go func() {
			defer wg.Done()
			replies[i] = w.text(number, body)
		}()
		time.Sleep(50 * time.Millisecond)
	}
	wg.Wait()
	if replies[0] != replyLate || replies[1] != replyBusy {
		t.Fatalf("replies %q", replies)
	}
	if n := len(w.heard("prompt")); n != 1 {
		t.Fatalf("%d prompts while the first bloom held the line", n)
	}
	time.Sleep(time.Second)
	if got := w.text(number, "three"); got != "echo: three" {
		t.Fatalf("after the grace, %q", got)
	}
}

func TestHumdGoingAwayEndsTheBloom(t *testing.T) {
	w := start(t, nil)
	began := time.Now()
	if got := w.text(number, "away"); got != replyAway {
		t.Fatalf("the texter got %q", got)
	}
	if took := time.Since(began); took > 2*time.Second {
		t.Fatalf("the reply waited %v", took)
	}
	time.Sleep(300 * time.Millisecond)
	if got := w.text(number, "back"); got != "echo: back" {
		t.Fatalf("once humd is back, %q", got)
	}
}

func TestTwiliosSignature(t *testing.T) {
	// The worked example in Twilio's webhook security docs.
	form := url.Values{
		"CallSid": {"CA1234567890ABCDE"},
		"Caller":  {"+12349013030"},
		"Digits":  {"1234"},
		"From":    {"+12349013030"},
		"To":      {"+18005551212"},
	}
	at := "https://mycompany.com/myapp.php?foo=1&bar=2"
	if got := sign("12345", at, form); got != "0/KCTR6DLpKmkAf8muzZqo1nDgQ=" {
		t.Fatalf("signature %q", got)
	}
	if !signed("12345", at, form, "0/KCTR6DLpKmkAf8muzZqo1nDgQ=") {
		t.Fatal("refused Twilio's own example")
	}
	if !signed("12345", "https://mycompany.com:443/myapp.php?foo=1&bar=2", form, "0/KCTR6DLpKmkAf8muzZqo1nDgQ=") {
		t.Fatal("refused it with the default port")
	}
	for _, bad := range []string{"", "0/KCTR6DLpKmkAf8muzZqo1nDgQ", sign("54321", at, form)} {
		if signed("12345", at, form, bad) {
			t.Errorf("took %q", bad)
		}
	}
	form.Set("Digits", "4321")
	if signed("12345", at, form, "0/KCTR6DLpKmkAf8muzZqo1nDgQ=") {
		t.Error("took a changed param")
	}
}

func TestPortVariants(t *testing.T) {
	for at, want := range map[string][]string{
		"https://h.example/sms":         {"https://h.example/sms", "https://h.example:443/sms"},
		"https://h.example:443/sms?a=1": {"https://h.example:443/sms?a=1", "https://h.example/sms?a=1"},
		"http://h.example":              {"http://h.example", "http://h.example:80"},
		"http://[::1]:80/sms":           {"http://[::1]:80/sms", "http://[::1]/sms"},
		"http://[::1]/sms":              {"http://[::1]/sms", "http://[::1]:80/sms"},
		"https://h.example:8443/sms":    {"https://h.example:8443/sms"},
	} {
		got := portVariants(at)
		if strings.Join(got, " ") != strings.Join(want, " ") {
			t.Errorf("%s: %q, want %q", at, got, want)
		}
	}
}

func TestOnlyASignedRequestIsAnsweredWhenATokenIsSet(t *testing.T) {
	t.Setenv("TWILIO_AUTH_TOKEN", "12345")
	t.Setenv("HUM_TWILIO_URL", "")
	w := start(t, nil)
	form := url.Values{"From": {number}, "Body": {"ping"}, "MessageSid": {"SM1"}}
	sig := func(at string) http.Header {
		return http.Header{"X-Twilio-Signature": {sign("12345", at, form)}}
	}
	if code, body := w.post("/sms", form, nil); code != http.StatusForbidden {
		t.Errorf("unsigned: %d %q", code, body)
	}
	if code, body := w.post("/sms", form, sig(w.srv.URL+"/webhook")); code != http.StatusForbidden {
		t.Errorf("signed for another URL: %d %q", code, body)
	}
	if code, body := w.post("/sms", form, sig(w.srv.URL+"/sms")); code != http.StatusOK || body != "echo: ping" {
		t.Errorf("signed: %d %q", code, body)
	}
	behind := sig("https" + strings.TrimPrefix(w.srv.URL, "http") + "/sms")
	behind.Set("X-Forwarded-Proto", "https")
	if code, body := w.post("/sms", form, behind); code != http.StatusOK || body != "echo: ping" {
		t.Errorf("signed for https, behind a proxy that says so: %d %q", code, body)
	}

	t.Setenv("HUM_TWILIO_URL", "https://hum.example.com/twilio/sms")
	w = start(t, nil)
	if code, body := w.post("/sms", form, sig("https://hum.example.com/twilio/sms")); code != http.StatusOK || body != "echo: ping" {
		t.Errorf("signed for HUM_TWILIO_URL: %d %q", code, body)
	}
	if code, body := w.post("/sms", form, sig(w.srv.URL+"/sms")); code != http.StatusForbidden {
		t.Errorf("signed for the local URL when HUM_TWILIO_URL is set: %d %q", code, body)
	}
}

func TestWithoutATokenAnUnsignedRequestIsAnswered(t *testing.T) {
	t.Setenv("TWILIO_AUTH_TOKEN", "")
	w := start(t, nil)
	if got := w.text(number, "ping"); got != "echo: ping" {
		t.Fatalf("reply %q", got)
	}
}

func TestABloomWhoseNurseIsGoneIsGrownAgain(t *testing.T) {
	w := start(t, nil)
	if got := w.text(number, "gone again"); got != "echo: gone again" {
		t.Fatalf("the texter got %q", got)
	}
	if n := len(w.heard("prompt")); n != 2 {
		t.Fatalf("%d prompts", n)
	}
}
