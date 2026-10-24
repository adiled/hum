//! The client in every language: the link's constants, read from Rust,
//! rendered into each language's client template.
//!
//! A bee in TS, Python, Go or C reaches humd through `hum bee link`
//! (hum-thrum's link), so its client is only the other end of two pipes.
//! What that end must agree on is read here from the Rust that defines
//! it: the link's version, exit codes and `hum` lookup from
//! `hum-thrum/src/link.rs`, the hello fields the client owns from
//! `hum-thrum/src/core.rs`, and the advisory codes from
//! `thrum-core/src/chi.rs` with their meaning from WIRE.md's advisories
//! table, so a code without a row fails the build. Each language's
//! client is a template in `codegen/templates/<lang>/`, protocol-free
//! transport only, and the constants land at its `@constants` line. A
//! template without that line is emitted as it is, under the generated
//! header, so every source file of every client is codegen's.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use syn::{Expr, Item, Lit};

use crate::paths;

#[derive(Debug, Clone)]
pub struct Advisory {
    pub wire: String,
    pub pascal: String,
    pub raised_by: String,
    pub doc: String,
}

#[derive(Debug, Clone)]
pub struct LinkSpec {
    pub version: i64,
    pub thrum_version: String,
    pub exits: Vec<(String, i64)>,
    pub client_owned: Vec<String>,
    pub hum_cli_env: String,
    pub hum_cli_name: String,
    pub hum_cli_default: String,
    pub advisories: Vec<Advisory>,
}

pub fn link_spec(
    link_rs: &Path,
    core_rs: &Path,
    chi_rs: &Path,
    lib_rs: &Path,
    wire_md: &Path,
) -> Result<LinkSpec> {
    let link = parse_rs(link_rs)?;
    let core = parse_rs(core_rs)?;
    let chi_src = fs::read_to_string(chi_rs).with_context(|| format!("read {}", chi_rs.display()))?;
    let lib_src = fs::read_to_string(lib_rs).with_context(|| format!("read {}", lib_rs.display()))?;
    let wire = fs::read_to_string(wire_md).with_context(|| format!("read {}", wire_md.display()))?;

    let exit_mod = link
        .items
        .iter()
        .find_map(|item| match item {
            Item::Mod(m) if m.ident == "exit" => m.content.as_ref().map(|(_, items)| items.clone()),
            _ => None,
        })
        .ok_or_else(|| anyhow!("{}: `pub mod exit` not found", paths::LINK_RS_REL))?;
    let exits: Vec<(String, i64)> = exit_mod
        .iter()
        .filter_map(|item| match item {
            Item::Const(c) => int_of(&c.expr).map(|v| (c.ident.to_string(), v)),
            _ => None,
        })
        .collect();
    if exits.is_empty() {
        bail!("{}: `mod exit` has no codes", paths::LINK_RS_REL);
    }

    let docs = advisory_docs(&wire)?;
    let mut advisories = Vec::new();
    for v in crate::extract_enum(&chi_src, "AdvisoryCode")? {
        let Some((raised_by, doc)) = docs.iter().find(|(w, _, _)| *w == v.wire).map(|(_, r, d)| (r, d)) else {
            bail!("advisory `{}` has no row in {}'s advisories table", v.wire, paths::WIRE_MD_REL);
        };
        advisories.push(Advisory {
            wire: v.wire,
            pascal: v.pascal,
            raised_by: raised_by.clone(),
            doc: doc.clone(),
        });
    }

    Ok(LinkSpec {
        version: const_int(&link, "LINK_VERSION")?,
        thrum_version: crate::extract_version(&lib_src)?,
        exits,
        client_owned: const_strs(&core, "CLIENT_OWNED")?,
        hum_cli_env: const_str(&link, "HUM_CLI_ENV")?,
        hum_cli_name: const_str(&link, "HUM_CLI_NAME")?,
        hum_cli_default: const_str(&link, "HUM_CLI_DEFAULT")?,
        advisories,
    })
}

fn parse_rs(path: &Path) -> Result<syn::File> {
    let src = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    syn::parse_file(&src).with_context(|| format!("parse {}", path.display()))
}

