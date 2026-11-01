//! The views: the envelope and every chi's body, emitted for TS, Python,
//! Go and C from `hum.thrum`'s projection (`Thrum::protocol_spec`).
//!
//! `rust_types` parses `thrum-core/src/views.rs` and `envelope.rs` with
//! `syn` into the same `TypeSpec` shape, so `rust::check` can hold the
//! Rust to the file field by field. Same constraint as the rest of
//! codegen: no dependency on `thrum-core`, only its source text.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use syn::{Field, Fields, Item, ItemStruct, Meta, Type};

use crate::paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repr {
    Str,
    Int {
        unsigned: bool,
    },
    Float,
    Bool,
    Json,
    Map(Box<Repr>),
    Arr(Box<Repr>),
    Ref(String),
}

#[derive(Debug, Clone)]
pub struct FieldSpec {
    pub rust: String,
    pub json: String,
    pub repr: Repr,
    pub optional: bool,
}

#[derive(Debug, Clone)]
pub struct TypeSpec {
    pub name: String,
    pub fields: Vec<FieldSpec>,
}

#[derive(Debug, Clone)]
pub struct ProtocolSpec {
    pub version: String,
    pub types: Vec<TypeSpec>,
}

pub fn rust_types(views_rs: &Path, envelope_rs: &Path) -> Result<Vec<TypeSpec>> {
    let mut types = Vec::new();
    let every_view: fn(&str) -> bool = |_| true;
    let the_envelope: fn(&str) -> bool = |name| name == crate::thrum::ENVELOPE_TYPE;
    for (path, keep) in [(views_rs, every_view), (envelope_rs, the_envelope)] {
        let src = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        for st in structs_in(&src, keep)? {
            types.push(st);
        }
    }
    Ok(types)
}

