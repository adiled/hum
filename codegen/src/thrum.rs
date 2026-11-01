//! The thrum, read from `hum.thrum`.
//!
//! One declaration per line, KEYWORD first; a DOC line documents the
//! line above it; a line whose first character is `#` is a comment. The
//! grammar is the file's own header. What comes out is the whole
//! definition: the envelope, the range of chis with who sounds and hears
//! each, the shapes and kinds their bodies carry, the patterns tones
//! make, and the advisories. Everything codegen emits is a projection
//! of it: [`Thrum::chi_spec`] for the registries, [`Thrum::protocol_spec`]
//! for the views, [`Thrum::advisories`] for the link's codes.
//!
//! The parser keeps each block raw until the file ends, since a field's
//! type may name a kind or a shape declared below it. `finish` resolves
//! every name, checks every rule the file states, and names the item at
//! fault.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};

use crate::link::Advisory;
use crate::paths;
use crate::protocol::{FieldSpec, ProtocolSpec, Repr, TypeSpec};
use crate::{ChiSpec, Variant};

pub const HUMD: &str = "humd";
pub const BEE: &str = "bee";
pub const CLIENT: &str = "client";
pub const ENSEMBLE: &str = "ensemble";
pub const ROLE_KIND: &str = "role";
pub const PETAL_KIND: &str = "petal-kind";
pub const PULSE_KIND: &str = "pulse-kind";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Str,
    Bool,
    Int,
    Uint,
    Float,
    Any,
    List(Box<Ty>),
    Map(Box<Ty>),
    Kind(String),
    Shape(String),
    Chi,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub key: String,
    pub ty: Ty,
    pub optional: bool,
    pub doc: String,
}

#[derive(Debug, Clone)]
pub struct ChiDef {
    pub name: String,
    pub doc: String,
    pub emits: Vec<String>,
    pub hears: Vec<String>,
    pub carries: Vec<String>,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Shape {
    pub name: String,
    pub doc: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Default)]
pub struct KindValue {
    pub name: String,
    pub doc: String,
}

#[derive(Debug, Clone)]
pub struct Kind {
    pub name: String,
    pub doc: String,
    pub values: Vec<KindValue>,
}

