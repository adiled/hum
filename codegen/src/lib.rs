//! Library face of the codegen — exposes `parse` + `emit_ts` so build
//! scripts can call them directly. The binary entry (`src/main.rs`) is
//! a thin CLI wrapper.
//!
//! No dependency on `thrum-core`. We parse `chi.rs` and `lib.rs` as text
//! so this crate can be a `[build-dependencies]` entry of `thrum-core`
//! without creating a cycle.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;

#[derive(Debug, Clone)]
pub struct Variant {
    /// PascalCase name as it appears in the Rust enum.
    pub pascal: String,
    /// kebab-case wire string (derived from PascalCase).
    pub wire: String,
    /// `///` doc lines stripped of leading slashes and one space, joined
    /// with a single space. Empty if no doc comments preceded the variant.
    pub doc: String,
}

#[derive(Debug, Clone)]
pub struct ChiSpec {
    pub version: String,
    pub chi: Vec<Variant>,
    pub pulse: Vec<Variant>,
}

/// Parse a chi.rs + lib.rs pair into a [`ChiSpec`].
pub fn parse(chi_rs: &Path, lib_rs: &Path) -> Result<ChiSpec> {
    let chi_src = fs::read_to_string(chi_rs)
        .with_context(|| format!("read {}", chi_rs.display()))?;
    let lib_src = fs::read_to_string(lib_rs)
        .with_context(|| format!("read {}", lib_rs.display()))?;
    Ok(ChiSpec {
        version: extract_version(&lib_src)?,
        chi: extract_enum(&chi_src, "Chi")?,
        pulse: extract_enum(&chi_src, "PulseKind")?,
    })
}

/// Emit the canonical TypeScript registry to `output`.
pub fn emit_ts(spec: &ChiSpec, output: &Path) -> Result<()> {
    let s = render_ts(spec);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(output, s).with_context(|| format!("write {}", output.display()))?;
    Ok(())
}

/// Emit the canonical TypeScript runtime helpers to `output`.
///
/// These are the JS analogues of `thrum-core/src/{prims,wane}.rs` —
/// `sigil`, `rid`, `dusk_in`/`is_dusk`, `WaneTracker`. Algorithms are
/// fixed by hum's protocol; generating them from one place keeps the
/// two implementations in lockstep without parity tests.
pub fn emit_helpers(output: &Path) -> Result<()> {
    let s = render_helpers();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(output, s).with_context(|| format!("write {}", output.display()))?;
    Ok(())
}

/// Emit the Python chi registry to `output` (typically `thrum-clients/python/thrum/chi.py`).
pub fn emit_py(spec: &ChiSpec, output: &Path) -> Result<()> {
    let s = render_py(spec);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(output, s).with_context(|| format!("write {}", output.display()))?;
    Ok(())
}

/// Emit the Python helpers (sigil, rid, dusk, WaneTracker) to `output`.
pub fn emit_py_helpers(output: &Path) -> Result<()> {
    let s = render_py_helpers();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(output, s).with_context(|| format!("write {}", output.display()))?;
    Ok(())
}

/// Emit the Go chi registry to `output` (typically `thrum-clients/go/thrum/chi.go`).
pub fn emit_go(spec: &ChiSpec, output: &Path) -> Result<()> {
    let s = render_go(spec);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(output, s).with_context(|| format!("write {}", output.display()))?;
    Ok(())
}

/// Emit the Go helpers (Sigil, Rid, DuskIn, WaneTracker) to `output`.
pub fn emit_go_helpers(output: &Path) -> Result<()> {
    let s = render_go_helpers();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(output, s).with_context(|| format!("write {}", output.display()))?;
    Ok(())
}

/// Default lookup of repo files relative to the codegen crate's manifest dir.
pub fn default_chi_rs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../thrum-core/src/chi.rs")
}

pub fn default_lib_rs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../thrum-core/src/lib.rs")
}

pub fn default_ts_out() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../thrum-clients/ts/chi.ts")
}

pub fn default_helpers_out() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../thrum-clients/ts/helpers.ts")
}

pub fn default_py_out() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../thrum-clients/python/thrum/chi.py")
}

pub fn default_py_helpers_out() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../thrum-clients/python/thrum/helpers.py")
}

