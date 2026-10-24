// The client end of `hum bee link`, for a bee written in Go.
//
// A Bee runs `hum bee link` and speaks to it over two pipes, one JSON
// object per line. The link is hum's own client, in Rust: it enters the
// bee, signs the hello, reads humd's verdict, reconnects when humd
// restarts, and checks every tone against the thrum. So this file knows
// no protocol, and a Go bee behaves exactly as a bee in any language.
//
//	bee := thrum.NewBee(thrum.Options{Hive: "twilio-sms", As: "main", Hello: hello})
//	bee.On(thrum.ChiPrompt, onPrompt)
//	bee.OnLost(stopTurns)
//	if err := bee.Start(ctx); err != nil { ... } // admitted, or *Refused, or *LinkError
//	err := bee.Wait()                              // how the link ended
//
// A tone goes to the handler for its sid if there is one, else to the
// handler for its chi, else to OnAny: an asker follows each session it
// opened by sid, a worker takes prompts by chi. Handlers run one at a
// time, in the order tones arrive, on the goroutine reading the link: start long work in a goroutine of its own,
// so the tone after it (a cancel) is not held up. Send is safe from any
// goroutine. A tone sent while humd is away is dropped by the link,
// which says so.

package thrum

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
)

// @constants

// Tone is one thrum tone, as JSON.
type Tone = map[string]any

// Options name the bee and its hello.
type Options struct {
	Hive  string
	As    string
	Hello map[string]any
	// Hum is the `hum` to run; HumCLI() when empty.
	Hum string
}

// Advisory is something the bee should fix. The link has already logged it.
type Advisory struct {
	Code    string         `json:"code"`
	Message string         `json:"message"`
	From    string         `json:"from"`
	Fields  map[string]any `json:"fields,omitempty"`
}

// Refused is humd refusing the bee.
type Refused struct{ Reason string }

func (r *Refused) Error() string { return "humd refused the bee: " + r.Reason }

// LinkError is the link failing to run, or ending without the bee closing it.
type LinkError struct{ Reason string }

func (e *LinkError) Error() string { return e.Reason }

// HumCLI is where `hum` is: $HUM_CLI, else `hum` on PATH, else ~/.local/bin/hum.
func HumCLI() string {
	if p := os.Getenv(HumCLIEnv); p != "" {
		return p
	}
	if p, err := exec.LookPath(HumCLIName); err == nil {
		return p
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, HumCLIDefault)
}

// Bee is one bee's client.
type Bee struct {
	opts Options

	mu       sync.Mutex
	bySid    map[string]func(Tone)
	byChi    map[string]func(Tone)
	any      func(Tone)
	nestled  []func()
	lost     []func()
	advisory []func(Advisory)
	id       string
	closing  bool
	refusal  string
	failure  error
	cmd      *exec.Cmd

	wmu   sync.Mutex
	stdin io.WriteCloser

	started   chan error
	startOnce sync.Once
	done      chan struct{}
	endOnce   sync.Once
	err       error
}

// NewBee makes a bee's client; nothing runs until Start.
func NewBee(opts Options) *Bee {
	return &Bee{
		opts:    opts,
		bySid:   map[string]func(Tone){},
		byChi:   map[string]func(Tone){},
		started: make(chan error, 1),
		done:    make(chan struct{}),
	}
}

// On handles every tone of one chi; a later handler replaces an earlier.
func (b *Bee) On(chi Chi, fn func(Tone)) *Bee {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.byChi[string(chi)] = fn
	return b
}

// OnSid handles every tone of one session, before any chi handler.
func (b *Bee) OnSid(sid string, fn func(Tone)) *Bee {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.bySid[sid] = fn
	return b
}

// OffSid stops following a session.
func (b *Bee) OffSid(sid string) *Bee {
	b.mu.Lock()
	defer b.mu.Unlock()
	delete(b.bySid, sid)
	return b
}

// OnAny handles every tone no sid or chi handler takes.
func (b *Bee) OnAny(fn func(Tone)) *Bee {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.any = fn
	return b
}

// OnNestled runs when humd admits the bee: at start, and after every reconnect.
func (b *Bee) OnNestled(fn func()) *Bee {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.nestled = append(b.nestled, fn)
	return b
}

// OnLost runs when humd goes away. The link is dialing again; stop what humd was waiting on.
func (b *Bee) OnLost(fn func()) *Bee {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.lost = append(b.lost, fn)
	return b
}

// OnAdvisory runs for something the bee should fix.
func (b *Bee) OnAdvisory(fn func(Advisory)) *Bee {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.advisory = append(b.advisory, fn)
	return b
}

// ID is the bee's id, once the link has entered it.
func (b *Bee) ID() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.id
}

// Start runs the link and returns once humd has admitted the bee.
func (b *Bee) Start(ctx context.Context) error {
	b.mu.Lock()
	if b.cmd != nil {
		b.mu.Unlock()
		return &LinkError{"this bee has already started"}
	}
	hum := b.opts.Hum
	if hum == "" {
		hum = HumCLI()
	}
	cmd := exec.Command(hum, "bee", "link", "--hive", b.opts.Hive, "--as", b.opts.As)
	cmd.Stderr = os.Stderr
	b.cmd = cmd
	b.mu.Unlock()

	stdin, err := cmd.StdinPipe()
	if err != nil {
		return b.fail(&LinkError{fmt.Sprintf("could not run %s: %v", hum, err)})
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return b.fail(&LinkError{fmt.Sprintf("could not run %s: %v", hum, err)})
	}
	if err := cmd.Start(); err != nil {
		return b.fail(&LinkError{fmt.Sprintf("could not run %s: %v", hum, err)})
	}
	b.wmu.Lock()
	b.stdin = stdin
	b.wmu.Unlock()
	hello := b.opts.Hello
	if hello == nil {
		hello = map[string]any{}
	}
	_ = b.write(map[string]any{"hello": hello})
	go b.read(stdout)

	select {
	case err := <-b.started:
		return err
	case <-ctx.Done():
		return ctx.Err()
	}
}