#[derive(Debug, Clone)]
pub struct Pattern {
    pub name: String,
    pub doc: String,
    pub key: Option<String>,
    pub open: Vec<String>,
    pub steps: Vec<String>,
    pub closes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AdvisoryDef {
    pub name: String,
    pub doc: String,
    pub by: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Thrum {
    pub version: String,
    pub envelope: Vec<Field>,
    pub chis: Vec<ChiDef>,
    pub shapes: Vec<Shape>,
    pub kinds: Vec<Kind>,
    pub patterns: Vec<Pattern>,
    pub advisories: Vec<AdvisoryDef>,
}

pub fn parse(path: &Path) -> Result<Thrum> {
    let src = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    parse_str(&src).with_context(|| paths::HUM_THRUM_REL.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Envelope,
    Chi,
    Shape,
    Kind,
    Pattern,
    Advisory,
}

impl Block {
    fn keyword(self) -> &'static str {
        match self {
            Block::Envelope => "ENVELOPE",
            Block::Chi => "CHI",
            Block::Shape => "SHAPE",
            Block::Kind => "KIND",
            Block::Pattern => "PATTERN",
            Block::Advisory => "ADVISORY",
        }
    }

    fn of(keyword: &str) -> Option<Block> {
        [
            Block::Envelope,
            Block::Chi,
            Block::Shape,
            Block::Kind,
            Block::Pattern,
            Block::Advisory,
        ]
        .into_iter()
        .find(|b| b.keyword() == keyword)
    }

    fn takes(self, property: &str) -> bool {
        match self {
            Block::Envelope | Block::Shape => property == "FIELD",
            Block::Chi => matches!(property, "FIELD" | "EMITS" | "HEARS" | "CARRIES"),
            Block::Kind => property == "VALUE",
            Block::Pattern => matches!(property, "KEY" | "OPEN" | "STEP" | "CLOSE"),
            Block::Advisory => property == "BY",
        }
    }
}

#[derive(Clone, Copy)]
enum Last {
    Block,
    Field,
    Value,
}

struct RawField {
    key: String,
    ty: String,
    doc: String,
}

struct Raw {
    block: Block,
    line: usize,
    name: String,
    doc: String,
    fields: Vec<RawField>,
    values: Vec<KindValue>,
    words: Vec<(&'static str, Vec<String>)>,
}

impl Raw {
    fn words(&self, property: &str) -> Option<&[String]> {
        self.words
            .iter()
            .find(|(p, _)| *p == property)
            .map(|(_, w)| w.as_slice())
    }

    fn at(&self) -> String {
        if self.name.is_empty() {
            format!("{} (line {})", self.block.keyword(), self.line)
        } else {
            format!("{} `{}` (line {})", self.block.keyword(), self.name, self.line)
        }
    }
}

const PROPERTIES: &[&str] = &[
    "FIELD", "VALUE", "EMITS", "HEARS", "CARRIES", "KEY", "OPEN", "STEP", "CLOSE", "BY",
];

pub fn parse_str(src: &str) -> Result<Thrum> {
    let mut version: Option<String> = None;
    let mut blocks: Vec<Raw> = Vec::new();
    let mut last = Last::Block;
    for (i, raw_line) in src.lines().enumerate() {
        let n = i + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (keyword, rest) = match line.split_once(char::is_whitespace) {
            Some((k, r)) => (k, r.trim()),
            None => (line, ""),
        };
        if keyword == "THRUM" {
            if version.is_some() {
                bail!("line {n}: THRUM declared twice");
            }
            if !blocks.is_empty() {
                bail!("line {n}: THRUM must come before any block");
            }
            if rest.split_whitespace().count() != 1 {
                bail!("line {n}: THRUM takes one version");
            }
            version = Some(rest.to_string());
            continue;
        }
        if version.is_none() {
            bail!("line {n}: the file must open with THRUM <version>");
        }
        if let Some(block) = Block::of(keyword) {
            let name = if block == Block::Envelope {
                if !rest.is_empty() {
                    bail!("line {n}: ENVELOPE takes no name");
                }
                String::new()
            } else {
                one_word(n, keyword, rest)?
            };
            blocks.push(Raw {
                block,
                line: n,
                name,
                doc: String::new(),
                fields: Vec::new(),
                values: Vec::new(),
                words: Vec::new(),
            });
            last = Last::Block;
            continue;
        }
        let Some(current) = blocks.last_mut() else {
            bail!("line {n}: {keyword} before any block");
        };
        if keyword == "DOC" {
            if rest.is_empty() {
                bail!("line {n}: DOC says nothing");
            }
            let doc = match last {
                Last::Block => &mut current.doc,
                Last::Field => &mut current.fields.last_mut().expect("a field was read").doc,
                Last::Value => &mut current.values.last_mut().expect("a value was read").doc,
            };
            if !doc.is_empty() {
                doc.push(' ');
            }
            doc.push_str(rest);
            continue;
        }
        let Some(property) = PROPERTIES.iter().copied().find(|p| *p == keyword) else {
            bail!("line {n}: unknown keyword {keyword}");
        };
        if !current.block.takes(property) {
            bail!(
                "line {n}: {property} does not belong in {}",
                current.block.keyword()
            );
        }
        match property {
            "FIELD" => {
                let Some((key, ty)) = rest.split_once(char::is_whitespace) else {
                    bail!("line {n}: FIELD takes a key and a type");
                };
                let ty = ty.trim();
                if ty.split_whitespace().count() != 1 {
                    bail!("line {n}: FIELD takes a key and one type");
                }
                current.fields.push(RawField {
                    key: key.to_string(),
                    ty: ty.to_string(),
                    doc: String::new(),
                });
                last = Last::Field;
            }
            "VALUE" => {
                current.values.push(KindValue {
                    name: one_word(n, keyword, rest)?,
                    doc: String::new(),
                });
                last = Last::Value;
            }
            _ => {
                if current.words(property).is_some() {
                    bail!("line {n}: {property} given twice");
                }
                let words: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
                if words.is_empty() {
                    bail!("line {n}: {property} names nothing");
                }
                if property == "KEY" && words.len() != 1 {
                    bail!("line {n}: KEY takes one field");
                }
                current.words.push((property, words));
                last = Last::Block;
            }
        }
    }
    finish(version.ok_or_else(|| anyhow!("no THRUM line"))?, blocks)
}

fn one_word(n: usize, keyword: &str, rest: &str) -> Result<String> {
    if rest.is_empty() || rest.split_whitespace().count() != 1 {
        bail!("line {n}: {keyword} takes one name");
    }
    Ok(rest.to_string())
}

fn finish(version: String, blocks: Vec<Raw>) -> Result<Thrum> {
    let mut kinds: Vec<Kind> = Vec::new();
    let mut kind_names = BTreeSet::new();
    for raw in blocks.iter().filter(|r| r.block == Block::Kind) {
        kebab(&raw.at(), &raw.name)?;
        if !kind_names.insert(raw.name.clone()) {
            bail!("{} declared twice", raw.at());
        }
        if raw.values.is_empty() {
            bail!("{} has no VALUE", raw.at());
        }
        let mut seen = BTreeSet::new();
        for v in &raw.values {
            kebab(&raw.at(), &v.name)?;
            if !seen.insert(v.name.clone()) {
                bail!("{} lists `{}` twice", raw.at(), v.name);
            }
            if v.doc.is_empty() {
                bail!("{} says nothing of `{}`: give it a DOC", raw.at(), v.name);
            }
        }
        kinds.push(Kind {
            name: raw.name.clone(),
            doc: raw.doc.clone(),
            values: raw.values.clone(),
        });
    }

    let mut shape_names = BTreeSet::new();
    for raw in blocks.iter().filter(|r| r.block == Block::Shape) {
        kebab(&raw.at(), &raw.name)?;
        if kind_names.contains(&raw.name) {
            bail!("{} is also a KIND", raw.at());
        }
        if !shape_names.insert(raw.name.clone()) {
            bail!("{} declared twice", raw.at());
        }
    }
    for name in kind_names.iter().chain(shape_names.iter()) {
        if PRIMITIVES.contains(&name.as_str()) || name == "chi" {
            bail!("`{name}` is a type of the thrum's own and cannot be a KIND or a SHAPE");
        }
    }

    let fields_of = |raw: &Raw| -> Result<Vec<Field>> {
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for f in &raw.fields {
            if f.key.is_empty()
                || !f
                    .key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                bail!("{} has a key that is not a wire key: `{}`", raw.at(), f.key);
            }
            if !seen.insert(f.key.clone()) {
                bail!("{} declares `{}` twice", raw.at(), f.key);
            }
            let (ty, optional) = ty_of(&f.ty, &kind_names, &shape_names)
                .with_context(|| format!("{}, FIELD {}", raw.at(), f.key))?;
            out.push(Field {
                key: f.key.clone(),
                ty,
                optional,
                doc: f.doc.clone(),
            });
        }
        Ok(out)
    };

    let mut envelopes = blocks.iter().filter(|r| r.block == Block::Envelope);
    let Some(envelope_raw) = envelopes.next() else {
        bail!("no ENVELOPE");
    };
    if envelopes.next().is_some() {
        bail!("ENVELOPE declared twice");
    }
    let envelope = fields_of(envelope_raw)?;
    let envelope_keys: BTreeSet<&str> = envelope.iter().map(|f| f.key.as_str()).collect();
    if !envelope.iter().any(|f| f.key == "chi" && f.ty == Ty::Chi && !f.optional) {
        bail!("the ENVELOPE must carry `chi chi`");
    }
    if !envelope.iter().any(|f| f.key == "rid" && f.ty == Ty::Str && !f.optional) {
        bail!("the ENVELOPE must carry `rid string`");
    }

    let mut shapes = Vec::new();
    for raw in blocks.iter().filter(|r| r.block == Block::Shape) {
        shapes.push(Shape {
            name: raw.name.clone(),
            doc: raw.doc.clone(),
            fields: fields_of(raw)?,
        });
    }

    let roles: Vec<String> = kinds
        .iter()
        .find(|k| k.name == ROLE_KIND)
        .map(|k| k.values.iter().map(|v| v.name.clone()).collect())
        .unwrap_or_default();
    let parties = |raw: &Raw, property: &str| -> Result<Vec<String>> {
        let Some(words) = raw.words(property) else {
            bail!("{} has no {property}", raw.at());
        };
        let mut out: Vec<String> = Vec::new();
        for w in words {
            if w == BEE {
                if roles.is_empty() {
                    bail!("{} says {property} bee, but there is no KIND {ROLE_KIND}", raw.at());
                }
                out.extend(roles.iter().cloned());
            } else if w == HUMD || roles.contains(w) {
                out.push(w.clone());
            } else {
                bail!(
                    "{} {property} `{w}`, which is no role, nor {BEE}, nor {HUMD}",
                    raw.at()
                );
            }
        }
        out.dedup();
        Ok(out)
    };

    let mut chis = Vec::new();
    let mut chi_names = BTreeSet::new();
    for raw in blocks.iter().filter(|r| r.block == Block::Chi) {
        kebab(&raw.at(), &raw.name)?;
        if !chi_names.insert(raw.name.clone()) {
            bail!("{} declared twice", raw.at());
        }
        if raw.doc.is_empty() {
            bail!("{} has no DOC", raw.at());
        }
        let carries: Vec<String> = raw.words("CARRIES").unwrap_or(&[]).to_vec();
        for c in &carries {
            if c != ENSEMBLE {
                bail!("{} CARRIES `{c}`; only {ENSEMBLE} is a distance", raw.at());
            }
        }
        chis.push(ChiDef {
            name: raw.name.clone(),
            doc: raw.doc.clone(),
            emits: parties(raw, "EMITS")?,
            hears: parties(raw, "HEARS")?,
            carries,
            fields: fields_of(raw)?,
        });
    }
    if chis.is_empty() {
        bail!("no CHI: the range is empty");
    }

    let mut patterns = Vec::new();
    let mut pattern_names = BTreeSet::new();
    for raw in blocks.iter().filter(|r| r.block == Block::Pattern) {
        kebab(&raw.at(), &raw.name)?;
        if !pattern_names.insert(raw.name.clone()) {
            bail!("{} declared twice", raw.at());
        }
        let chis_named = |property: &str, required: bool| -> Result<Vec<String>> {
            let words = raw.words(property).unwrap_or(&[]);
            if required && words.is_empty() {
                bail!("{} has no {property}", raw.at());
            }
            for w in words {
                if !chi_names.contains(w) {
                    bail!("{} {property} `{w}`, which is no chi", raw.at());
                }
            }
            Ok(words.to_vec())
        };
        let open = chis_named("OPEN", true)?;
        let steps = chis_named("STEP", false)?;
        let closes = chis_named("CLOSE", true)?;
        let key = raw.words("KEY").map(|w| w[0].clone());
        if let Some(k) = &key {
            let in_open = chis
                .iter()
                .filter(|c| open.contains(&c.name))
                .any(|c| c.fields.iter().any(|f| &f.key == k));
            if !envelope_keys.contains(k.as_str()) && !in_open {
                bail!(
                    "{} KEY `{k}`, which is neither of the envelope nor of what opens it",
                    raw.at()
                );
            }
        }
        patterns.push(Pattern {
            name: raw.name.clone(),
            doc: raw.doc.clone(),
            key,
            open,
            steps,
            closes,
        });
    }

    let mut advisories = Vec::new();
    let mut advisory_names = BTreeSet::new();
    for raw in blocks.iter().filter(|r| r.block == Block::Advisory) {
        kebab(&raw.at(), &raw.name)?;
        if !advisory_names.insert(raw.name.clone()) {
            bail!("{} declared twice", raw.at());
        }
        if raw.doc.is_empty() {
            bail!("{} has no DOC", raw.at());
        }
        let Some(by) = raw.words("BY") else {
            bail!("{} has no BY", raw.at());
        };
        for b in by {
            if b != CLIENT && b != HUMD {
                bail!("{} BY `{b}`; an advisory is raised by {CLIENT} or {HUMD}", raw.at());
            }
        }
        advisories.push(AdvisoryDef {
            name: raw.name.clone(),
            doc: raw.doc.clone(),
            by: by.to_vec(),
        });
    }

    Ok(Thrum {
        version,
        envelope,
        chis,
        shapes,
        kinds,
        patterns,
        advisories,
    })
}

const PRIMITIVES: &[&str] = &["string", "bool", "int", "uint", "float", "any"];

fn ty_of(src: &str, kinds: &BTreeSet<String>, shapes: &BTreeSet<String>) -> Result<(Ty, bool)> {
    let (inner, optional) = match src.strip_suffix('?') {
        Some(s) => (s, true),
        None => (src, false),
    };
    Ok((inner_ty(inner, kinds, shapes)?, optional))
}

fn inner_ty(src: &str, kinds: &BTreeSet<String>, shapes: &BTreeSet<String>) -> Result<Ty> {
    if let Some(s) = src.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return Ok(Ty::List(Box::new(inner_ty(s, kinds, shapes)?)));
    }
    if let Some(s) = src.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        return Ok(Ty::Map(Box::new(inner_ty(s, kinds, shapes)?)));
    }
    Ok(match src {
        "string" => Ty::Str,
        "bool" => Ty::Bool,
        "int" => Ty::Int,
        "uint" => Ty::Uint,
        "float" => Ty::Float,
        "any" => Ty::Any,
        "chi" => Ty::Chi,
        "" => bail!("a type is missing"),
        name if kinds.contains(name) => Ty::Kind(name.to_string()),
        name if shapes.contains(name) => Ty::Shape(name.to_string()),
        name => bail!("unknown type `{name}`: no such KIND or SHAPE"),
    })
}