fn const_expr<'a>(file: &'a syn::File, name: &str) -> Result<&'a Expr> {
    file.items
        .iter()
        .find_map(|item| match item {
            Item::Const(c) if c.ident == name => Some(c.expr.as_ref()),
            _ => None,
        })
        .ok_or_else(|| anyhow!("const {name} not found"))
}

fn int_of(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Lit(l) => match &l.lit {
            Lit::Int(i) => i.base10_parse().ok(),
            _ => None,
        },
        _ => None,
    }
}

fn str_of(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(l) => match &l.lit {
            Lit::Str(s) => Some(s.value()),
            _ => None,
        },
        _ => None,
    }
}

fn const_int(file: &syn::File, name: &str) -> Result<i64> {
    int_of(const_expr(file, name)?).ok_or_else(|| anyhow!("const {name} is not an integer"))
}

fn const_str(file: &syn::File, name: &str) -> Result<String> {
    str_of(const_expr(file, name)?).ok_or_else(|| anyhow!("const {name} is not a string"))
}

fn const_strs(file: &syn::File, name: &str) -> Result<Vec<String>> {
    let expr = const_expr(file, name)?;
    let Expr::Reference(r) = expr else { bail!("const {name} is not &[..]") };
    let Expr::Array(a) = r.expr.as_ref() else { bail!("const {name} is not &[..]") };
    a.elems
        .iter()
        .map(|e| str_of(e).ok_or_else(|| anyhow!("const {name} holds a non-string")))
        .collect()
}

fn advisory_docs(wire_md: &str) -> Result<Vec<(String, String, String)>> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in wire_md.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            inside = line == "### Advisories";
            continue;
        }
        if !inside || !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() != 3 {
            continue;
        }
        let Some(code) = cells[0].strip_prefix('`').and_then(|c| c.strip_suffix('`')) else {
            continue;
        };
        out.push((code.to_string(), cells[1].to_string(), cells[2].to_string()));
    }
    if out.is_empty() {
        bail!("no advisories table in {}", paths::WIRE_MD_REL);
    }
    Ok(out)
}

fn header(comment: &str, template: &str) -> String {
    format!(
        "{c} @generated by `cargo run -p codegen` from {t}\n\
         {c} and {link}, {core}, {chi}. DO NOT EDIT.\n\
         {c}\n\
         {c} The client is hum-thrum's, behind `hum bee link`; this file is the\n\
         {c} other end of its pipes. Edit the template or the Rust, then regen.\n\n",
        c = comment,
        t = template,
        link = paths::LINK_RS_REL,
        core = paths::CORE_RS_REL,
        chi = paths::CHI_RS_REL,
    )
}

fn camel(pascal: &str) -> String {
    let mut c = pascal.chars();
    match c.next() {
        Some(f) => f.to_ascii_lowercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

fn screaming(pascal: &str) -> String {
    let mut out = String::new();
    for (i, ch) in pascal.chars().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_uppercase());
    }
    out
}

fn title(screaming: &str) -> String {
    screaming
        .split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_string() + &c.as_str().to_ascii_lowercase(),
                None => String::new(),
            }
        })
        .collect()
}

fn quoted(items: &[String]) -> String {
    items.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", ")
}

pub fn constants_ts(spec: &LinkSpec) -> String {
    let mut s = String::new();
    s.push_str("/** The version of the lines between this client and `hum bee link`. */\n");
    s.push_str(&format!("export const LINK_VERSION = {};\n\n", spec.version));
    s.push_str("/** How `hum bee link` exits. */\nexport const LINK_EXIT = {\n");
    for (name, code) in &spec.exits {
        s.push_str(&format!("  {}: {code},\n", name.to_ascii_lowercase()));
    }
    s.push_str("} as const;\n\n");
    s.push_str("/** Hello fields the client owns; a bee's own values for them are ignored. */\n");
    s.push_str(&format!("export const CLIENT_OWNED = [{}] as const;\n\n", quoted(&spec.client_owned)));
    s.push_str(&format!(
        "/** Where `hum` is found: ${env}, else `{name}` on PATH, else ~/{def}. */\n\
         export const HUM_CLI_ENV = \"{env}\";\nexport const HUM_CLI_NAME = \"{name}\";\n\
         export const HUM_CLI_DEFAULT = \"{def}\";\n\n",
        env = spec.hum_cli_env,
        name = spec.hum_cli_name,
        def = spec.hum_cli_default
    ));
    s.push_str("/** Advisory codes (WIRE.md, Advisories). */\nexport const AdvisoryCode = {\n");
    for a in &spec.advisories {
        s.push_str(&format!("  /** {} ({}) */\n  {}: \"{}\",\n", a.doc, a.raised_by, camel(&a.pascal), a.wire));
    }
    s.push_str("} as const;\nexport type AdvisoryCodeT = (typeof AdvisoryCode)[keyof typeof AdvisoryCode];\n");
    s
}

