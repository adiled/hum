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

/// Default lookup of repo files relative to the codegen crate's manifest dir.
pub fn default_chi_rs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../thrum-core/src/chi.rs")
}

pub fn default_lib_rs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../thrum-core/src/lib.rs")
}

pub fn default_ts_out() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../thrum/chi.ts")
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

// ── TS rendering ──────────────────────────────────────────────────────────

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
    s.push_str("// are situational. `ext` is the nestling-private extension bag — thrum\n");
    s.push_str("// core ignores it, each nestling owns its own key.\n");
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