fn kebab(at: &str, name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.ends_with('-')
        && !name.contains("--");
    if !ok {
        bail!("{at}: `{name}` is not kebab-case");
    }
    Ok(())
}

impl Thrum {
    pub fn kind(&self, name: &str) -> Result<&Kind> {
        self.kinds
            .iter()
            .find(|k| k.name == name)
            .ok_or_else(|| anyhow!("{} declares no KIND {name}", paths::HUM_THRUM_REL))
    }

    pub fn roles(&self) -> Vec<String> {
        self.kind(ROLE_KIND)
            .map(|k| k.values.iter().map(|v| v.name.clone()).collect())
            .unwrap_or_default()
    }

    pub fn chi_spec(&self) -> Result<ChiSpec> {
        let variants = |kind: &Kind| -> Vec<Variant> {
            kind.values
                .iter()
                .map(|v| Variant {
                    pascal: pascal(&v.name),
                    wire: v.name.clone(),
                    doc: v.doc.clone(),
                })
                .collect()
        };
        Ok(ChiSpec {
            version: self.version.clone(),
            chi: self
                .chis
                .iter()
                .map(|c| Variant {
                    pascal: pascal(&c.name),
                    wire: c.name.clone(),
                    doc: c.doc.clone(),
                })
                .collect(),
            pulse: variants(self.kind(PULSE_KIND)?),
            petal: variants(self.kind(PETAL_KIND)?),
            role: variants(self.kind(ROLE_KIND)?),
        })
    }