pub fn constants_py(spec: &LinkSpec) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "LINK_VERSION = {}\n\"\"\"The version of the lines between this client and `hum bee link`.\"\"\"\n\n",
        spec.version
    ));
    s.push_str("\nclass LinkExit:\n    \"\"\"How `hum bee link` exits.\"\"\"\n\n");
    for (name, code) in &spec.exits {
        s.push_str(&format!("    {name} = {code}\n"));
    }
    s.push_str(&format!(
        "\n\nCLIENT_OWNED = ({},)\n\"\"\"Hello fields the client owns; a bee's own values for them are ignored.\"\"\"\n\n",
        quoted(&spec.client_owned)
    ));
    s.push_str(&format!(
        "HUM_CLI_ENV = \"{env}\"\nHUM_CLI_NAME = \"{name}\"\nHUM_CLI_DEFAULT = \"{def}\"\n\
         \"\"\"Where `hum` is found: ${env}, else `{name}` on PATH, else ~/{def}.\"\"\"\n\n",
        env = spec.hum_cli_env,
        name = spec.hum_cli_name,
        def = spec.hum_cli_default
    ));
    s.push_str("\nclass AdvisoryCode:\n    \"\"\"Advisory codes (WIRE.md, Advisories).\"\"\"\n\n");
    for a in &spec.advisories {
        s.push_str(&format!("    {} = \"{}\"\n    \"\"\"{} ({})\"\"\"\n\n", screaming(&a.pascal), a.wire, a.doc, a.raised_by));
    }
    s
}

pub fn constants_go(spec: &LinkSpec) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "// LinkVersion is the version of the lines between this client and `hum bee link`.\n\
         const LinkVersion = {}\n\n",
        spec.version
    ));
    s.push_str("// How `hum bee link` exits.\nconst (\n");
    for (name, code) in &spec.exits {
        s.push_str(&format!("\tLinkExit{} = {code}\n", title(name)));
    }
    s.push_str(")\n\n");
    s.push_str(&format!(
        "// ClientOwned are the hello fields the client owns; a bee's own values for them are ignored.\n\
         var ClientOwned = []string{{{}}}\n\n",
        quoted(&spec.client_owned)
    ));
    s.push_str(&format!(
        "// Where `hum` is found: ${env}, else `{name}` on PATH, else ~/{def}.\n\
         const (\n\tHumCLIEnv     = \"{env}\"\n\tHumCLIName    = \"{name}\"\n\tHumCLIDefault = \"{def}\"\n)\n\n",
        env = spec.hum_cli_env,
        name = spec.hum_cli_name,
        def = spec.hum_cli_default
    ));
    s.push_str("// AdvisoryCode names an advisory (WIRE.md, Advisories).\ntype AdvisoryCode string\n\nconst (\n");
    for a in &spec.advisories {
        s.push_str(&format!(
            "\t// Advisory{} : {} ({})\n\tAdvisory{} AdvisoryCode = \"{}\"\n",
            a.pascal, a.doc, a.raised_by, a.pascal, a.wire
        ));
    }
    s.push_str(")\n");
    s
}