// Send sends a tone. The client gives it a rid when it has none.
func (b *Bee) Send(tone Tone) error {
	return b.write(tone)
}

// Hello changes the hello; a field set to nil is dropped.
func (b *Bee) Hello(patch map[string]any) error {
	return b.write(map[string]any{"hello": patch})
}

// Close closes the link: what was sent goes out, then the link exits.
func (b *Bee) Close() error {
	b.mu.Lock()
	b.closing = true
	started := b.cmd != nil
	b.mu.Unlock()
	b.wmu.Lock()
	stdin := b.stdin
	b.wmu.Unlock()
	if stdin != nil {
		_ = stdin.Close()
	} else if !started {
		b.end(nil)
	}
	return b.Wait()
}

// Wait returns once the link has exited: nil after Close, *Refused or *LinkError otherwise.
func (b *Bee) Wait() error {
	<-b.done
	return b.err
}

func (b *Bee) write(msg any) error {
	line, err := json.Marshal(msg)
	if err != nil {
		return err
	}
	b.wmu.Lock()
	defer b.wmu.Unlock()
	if b.stdin == nil {
		return &LinkError{"this bee has not started"}
	}
	_, err = b.stdin.Write(append(line, '\n'))
	return err
}

func (b *Bee) read(stdout io.Reader) {
	r := bufio.NewReader(stdout)
	for {
		line, err := r.ReadBytes('\n')
		if len(line) > 0 {
			b.receive(line)
		}
		if err != nil {
			break
		}
	}
	_ = b.cmd.Wait()
	code := -1
	if b.cmd.ProcessState != nil {
		code = b.cmd.ProcessState.ExitCode()
	}
	b.exited(code)
}

func (b *Bee) receive(line []byte) {
	var msg map[string]any
	if json.Unmarshal(line, &msg) != nil {
		return
	}
	if chi, ok := msg["chi"].(string); ok {
		sid, _ := msg["sid"].(string)
		b.mu.Lock()
		fn := b.bySid[sid]
		if fn == nil {
			fn = b.byChi[chi]
		}
		if fn == nil {
			fn = b.any
		}
		b.mu.Unlock()
		if fn != nil {
			call(func() { fn(msg) })
		}
		return
	}
	link, _ := msg["link"].(string)
	switch link {
	case "ready":
		version, _ := msg["linkVersion"].(float64)
		if int(version) != LinkVersion {
			b.mu.Lock()
			b.failure = &LinkError{fmt.Sprintf(
				"hum speaks link version %v and this client %d; rebuild the hive against the installed hum",
				msg["linkVersion"], LinkVersion)}
			b.mu.Unlock()
			_ = b.cmd.Process.Kill()
			return
		}
		b.mu.Lock()
		b.id, _ = msg["bee"].(string)
		b.mu.Unlock()
	case "nestled":
		b.startOnce.Do(func() { b.started <- nil })
		b.mu.Lock()
		fns := append([]func(){}, b.nestled...)
		b.mu.Unlock()
		for _, fn := range fns {
			call(fn)
		}
	case "lost":
		b.mu.Lock()
		fns := append([]func(){}, b.lost...)
		b.mu.Unlock()
		for _, fn := range fns {
			call(fn)
		}
	case "advisory":
		a := Advisory{Fields: map[string]any{}}
		a.Code, _ = msg["code"].(string)
		a.Message, _ = msg["message"].(string)
		a.From, _ = msg["from"].(string)
		if f, ok := msg["fields"].(map[string]any); ok {
			a.Fields = f
		}
		b.mu.Lock()
		fns := append([]func(Advisory){}, b.advisory...)
		b.mu.Unlock()
		for _, fn := range fns {
			call(func() { fn(a) })
		}
	case "refused":
		b.mu.Lock()
		b.refusal, _ = msg["error"].(string)
		b.mu.Unlock()
	}
}

func (b *Bee) exited(code int) {
	b.mu.Lock()
	refusal, failure, closing := b.refusal, b.failure, b.closing
	b.mu.Unlock()
	switch {
	case code == LinkExitRefused:
		if refusal == "" {
			refusal = "refused"
		}
		b.end(&Refused{refusal})
	case failure != nil:
		b.end(failure)
	case code == LinkExitClosed && closing:
		b.end(nil)
	default:
		how := fmt.Sprintf("exited %d", code)
		if code < 0 {
			how = "was killed"
		}
		if code == LinkExitUsage {
			how += ", a usage error"
		}
		b.end(&LinkError{"the link " + how})
	}
}

func (b *Bee) fail(err error) error {
	b.end(err)
	return err
}

func (b *Bee) end(err error) {
	b.startOnce.Do(func() {
		if err == nil {
			b.started <- &LinkError{"the link closed before humd admitted the bee"}
		} else {
			b.started <- err
		}
	})
	b.endOnce.Do(func() {
		b.err = err
		close(b.done)
	})
}

func call(fn func()) {
	defer func() {
		if r := recover(); r != nil {
			fmt.Fprintf(os.Stderr, "thrum: a handler failed: %v\n", r)
		}
	}()
	fn()
}