pub fn default_go_out() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../thrum-clients/go/thrum/chi.go")
}

pub fn default_go_helpers_out() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../thrum-clients/go/thrum/helpers.go")
}

// ── parsing ────────────────────────────────────────────────────────────────

fn extract_version(lib: &str) -> Result<String> {
    let re = Regex::new(r#"pub\s+const\s+THRUM_VERSION\s*:\s*&\s*str\s*=\s*"([^"]+)""#)?;
    let caps = re.captures(lib).ok_or_else(|| anyhow!("THRUM_VERSION not found in lib.rs"))?;
    Ok(caps[1].to_string())
}

/// Find `pub enum <name> {`, then walk forward collecting each variant
/// (PascalCase identifier followed by `,` or `}`) along with any `///`
/// doc comments that immediately precede it.
fn extract_enum(src: &str, name: &str) -> Result<Vec<Variant>> {
    let opener = Regex::new(&format!(r"pub\s+enum\s+{}\s*\{{", regex::escape(name)))?;
    let m = opener.find(src).ok_or_else(|| anyhow!("`pub enum {}` not found", name))?;
    let body_start = m.end();
    let bytes = src.as_bytes();
    let mut depth = 1usize;
    let mut i = body_start;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 { break; }
            }
            _ => {}
        }
        i += 1;
    }
    if depth != 0 { bail!("unterminated `{}` block", name); }
    let body = &src[body_start..i];

    let mut out = Vec::new();
    let mut pending_doc: Vec<String> = Vec::new();

    for raw_line in body.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            pending_doc.clear();
            continue;
        }
        if let Some(rest) = line.strip_prefix("///") {
            pending_doc.push(rest.trim_start().trim_end().to_string());
            continue;
        }
        if line.starts_with("//") {
            // Plain comments (section headers etc.) reset the doc streak.
            pending_doc.clear();
            continue;
        }
        let ident_end = line.find([',', ' ', '#', '(']).unwrap_or(line.len());
        let ident = &line[..ident_end];
        if !ident.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            pending_doc.clear();
            continue;
        }
        let pascal = ident.to_string();
        let wire = pascal_to_kebab(&pascal);
        let doc = pending_doc.join(" ").trim().to_string();
        pending_doc.clear();
        out.push(Variant { pascal, wire, doc });
    }
    if out.is_empty() { bail!("`{}` block parsed but yielded no variants", name); }
    Ok(out)
}

fn pascal_to_kebab(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 { out.push('-'); }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn pascal_to_camel(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_ascii_lowercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// `ToolCall` → `TOOL_CALL`. Python const convention.
fn pascal_to_screaming_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c);
        } else {
            out.push(c.to_ascii_uppercase());
        }
    }
    out
}

// ── TS rendering ──────────────────────────────────────────────────────────

fn render_helpers() -> String {
    // Pinned by hum's protocol. If you change any of these, the wire
    // breaks — bump THRUM_VERSION and verify Rust parity.
    const SRC: &str = r#"// @generated by `cargo run -p codegen` from thrum-core — DO NOT EDIT.
//
// Runtime helpers that mirror thrum-core/src/{prims,wane}.rs. Generated
// so the TS side cannot drift from the Rust side. If you need a new
// helper, add it in Rust first and extend codegen's render_helpers.

import { createHash } from "crypto";

/**
 * Deterministic identity for a (nest, session) pair.
 *
 * `nest` is the nest-kind namespace — "claude-cli", "claude-repl", or
 * any other future nest implementation. Required; no default — the
 * protocol layer must not know about specific kinds.
 *
 * Returns the lowercase hex of the first 6 sha256 bytes (12 chars).
 */
export function sigil(sid: string, nest: string): string {
  return createHash("sha256")
    .update(`${nest}:${sid}`)
    .digest("hex")
    .slice(0, 12);
}

/** Monotonic request id — base36 timestamp + counter. */
let __ridCounter = 0;
export function rid(): string {
  return `${Date.now().toString(36)}-${(__ridCounter++).toString(36)}`;
}

/** Absolute ms timestamp ms in the future. */
export function duskIn(ms: number): number {
  return Date.now() + ms;
}

/** True iff `tone.dusk` is set and already past. */
export function isDusk(tone: { dusk?: number }): boolean {
  return typeof tone.dusk === "number" && Date.now() > tone.dusk;
}

/**
 * Drift detection. Monotonic counter per sigil. Both sides track their
 * own wane; when wanes diverge, drift is visible — the stale side
 * resyncs. Mirrors thrum_core::WaneTracker.
 */
export class WaneTracker {
  private counters = new Map<string, number>();
  get(s: string): number { return this.counters.get(s) ?? 0; }
  tick(s: string): number {
    const next = (this.counters.get(s) ?? 0) + 1;
    this.counters.set(s, next);
    return next;
  }
  set(s: string, value: number): void { this.counters.set(s, value); }
  behind(s: string, remote: number): boolean { return remote > this.get(s); }
}
"#;
    SRC.to_string()
}

