//! web2: the web2 world as one tendril, `wrepl`.
//!
//! `web2 serve` is the hive's bee, an attendant serving `wrepl` on the
//! thrum. Everything else is for a person at this machine: `web2` alone is
//! the same REPL a bloom reaches, `web2 run <line>` one line of it, and
//! `web2 login`, `logout`, `status`, `refresh` and `import` keep the
//! services it reaches.

mod auth;
mod bee;
mod catalogue;
mod config;
mod engine;
mod import;
mod mcp;
mod openapi;
mod render;

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;

use anyhow::{Result, bail};

use crate::engine::Engine;

const USAGE: &str = "\
web2: the web2 world as one tendril, wrepl.

  web2                     the wrepl REPL, the same one a bloom reaches (`help` lists the services)
  web2 run <line>          one wrepl line, answered here
  web2 serve               the bee: an attendant serving wrepl on the thrum
  web2 login <service>     sign a service in (a token at the prompt, or piped with --token-stdin; or OAuth in a browser)
  web2 logout <service>    forget what web2 holds for a service
  web2 status              the services, and how each is signed in
  web2 refresh [service]   read specs and tool lists again
  web2 import [--dry-run]  copy in the MCP servers the harnesses on this machine already know

The services live in web2.json; `web2 status` says where.";

fn init_tracing(level: &str) {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .with_writer(std::io::stderr)
        .init();
}

fn engine() -> Result<Engine> {
    Ok(Engine::new(config::load()?, config::state_dir()))
}

#[tokio::main]
async fn main() -> ExitCode {
    hum_paths::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verb = args.first().map(String::as_str).unwrap_or("");
    init_tracing(if verb == "serve" { "info" } else { "warn" });
    let outcome = match verb {
        "serve" => bee::serve().await.map(|_| ExitCode::SUCCESS),
        "run" => run(&args[1..]).await,
        "login" => login(&args[1..]).await,
        "logout" => logout(&args[1..]),
        "status" => status().await,
        "refresh" => refresh(&args[1..]).await,
        "import" => import(&args[1..]),
        "" => repl().await,
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        other => {
            eprintln!("web2 has no verb {other:?}.\n\n{USAGE}");
            Ok(ExitCode::from(2))
        }
    };
    match outcome {
        Ok(code) => code,
        Err(e) => {
            eprintln!("web2: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(words: &[String]) -> Result<ExitCode> {
    let line = match words {
        [one] => one.clone(),
        many => shlex::try_join(many.iter().map(String::as_str)).unwrap_or_else(|_| many.join(" ")),
    };
    let answer = engine()?.run(&line).await;
    if answer.is_error {
        eprintln!("{}", answer.output);
        return Ok(ExitCode::FAILURE);
    }
    println!("{}", answer.output);
    Ok(ExitCode::SUCCESS)
}

async fn repl() -> Result<ExitCode> {
    let engine = engine()?;
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        let mut failed = false;
        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let answer = engine.run(&line).await;
            failed |= answer.is_error;
            println!("{}", answer.output);
        }
        return Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS });
    }
    println!("wrepl, as a bloom sees it. `help` lists the services; `exit` leaves.");
    loop {
        print!("wrepl> ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            println!();
            break;
        }
        let line = line.trim();
        if matches!(line, "exit" | "quit") {
            break;
        }
        if line.is_empty() {
            continue;
        }
        let answer = engine.run(line).await;
        if answer.is_error {
            eprintln!("{}", answer.output);
        } else {
            println!("{}", answer.output);
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn login(args: &[String]) -> Result<ExitCode> {
    let token_stdin = args.iter().any(|a| a == "--token-stdin");
    let Some(name) = args.iter().find(|a| !a.starts_with("--")) else {
        bail!("web2 login <service>");
    };
    println!("{}", engine()?.login(name, token_stdin).await?);
    Ok(ExitCode::SUCCESS)
}

fn logout(args: &[String]) -> Result<ExitCode> {
    let [name] = args else { bail!("web2 logout <service>") };
    let creds = auth::Creds::at(&config::state_dir());
    if creds.remove(name)? {
        println!("web2 holds nothing for {name} now");
    } else {
        println!("web2 held nothing for {name}");
    }
    Ok(ExitCode::SUCCESS)
}

async fn status() -> Result<ExitCode> {
    println!("config      {}", config::config_path().display());
    println!("state       {}", config::state_dir().display());
    match config::load() {
        Ok(c) => {
            let engine = Engine::new(c, config::state_dir());
            println!();
            print!("{}", engine.services_help());
        }
        Err(e) => println!("\nthe config is broken: {e:#}"),
    }
    Ok(ExitCode::SUCCESS)
}

async fn refresh(args: &[String]) -> Result<ExitCode> {
    let engine = engine()?;
    let names: Vec<String> = if args.is_empty() { engine.config.services.keys().cloned().collect() } else { args.to_vec() };
    let mut failed = false;
    for name in names {
        let Some(service) = engine.config.services.get(&name) else {
            eprintln!("{name}: no such service");
            failed = true;
            continue;
        };
        match engine.refresh(service).await {
            Ok(n) => println!("{name}: {n} commands"),
            Err(e) => {
                eprintln!("{e}");
                failed = true;
            }
        }
    }
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

fn import(args: &[String]) -> Result<ExitCode> {
    let dry = args.iter().any(|a| a == "--dry-run");
    let path = config::config_path();
    let existing: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str(&t)?,
        Err(_) => serde_json::json!({}),
    };
    let mut found = Vec::new();
    for (label, source) in import::sources() {
        match import::read_source(&label, &source) {
            Ok(mut f) => found.append(&mut f),
            Err(e) => eprintln!("{label}: {e:#}"),
        }
    }
    let plan = import::plan(&existing, found);
    for f in &plan.added {
        println!("+ {} (from {})", f.name, f.from);
    }
    for (name, why) in &plan.skipped {
        println!("  {name}: {why}");
    }
    if plan.added.is_empty() {
        println!("nothing to add");
        return Ok(ExitCode::SUCCESS);
    }
    if dry {
        println!("(a dry run: {} not written)", path.display());
    } else {
        import::apply(&path, existing, &plan)?;
        println!("written to {}; each one only reads until web2.json turns its writes on", path.display());
    }
    Ok(ExitCode::SUCCESS)
}
