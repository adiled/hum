//! The link: this client on stdio, for a bee written in anything.
//!
//! A bee in TS, Python, Go or C does not speak to humd. It spawns
//! `hum bee link --hive <hive> --as <label>` with two pipes and speaks to
//! the link, one JSON object per line each way, and the link is the
//! [`Bee`] a Rust bee would hold: it enters the bee, finds the socket,
//! signs the hello, reads humd's verdict, reconnects, checks every tone
//! against the spec. The generated client in each language is the other
//! end of these pipes and nothing more.
//!
//! A line with a `chi` is a tone, either way, passed through as it is.
//! A line without one is the link's own:
//!
//! - link to bee: `{"link":"ready","linkVersion":N,"thrum":V,"bee":id,
//!   "hive":h,"as":l}` first, then `{"link":"nestled"}`,
//!   `{"link":"astray"}`, `{"link":"advisory","code":..,"message":..,
//!   "from":..,"fields":{..}}`, and `{"link":"refused","error":..}` last.
//! - bee to link: `{"hello":{..}}` first, the bee's own hello fields;
//!   later `{"hello":{..}}` again to change them (a null drops a field).
//!
//! The link connects only once the first hello has arrived. A closed
//! stdin is the bee closing: the link sends what it was given, closes,
//! and exits [`exit::CLOSED`]. A refusal is reported and the link exits
//! [`exit::REFUSED`], and so does a bee its hive cannot enter. A first
//! line that is not a hello exits [`exit::USAGE`]. The link outlives
//! interrupts and terminations: its life is the pipe's, so a bee that is
//! stopped or killed takes it along, and one shutting down cleanly is
//! not cut off mid-sentence. It logs to stderr, which is the bee's.
//!
//! [`LINK_VERSION`] changes when these lines do. A client built for
//! another version stops at `ready`; rebuilding the hive brings it level.
//!
//! A client finds `hum` at [`HUM_CLI_ENV`] when it is set, else as
//! [`HUM_CLI_NAME`] on `PATH`, else at [`HUM_CLI_DEFAULT`] under the home
//! directory, where `./install` puts it. (`HUM_BIN` is humd's.)

use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::core::Endpoint;
use crate::drive::{Bee, Event, Options};

pub const LINK_VERSION: u32 = 1;

pub const HUM_CLI_ENV: &str = "HUM_CLI";
pub const HUM_CLI_NAME: &str = "hum";
pub const HUM_CLI_DEFAULT: &str = ".local/bin/hum";

pub mod exit {
    pub const CLOSED: i32 = 0;
    pub const FAILED: i32 = 1;
    pub const USAGE: i32 = 2;
    pub const REFUSED: i32 = 3;
}

fn not_a_tone(message: &str) -> Value {
    json!({
        "link": "advisory",
        "code": thrum_core::AdvisoryCode::NotATone.as_str(),
        "message": message,
        "from": "client",
    })
}

pub async fn serve(hive: &str, label: &str, endpoint: Endpoint) -> i32 {
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(line) = out_rx.recv().await {
            let mut bytes = serde_json::to_vec(&line).unwrap_or_default();
            bytes.push(b'\n');
            if stdout.write_all(&bytes).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });
    let code = run(hive, label, endpoint, &out_tx).await;
    drop(out_tx);
    let _ = writer.await;
    code
}

async fn run(hive: &str, label: &str, endpoint: Endpoint, out: &mpsc::UnboundedSender<Value>) -> i32 {
    let key = match hum_identity::enter_as(hive, label) {
        Ok(key) => key,
        Err(e) => {
            tracing::error!(hive, label, err = %e, "link.enter.failed");
            let _ = out.send(json!({"link": "refused", "error": e.to_string()}));
            return exit::REFUSED;
        }
    };
    let _ = out.send(json!({
        "link": "ready",
        "linkVersion": LINK_VERSION,
        "thrum": thrum_core::THRUM_VERSION,
        "bee": key.id(),
        "hive": hive,
        "as": label,
    }));

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let hello = loop {
        match lines.next_line().await {
            Ok(Some(line)) if line.trim().is_empty() => continue,
            Ok(Some(line)) => match serde_json::from_str::<Value>(&line) {
                Ok(Value::Object(mut msg)) if msg.get("chi").is_none() && msg.contains_key("hello") => {
                    match msg.remove("hello") {
                        Some(Value::Object(hello)) => break hello,
                        _ => {
                            tracing::error!("link.usage — the hello is a JSON object");
                            return exit::USAGE;
                        }
                    }
                }
                _ => {
                    tracing::error!("link.usage — the first line to the link is {{\"hello\":{{..}}}}");
                    return exit::USAGE;
                }
            },
            Ok(None) => return exit::CLOSED,
            Err(e) => {
                tracing::error!(err = %e, "link.stdin.failed");
                return exit::FAILED;
            }
        }
    };

    let mut bee = Bee::spawn(
        Options {
            hive: hive.to_string(),
            label: label.to_string(),
            hello,
            endpoint,
        },
        key,
    );
    let mut stdin_open = true;
    loop {
        tokio::select! {
            line = lines.next_line(), if stdin_open => match line {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Value>(&line) {
                        Ok(Value::Object(msg)) if msg.contains_key("chi") => bee.send(Value::Object(msg)),
                        Ok(Value::Object(mut msg)) if msg.contains_key("hello") => match msg.remove("hello") {
                            Some(patch @ Value::Object(_)) => bee.hello(patch),
                            _ => {
                                let _ = out.send(not_a_tone("a hello change is {\"hello\":{..}}; dropped"));
                            }
                        },
                        Ok(_) => {
                            let _ = out.send(not_a_tone("a line to the link is a tone or {\"hello\":{..}}; dropped"));
                        }
                        Err(_) => {
                            let _ = out.send(not_a_tone("a line to the link is one JSON object; dropped"));
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    stdin_open = false;
                    bee.close();
                }
            },
            event = bee.next() => match event {
                Some(Event::Tone(tone)) => {
                    let _ = out.send(tone);
                }
                Some(Event::Nestled) => {
                    let _ = out.send(json!({"link": "nestled"}));
                }
                Some(Event::Astray) => {
                    let _ = out.send(json!({"link": "astray"}));
                }
                Some(Event::Advisory(advisory)) => {
                    let mut line = Map::new();
                    line.insert("link".into(), json!("advisory"));
                    if let Ok(Value::Object(fields)) = serde_json::to_value(&advisory) {
                        line.extend(fields);
                    }
                    let _ = out.send(Value::Object(line));
                }
                Some(Event::Refused(why)) => {
                    let _ = out.send(json!({"link": "refused", "error": why}));
                    return exit::REFUSED;
                }
                None => return if stdin_open { exit::FAILED } else { exit::CLOSED },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_hum_is_where_install_puts_it() {
        assert_eq!(
            hum_paths::home().join(HUM_CLI_DEFAULT),
            hum_paths::hum_bin(HUM_CLI_NAME)
        );
    }
}