fn render_ts(spec: &ChiSpec) -> String {
    let mut s = String::new();
    s.push_str("// @generated by `cargo run -p codegen` from thrum-core/src/chi.rs — DO NOT EDIT.\n");
    s.push_str("//\n");
    s.push_str("// Rust is the canonical home of the wire registry. Hand-edit chi.rs;\n");
    s.push_str("// the file is regenerated on every cargo build of thrum-core (build.rs).\n");
    s.push_str("// Manual regen: `cargo run -p codegen`.\n\n");

    s.push_str(&format!("export const THRUM_VERSION = \"{}\" as const;\n\n", spec.version));

    s.push_str("// Every wire-known chi value. Adding a new variant bumps the\n");
    s.push_str("// protocol minor; renaming/removing bumps major.\n");
    s.push_str("export const Chi = {\n");
    for v in &spec.chi {
        if !v.doc.is_empty() {
            s.push_str(&format!("  /** {} */\n", v.doc));
        }
        s.push_str(&format!("  {}: \"{}\",\n", pascal_to_camel(&v.pascal), v.wire));
    }
    s.push_str("} as const;\n");
    s.push_str("export type ChiKind = typeof Chi[keyof typeof Chi];\n\n");
    s.push_str("export const ALL_CHI: ReadonlySet<ChiKind> = new Set(Object.values(Chi));\n");
    s.push_str("export function isValidChi(s: string): s is ChiKind { return ALL_CHI.has(s as ChiKind); }\n\n");

    s.push_str("// pulse.kind is its own enum within chi:\"pulse\" tones.\n");
    s.push_str("export const PulseKind = {\n");
    for v in &spec.pulse {
        if !v.doc.is_empty() {
            s.push_str(&format!("  /** {} */\n", v.doc));
        }
        s.push_str(&format!("  {}: \"{}\",\n", pascal_to_camel(&v.pascal), v.wire));
    }
    s.push_str("} as const;\n");
    s.push_str("export type PulseKindT = typeof PulseKind[keyof typeof PulseKind];\n\n");

    s.push_str("// Fields every tone may carry. `chi` and `rid` are required; the rest\n");
    s.push_str("// are situational. `ext` is the bee-private extension bag — thrum\n");
    s.push_str("// core ignores it, each bee owns its own key.\n");
    s.push_str("export interface Envelope {\n");
    s.push_str("  chi: ChiKind;\n");
    s.push_str("  rid: string;\n");
    s.push_str("  from?: string;\n");
    s.push_str("  to?: string;\n");
    s.push_str("  sigil?: string;\n");
    s.push_str("  sid?: string;\n");
    s.push_str("  wane?: number;\n");
    s.push_str("  sentAt?: number;\n");
    s.push_str("  dusk?: number;\n");
    s.push_str("  ext?: Record<string, Record<string, unknown>>;\n");
    s.push_str("}\n\n");

    s.push_str("export type Tone = Envelope & Record<string, unknown>;\n\n");

    s.push_str("export function isEnvelope(x: unknown): x is Envelope {\n");
    s.push_str("  if (!x || typeof x !== \"object\") return false;\n");
    s.push_str("  const o = x as Record<string, unknown>;\n");
    s.push_str("  return typeof o.chi === \"string\"\n");
    s.push_str("    && (typeof o.rid === \"string\" || o.rid === undefined)\n");
    s.push_str("    && (o.sid === undefined || typeof o.sid === \"string\");\n");
    s.push_str("}\n\n");

    s.push_str("export function isKnownTone(x: unknown): x is Tone {\n");
    s.push_str("  return isEnvelope(x) && isValidChi((x as Envelope).chi as string);\n");
    s.push_str("}\n");

    s
}

