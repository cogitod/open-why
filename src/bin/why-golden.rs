use anyhow::Result;
use clap::Parser;
use open_why::{Record, Store};
use serde::Deserialize;
use std::path::PathBuf;

/// Run the golden retrieval parity set against open-why.
///
/// The fixture records, for each query, the top result from a trusted reference run. This
/// binary checks open-why's top result by stable ID and reports the full top three for
/// inspecting near-misses.
///
/// Preconditions: the open-why store holds the corpus used by the reference run, and an
/// equivalent embedder is configured (`OPEN_WHY_EMBED_MODEL_PATH` or `why fetch-model`) when
/// the semantic arm is part of the reference.
#[derive(Parser)]
#[command(
    name = "why-golden",
    about = "Run the golden retrieval parity set against open-why"
)]
struct Cli {
    #[arg(long, default_value = "tests/fixtures/golden-queries.json")]
    fixture: PathBuf,
}

#[derive(Deserialize)]
struct Fixture {
    #[allow(dead_code)]
    description: String,
    scope: String,
    captured_at: String,
    queries: Vec<Query>,
}

#[derive(Deserialize)]
struct Query {
    query: String,
    #[serde(default)]
    types: Vec<String>,
    expected: Expected,
}

#[derive(Deserialize)]
struct Expected {
    id: String,
    title: String,
    #[serde(rename = "type")]
    kind: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let text = std::fs::read_to_string(&cli.fixture)?;
    let fixture: Fixture = serde_json::from_str(&text)?;

    let store = Store::open_default()?;
    let scope = fixture.scope.as_str();

    let semantic = std::env::var("OPEN_WHY_EMBED_MODEL_PATH").is_ok()
        || std::env::var("OPEN_WHY_EMBED_URL").is_ok();
    println!(
        "golden parity: {} queries (scope `{}`)",
        fixture.queries.len(),
        scope
    );
    println!("reference captured: {}", fixture.captured_at);
    println!(
        "semantic arm: {}",
        if semantic {
            "on"
        } else {
            "OFF (lexical-first)"
        }
    );
    println!();

    let mut pass = 0;
    let mut fail = 0;
    for q in &fixture.queries {
        let hits: Vec<Record> = store.search_records(&q.query, &[scope], &q.types, 3)?;
        let top_id = hits.first().map(|r| r.id.as_str()).unwrap_or("");
        let ok = top_id == q.expected.id;
        if ok {
            pass += 1;
        } else {
            fail += 1;
        }
        println!("{} \"{}\"", if ok { "PASS" } else { "FAIL" }, q.query);
        println!(
            "  expected: {} [{}] ({})",
            q.expected.title, q.expected.kind, q.expected.id
        );
        if hits.is_empty() {
            println!("  (no results)");
        }
        for (i, h) in hits.iter().enumerate() {
            let label = if i == 0 {
                "top-1"
            } else {
                &format!("#{}", i + 1)
            };
            println!("  {label:<5} {} [{}] {}", h.title, h.kind, h.id);
        }
        println!();
    }

    println!("== {} pass, {} fail ==", pass, fail);
    if fail > 0 {
        std::process::exit(1);
    }
    Ok(())
}