fn structs_in(src: &str, keep: fn(&str) -> bool) -> Result<Vec<TypeSpec>> {
    let file = syn::parse_file(src).map_err(|e| anyhow::anyhow!("syn parse: {e}"))?;
    let mut out = Vec::new();
    for item in file.items {
        let Item::Struct(s) = item else { continue };
        let name = s.ident.to_string();
        if !keep(&name) {
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

fn simplify(ty: &Type) -> (bool, Repr) {
    if let Type::Path(p) = ty
        && let Some(seg) = p.path.segments.first()
        && seg.ident == "Option"
        && let syn::PathArguments::AngleBracketed(a) = &seg.arguments
        && let Some(syn::GenericArgument::Type(inner)) = a.args.first()
    {
        return (true, simplify(inner).1);
    }
    (false, repr_of(ty))
}

fn repr_of(ty: &Type) -> Repr {
    let Type::Path(p) = ty else {
        return Repr::Json;
    };
    let seg = &p.path.segments[0];
    match seg.ident.to_string().as_str() {
        "String" => Repr::Str,
        "i64" | "i32" | "i16" => Repr::Int { unsigned: false },
        "u64" | "u32" | "u16" => Repr::Int { unsigned: true },
        "f64" | "f32" => Repr::Float,
        "bool" => Repr::Bool,
        "Value" => Repr::Json,
        "Vec" => {
            let inner = angle_ty(&seg.arguments, VEC_VALUE_ARG).unwrap_or(Repr::Json);
            Repr::Arr(Box::new(inner))
        }
        "BTreeMap" => {
            let inner = angle_ty(&seg.arguments, MAP_VALUE_ARG).unwrap_or(Repr::Json);
            Repr::Map(Box::new(inner))
        }
        other => {
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

const VEC_VALUE_ARG: usize = 0;

const MAP_VALUE_ARG: usize = 1;

fn angle_ty(args: &syn::PathArguments, nth: usize) -> Option<Repr> {
    if let syn::PathArguments::AngleBracketed(a) = args
        && let Some(syn::GenericArgument::Type(t)) = a.args.iter().nth(nth) {
            return Some(simplify(t).1);
        }
    None
}

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

fn regen_header(comment: &str) -> String {
    format!(
        "{c} @generated by `cargo run -p codegen` from {thrum} — DO NOT EDIT.\n\
         {c}\n\
         {c} The envelope and every chi's body, as {thrum} declares them. The\n\
         {c} Rust views in thrum-core are held to the same file, so a tone is\n\
         {c} the same shape in every language. Manual regen: `cargo run -p codegen`.\n\n",
        c = comment,
        thrum = paths::HUM_THRUM_REL,
    )
}

pub fn emit_protocol_ts(spec: &ProtocolSpec, output: &Path) -> Result<()> {
    let mut s = String::new();
    s.push_str(&regen_header("//"));
    s.push_str("export type { ChiKind } from \"./chi.ts\";\n");
    s.push_str("import { isValidChi, type ChiKind } from \"./chi.ts\";\n\n");

    for t in &spec.types {
        s.push_str(&format!("export interface {} {{\n", t.name));
        for f in &t.fields {
            let opt = if f.optional { "?" } else { "" };
            s.push_str(&format!("  {}{}: {};\n", f.json, opt, ts_repr(&f.repr)));
        }
        s.push_str("}\n\n");
    }

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
    s.push_str("// A tone: the envelope and its chi's declared body, the same tone\n");
    s.push_str("// whether a bee sends it or hears it. Its rid may be left out; hum's\n");
    s.push_str("// client gives a tone one when it has none, and humd's breath has none.\n");
    s.push_str("export type Tone<C extends keyof ToneViews = keyof ToneViews> = {\n");
    s.push_str("  [K in C]: { chi: K; rid?: string } & Omit<Envelope, \"chi\" | \"rid\"> & ToneViews[K];\n");
    s.push_str("}[C];\n\n");
    s.push_str("export function isTone(x: unknown): x is Tone {\n");
    s.push_str("  if (!x || typeof x !== \"object\") return false;\n");
    s.push_str("  const chi = (x as { chi?: unknown }).chi;\n");
    s.push_str("  return typeof chi === \"string\" && isValidChi(chi);\n");
    s.push_str("}\n");
    write_out(output, &s)
}

fn names_body_view(name: &str) -> bool {
    name.ends_with("Body")
}

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
        Repr::Int { .. } | Repr::Float => "number".into(),
        Repr::Bool => "boolean".into(),
        Repr::Json => "unknown".into(),
        Repr::Map(inner) => format!("Record<string, {}>", ts_repr(inner)),
        Repr::Arr(inner) => format!("{}[]", ts_repr(inner)),
        Repr::Ref(name) if name == "Chi" => "ChiKind".into(),
        Repr::Ref(name) => name.clone(),
    }
}

pub fn emit_protocol_py(spec: &ProtocolSpec, output: &Path) -> Result<()> {
    let mut s = String::new();
    s.push_str(&regen_header("#"));
    s.push_str("from __future__ import annotations\n\n");
    s.push_str("from dataclasses import dataclass\n");
    s.push_str("from typing import Any, ClassVar, Optional\n\n");

    for t in &spec.types {
        s.push_str(&format!("@dataclass\nclass {}:\n", t.name));
        if t.fields.is_empty() {
            s.push_str("    pass\n\n");
            continue;
        }
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
        s.push_str("    # field name -> wire key, as hum.thrum declares it\n");
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
        Repr::Float => "float",
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

fn py_field_name(rust: &str) -> String {
    match rust {
        "from" => "from_".to_string(),
        other => other.to_string(),
    }
}

fn map_opt(optional: bool, inner: &str) -> String {
    if optional {
        format!("Optional[{inner}]")
    } else {
        inner.to_string()
    }
}

pub fn emit_protocol_go(spec: &ProtocolSpec, output: &Path) -> Result<()> {
    let mut s = String::new();
    s.push_str(&regen_header("//"));
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

pub fn emit_protocol_c(spec: &ProtocolSpec, output: &Path) -> Result<()> {
    write_out(output, &render_protocol_c(spec))
}

fn render_protocol_c(spec: &ProtocolSpec) -> String {
    let mut s = String::new();
    s.push_str(&regen_header("//"));
    s.push_str("#ifndef THRUM_PROTOCOL_H\n#define THRUM_PROTOCOL_H\n\n");
    s.push_str("#include <stdbool.h>\n#include <stddef.h>\n\n");
    s.push_str(
        "typedef enum {\n\
         \x20   THRUM_FIELD_STRING,\n\
         \x20   THRUM_FIELD_INT,\n\
         \x20   THRUM_FIELD_UINT,\n\
         \x20   THRUM_FIELD_NUMBER,\n\
         \x20   THRUM_FIELD_BOOL,\n\
         \x20   THRUM_FIELD_ARRAY,\n\
         \x20   THRUM_FIELD_OBJECT,\n\
         \x20   THRUM_FIELD_ANY\n\
         } thrum_field_kind;\n\n\
         typedef struct {\n\
         \x20   const char *json;\n\
         \x20   thrum_field_kind kind;\n\
         \x20   bool optional;\n\
         } thrum_field;\n\n\
         typedef struct {\n\
         \x20   const char *chi;\n\
         \x20   const thrum_field *fields;\n\
         \x20   size_t count;\n\
         } thrum_view;\n\n",
    );

    for t in &spec.types {
        if t.fields.is_empty() {
            continue;
        }
        s.push_str(&format!(
            "static const thrum_field {}[{}] = {{\n",
            c_fields_ident(&t.name),
            t.fields.len()
        ));
        for f in &t.fields {
            s.push_str(&format!(
                "    {{\"{}\", {}, {}}},\n",
                f.json,
                c_kind(&f.repr, spec),
                f.optional
            ));
        }
        s.push_str("};\n\n");
    }

    if let Some(env) = spec.types.iter().find(|t| t.name == crate::thrum::ENVELOPE_TYPE) {
        s.push_str(&format!(
            "static const thrum_view THRUM_ENVELOPE = {{NULL, {}, {}}};\n\n",
            c_fields_ref(env),
            env.fields.len()
        ));
    }

    let views: Vec<&TypeSpec> = spec.types.iter().filter(|t| names_body_view(&t.name)).collect();
    s.push_str(&format!("#define THRUM_VIEW_COUNT {}\n\n", views.len()));
    s.push_str("static const thrum_view THRUM_VIEWS[THRUM_VIEW_COUNT] = {\n");
    for t in &views {
        s.push_str(&format!(
            "    {{\"{}\", {}, {}}},\n",
            body_chi(&t.name),
            c_fields_ref(t),
            t.fields.len()
        ));
    }
    s.push_str("};\n\n#endif\n");
    s
}

fn c_fields_ident(name: &str) -> String {
    format!("THRUM_FIELDS_{}", crate::pascal_to_screaming_snake(name))
}

fn c_fields_ref(t: &TypeSpec) -> String {
    if t.fields.is_empty() {
        "NULL".into()
    } else {
        c_fields_ident(&t.name)
    }
}

fn c_kind(r: &Repr, spec: &ProtocolSpec) -> &'static str {
    match r {
        Repr::Str => "THRUM_FIELD_STRING",
        Repr::Int { unsigned: false } => "THRUM_FIELD_INT",
        Repr::Int { unsigned: true } => "THRUM_FIELD_UINT",
        Repr::Float => "THRUM_FIELD_NUMBER",
        Repr::Bool => "THRUM_FIELD_BOOL",
        Repr::Json => "THRUM_FIELD_ANY",
        Repr::Map(_) => "THRUM_FIELD_OBJECT",
        Repr::Arr(_) => "THRUM_FIELD_ARRAY",
        Repr::Ref(name) if name == "Chi" => "THRUM_FIELD_STRING",
        Repr::Ref(name) if name == "Map" || spec.types.iter().any(|t| &t.name == name) => {
            "THRUM_FIELD_OBJECT"
        }
        Repr::Ref(_) => "THRUM_FIELD_ANY",
    }
}

fn pascal_of(name: &str) -> String {
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
        Repr::Float => "float64".to_string(),
        Repr::Bool => "bool".to_string(),
        Repr::Json => "json.RawMessage".to_string(),
        Repr::Map(inner) => format!("map[string]{}", go_repr(inner, false)),
        Repr::Arr(inner) => format!("[]{}", go_repr(inner, false)),
        Repr::Ref(name) => name.clone(),
    };
    if optional && matches!(r, Repr::Str | Repr::Int { .. } | Repr::Float | Repr::Bool | Repr::Ref(_)) {
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
        assert_eq!(body_chi("TendrilReachBody"), "tendril-reach");
    }

    #[test]
    fn pascal_field_names() {
        assert_eq!(snake_to_pascal("proto_version"), "ProtoVersion");
        assert_eq!(snake_to_pascal("humd_id"), "HumdId");
        assert_eq!(snake_to_pascal("block_idx"), "BlockIdx");
    }

    #[test]
    fn parses_views_slice() {
        let src = r#"
            #[derive(Serialize, Deserialize)]
            pub struct UnfurlBody {
                #[serde(rename = "callId")]
                pub call_id: String,
                #[serde(skip_serializing_if = "Option::is_none")]
                pub grain: Option<serde_json::Value>,
                #[serde(skip_serializing_if = "Option::is_none")]
                pub tools: Option<Vec<String>>,
                #[serde(skip_serializing_if = "Option::is_none")]
                pub load: Option<BTreeMap<String, u64>>,
                pub temperature: f64,
            }
        "#;
        let ts = structs_in(src, |_| true).unwrap();
        assert_eq!(ts.len(), 1);
        let f = &ts[0].fields;
        assert_eq!(f.len(), 5);
        assert_eq!(f[0].json, "callId");
        assert!(f[0].repr.matches(&Repr::Str));
        assert!(f[1].optional);
        assert!(matches!(f[1].repr, Repr::Json));
        assert!(matches!(f[2].repr, Repr::Arr(_)));
        assert!(matches!(f[3].repr, Repr::Map(_)));
        assert!(matches!(f[4].repr, Repr::Float));
    }

    #[test]
    fn the_c_views_carry_every_declared_key() {
        let spec = crate::thrum::parse(&paths::hum_thrum()).unwrap().protocol_spec();
        let c = render_protocol_c(&spec);
        assert!(c.contains("{\"kind\", THRUM_FIELD_STRING, false},"));
        assert!(c.contains("{\"temperature\", THRUM_FIELD_NUMBER, true},"));
        assert!(c.contains("{\"signedAt\", THRUM_FIELD_INT, false},"));
        assert!(c.contains("{\"chi\", THRUM_FIELD_STRING, false},"));
        assert!(c.contains("{\"ext\", THRUM_FIELD_OBJECT, true},"));
        assert!(c.contains("{\"cancel\", NULL, 0},"));
        assert!(c.contains("{\"hello\", THRUM_FIELDS_HELLO_BODY, "));
        let views = spec.types.iter().filter(|t| names_body_view(&t.name)).count();
        assert!(c.contains(&format!("#define THRUM_VIEW_COUNT {views}\n")));
    }

    #[test]
    fn parses_real_views_renames() {
        let types = rust_types(&paths::views_rs(), &paths::envelope_rs()).unwrap();
        let unfurl = types
            .iter()
            .find(|t| t.name == "UnfurlBody")
            .expect("UnfurlBody in views.rs");
        for f in &unfurl.fields {
            if f.rust == "call_id" {
                assert_eq!(f.json, "callId");
            }
        }
        assert!(unfurl.fields.iter().any(|f| f.json == "sap"));
        let prompt = types.iter().find(|t| t.name == "PromptBody").unwrap();
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