// ── Python rendering ──────────────────────────────────────────────────────

fn render_py(spec: &ChiSpec) -> String {
    let mut s = String::new();
    s.push_str("# @generated by `cargo run -p codegen` from thrum-core/src/chi.rs — DO NOT EDIT.\n");
    s.push_str("#\n");
    s.push_str("# Rust is the canonical home of the wire registry. Hand-edit chi.rs;\n");
    s.push_str("# the file is regenerated on every cargo build of thrum-core (build.rs).\n");
    s.push_str("# Manual regen: `cargo run -p codegen`.\n\n");
    s.push_str("from __future__ import annotations\n\n");
    s.push_str(&format!("THRUM_VERSION = \"{}\"\n\n", spec.version));

    s.push_str("class Chi:\n");
    s.push_str("    \"\"\"Every wire-known chi value. Adding a new variant bumps the\n");
    s.push_str("    protocol minor; renaming/removing bumps major.\"\"\"\n");
    for v in &spec.chi {
        if !v.doc.is_empty() {
            s.push_str(&format!("    # {}\n", v.doc));
        }
        s.push_str(&format!(
            "    {}: str = \"{}\"\n",
            pascal_to_screaming_snake(&v.pascal),
            v.wire
        ));
    }
    s.push('\n');

    // The chi value set, for membership checks.
    s.push_str("ALL_CHI: frozenset[str] = frozenset({\n");
    for v in &spec.chi {
        s.push_str(&format!("    \"{}\",\n", v.wire));
    }
    s.push_str("})\n\n");
    s.push_str("def is_valid_chi(value: str) -> bool:\n");
    s.push_str("    return value in ALL_CHI\n\n");

    s.push_str("class PulseKind:\n");
    s.push_str("    \"\"\"pulse.kind enum within chi:'pulse' tones.\"\"\"\n");
    for v in &spec.pulse {
        if !v.doc.is_empty() {
            s.push_str(&format!("    # {}\n", v.doc));
        }
        s.push_str(&format!(
            "    {}: str = \"{}\"\n",
            pascal_to_screaming_snake(&v.pascal),
            v.wire
        ));
    }
    s.push('\n');

    s
}

fn render_py_helpers() -> String {
    const SRC: &str = r#"# @generated by `cargo run -p codegen` from thrum-core — DO NOT EDIT.
#
# Runtime helpers that mirror thrum-core/src/{prims,wane}.rs. Generated
# so the Python side cannot drift from the Rust side.

from __future__ import annotations

import hashlib
import os
import threading
import time
from typing import Any, Mapping


def sigil(sid: str, nest: str) -> str:
    """Deterministic 12-char identity for a (nest, sid) pair.

    Returns lowercase hex of the first 6 sha256 bytes of `nest:sid`.
    Required-`nest`, no fallback — the protocol layer must not know
    about specific nest kinds.
    """
    h = hashlib.sha256(f"{nest}:{sid}".encode("utf-8")).digest()
    return h[:6].hex()


def now_ms() -> int:
    """Wall-clock milliseconds since the Unix epoch."""
    return int(time.time() * 1000)


_RID_LOCK = threading.Lock()
_RID_COUNTER = 0


def _base36(n: int) -> str:
    if n == 0:
        return "0"
    alphabet = "0123456789abcdefghijklmnopqrstuvwxyz"
    out = []
    while n > 0:
        out.append(alphabet[n % 36])
        n //= 36
    return "".join(reversed(out))


def rid() -> str:
    """Monotonic correlation id: '{base36-ms}-{base36-counter}'."""
    global _RID_COUNTER
    with _RID_LOCK:
        n = _RID_COUNTER
        _RID_COUNTER += 1
    return f"{_base36(now_ms())}-{_base36(n)}"


def dusk_in(ms: int) -> int:
    """Absolute ms timestamp at which a tone with this dusk expires."""
    return now_ms() + ms


def is_dusk(tone: Mapping[str, Any]) -> bool:
    """True iff `tone.dusk` is set and already past."""
    d = tone.get("dusk")
    return isinstance(d, (int, float)) and now_ms() > d


class WaneTracker:
    """Lamport clock per sigil. Both sides track their own wane;
    divergence triggers resync via chi:'wane-sync'."""

    def __init__(self) -> None:
        self._counters: dict[str, int] = {}
        self._lock = threading.Lock()

    def get(self, sigil: str) -> int:
        with self._lock:
            return self._counters.get(sigil, 0)

    def tick(self, sigil: str) -> int:
        with self._lock:
            n = self._counters.get(sigil, 0) + 1
            self._counters[sigil] = n
            return n

    def set(self, sigil: str, value: int) -> None:
        with self._lock:
            self._counters[sigil] = value

    def behind(self, sigil: str, remote: int) -> bool:
        with self._lock:
            return remote > self._counters.get(sigil, 0)


def default_socket_path() -> str:
    """Resolve the humd thrum socket per WIRE.md priority:
    HUM_THRUM_SOCK > $XDG_RUNTIME_DIR/hum/thrum.sock > /run/user/<uid>/hum/thrum.sock."""
    explicit = os.environ.get("HUM_THRUM_SOCK")
    if explicit:
        return explicit
    runtime = os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.geteuid()}"
    return os.path.join(runtime, "hum", "thrum.sock")
