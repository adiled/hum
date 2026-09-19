//! Protocol-view codegen: parse `thrum-core/src/views.rs` + `envelope.rs`
//! with `syn` and emit typed per-chi bodies for TS / Python / Go.
//!
//! Same constraint as the rest of codegen: no dependency on `thrum-core`.
//! We parse source text, so this crate stays an ordinary build dependency
//! of thrum-core (no cycle).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use syn::{Field, Fields, Item, ItemStruct, Meta, Type};

use crate::paths;

/// Simplified wire type — everything `views.rs` / `envelope.rs` can
/// express that the client languages need to render.
#[derive(Debug, Clone)]
pub enum Repr {
    Str,
    Int {
        unsigned: bool,
    },
    Bool,
    /// `serde_json::Value` — opaque JSON.
    Json,
    /// `BTreeMap<String, V>`.
    Map(Box<Repr>),
    /// `Vec<T>`.
    Arr(Box<Repr>),
    /// Reference to another generated type (`DroneLoad`, …).
    Ref(String),
}

#[derive(Debug, Clone)]
pub struct FieldSpec {
    /// Rust (snake_case) name, also the Python field name.
    pub rust: String,
    /// Wire key — from `#[serde(rename = "...")]` else the rust name.
    pub json: String,
    pub repr: Repr,
    pub optional: bool,
}

#[derive(Debug, Clone)]
pub struct TypeSpec {
    /// `HelloBody`, `Envelope`, …
    pub name: String,
    pub fields: Vec<FieldSpec>,
}

#[derive(Debug, Clone)]
pub struct ProtocolSpec {
    pub version: String,
    pub types: Vec<TypeSpec>,
}

/// Parse a views.rs + envelope.rs pair (plus lib.rs for the version).
pub fn protocol_spec(views_rs: &Path, envelope_rs: &Path, lib_rs: &Path) -> Result<ProtocolSpec> {
    let lib_src =
        fs::read_to_string(lib_rs).with_context(|| format!("read {}", lib_rs.display()))?;
    let version = extract_version(&lib_src)?
        .ok_or_else(|| anyhow!("THRUM_VERSION not found in {}", lib_rs.display()))?;

    let mut types = Vec::new();
    for path in [views_rs, envelope_rs] {
        let src = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        for st in structs_in(&src)? {
            types.push(st);
        }
    }
    Ok(ProtocolSpec { version, types })
}

fn structs_in(src: &str) -> Result<Vec<TypeSpec>> {
    let file = syn::parse_file(src).map_err(|e| anyhow::anyhow!("syn parse: {e}"))?;
    let mut out = Vec::new();
    for item in file.items {
        let Item::Struct(s) = item else { continue };
        let name = s.ident.to_string();
        // Only wire-facing types: the envelope, per-chi bodies, and the
        // nested body-support structs (DroneLoad). Test-only types never
        // appear here — unit-test structs are filtered by these names.
        if name != "Envelope" && name != "DroneLoad" && !name.ends_with("Body") {
            continue;
        }
        out.push(type_spec(s));
    }
    Ok(out)
}

fn type_spec(s: ItemStruct) -> TypeSpec {
    let mut fields = Vec::new();
    if let Fields::Named(named) = &s.fields {
        for f in &named.named {
            let Some(field) = field_spec(f) else { continue };
            fields.push(field);
        }
    }
    TypeSpec {
        name: s.ident.to_string(),
        fields,
    }
}

fn field_spec(f: &Field) -> Option<FieldSpec> {
    let rust = f.ident.as_ref()?.to_string();
    let json = serde_rename(f).unwrap_or_else(|| rust.clone());
    let (optional, repr) = simplify(&f.ty);
    Some(FieldSpec {
        rust,
        json,
        repr,
        optional,
    })
}

/// `Option<T>` → (true, T); anything else → (false, itself).
fn simplify(ty: &Type) -> (bool, Repr) {
    if let Type::Path(p) = ty {
        // Check the leading segment name directly: syn's `is_ident` rejects
        // paths with generic arguments, so `Option<String>` would slip by.
        if let Some(seg) = p.path.segments.first() {
            if seg.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(a) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = a.args.first() {
                        return (true, simplify(inner).1);
                    }
                }
            }
        }
    }
    (false, repr_of(ty))
}

