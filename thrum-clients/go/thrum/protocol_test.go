package thrum

// Golden-wire parse test: every committed tone byte-decodes into its
// generated view struct without losing or inventing keys. When views.rs
// changes a wire key or shape, regen + rerun me.

import (
	"bufio"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

var envKeys = map[string]bool{
	"chi": true, "rid": true, "from": true, "to": true, "sigil": true,
	"sid": true, "wane": true, "sentAt": true, "dusk": true, "ext": true,
}

func goldenLines(t *testing.T) []map[string]json.RawMessage {
	t.Helper()
	fixture := filepath.Join("..", "..", "..", "thrum-core", "tests", "fixtures", "golden.ndjson")
	f, err := os.Open(fixture)
	if err != nil {
		t.Fatalf("open fixture: %v", err)
	}
	defer f.Close()
	var out []map[string]json.RawMessage
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" {
			continue
		}
		var m map[string]json.RawMessage
		if err := json.Unmarshal([]byte(line), &m); err != nil {
			t.Fatalf("fixture line: %v", err)
		}
		out = append(out, m)
	}
	return out
}

func TestGoldenDecodesIntoViews(t *testing.T) {
	tone := goldenLines(t)
	for i, m := range tone {
		var chi Chi
		if err := json.Unmarshal(m["chi"], &chi); err != nil {
			t.Fatalf("line %d: chi: %v", i, err)
		}
		if !IsValidChi(string(chi)) {
			t.Fatalf("line %d: unknown chi %q", i, chi)
		}
		body := make(map[string]json.RawMessage)
		for k, v := range m {
			if !envKeys[k] {
				body[k] = v
			}
		}
		pick, ok := ToneViews[chi]
		if !ok {
			t.Fatalf("line %d: chi %q has no view", i, chi)
		}
		// Allocate a fresh instance of the body view type and unmarshal
		// into it — this is the decode path real balks take.
		blob, _ := json.Marshal(body)
		inst := reflect.New(reflect.TypeOf(pick))
		if err := json.Unmarshal(blob, inst.Interface()); err != nil {
			t.Fatalf("line %d (%s): decode into %T: %v", i, chi, pick, err)
		}
		t.Logf("line %d: %s -> %+v", i, chi, inst.Elem().Interface())
	}
}

func TestGoldenSpotChecks(t *testing.T) {
	for _, m := range goldenLines(t) {
		var chi Chi
		json.Unmarshal(m["chi"], &chi)
		body := make(map[string]json.RawMessage)
		for k, v := range m {
			if !envKeys[k] {
				body[k] = v
			}
		}
		switch chi {
		case ChiHello:
			var h HelloBody
			blob, _ := json.Marshal(body)
			if err := json.Unmarshal(blob, &h); err != nil {
				t.Fatal(err)
			}
			if h.ProtoVersion != "0.7.0" || h.Bee != "claude-cli" {
				t.Fatalf("hello mismatch: %+v", h)
			}
		case ChiToolResult:
			var r ToolResultBody
			blob, _ := json.Marshal(body)
			if err := json.Unmarshal(blob, &r); err != nil {
				t.Fatal(err)
			}
			if r.CallId != "call-1" || r.Output == nil || *r.Output != "file contents" {
				t.Fatalf("tool-result mismatch: %+v", r)
			}
		case ChiWaneSync:
			var w WaneSyncBody
			blob, _ := json.Marshal(body)
			if err := json.Unmarshal(blob, &w); err != nil {
				t.Fatal(err)
			}
			if w.Snapshot["a1b2c3d4e5f6"] != 12 {
				t.Fatalf("wane-sync mismatch: %+v", w)
			}
		case ChiEcho:
			var e EchoBody
			blob, _ := json.Marshal(body)
			if err := json.Unmarshal(blob, &e); err != nil {
				t.Fatal(err)
			}
			if !e.Ok {
				t.Fatalf("echo mismatch: %+v", e)
			}
		}
	}
}