    pub fn protocol_spec(&self) -> ProtocolSpec {
        let mut types = vec![TypeSpec {
            name: ENVELOPE_TYPE.to_string(),
            fields: field_specs(&self.envelope),
        }];
        for c in &self.chis {
            types.push(TypeSpec {
                name: body_name(&c.name),
                fields: field_specs(&c.fields),
            });
        }
        for s in &self.shapes {
            types.push(TypeSpec {
                name: pascal(&s.name),
                fields: field_specs(&s.fields),
            });
        }
        ProtocolSpec {
            version: self.version.clone(),
            types,
        }
    }

    pub fn advisories(&self) -> Vec<Advisory> {
        self.advisories
            .iter()
            .map(|a| Advisory {
                wire: a.name.clone(),
                pascal: pascal(&a.name),
                raised_by: a.by.join(", "),
                doc: a.doc.clone(),
            })
            .collect()
    }
}

pub const ENVELOPE_TYPE: &str = "Envelope";

pub fn body_name(chi: &str) -> String {
    format!("{}Body", pascal(chi))
}

fn field_specs(fields: &[Field]) -> Vec<FieldSpec> {
    fields
        .iter()
        .map(|f| FieldSpec {
            rust: snake(&f.key),
            json: f.key.clone(),
            repr: repr(&f.ty),
            optional: f.optional,
        })
        .collect()
}