fn repr_of(ty: &Type) -> Repr {
    let Type::Path(p) = ty else {
        return Repr::Json; // opaque fallback (bare trait object, etc.)
    };
    let seg = &p.path.segments[0];
    match seg.ident.to_string().as_str() {
        "String" => Repr::Str,
        "i64" | "i32" | "i16" => Repr::Int { unsigned: false },
        "u64" | "u32" | "u16" => Repr::Int { unsigned: true },
        "bool" => Repr::Bool,
        "Value" => Repr::Json,
        "Vec" => {
            let inner = angle_ty(&seg.arguments, 0).unwrap_or_else(|| Repr::Json);
            Repr::Arr(Box::new(inner))
        }
        "BTreeMap" => {
            // The value type is the SECOND generic arg (first is the key).
            let inner = angle_ty(&seg.arguments, 1).unwrap_or_else(|| Repr::Json);
            Repr::Map(Box::new(inner))
        }
        other => {
            // Multi-segment paths (serde_json::Value) land on the last
            // segment; otherwise treat as a reference to another type.
            if p.path.segments.len() > 1 {
                let last = &p.path.segments[p.path.segments.len() - 1];
                return repr_of(&Type::Path(syn::TypePath {
                    qself: None,
                    path: last.clone().into(),
                }));
            }
            Repr::Ref(other.to_string())
        }
    }
}

fn angle_ty(args: &syn::PathArguments, nth: usize) -> Option<Repr> {
    if let syn::PathArguments::AngleBracketed(a) = args {
        if let Some(syn::GenericArgument::Type(t)) = a.args.iter().nth(nth) {
            return Some(simplify(t).1);
        }
    }
    None
}

/// Verify every Chi variant has a matching `{Pascal}Body` view, and every
/// body view maps back to a real chi. Called on every thrum-core build so
/// a new chi without a view (or an orphan view) fails fast instead of
/// silently generating a partial client.
pub fn check_coverage(chis: &[crate::Variant], proto: &ProtocolSpec) -> Result<()> {
    for v in chis {
        let expect = format!("{}Body", v.pascal);
        if !proto.types.iter().any(|t| t.name == expect) {
            anyhow::bail!(
                "chi `{}` ({}) has no body view `{expect}` in {}",
                v.wire,
                v.pascal,
                paths::VIEWS_RS_REL
            );
        }
    }
    let mut wire_names: Vec<String> = proto
        .types
        .iter()
        .filter(|t| names_body_view(&t.name))
        .map(|t| body_chi(&t.name))
        .collect();
    let mut chi_wires: Vec<String> = chis.iter().map(|v| v.wire.clone()).collect();
    wire_names.sort();
    chi_wires.sort();
    if wire_names != chi_wires {
        anyhow::bail!(
            "view <-> chi mismatch in {}: views {wire_names:?} vs chis {chi_wires:?}",
            paths::VIEWS_RS_REL
        );
    }
    Ok(())
}

/// First `#[serde(rename = "...")]` value on the field, if any.
///
/// Parsed by regex over the raw attribute tokens rather than
/// `parse_nested_meta`: that API's handling of unconsumed `key = value`
/// pairs changed across syn 2.x patch releases, and the token text of
/// `rename = "…"` is verbatim in both, so regex is version-proof.
fn serde_rename(f: &Field) -> Option<String> {
    let re = regex::Regex::new(r#"rename\s*=\s*"([^"]*)""#).expect("static regex");
    for attr in &f.attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let Meta::List(list) = &attr.meta else {
            continue;
        };
        if let Some(cap) = re.captures(&list.tokens.to_string()) {
            return Some(cap[1].to_string());
        }
    }
    None
}

