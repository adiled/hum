// Client — connect to humd's NDJSON socket, send/receive tones.
//
// Spec: ../../thrum/WIRE.md. Open Unix stream, write JSON+'\n',
// read JSON+'\n', dispatch by sid. No magic.

package thrum

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
	"sync"
)

// Handler is invoked once per inbound tone matching a registered sid
// (or the wildcard for tones without a registered sid). Handlers run
// inline on the read loop; do non-trivial work in a goroutine.
type Handler func(Tone)

// Client is a thrum NDJSON client. One per logical nestler connection.
type Client struct {
	socketPath string

	mu        sync.Mutex
	conn      net.Conn
	writer    *bufio.Writer
	handlers  map[string]Handler
	wildcard  Handler
	connected bool
}

// NewClient builds a client targeting `socketPath`. If empty,
// DefaultSocketPath() is used.
func NewClient(socketPath string) *Client {
	if socketPath == "" {
		socketPath = DefaultSocketPath()
	}
	return &Client{
		socketPath: socketPath,
		handlers:   make(map[string]Handler),
	}
}

// Connect opens the Unix socket. Idempotent.
func (c *Client) Connect(ctx context.Context) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.connected {
		return nil
	}
	var d net.Dialer
	conn, err := d.DialContext(ctx, "unix", c.socketPath)
	if err != nil {
		return err
	}
	c.conn = conn
	c.writer = bufio.NewWriter(conn)
	c.connected = true
	return nil
}

// Send serializes `tone` as NDJSON and writes one frame.
func (c *Client) Send(tone Tone) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if !c.connected {
		return errors.New("thrum: not connected")
	}
	b, err := json.Marshal(tone)
	if err != nil {
		return err
	}
	if _, err := c.writer.Write(b); err != nil {
		return err
	}
	if err := c.writer.WriteByte('\n'); err != nil {
		return err
	}
	return c.writer.Flush()
}

// On registers a handler for tones with the given sid.
func (c *Client) On(sid string, h Handler) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.handlers[sid] = h
}

// Off removes a sid handler.
func (c *Client) Off(sid string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	delete(c.handlers, sid)
}

// OnAny registers a catch-all for tones without a registered sid.
func (c *Client) OnAny(h Handler) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.wildcard = h
}

// Run reads frames until the socket closes or ctx is cancelled.
// Dispatches each tone to its sid handler (or the wildcard).
func (c *Client) Run(ctx context.Context) error {
	c.mu.Lock()
	conn := c.conn
	c.mu.Unlock()
	if conn == nil {
		if err := c.Connect(ctx); err != nil {
			return err
		}
		c.mu.Lock()
		conn = c.conn
		c.mu.Unlock()
	}
	reader := bufio.NewReader(conn)
	go func() {
		<-ctx.Done()
		_ = conn.Close()
	}()
	for {
		line, err := reader.ReadBytes('\n')
		if len(line) > 0 {
			c.dispatch(line)
		}
		if err != nil {
			if errors.Is(err, io.EOF) || errors.Is(err, net.ErrClosed) {
				return nil
			}
			return err
		}
	}
}

func (c *Client) dispatch(raw []byte) {
	// Strip trailing newline + any stray whitespace.
	for len(raw) > 0 && (raw[len(raw)-1] == '\n' || raw[len(raw)-1] == '\r') {
		raw = raw[:len(raw)-1]
	}
	if len(raw) == 0 {
		return
	}
	var tone Tone
	if err := json.Unmarshal(raw, &tone); err != nil {
		return
	}
	c.mu.Lock()
	var h Handler
	if sid, ok := tone["sid"].(string); ok {
		h = c.handlers[sid]
	}
	if h == nil {
		h = c.wildcard
	}
	c.mu.Unlock()
	if h != nil {
		defer func() { _ = recover() }()
		h(tone)
	}
}

// Close shuts the connection. Safe to call concurrently with Run.
func (c *Client) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.conn == nil {
		return nil
	}
	err := c.conn.Close()
	c.conn = nil
	c.writer = nil
	c.connected = false
	return err
}