fn repr(ty: &Ty) -> Repr {
    match ty {
        Ty::Str | Ty::Kind(_) => Repr::Str,
        Ty::Bool => Repr::Bool,
        Ty::Int => Repr::Int { unsigned: false },
        Ty::Uint => Repr::Int { unsigned: true },
        Ty::Float => Repr::Float,
        Ty::Any => Repr::Json,
        Ty::List(t) => Repr::Arr(Box::new(repr(t))),
        Ty::Map(t) => Repr::Map(Box::new(repr(t))),
        Ty::Shape(name) => Repr::Ref(pascal(name)),
        Ty::Chi => Repr::Ref("Chi".to_string()),
    }
}

pub fn pascal(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for part in name.split(['-', '_']) {
        let mut cs = part.chars();
        if let Some(f) = cs.next() {
            out.push(f.to_ascii_uppercase());
            out.push_str(cs.as_str());
        }
    }
    out
}

pub fn snake(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 4);
    for c in key.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: &str = "\
# a small thrum
THRUM 1.2.3

ENVELOPE
DOC the frame
FIELD chi chi
FIELD rid string
FIELD sid string?
DOC the line

CHI hello
DOC announce self
EMITS bee
HEARS humd
FIELD role role
FIELD headroom headroom?
DOC room

CHI prompt
DOC open a bloom
EMITS forager
HEARS nurse
CARRIES ensemble
FIELD content [petal]?
FIELD seasons {uint}?

