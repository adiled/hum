//! A service's commands, and how a command line becomes a command's input.
//!
//! Every operation of an OpenAPI spec and every tool of an MCP server is a
//! command with a JSON Schema for its input. A command line names one and
//! passes flags: `--name value`, `--name=value`, `--flag` and `--no-flag`
//! for booleans, a flag repeated for an array, and words with no flag fill
//! the required inputs in order. `--json '{…}'` gives the whole input, and
//! flags laid over it win. `--jq '<filter>'` narrows a JSON result, so both
//! names are web2's own, never a command's.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const JSON_FLAG: &str = "json";
pub const JQ_FLAG: &str = "jq";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub name: String,
    pub original: String,
    pub summary: String,
    pub description: String,
    pub group: Option<String>,
    pub write: bool,
    pub input: Value,
    pub target: Target,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Target {
    Http(HttpOp),
    Mcp { tool: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpOp {
    pub method: String,
    pub path: String,
    pub params: Vec<HttpParam>,
    pub body: Body,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpParam {
    pub name: String,
    pub flag: String,
    pub at: ParamAt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParamAt {
    Path,
    Query,
    Header,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Body {
    None,
    Fields { content_type: String, fields: Vec<(String, String)> },
    Whole { content_type: String, flag: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catalogue {
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    pub args: Map<String, Value>,
    pub jq: Option<String>,
}

pub fn normalize(name: &str) -> String {
    name.chars().filter(|c| *c != '-' && *c != '_').flat_map(char::to_lowercase).collect()
}

pub fn kebab(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let mut prev: Option<char> = None;
    for c in name.chars() {
        match c {
            '_' | ' ' => out.push('-'),
            c if c.is_ascii_uppercase() => {
                if prev.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit()) {
                    out.push('-');
                }
                out.push(c.to_ascii_lowercase());
            }
            c => out.push(c),
        }
        prev = Some(c);
    }
    let mut squeezed = String::with_capacity(out.len());
    for c in out.chars() {
        if c == '-' && squeezed.ends_with('-') {
            continue;
        }
        squeezed.push(c);
    }
    squeezed.trim_matches('-').to_string()
}

impl Catalogue {
    pub fn new(mut commands: Vec<Command>) -> Catalogue {
        let mut seen = std::collections::HashSet::new();
        for c in &mut commands {
            if !seen.insert(normalize(&c.name)) {
                c.name = c.original.clone();
                seen.insert(normalize(&c.name));
            }
        }
        commands.sort_by(|a, b| a.group.cmp(&b.group).then_with(|| a.name.cmp(&b.name)));
        Catalogue { commands }
    }

    pub fn find(&self, name: &str) -> Option<&Command> {
        self.commands
            .iter()
            .find(|c| c.name == name || c.original == name)
            .or_else(|| {
                let n = normalize(name);
                self.commands.iter().find(|c| normalize(&c.name) == n || normalize(&c.original) == n)
            })
    }

    pub fn suggest(&self, name: &str) -> Vec<String> {
        suggest(name, self.commands.iter().map(|c| c.name.as_str()))
    }

    pub fn groups(&self) -> Vec<(String, usize)> {
        let mut groups: Vec<(String, usize)> = Vec::new();
        for c in &self.commands {
            let g = c.group.clone().unwrap_or_else(|| "(other)".into());
            match groups.iter_mut().find(|(name, _)| *name == g) {
                Some((_, n)) => *n += 1,
                None => groups.push((g, 1)),
            }
        }
        groups
    }

    pub fn matching(&self, words: &[String]) -> Vec<&Command> {
        let words: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
        let in_group: Vec<&Command> = self
            .commands
            .iter()
            .filter(|c| words.len() == 1 && c.group.as_deref().is_some_and(|g| g.eq_ignore_ascii_case(&words[0])))
            .collect();
        if !in_group.is_empty() {
            return in_group;
        }
        let every = |hay: String| words.iter().all(|w| hay.contains(w.as_str()));
        let segments = |c: &Command| -> Vec<String> {
            c.name.to_lowercase().split(['/', '-', '_', '.']).map(str::to_string).collect()
        };
        let tiers: [&dyn Fn(&Command) -> bool; 3] = [
            &|c| {
                let segs = segments(c);
                words.iter().all(|w| segs.contains(w))
            },
            &|c| every(c.name.to_lowercase()),
            &|c| every(format!("{} {} {}", c.name, c.summary, c.group.as_deref().unwrap_or("")).to_lowercase()),
        ];
        for tier in tiers {
            let found: Vec<&Command> = self.commands.iter().filter(|c| tier(c)).collect();
            if !found.is_empty() {
                return found;
            }
        }
        Vec::new()
    }
}

pub fn suggest<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Vec<String> {
    let n = normalize(name);
    let mut scored: Vec<(f64, &str)> = candidates
        .map(|c| {
            let cn = normalize(c);
            let score = if !n.is_empty() && (cn.contains(&n) || n.contains(&cn)) {
                0.95
            } else {
                strsim::jaro_winkler(&n, &cn)
            };
            (score, c)
        })
        .filter(|(s, _)| *s >= 0.82)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(3).map(|(_, c)| c.to_string()).collect()
}

fn properties(schema: &Value) -> Vec<(String, Value)> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| p.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

fn required(schema: &Value) -> Vec<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn schema_type(schema: &Value) -> Option<String> {
    match schema.get("type") {
        Some(Value::String(t)) => Some(t.clone()),
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).find(|t| *t != "null").map(str::to_string),
        _ => None,
    }
}

impl Command {
    pub fn flags(&self) -> Vec<(String, Value)> {
        properties(&self.input)
    }

    pub fn required(&self) -> Vec<String> {
        required(&self.input)
    }

    fn property(&self, flag: &str) -> Option<(String, Value)> {
        let props = self.flags();
        props
            .iter()
            .find(|(k, _)| k == flag)
            .or_else(|| {
                let n = normalize(flag);
                props.iter().find(|(k, _)| normalize(k) == n)
            })
            .cloned()
    }

    pub fn build_input(&self, words: &[String]) -> Result<Input, String> {
        let mut args = Map::new();
        let mut given = Map::new();
        let mut jq = None;
        let mut loose: Vec<String> = Vec::new();
        let mut i = 0;
        while i < words.len() {
            let word = &words[i];
            i += 1;
            let Some(flag) = word.strip_prefix("--").filter(|f| !f.is_empty()) else {
                if word == "--" {
                    loose.extend(words[i..].iter().cloned());
                    break;
                }
                loose.push(word.clone());
                continue;
            };
            let (name, inline) = match flag.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                None => (flag.to_string(), None),
            };
            let mut take_value = |what: &str| -> Result<String, String> {
                if let Some(v) = inline.clone() {
                    return Ok(v);
                }
                if i < words.len() {
                    i += 1;
                    return Ok(words[i - 1].clone());
                }
                Err(format!("--{what} needs a value"))
            };
            if name == JSON_FLAG {
                let text = take_value(JSON_FLAG)?;
                match serde_json::from_str::<Value>(&text) {
                    Ok(Value::Object(obj)) => args.extend(obj),
                    Ok(_) => return Err("--json takes a JSON object, the command's whole input".into()),
                    Err(e) => return Err(format!("--json is not JSON: {e}")),
                }
                continue;
            }
            if name == JQ_FLAG {
                jq = Some(take_value(JQ_FLAG)?);
                continue;
            }
            let (prop, schema, value) = match self.property(&name) {
                Some((prop, schema)) => {
                    let value = if schema_type(&schema).as_deref() == Some("boolean") {
                        match inline.as_deref() {
                            Some(v) => parse_bool(&name, v)?,
                            None => Value::Bool(true),
                        }
                    } else {
                        coerce(&name, &take_value(&name)?, &schema)?
                    };
                    (prop, schema, value)
                }
                None => match name.strip_prefix("no-").and_then(|n| self.property(n)) {
                    Some((prop, schema)) if schema_type(&schema).as_deref() == Some("boolean") && inline.is_none() => {
                        (prop, schema, Value::Bool(false))
                    }
                    _ => return Err(self.unknown_flag(&name)),
                },
            };
            if schema_type(&schema).as_deref() == Some("array") && !value.is_array() {
                match given.entry(prop).or_insert_with(|| json!([])) {
                    Value::Array(items) => items.push(value),
                    slot => *slot = json!([value]),
                }
            } else {
                given.insert(prop, value);
            }
        }
        args.extend(given);
        let open_now: Vec<String> = self.required().into_iter().filter(|r| !args.contains_key(r)).collect();
        let mut open = open_now.into_iter();
        for word in loose {
            let Some(prop) = open.next() else {
                return Err(format!(
                    "`{word}` fills no input: every required one is set; give the rest as flags, like `--name value`"
                ));
            };
            let schema = self.property(&prop).map(|(_, s)| s).unwrap_or(Value::Null);
            args.insert(prop.clone(), coerce(&prop, &word, &schema)?);
        }
        let missing: Vec<String> = self.required().into_iter().filter(|r| !args.contains_key(r)).collect();
        if !missing.is_empty() {
            let flags: Vec<String> = missing.iter().map(|m| format!("--{m}")).collect();
            return Err(format!("{} needs {}", self.name, flags.join(", ")));
        }
        Ok(Input { args, jq })
    }

    fn unknown_flag(&self, name: &str) -> String {
        let props = self.flags();
        let mut msg = format!("{} has no flag --{name}", self.name);
        let close = suggest(name, props.iter().map(|(k, _)| k.as_str()));
        if let Some(best) = close.first() {
            msg.push_str(&format!("; did you mean --{best}?"));
        } else if props.is_empty() {
            msg.push_str("; it takes no input");
        } else {
            let all: Vec<String> = props.iter().map(|(k, _)| format!("--{k}")).collect();
            msg.push_str(&format!("; its flags are {}", all.join(" ")));
        }
        msg
    }

    pub fn usage(&self, service: &str) -> String {
        let mut out = format!("{service} {}", self.name);
        if !self.summary.is_empty() {
            out.push_str(&format!(" — {}", self.summary));
        }
        out.push('\n');
        if let Target::Http(op) = &self.target {
            out.push_str(&format!("{} {}\n", op.method.to_uppercase(), op.path));
        }
        if !self.description.is_empty() && self.description != self.summary {
            out.push('\n');
            out.push_str(&clip(&self.description, 1200));
            out.push('\n');
        }
        let required = self.required();
        let flags = self.flags();
        if flags.is_empty() {
            out.push_str("\nIt takes no input.\n");
        } else {
            out.push('\n');
            let rows: Vec<(String, String)> = flags
                .iter()
                .map(|(name, schema)| {
                    let ty = describe_type(schema);
                    let left = if ty == "boolean" { format!("--{name}") } else { format!("--{name} <{ty}>") };
                    let mut right = String::new();
                    if required.contains(name) {
                        right.push_str("required. ");
                    }
                    if let Some(choices) = schema.get("enum").and_then(Value::as_array) {
                        let choices: Vec<String> = choices.iter().map(|c| c.as_str().map(str::to_string).unwrap_or_else(|| c.to_string())).collect();
                        right.push_str(&format!("One of {}. ", choices.join(", ")));
                    }
                    if let Some(d) = schema.get("description").and_then(Value::as_str) {
                        right.push_str(&clip(&first_line(d), 160));
                    }
                    if let Some(def) = schema.get("default") {
                        right.push_str(&format!(" (default {def})"));
                    }
                    (left, right.trim().to_string())
                })
                .collect();
            let width = rows.iter().map(|(l, _)| l.len()).max().unwrap_or(0).min(36);
            for (left, right) in rows {
                if right.is_empty() {
                    out.push_str(&format!("  {left}\n"));
                } else {
                    out.push_str(&format!("  {left:<width$}  {right}\n"));
                }
            }
            if !required.is_empty() {
                out.push_str(&format!("\nWords with no flag fill {} in that order.\n", required.join(", ")));
            }
        }
        out.push_str("--json '{…}' gives the whole input; --jq '<filter>' narrows a JSON result.\n");
        out
    }
}

pub fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string()
}

pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

fn describe_type(schema: &Value) -> String {
    match schema_type(schema).as_deref() {
        Some("array") => {
            let item = schema.get("items").map(describe_type).unwrap_or_else(|| "value".into());
            format!("{item}…")
        }
        Some(t) => t.to_string(),
        None if schema.get("enum").is_some() => "string".into(),
        None => "json".into(),
    }
}

fn parse_bool(name: &str, text: &str) -> Result<Value, String> {
    match text.to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" | "on" => Ok(Value::Bool(true)),
        "false" | "no" | "0" | "off" => Ok(Value::Bool(false)),
        _ => Err(format!("--{name} is true or false, not {text:?}")),
    }
}

fn coerce(name: &str, text: &str, schema: &Value) -> Result<Value, String> {
    let ty = schema_type(schema);
    match ty.as_deref() {
        Some("string") => Ok(Value::String(text.to_string())),
        Some("integer") => text
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| format!("--{name} is a whole number, not {text:?}")),
        Some("number") => text
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| format!("--{name} is a number, not {text:?}")),
        Some("boolean") => parse_bool(name, text),
        Some("array") => {
            if text.trim_start().starts_with('[') {
                return serde_json::from_str(text).map_err(|e| format!("--{name} looks like a JSON array but is not one: {e}"));
            }
            let item = schema.get("items").cloned().unwrap_or(Value::Null);
            coerce(name, text, &item)
        }
        Some("object") => match serde_json::from_str::<Value>(text) {
            Ok(v @ Value::Object(_)) => Ok(v),
            _ => Err(format!("--{name} takes a JSON object, like --{name} '{{\"key\": \"value\"}}'")),
        },
        _ => Ok(serde_json::from_str::<Value>(text)
            .ok()
            .filter(|v| !v.is_string())
            .unwrap_or_else(|| Value::String(text.to_string()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(input: Value) -> Command {
        Command {
            name: "issues/create".into(),
            original: "issues/create".into(),
            summary: "Create an issue".into(),
            description: String::new(),
            group: Some("issues".into()),
            write: true,
            input,
            target: Target::Mcp { tool: "x".into() },
        }
    }

    fn words(line: &str) -> Vec<String> {
        shlex::split(line).unwrap()
    }

    fn issue() -> Command {
        command(json!({
            "type": "object",
            "properties": {
                "owner": { "type": "string" },
                "repo": { "type": "string" },
                "title": { "type": "string" },
                "labels": { "type": "array", "items": { "type": "string" } },
                "milestone": { "type": "integer" },
                "draft": { "type": "boolean" },
                "extra": { "type": "object" }
            },
            "required": ["owner", "repo", "title"]
        }))
    }

    #[test]
    fn flags_become_input_typed_by_the_schema() {
        let input = issue()
            .build_input(&words("--owner rust-lang --repo=rust --title 'it broke' --labels bug --labels p1 --milestone 3 --draft"))
            .unwrap();
        assert_eq!(
            Value::Object(input.args),
            json!({ "owner": "rust-lang", "repo": "rust", "title": "it broke", "labels": ["bug", "p1"], "milestone": 3, "draft": true })
        );
    }

    #[test]
    fn words_with_no_flag_fill_the_required_inputs_in_order() {
        let input = issue().build_input(&words("rust-lang rust 'it broke' --no-draft")).unwrap();
        assert_eq!(input.args["title"], "it broke");
        assert_eq!(input.args["draft"], false);
        let err = issue().build_input(&words("a b c d")).unwrap_err();
        assert!(err.contains("`d` fills no input"), "{err}");
    }

    #[test]
    fn json_gives_the_whole_input_and_flags_win_over_it() {
        let input = issue()
            .build_input(&words(r#"--json '{"owner":"a","repo":"b","title":"t","milestone":1}' --milestone 2 --jq .number"#))
            .unwrap();
        assert_eq!(input.args["milestone"], 2);
        assert_eq!(input.jq.as_deref(), Some(".number"));
    }

    #[test]
    fn flag_names_match_across_kebab_snake_and_camel() {
        let c = command(json!({ "type": "object", "properties": { "per_page": { "type": "integer" }, "sortOrder": { "type": "string" } } }));
        let input = c.build_input(&words("--per-page 5 --sort-order asc")).unwrap();
        assert_eq!(input.args["per_page"], 5);
        assert_eq!(input.args["sortOrder"], "asc");
    }

    #[test]
    fn mistakes_say_what_would_have_worked() {
        let err = issue().build_input(&words("--owner a --repo b")).unwrap_err();
        assert!(err.contains("needs --title"), "{err}");
        let err = issue().build_input(&words("--titel x")).unwrap_err();
        assert!(err.contains("did you mean --title"), "{err}");
        let err = issue().build_input(&words("a b c --milestone soon")).unwrap_err();
        assert!(err.contains("whole number"), "{err}");
        let err = issue().build_input(&words("a b c --extra notjson")).unwrap_err();
        assert!(err.contains("JSON object"), "{err}");
    }

    #[test]
    fn names_kebab_and_lookups_forgive_the_spelling() {
        assert_eq!(kebab("search_issues"), "search-issues");
        assert_eq!(kebab("getIssueById"), "get-issue-by-id");
        assert_eq!(kebab("issues/list-for-repo"), "issues/list-for-repo");
        assert_eq!(kebab("Get  Thing__2"), "get-thing-2");
        let cat = Catalogue::new(vec![issue()]);
        assert!(cat.find("issues/create").is_some());
        assert!(cat.find("issues/CREATE").is_some());
        assert_eq!(cat.suggest("issues/creat"), ["issues/create"]);
    }

    #[test]
    fn usage_lists_flags_with_what_they_need() {
        let u = issue().usage("github");
        assert!(u.starts_with("github issues/create — Create an issue"), "{u}");
        assert!(u.contains("--title <string>"), "{u}");
        assert!(u.contains("--labels <string…>"), "{u}");
        assert!(u.contains("--draft  ") || u.contains("--draft\n"), "{u}");
        assert!(u.contains("fill owner, repo, title in that order"), "{u}");
    }
}
