//! WIRE.md's generated regions, read out of the thrum as Markdown.
//!
//! WIRE.md is prose, hand-written, with regions between
//! `<!-- thrum: NAME -->` and `<!-- /thrum -->` that codegen fills from
//! `hum.thrum`: the envelope, the range, the roles, each chi, the
//! shapes, the kinds, the patterns and the advisories. `fill` rewrites
//! what lies between the markers and leaves every other line as it was,
//! so the prose stays the document's and the tables stay the file's.
//! Every region must appear exactly once, so nothing the file declares
//! goes unsaid, and `--check` holds the document to the file.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::paths;
use crate::thrum::{BEE, Field, Thrum, Ty};

pub const REGIONS: &[&str] = &[
    "envelope",
    "range",
    "roles",
    "chis",
    "shapes",
    "kinds",
    "patterns",
    "advisories",
];

pub fn emit(thrum: &Thrum, source: &Path, output: &Path) -> Result<()> {
    let src = crate::read(source)?;
    let filled = fill(&src, thrum).with_context(|| paths::WIRE_MD_REL.to_string())?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(output, filled).with_context(|| format!("write {}", output.display()))
}

pub fn fill(wire_md: &str, thrum: &Thrum) -> Result<String> {
    let mut out = String::with_capacity(wire_md.len() + 16 * 1024);
    let mut open: Option<&str> = None;
    let mut seen: Vec<&str> = Vec::new();
    for (i, line) in wire_md.lines().enumerate() {
        let n = i + 1;
        if let Some(name) = opens(line) {
            if let Some(inside) = open {
                bail!("line {n}: region `{name}` opens inside `{inside}`");
            }
            let Some(region) = REGIONS.iter().copied().find(|r| *r == name) else {
                bail!(
                    "line {n}: no region `{name}`; the regions are {}",
                    REGIONS.join(", ")
                );
            };
            if seen.contains(&region) {
                bail!("line {n}: region `{name}` appears twice");
            }
            seen.push(region);
            open = Some(region);
            out.push_str(line);
            out.push('\n');
            out.push_str(&render(region, thrum));
            continue;
        }
        if closes(line) {
            if open.is_none() {
                bail!("line {n}: a region closes that never opened");
            }
            open = None;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if open.is_some() {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if let Some(name) = open {
        bail!("region `{name}` never closes");
    }
    for region in REGIONS {
        if !seen.contains(region) {
            bail!(
                "no region `{region}`: put `<!-- thrum: {region} -->` and `<!-- /thrum -->` where it belongs"
            );
        }
    }
    Ok(out)
}

fn opens(line: &str) -> Option<&str> {
    let t = line.trim();
    if !t.ends_with("-->") {
        return None;
    }
    let rest = t.strip_prefix("<!--")?.trim_start().strip_prefix("thrum:")?.trim_start();
    let name = rest.split_whitespace().next()?;
    if name == "-->" { None } else { Some(name) }
}

fn closes(line: &str) -> bool {
    line.trim() == "<!-- /thrum -->"
}

fn render(region: &str, t: &Thrum) -> String {
    let body = match region {
        "envelope" => envelope(t),
        "range" => range(t),
        "roles" => roles(t),
        "chis" => chis(t),
        "shapes" => shapes(t),
        "kinds" => kinds(t),
        "patterns" => patterns(t),
        "advisories" => advisories(t),
        _ => String::new(),
    };
    format!("\n{body}\n")
}

fn envelope(t: &Thrum) -> String {
    field_table("key", &t.envelope)
}

fn range(t: &Thrum) -> String {
    let mut s = String::from("| chi | sounded by | heard by | crosses humds | meaning |\n|---|---|---|---|---|\n");
    for c in &t.chis {
        let crosses = if c.carries.iter().any(|d| d == "ensemble") { "yes" } else { "" };
        s.push_str(&format!(
            "| {} | {} | {} | {crosses} | {} |\n",
            link(&c.name),
            parties(t, &c.emits, true),
            parties(t, &c.hears, true),
            cell(&c.doc)
        ));
    }
    s
}

fn roles(t: &Thrum) -> String {
    let mut s = String::from("| role | does | sounds | hears |\n|---|---|---|---|\n");
    for role in &t.roles() {
        let doc = t
            .kind(crate::thrum::ROLE_KIND)
            .ok()
            .and_then(|k| k.values.iter().find(|v| &v.name == role))
            .map(|v| v.doc.clone())
            .unwrap_or_default();
        let sounds: Vec<String> = t.chis.iter().filter(|c| c.emits.contains(role)).map(|c| link(&c.name)).collect();
        let hears: Vec<String> = t.chis.iter().filter(|c| c.hears.contains(role)).map(|c| link(&c.name)).collect();
        s.push_str(&format!(
            "| **{role}** | {} | {} | {} |\n",
            cell(&doc),
            sounds.join(", "),
            hears.join(", ")
        ));
    }
    s
}

fn chis(t: &Thrum) -> String {
    let mut s = String::new();
    for c in &t.chis {
        s.push_str(&format!("#### `{}`\n\n{}\n\n", c.name, c.doc));
        s.push_str(&format!(
            "`EMITS {}` · `HEARS {}`",
            parties(t, &c.emits, false),
            parties(t, &c.hears, false)
        ));
        for d in &c.carries {
            s.push_str(&format!(" · `CARRIES {d}`"));
        }
        s.push_str("\n\n");
        if c.fields.is_empty() {
            s.push_str("No body.\n\n");
        } else {
            s.push_str(&field_table("field", &c.fields));
            s.push('\n');
        }
    }
    s.trim_end().to_string() + "\n"
}

fn shapes(t: &Thrum) -> String {
    let mut s = String::new();
    for shape in &t.shapes {
        s.push_str(&format!("#### `{}`\n\n", shape.name));
        if !shape.doc.is_empty() {
            s.push_str(&format!("{}\n\n", shape.doc));
        }
        s.push_str(&field_table("field", &shape.fields));
        s.push('\n');
    }
    s.trim_end().to_string() + "\n"
}

fn kinds(t: &Thrum) -> String {
    let mut s = String::new();
    for kind in &t.kinds {
        s.push_str(&format!("#### `{}`\n\n", kind.name));
        if !kind.doc.is_empty() {
            s.push_str(&format!("{}\n\n", kind.doc));
        }
        s.push_str("| value | meaning |\n|---|---|\n");
        for v in &kind.values {
            s.push_str(&format!("| `{}` | {} |\n", v.name, cell(&v.doc)));
        }
        s.push('\n');
    }
    s.trim_end().to_string() + "\n"
}

fn patterns(t: &Thrum) -> String {
    let mut s = String::from(
        "| pattern | key | opens with | grows by | closes with | meaning |\n|---|---|---|---|---|---|\n",
    );
    for p in &t.patterns {
        let key = match &p.key {
            Some(k) => format!("`{k}`"),
            None => "the connection".to_string(),
        };
        s.push_str(&format!(
            "| **{}** | {key} | {} | {} | {} | {} |\n",
            p.name,
            codes(&p.open),
            codes(&p.steps),
            codes(&p.closes),
            cell(&p.doc)
        ));
    }
    s
}

fn advisories(t: &Thrum) -> String {
    let mut s = String::from("| code | raised by | meaning |\n|---|---|---|\n");
    for a in &t.advisories {
        s.push_str(&format!("| `{}` | {} | {} |\n", a.name, a.by.join(", "), cell(&a.doc)));
    }
    s
}

fn field_table(first: &str, fields: &[Field]) -> String {
    let mut s = format!("| {first} | type | meaning |\n|---|---|---|\n");
    for f in fields {
        s.push_str(&format!(
            "| `{}` | `{}` | {} |\n",
            f.key,
            ty_text(&f.ty, f.optional),
            cell(&f.doc)
        ));
    }
    s
}

pub fn ty_text(ty: &Ty, optional: bool) -> String {
    let base = match ty {
        Ty::Str => "string".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::Int => "int".to_string(),
        Ty::Uint => "uint".to_string(),
        Ty::Float => "float".to_string(),
        Ty::Any => "any".to_string(),
        Ty::List(inner) => format!("[{}]", ty_text(inner, false)),
        Ty::Map(inner) => format!("{{{}}}", ty_text(inner, false)),
        Ty::Kind(name) | Ty::Shape(name) => name.clone(),
        Ty::Chi => "chi".to_string(),
    };
    if optional { format!("{base}?") } else { base }
}

fn parties(t: &Thrum, who: &[String], prose: bool) -> String {
    let roles = t.roles();
    let every_role = !roles.is_empty() && roles.iter().all(|r| who.contains(r));
    let mut words: Vec<String> = Vec::new();
    if every_role {
        words.push(if prose { "any bee".to_string() } else { BEE.to_string() });
    }
    for w in who {
        if !(every_role && roles.contains(w)) {
            words.push(w.clone());
        }
    }
    words.join(if prose { ", " } else { " " })
}

fn link(chi: &str) -> String {
    format!("[`{chi}`](#{chi})")
}

fn codes(names: &[String]) -> String {
    names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")
}

fn cell(doc: &str) -> String {
    doc.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> Thrum {
        crate::thrum::parse_str(
            "THRUM 1.0.0\n\
             ENVELOPE\nFIELD chi chi\nDOC which\nFIELD rid string\nFIELD sid string?\n\
             CHI hello\nDOC announce self\nEMITS bee\nHEARS humd\nFIELD role role\n\
             CHI prompt\nDOC open a bloom\nEMITS forager\nHEARS nurse\nCARRIES ensemble\nFIELD content [petal]?\nDOC the petals\n\
             CHI cancel\nDOC stop it\nEMITS forager\nHEARS nurse\n\
             SHAPE petal\nDOC one unit\nFIELD kind petal-kind\n\
             KIND role\nVALUE forager\nDOC opens\nVALUE nurse\nDOC grows\n\
             KIND petal-kind\nVALUE text\nDOC words\n\
             PATTERN bloom\nDOC a turn\nKEY sid\nOPEN prompt\nCLOSE cancel\n\
             ADVISORY unknown-chi\nBY client\nDOC no such chi\n",
        )
        .unwrap()
    }

    const DOC: &str = "# spec\n\nprose\n\n<!-- thrum: envelope -->\nold\n<!-- /thrum -->\n\
        <!-- thrum: range -->\n<!-- /thrum -->\n<!-- thrum: roles -->\n<!-- /thrum -->\n\
        <!-- thrum: chis -->\n<!-- /thrum -->\n<!-- thrum: shapes -->\n<!-- /thrum -->\n\
        <!-- thrum: kinds -->\n<!-- /thrum -->\n<!-- thrum: patterns -->\n<!-- /thrum -->\n\
        <!-- thrum: advisories -->\n<!-- /thrum -->\n\nafter\n";

    #[test]
    fn the_regions_are_filled_and_the_prose_is_kept() {
        let out = fill(DOC, &small()).unwrap();
        assert!(out.starts_with("# spec\n\nprose\n\n<!-- thrum: envelope -->\n\n| key | type | meaning |\n"));
        assert!(!out.contains("\nold\n"));
        assert!(out.contains("| `chi` | `chi` | which |\n"));
        assert!(out.contains("| `sid` | `string?` |  |\n"));
        assert!(out.contains("| [`prompt`](#prompt) | forager | nurse | yes | open a bloom |\n"));
        assert!(out.contains("| [`hello`](#hello) | any bee | humd |  | announce self |\n"));
        assert!(out.contains("| **forager** | opens | [`hello`](#hello), [`prompt`](#prompt), [`cancel`](#cancel) |  |\n"));
        assert!(out.contains("#### `prompt`\n\nopen a bloom\n\n`EMITS forager` · `HEARS nurse` · `CARRIES ensemble`\n\n| field | type | meaning |\n|---|---|---|\n| `content` | `[petal]?` | the petals |\n"));
        assert!(out.contains("#### `hello`\n\nannounce self\n\n`EMITS bee` · `HEARS humd`\n\n| field |"));
        assert!(out.contains("#### `cancel`\n\nstop it\n\n`EMITS forager` · `HEARS nurse`\n\nNo body.\n"));
        assert!(out.contains("#### `petal`\n\none unit\n\n| field | type | meaning |\n|---|---|---|\n| `kind` | `petal-kind` |  |\n"));
        assert!(out.contains("#### `role`\n\n| value | meaning |\n|---|---|\n| `forager` | opens |\n| `nurse` | grows |\n"));
        assert!(out.contains("| **bloom** | `sid` | `prompt` |  | `cancel` | a turn |\n"));
        assert!(out.contains("| `unknown-chi` | client | no such chi |\n"));
        assert!(out.ends_with("<!-- /thrum -->\n\nafter\n"));
        assert_eq!(fill(&out, &small()).unwrap(), out, "filling twice is filling once");
    }

    #[test]
    fn every_region_must_be_there_once_and_whole() {
        let t = small();
        let err = |doc: &str| fill(doc, &t).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err(&DOC.replace("<!-- thrum: kinds -->\n<!-- /thrum -->\n", "")).contains("no region `kinds`"));
        assert!(err(&DOC.replace("thrum: roles", "thrum: rolls")).contains("no region `rolls`"));
        assert!(err(&format!("{DOC}<!-- thrum: range -->\n<!-- /thrum -->\n")).contains("appears twice"));
        assert!(err(&DOC.replace("old\n<!-- /thrum -->\n", "old\n")).contains("opens inside `envelope`"));
        assert!(err(&format!("<!-- /thrum -->\n{DOC}")).contains("never opened"));
        assert!(err(&DOC.replace("<!-- thrum: advisories -->\n<!-- /thrum -->\n", "<!-- thrum: advisories -->\n")).contains("never closes"));
    }

    #[test]
    fn the_spec_itself_fills() {
        let thrum = crate::thrum::parse(&paths::hum_thrum()).unwrap();
        let src = crate::read(&paths::wire_md()).unwrap();
        let out = fill(&src, &thrum).unwrap();
        assert!(out.contains("#### `prompt`"));
        assert!(out.contains("| **nurse** |"));
    }
}