"#;
    SRC.to_string()
}

// ── Go rendering ──────────────────────────────────────────────────────────

fn render_go(spec: &ChiSpec) -> String {
    let mut s = String::new();
    s.push_str("// @generated by `cargo run -p codegen` from thrum-core/src/chi.rs — DO NOT EDIT.\n");
    s.push_str("//\n");
    s.push_str("// Rust is the canonical home of the wire registry. Hand-edit chi.rs;\n");
    s.push_str("// the file is regenerated on every cargo build of thrum-core (build.rs).\n");
    s.push_str("// Manual regen: `cargo run -p codegen`.\n\n");
    s.push_str("package thrum\n\n");

    s.push_str(&format!("const ThrumVersion = \"{}\"\n\n", spec.version));

    s.push_str("// Chi is the discriminator on every tone.\n");
    s.push_str("type Chi string\n\n");
    s.push_str("// Every wire-known chi value. Adding a new variant bumps the\n");
    s.push_str("// protocol minor; renaming/removing bumps major.\n");
    s.push_str("const (\n");
    for v in &spec.chi {
        if !v.doc.is_empty() {
            s.push_str(&format!("    // {}\n", v.doc));
        }
        s.push_str(&format!("    Chi{} Chi = \"{}\"\n", v.pascal, v.wire));
    }
    s.push_str(")\n\n");

    s.push_str("// AllChi is the set of every known chi value for membership checks.\n");
    s.push_str("var AllChi = map[Chi]struct{}{\n");
    for v in &spec.chi {
        s.push_str(&format!("    Chi{}: {{}},\n", v.pascal));
    }
    s.push_str("}\n\n");
    s.push_str("// IsValidChi returns true if value is a known chi.\n");
    s.push_str("func IsValidChi(value string) bool {\n");
    s.push_str("    _, ok := AllChi[Chi(value)]\n");
    s.push_str("    return ok\n");
    s.push_str("}\n\n");

    s.push_str("// PulseKind is the kind field within chi:\"pulse\" tones.\n");
    s.push_str("type PulseKind string\n\n");
    s.push_str("const (\n");
    for v in &spec.pulse {
        if !v.doc.is_empty() {
            s.push_str(&format!("    // {}\n", v.doc));
        }
        s.push_str(&format!("    PulseKind{} PulseKind = \"{}\"\n", v.pascal, v.wire));
    }
    s.push_str(")\n");

    s
}

fn render_go_helpers() -> String {
    const SRC: &str = r#"// @generated by `cargo run -p codegen` from thrum-core — DO NOT EDIT.
//
// Runtime helpers that mirror thrum-core/src/{prims,wane}.rs. Generated
// so the Go side cannot drift from the Rust side.

package thrum

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"sync"
	"sync/atomic"
	"time"
)

// Sigil returns the 12-char content-addressable identifier for a
// (nest, sid) pair: lowercase hex of the first 6 sha256 bytes of
// "nest:sid". Stable across reconnects.
func Sigil(sid, nest string) string {
	h := sha256.Sum256([]byte(nest + ":" + sid))
	return hex.EncodeToString(h[:6])
}