SHAPE headroom
FIELD freeSlots uint

SHAPE petal
FIELD kind petal-kind
FIELD text string?

KIND role
VALUE forager
DOC opens
VALUE nurse
DOC grows

KIND petal-kind
VALUE text
DOC words
DOC and more words

KIND pulse-kind
VALUE cell-ready
DOC ready

PATTERN bloom
KEY sid
OPEN prompt
CLOSE prompt

ADVISORY undeclared-key
BY client humd
DOC a stray key
";

    #[test]
    fn a_small_thrum_is_read_whole() {
        let t = parse_str(SMALL).unwrap();
        assert_eq!(t.version, "1.2.3");
        assert_eq!(t.envelope.len(), 3);
        assert_eq!(t.envelope[2].doc, "the line");
        assert!(t.envelope[2].optional);
        let hello = &t.chis[0];
        assert_eq!(hello.emits, ["forager", "nurse"]);
        assert_eq!(hello.hears, ["humd"]);
        assert_eq!(hello.fields[0].ty, Ty::Kind("role".into()));
        assert_eq!(hello.fields[1].ty, Ty::Shape("headroom".into()));
        assert_eq!(hello.fields[1].doc, "room");
        let prompt = &t.chis[1];
        assert_eq!(prompt.carries, ["ensemble"]);
        assert_eq!(prompt.fields[0].ty, Ty::List(Box::new(Ty::Shape("petal".into()))));
        assert_eq!(prompt.fields[1].ty, Ty::Map(Box::new(Ty::Uint)));
        assert_eq!(t.kind("petal-kind").unwrap().values[0].doc, "words and more words");
        assert_eq!(t.patterns[0].key.as_deref(), Some("sid"));
        assert_eq!(t.advisories[0].by, ["client", "humd"]);
    }

    #[test]
    fn the_projections_name_things_as_the_rust_does() {
        let t = parse_str(SMALL).unwrap();
        let chi = t.chi_spec().unwrap();
        assert_eq!(chi.chi[0].pascal, "Hello");
        assert_eq!(chi.petal[0].wire, "text");
        assert_eq!(chi.role.len(), 2);
        let proto = t.protocol_spec();
        let names: Vec<&str> = proto.types.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Envelope", "HelloBody", "PromptBody", "Headroom", "Petal"]);
        let hello = &proto.types[1];
        assert_eq!(hello.fields[0].repr, Repr::Str);
        assert_eq!(hello.fields[1].repr, Repr::Ref("Headroom".into()));
        assert!(hello.fields[1].optional);
        let headroom = &proto.types[3];
        assert_eq!(headroom.fields[0].rust, "free_slots");
        assert_eq!(headroom.fields[0].json, "freeSlots");
        assert_eq!(t.advisories()[0].raised_by, "client, humd");
    }

    #[test]
    fn names_turn_as_each_language_wants() {
        assert_eq!(pascal("kad-find-node-resp"), "KadFindNodeResp");
        assert_eq!(pascal("drone-load"), "DroneLoad");
        assert_eq!(body_name("tendril-reach"), "TendrilReachBody");
        assert_eq!(snake("callId"), "call_id");
        assert_eq!(snake("humd_id"), "humd_id");
        assert_eq!(snake("sentAt"), "sent_at");
        assert_eq!(snake("from"), "from");
    }

    fn fails_with(src: &str, needle: &str) {
        let err = parse_str(src).err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(err.contains(needle), "wanted `{needle}` in `{err}`");
    }

    #[test]
    fn every_fault_is_named() {
        fails_with("ENVELOPE\n", "must open with THRUM");
        fails_with(&SMALL.replace("FIELD content [petal]?", "FIELD content [petals]?"), "unknown type `petals`");
        fails_with(&SMALL.replace("EMITS forager", "EMITS queen"), "`queen`, which is no role");
        fails_with(&SMALL.replace("CARRIES ensemble", "CARRIES chain"), "only ensemble is a distance");
        fails_with(&SMALL.replace("DOC open a bloom\n", ""), "CHI `prompt` (line 19) has no DOC");
        fails_with(&SMALL.replace("KEY sid", "KEY petal"), "neither of the envelope nor of what opens it");
        fails_with(&SMALL.replace("OPEN prompt", "OPEN wilt"), "`wilt`, which is no chi");
        fails_with(&SMALL.replace("BY client humd", "BY bee"), "raised by client or humd");
        fails_with(&SMALL.replace("FIELD rid string", "FIELD rid string?"), "must carry `rid string`");
        fails_with(&SMALL.replace("SHAPE petal", "SHAPE role"), "is also a KIND");
        fails_with(&format!("{SMALL}\nCHI Hello\nDOC x\nEMITS bee\nHEARS humd\n"), "not kebab-case");
        fails_with(&format!("{SMALL}\nWHATEVER x\n"), "unknown keyword WHATEVER");
        fails_with(&SMALL.replace("KIND role\n", "KIND role\nFIELD x string\n"), "FIELD does not belong in KIND");
    }

    #[test]
    fn the_thrum_itself_reads() {
        let t = parse(&paths::hum_thrum()).unwrap();
        assert!(t.chis.iter().any(|c| c.name == "prompt"));
        assert!(t.chis.iter().all(|c| !c.doc.is_empty()));
        let spec = t.chi_spec().unwrap();
        assert_eq!(spec.version, t.version);
        assert_eq!(spec.role.len(), 4);
    }
}