pub fn constants_c(spec: &LinkSpec) -> String {
    let mut s = String::new();
    s.push_str("/* The version of the lines between this client and `hum bee link`. */\n");
    s.push_str(&format!("#define THRUM_LINK_VERSION {}\n\n", spec.version));
    s.push_str("/* How `hum bee link` exits, and what thrum_bee_start and thrum_bee_wait return. */\n");
    for (name, code) in &spec.exits {
        s.push_str(&format!("#define THRUM_LINK_{name} {code}\n"));
    }
    s.push_str(&format!(
        "\n/* Hello fields the client owns; a bee's own values for them are ignored. */\n\
         #define THRUM_CLIENT_OWNED {{ {} }}\n\n",
        quoted(&spec.client_owned)
    ));
    s.push_str(&format!(
        "/* Where `hum` is found: ${env}, else `{name}` on PATH, else ~/{def}. */\n\
         #define THRUM_HUM_CLI_ENV \"{env}\"\n#define THRUM_HUM_CLI_NAME \"{name}\"\n\
         #define THRUM_HUM_CLI_DEFAULT \"{def}\"\n\n",
        env = spec.hum_cli_env,
        name = spec.hum_cli_name,
        def = spec.hum_cli_default
    ));
    s.push_str("/* Advisory codes (WIRE.md, Advisories). */\n");
    for a in &spec.advisories {
        s.push_str(&format!(
            "#define THRUM_ADVISORY_{} \"{}\" /* {} ({}) */\n",
            screaming(&a.pascal),
            a.wire,
            a.doc,
            a.raised_by
        ));
    }
    s
}

fn is_marker(line: &str) -> bool {
    line.trim()
        .trim_start_matches(['/', '#', '*'])
        .trim_end_matches(['/', '*'])
        .trim()
        == "@constants"
}

pub fn render(template_rel: &str, template: &str, comment: &str, constants: Option<&str>) -> Result<String> {
    let mut out = header(comment, template_rel);
    let mut placed = false;
    for line in template.lines() {
        if is_marker(line) {
            let Some(c) = constants else {
                bail!("{template_rel} asks for constants it is not given");
            };
            out.push_str(c);
            placed = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if constants.is_some() && !placed {
        bail!("{template_rel} has no `@constants` line");
    }
    Ok(out)
}

pub fn emit_client(spec: &LinkSpec, client: &paths::Client, output: &Path) -> Result<()> {
    let template = fs::read_to_string(client.template_path())
        .with_context(|| format!("read {}", client.template_rel()))?;
    let constants = client.constants.then(|| match client.target {
        "ts" => constants_ts(spec),
        "python" => constants_py(spec),
        "go" => constants_go(spec),
        _ => constants_c(spec),
    });
    let s = render(&client.template_rel(), &template, client.comment, constants.as_deref())?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(output, s).with_context(|| format!("write {}", output.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LinkSpec {
        link_spec(
            &paths::link_rs(),
            &paths::core_rs(),
            &paths::chi_rs(),
            &paths::lib_rs(),
            &paths::wire_md(),
        )
        .unwrap()
    }

    #[test]
    fn the_link_is_read_from_the_rust() {
        let s = spec();
        assert!(s.version >= 1);
        assert!(s.exits.iter().any(|(n, c)| n == "REFUSED" && *c == 3));
        assert!(s.client_owned.contains(&"signature".to_string()));
        assert_eq!(s.hum_cli_env, "HUM_CLI");
        assert!(s.advisories.iter().any(|a| a.wire == "undeclared-key" && a.raised_by.contains("humd")));
    }

    #[test]
    fn constants_land_at_their_line() {
        let out = render("t", "import x\n# @constants\nbody\n", "#", Some("K = 1\n")).unwrap();
        assert!(out.contains("import x\nK = 1\nbody\n"));
        assert!(render("t", "body\n", "#", Some("K\n")).is_err());
        assert!(render("t", "# @constants\n", "#", None).is_err());
        assert!(is_marker("/* @constants */") && is_marker("// @constants") && !is_marker("x @constants"));
    }

    #[test]
    fn names_follow_each_language() {
        assert_eq!(camel("DidNotReturn"), "didNotReturn");
        assert_eq!(screaming("DidNotReturn"), "DID_NOT_RETURN");
        assert_eq!(title("REFUSED"), "Refused");
    }
}