/// THRUM_VERSION from lib.rs (same regex as the chi registry parser).
fn extract_version(lib: &str) -> Result<Option<String>> {
    let re = regex::Regex::new(r#"pub\s+const\s+THRUM_VERSION\s*:\s*&\s*str\s*=\s*"([^"]+)""#)?;
    Ok(re.captures(lib).map(|c| c[1].to_string()))
}

// ── emitters ───────────────────────────────────────────────────────────

fn regen_header(comment: &str, refs: &[&str]) -> String {
    let refs = refs.join(", ");
    format!(
        "{c} @generated by `cargo run -p codegen` from {refs} — DO NOT EDIT.\n\
         {c}\n\
         {c} Canonical wire views: every chi body is a Rust struct in\n\
         {c} thrum-core/src/views.rs, the envelope in envelope.rs. These\n\
         {c} client mirrors come out of the same source — no hand-sync.\n\
         {c} Manual regen: `cargo run -p codegen`.\n\n",
        c = comment,
    )
}

/// Emit `protocol.ts` to `output`.
pub fn emit_protocol_ts(spec: &ProtocolSpec, output: &Path) -> Result<()> {
    let mut s = String::new();
    s.push_str(&regen_header(
        "//",
        &[paths::VIEWS_RS_REL, paths::ENVELOPE_RS_REL],
    ));
    s.push_str("export type { ChiKind } from \"./chi.ts\";\n");
    s.push_str("import type { Chi } from \"./chi.ts\";\n\n");

    for t in &spec.types {
        s.push_str(&format!("export interface {} {{\n", t.name));
        for f in &t.fields {
            let opt = if f.optional { "?" } else { "" };
            s.push_str(&format!("  {}{}: {};\n", f.json, opt, ts_repr(&f.repr)));
        }
        s.push_str("}\n\n");
    }

    // chi → view type map for discriminated tones.
    s.push_str("// Body view per chi wire value.\n");
    s.push_str("export interface ToneViews {\n");
    for t in &spec.types {
        if !names_body_view(&t.name) {
            continue;
        }
        let wire = body_chi(&t.name);
        s.push_str(&format!("  \"{}\": {};\n", wire, t.name));
    }
    s.push_str("}\n\n");
    s.push_str("export type ToneView<C extends keyof ToneViews> = Envelope & ToneViews[C];\n");
    write_out(output, &s)
}

/// True for structs that are per-chi bodies (skips Envelope and the
/// nested support structs in chi → view maps).
fn names_body_view(name: &str) -> bool {
    name.ends_with("Body")
}

/// `HelloBody` → `hello`; `KadFindNodeRespBody` → `kad-find-node-resp`.
fn body_chi(name: &str) -> String {
    let pascal = name.strip_suffix("Body").unwrap_or(name);
    let mut out = String::with_capacity(pascal.len() + 4);
    for (i, c) in pascal.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn ts_repr(r: &Repr) -> String {
    match r {
        Repr::Str => "string".into(),
        Repr::Int { .. } => "number".into(),
        Repr::Bool => "boolean".into(),
        Repr::Json => "unknown".into(),
        Repr::Map(inner) => format!("Record<string, {}>", ts_repr(inner)),
        Repr::Arr(inner) => format!("{}[]", ts_repr(inner)),
        Repr::Ref(name) => name.clone(),
    }
}

/// Emit `protocol.py` to `output`.
pub fn emit_protocol_py(spec: &ProtocolSpec, output: &Path) -> Result<()> {
    let mut s = String::new();
    s.push_str(&regen_header(
        "#",
        &[paths::VIEWS_RS_REL, paths::ENVELOPE_RS_REL],
    ));
    s.push_str("from __future__ import annotations\n\n");
    s.push_str("from dataclasses import dataclass\n");
    s.push_str("from typing import Any, ClassVar, Optional\n\n");

    for t in &spec.types {
        s.push_str(&format!("@dataclass\nclass {}:\n", t.name));
        if t.fields.is_empty() {
            s.push_str("    pass\n\n");
            continue;
        }
        // Dataclasses require defaulted (optional) fields after required
        // ones — Rust order is wire order and mixes them freely.
        let ordered: Vec<&FieldSpec> = t
            .fields
            .iter()
            .filter(|f| !f.optional)
            .chain(t.fields.iter().filter(|f| f.optional))
            .collect();
        let mut pairs = Vec::new();
        for f in ordered.iter() {
            let python = py_field_name(&f.rust);
            let default = if f.optional { " = None" } else { "" };
            s.push_str(&format!(
                "    {}: {}{}\n",
                python,
                py_repr(&f.repr, f.optional),
                default
            ));
            pairs.push(format!("        {:?}: {:?},", python, f.json));
        }
        s.push('\n');
        s.push_str("    # rust field name -> wire key (single source in views.rs)\n");
        s.push_str("    _wire: ClassVar[dict[str, str]] = {\n");
        for p in &pairs {
            s.push_str(&format!("{p}\n"));
        }
        s.push_str("    }\n\n");
    }

    s.push_str(
        "# Serialize a view to the flat wire body (envelope keys not included).\n\
         # Absent optionals, like skip_serializing_if on the Rust side, are omitted.\n\
         def to_wire(view: Any) -> dict[str, Any]:\n\
         \x20\x20\x20\x20payload: dict[str, Any] = {}\n\
         \x20\x20\x20\x20for rust_name, wire_key in type(view)._wire.items():\n\
         \x20\x20\x20\x20\x20\x20\x20\x20value = getattr(view, rust_name)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20if value is not None:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20payload[wire_key] = value\n\
         \x20\x20\x20\x20return payload\n\n",
    );
    s.push_str(
        "# Parse a flat wire body into the view for `cls`, ignoring envelope\n\
         # keys (already stripped) and any unknown keys. Missing required\n\
         # fields raise TypeError, surfacing spec drift loudly.\n\
         def from_wire(cls: type, payload: dict[str, Any]) -> Any:\n\
         \x20\x20\x20\x20wire_to_rust = {j: f for f, j in cls._wire.items()}\n\
         \x20\x20\x20\x20kwargs = {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20wire_to_rust[k]: v for k, v in payload.items() if k in wire_to_rust\n\
         \x20\x20\x20\x20}\n\
         \x20\x20\x20\x20return cls(**kwargs)\n\n",
    );

    s.push_str("# chi wire value -> body view class.\n");
    s.push_str("TONE_VIEWS: dict[str, type] = {\n");
    for t in &spec.types {
        if !names_body_view(&t.name) {
            continue;
        }
        s.push_str(&format!("    \"{}\": {},\n", body_chi(&t.name), t.name));
    }
    s.push_str("}\n");
    write_out(output, &s)
}

fn py_repr(r: &Repr, optional: bool) -> String {
    let base = match r {
        Repr::Str => "str",
        Repr::Int { .. } => "int",
        Repr::Bool => "bool",
        Repr::Json => "Any",
        Repr::Map(inner) => {
            return map_opt(optional, &format!("dict[str, {}]", py_repr(inner, false)));
        }
        Repr::Arr(inner) => return map_opt(optional, &format!("list[{}]", py_repr(inner, false))),
        Repr::Ref(name) => name,
    };
    if optional {
        format!("Optional[{base}]")
    } else {
        base.to_string()
    }
}

/// Rust snake field name → valid Python identifier. `from` collides with
/// the `from` keyword (gossip-publish body).
fn py_field_name(rust: &str) -> String {
    match rust {
        "from" => "from_".to_string(),
        other => other.to_string(),
    }
}

/// Optional-list/map annotations still need `Optional` even though `None`
/// is a legal value: `list[str] = None` fails type checkers.
fn map_opt(optional: bool, inner: &str) -> String {
    if optional {
        format!("Optional[{inner}]")
    } else {
        inner.to_string()
    }
}

/// Emit `protocol.go` to `output`.
pub fn emit_protocol_go(spec: &ProtocolSpec, output: &Path) -> Result<()> {
    let mut s = String::new();
    s.push_str(&regen_header(
        "//",
        &[paths::VIEWS_RS_REL, paths::ENVELOPE_RS_REL],
    ));
    s.push_str("package thrum\n\n");
    s.push_str("import \"encoding/json\"\n\n");

    for t in &spec.types {
        s.push_str(&format!("type {} struct {{\n", t.name));
        if t.fields.is_empty() {
            s.push_str("}\n\n");
            continue;
        }
        for f in &t.fields {
            let tag = if f.optional {
                format!("json:\"{},omitempty\"", f.json)
            } else {
                format!("json:\"{}\"", f.json)
            };
            s.push_str(&format!(
                "\t{} {} `{}`\n",
                snake_to_pascal(&f.rust),
                go_repr(&f.repr, f.optional),
                tag
            ));
        }
        s.push_str("}\n\n");
    }

    s.push_str("// ToneViews maps each chi wire value to its body view type.\n");
    s.push_str("var ToneViews = map[Chi]any{\n");
    for t in &spec.types {
        if !names_body_view(&t.name) {
            continue;
        }
        s.push_str(&format!("\tChi{}: {}{{}},\n", pascal_of(&t.name), t.name));
    }
    s.push_str("}\n");
    write_out(output, &s)
}

fn pascal_of(name: &str) -> String {
    // HelloBody → HelloBody (already Pascal); strip impossible suffix.
    name.strip_suffix("Body")
        .map(|p| p.to_string())
        .unwrap_or_else(|| name.to_string())
}

fn go_repr(r: &Repr, optional: bool) -> String {
    let base = match r {
        Repr::Str => "string".to_string(),
        Repr::Int { unsigned } => {
            if *unsigned {
                "uint64".into()
            } else {
                "int64".into()
            }
        }
        Repr::Bool => "bool".to_string(),
        Repr::Json => "json.RawMessage".to_string(),
        Repr::Map(inner) => format!("map[string]{}", go_repr(inner, false)),
        Repr::Arr(inner) => format!("[]{}", go_repr(inner, false)),
        Repr::Ref(name) => name.clone(),
    };
    if optional && matches!(r, Repr::Str | Repr::Int { .. } | Repr::Bool | Repr::Ref(_)) {
        format!("*{base}")
    } else {
        base
    }
}

fn snake_to_pascal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for part in s.split('_') {
        let mut cs = part.chars();
        if let Some(f) = cs.next() {
            out.push(f.to_ascii_uppercase());
        }
        out.push_str(cs.as_str());
    }
    out
}

fn write_out(output: &Path, s: &str) -> Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(output, s).with_context(|| format!("write {}", output.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kebab_of_body() {
        assert_eq!(body_chi("HelloBody"), "hello");
        assert_eq!(body_chi("KadFindNodeRespBody"), "kad-find-node-resp");
        assert_eq!(body_chi("ToolCallBody"), "tool-call");
    }

    #[test]
    fn pascal_field_names() {
        assert_eq!(snake_to_pascal("proto_version"), "ProtoVersion");
        assert_eq!(snake_to_pascal("humd_id"), "HumdId");
        assert_eq!(snake_to_pascal("block_idx"), "BlockIdx");
    }

    #[test]
    fn parses_views_slice() {
        // A tiny slice of the wire spec enough to exercise Option / Vec /
        // BTreeMap / serde rename.
        let src = r#"
            #[derive(Serialize, Deserialize)]
            pub struct ChunkBody {
                #[serde(rename = "chunkType")]
                pub chunk_type: String,
                #[serde(skip_serializing_if = "Option::is_none")]
                pub delta: Option<serde_json::Value>,
                #[serde(skip_serializing_if = "Option::is_none")]
                pub tools: Option<Vec<String>>,
                #[serde(skip_serializing_if = "Option::is_none")]
                pub load: Option<BTreeMap<String, u64>>,
            }
        "#;
        let ts = structs_in(src).unwrap();
        assert_eq!(ts.len(), 1);
        let f = &ts[0].fields;
        assert_eq!(f.len(), 4);
        assert_eq!(f[0].json, "chunkType");
        assert_eq!(f[0].repr.matches(&Repr::Str), true);
        assert_eq!(f[1].optional, true);
        assert!(matches!(f[1].repr, Repr::Json));
        assert!(matches!(f[2].repr, Repr::Arr(_)));
        assert!(matches!(f[3].repr, Repr::Map(_)));
    }

    #[test]
    fn parses_real_views_renames() {
        let spec =
            protocol_spec(&paths::views_rs(), &paths::envelope_rs(), &paths::lib_rs()).unwrap();
        let chunk = spec
            .types
            .iter()
            .find(|t| t.name == "ChunkBody")
            .expect("ChunkBody in spec");
        for f in &chunk.fields {
            match f.rust.as_str() {
                "chunk_type" => assert_eq!(f.json, "chunkType"),
                "block_idx" => assert_eq!(f.json, "blockIdx"),
                "partial_json" => assert_eq!(f.json, "partialJson"),
                "delta" => assert_eq!(f.json, "delta"),
                other => panic!("unexpected ChunkBody field {other}"),
            }
        }
        let prompt = spec.types.iter().find(|t| t.name == "PromptBody").unwrap();
        for f in &prompt.fields {
            if f.rust == "model_id" {
                assert_eq!(f.json, "modelId");
            }
            if f.rust == "system_prompt" {
                assert_eq!(f.json, "systemPrompt");
            }
        }
    }

    impl Repr {
        fn matches(&self, other: &Repr) -> bool {
            std::mem::discriminant(self) == std::mem::discriminant(other)
        }
    }
}
