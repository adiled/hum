//! The Rust, held to the thrum.
//!
//! thrum-core's `chi.rs`, `views.rs`, `envelope.rs` and `lib.rs` are
//! hand-written readings of `hum.thrum`. This check parses them as text
//! and compares every chi, kind, advisory code, body field and the
//! version against the file, naming each difference in the file's own
//! words. `cargo run -p codegen -- --check` fails on any, and the build
//! script warns, so Rust that disagrees with the thrum does not pass.

use std::path::Path;

use anyhow::{Result, bail};

use crate::paths;
use crate::protocol::{Repr, TypeSpec};
use crate::thrum::{self, Thrum};

pub fn check(
    thrum: &Thrum,
    chi_rs: &Path,
    views_rs: &Path,
    envelope_rs: &Path,
    lib_rs: &Path,
) -> Result<()> {
    let chi_src = crate::read(chi_rs)?;
    let lib_src = crate::read(lib_rs)?;
    let mut faults: Vec<String> = Vec::new();

    let version = crate::extract_version(&lib_src)?;
    if version != thrum.version {
        faults.push(format!(
            "THRUM {} here, THRUM_VERSION {version} in {}",
            thrum.version,
            paths::LIB_RS_REL
        ));
    }

    let names = |enum_name: &str| -> Result<Vec<String>> {
        Ok(crate::extract_enum(&chi_src, enum_name)?
            .into_iter()
            .map(|v| v.wire)
            .collect())
    };
    let chis: Vec<String> = thrum.chis.iter().map(|c| c.name.clone()).collect();
    compare_names(&mut faults, "chi", &chis, &names("Chi")?, "enum Chi");
    for (kind, enum_name) in [
        (thrum::PULSE_KIND, "PulseKind"),
        (thrum::PETAL_KIND, "PetalKind"),
        (thrum::ROLE_KIND, "Role"),
    ] {
        let want: Vec<String> = thrum.kind(kind)?.values.iter().map(|v| v.name.clone()).collect();
        compare_names(&mut faults, kind, &want, &names(enum_name)?, &format!("enum {enum_name}"));
    }
    let advisories: Vec<String> = thrum.advisories.iter().map(|a| a.name.clone()).collect();
    compare_names(&mut faults, "advisory", &advisories, &names("AdvisoryCode")?, "enum AdvisoryCode");

    let want = thrum.protocol_spec();
    let have = crate::protocol::rust_types(views_rs, envelope_rs)?;
    for t in &want.types {
        match have.iter().find(|h| h.name == t.name) {
            None => faults.push(format!("{} has no `{}`", paths::VIEWS_RS_REL, t.name)),
            Some(h) => compare_fields(&mut faults, t, h),
        }
    }
    for h in &have {
        if !want.types.iter().any(|t| t.name == h.name) {
            faults.push(format!(
                "{} declares `{}`, which {} does not",
                paths::VIEWS_RS_REL,
                h.name,
                paths::HUM_THRUM_REL
            ));
        }
    }

    if faults.is_empty() {
        return Ok(());
    }
    bail!(
        "the Rust disagrees with {}:\n  {}",
        paths::HUM_THRUM_REL,
        faults.join("\n  ")
    )
}

fn compare_names(faults: &mut Vec<String>, what: &str, want: &[String], have: &[String], at: &str) {
    for w in want {
        if !have.contains(w) {
            faults.push(format!("{what} `{w}` is not in {at} ({})", paths::CHI_RS_REL));
        }
    }
    for h in have {
        if !want.contains(h) {
            faults.push(format!(
                "{at} ({}) has `{h}`, which {} does not declare as a {what}",
                paths::CHI_RS_REL,
                paths::HUM_THRUM_REL
            ));
        }
    }
}

fn compare_fields(faults: &mut Vec<String>, want: &TypeSpec, have: &TypeSpec) {
    for f in &want.fields {
        match have.fields.iter().find(|h| h.json == f.json) {
            None => faults.push(format!(
                "`{}.{}` is not in {}",
                want.name,
                f.json,
                paths::VIEWS_RS_REL
            )),
            Some(h) => {
                if h.optional != f.optional || h.repr != f.repr {
                    faults.push(format!(
                        "`{}.{}` is `{}` in {}, `{}` in {}",
                        want.name,
                        f.json,
                        show(&f.repr, f.optional),
                        paths::HUM_THRUM_REL,
                        show(&h.repr, h.optional),
                        paths::VIEWS_RS_REL
                    ));
                }
            }
        }
    }
    for h in &have.fields {
        if !want.fields.iter().any(|f| f.json == h.json) {
            faults.push(format!(
                "{} declares `{}.{}`, which {} does not",
                paths::VIEWS_RS_REL,
                have.name,
                h.json,
                paths::HUM_THRUM_REL
            ));
        }
    }
}

fn show(repr: &Repr, optional: bool) -> String {
    let base = match repr {
        Repr::Str => "string".to_string(),
        Repr::Int { unsigned: true } => "uint".to_string(),
        Repr::Int { unsigned: false } => "int".to_string(),
        Repr::Float => "float".to_string(),
        Repr::Bool => "bool".to_string(),
        Repr::Json => "any".to_string(),
        Repr::Map(inner) => format!("{{{}}}", show(inner, false)),
        Repr::Arr(inner) => format!("[{}]", show(inner, false)),
        Repr::Ref(name) if name == "Chi" => "chi".to_string(),
        Repr::Ref(name) => kebab(name),
    };
    if optional { format!("{base}?") } else { base }
}

fn kebab(pascal: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rust_agrees_with_the_thrum() {
        let thrum = thrum::parse(&paths::hum_thrum()).unwrap();
        check(
            &thrum,
            &paths::chi_rs(),
            &paths::views_rs(),
            &paths::envelope_rs(),
            &paths::lib_rs(),
        )
        .unwrap();
    }

    #[test]
    fn a_difference_is_said_in_the_thrum_words() {
        assert_eq!(show(&Repr::Arr(Box::new(Repr::Ref("Petal".into()))), true), "[petal]?");
        assert_eq!(show(&Repr::Map(Box::new(Repr::Map(Box::new(Repr::Json)))), true), "{{any}}?");
        assert_eq!(show(&Repr::Int { unsigned: true }, false), "uint");
        assert_eq!(show(&Repr::Ref("Chi".into()), false), "chi");
        assert_eq!(kebab("DroneLoad"), "drone-load");
    }
}