// NowMs returns wall-clock milliseconds since the Unix epoch.
func NowMs() int64 { return time.Now().UnixMilli() }

var ridCounter uint64

func base36(n uint64) string {
	if n == 0 {
		return "0"
	}
	return strconv.FormatUint(n, 36)
}

// Rid mints a monotonic correlation id: "{base36-ms}-{base36-counter}".
func Rid() string {
	n := atomic.AddUint64(&ridCounter, 1) - 1
	return fmt.Sprintf("%s-%s", base36(uint64(NowMs())), base36(n))
}

// DuskIn returns the absolute ms timestamp at which a tone with this
// dusk expires.
func DuskIn(ms int64) int64 { return NowMs() + ms }

// Tone is the loose-JSON envelope every chi rides inside.
type Tone map[string]any

// IsDusk reports whether tone.dusk is set and already past.
func IsDusk(t Tone) bool {
	switch d := t["dusk"].(type) {
	case float64:
		return NowMs() > int64(d)
	case int64:
		return NowMs() > d
	case int:
		return NowMs() > int64(d)
	}
	return false
}

// WaneTracker is one Lamport clock per sigil. Both sides track their
// own wane; divergence triggers resync via chi:"wane-sync".
type WaneTracker struct {
	mu       sync.Mutex
	counters map[string]int64
}

func NewWaneTracker() *WaneTracker {
	return &WaneTracker{counters: make(map[string]int64)}
}

func (w *WaneTracker) Get(sigil string) int64 {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.counters[sigil]
}

func (w *WaneTracker) Tick(sigil string) int64 {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.counters[sigil]++
	return w.counters[sigil]
}

func (w *WaneTracker) Set(sigil string, value int64) {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.counters[sigil] = value
}

func (w *WaneTracker) Behind(sigil string, remote int64) bool {
	w.mu.Lock()
	defer w.mu.Unlock()
	return remote > w.counters[sigil]
}

// DefaultSocketPath resolves the humd thrum socket per WIRE.md priority:
// HUM_THRUM_SOCK > $XDG_RUNTIME_DIR/hum/thrum.sock > /run/user/<uid>/hum/thrum.sock.
func DefaultSocketPath() string {
	if explicit := os.Getenv("HUM_THRUM_SOCK"); explicit != "" {
		return explicit
	}
	runtime := os.Getenv("XDG_RUNTIME_DIR")
	if runtime == "" {
		runtime = fmt.Sprintf("/run/user/%d", os.Geteuid())
	}
	return filepath.Join(runtime, "hum", "thrum.sock")
}
"#;
    SRC.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kebab_and_camel_are_consistent() {
        assert_eq!(pascal_to_kebab("ToolCall"), "tool-call");
        assert_eq!(pascal_to_kebab("DroneRetrofit"), "drone-retrofit");
        assert_eq!(pascal_to_kebab("Hello"), "hello");
        assert_eq!(pascal_to_camel("ToolCall"), "toolCall");
        assert_eq!(pascal_to_camel("Hello"), "hello");
        assert_eq!(pascal_to_screaming_snake("ToolCall"), "TOOL_CALL");
        assert_eq!(pascal_to_screaming_snake("DroneRetrofit"), "DRONE_RETROFIT");
        assert_eq!(pascal_to_screaming_snake("Hello"), "HELLO");
    }

    #[test]
    fn extracts_version() {
        let src = "use foo;\npub const THRUM_VERSION: &str = \"0.3.0\";\n";
        assert_eq!(extract_version(src).unwrap(), "0.3.0");
    }

    #[test]
    fn extracts_variants_with_docs() {
        let src = r#"
            pub enum Chi {
                /// announce self
                Hello,
                /// start a turn
                Prompt,
                // ── Section header ────
                /// turn done
                Finish,
            }
        "#;
        let vs = extract_enum(src, "Chi").unwrap();
        assert_eq!(vs.len(), 3);
        assert_eq!(vs[0].pascal, "Hello");
        assert_eq!(vs[0].wire, "hello");
        assert_eq!(vs[0].doc, "announce self");
        assert_eq!(vs[2].doc, "turn done");
    }
}
