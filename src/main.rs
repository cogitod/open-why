use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use open_why::{answer, db, mcp, miner, store};

#[derive(Parser)]
#[command(name = "open-why", version, about = "Ask any repository why a decision was made — with the evidence.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Index a repository's decision history into the store
    Init {
        /// Path or git URL (default: current directory)
        repo: Option<String>,
    },
    /// Ask why a decision was made
    Why {
        /// The question to answer
        question: String,
        /// Path or git URL (default: current directory)
        #[arg(long)]
        repo: Option<String>,
        /// Number of results to show
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
    /// Capture a decision into the store
    Capture {
        /// Decision kind: decision, fact, reference, pattern, doc, ...
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        title: String,
        #[arg(long)]
        content: String,
        /// Importance 0..1 (default 0.5)
        #[arg(long)]
        importance: Option<f64>,
        /// Scope (default: global)
        #[arg(long)]
        scope: Option<String>,
        /// Optional externally-minted id (preserved verbatim)
        #[arg(long)]
        id: Option<String>,
        /// Optional ISO validity start (default: now)
        #[arg(long)]
        valid_from: Option<String>,
        /// Optional stable key; re-capturing the same key retires the prior current record
        #[arg(long)]
        fact_key: Option<String>,
        /// Id of an older decision this one supersedes
        #[arg(long)]
        supersedes: Option<String>,
    },
    /// Search the decision store
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        scope: Option<String>,
        /// Optional kind facet (comma-separated): decision, fact, reference, project, ...
        #[arg(long, value_delimiter = ',')]
        types: Vec<String>,
    },
    /// Fetch one decision by id
    Get {
        id: String,
    },
    /// Link a git commit to a decision (the "why" for that commit)
    Link {
        commit: String,
        decision: String,
        #[arg(long)]
        subject: Option<String>,
    },
    /// Bulk-import externally-minted decisions from JSON on stdin
    Import {
        /// Path to a JSON file (default: read JSON array from stdin)
        #[arg(long)]
        file: Option<String>,
    },
    /// Run as an MCP stdio server
    Serve {},
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { repo } => {
            let repo = miner::resolve_repo(repo)?;
            let decisions = miner::mine(&repo)?;
            let store = db::Store::open_default()?;
            let scope = store::scope_for(&repo);
            store.import_decisions(&scope, &decisions)?;
            println!("indexed {} decisions (scope: {scope})", decisions.len());
        }
        Command::Why {
            question,
            repo,
            limit,
        } => {
            let repo = miner::resolve_repo(repo)?;
            print!("{}", answer::ask(&question, &repo, limit)?);
        }
        Command::Capture {
            kind,
            title,
            content,
            importance,
            scope,
            id,
            valid_from,
            fact_key,
            supersedes,
        } => {
            let store = db::Store::open_default()?;
            let scope = scope.unwrap_or_else(|| "global".to_string());
            let d = store::Decision {
                subject: title,
                body: content,
                kind: kind.unwrap_or_else(|| "decision".to_string()),
                importance: importance.unwrap_or(0.5),
                source: "capture".to_string(),
                ..store::Decision::default()
            };
            let result = match id.as_deref() {
                Some(id) if !id.is_empty() => {
                    store.capture_external(&d, &scope, id, valid_from.as_deref(), fact_key.as_deref(), supersedes.as_deref())
                }
                _ => store.capture(&d, &scope, supersedes.as_deref()),
            };
            let id = result?;
            println!("captured decision {id} (scope: {scope})");
        }
        Command::Search {
            query,
            limit,
            scope,
            types,
        } => {
            let store = db::Store::open_default()?;
            let scope = scope.unwrap_or_else(|| "global".to_string());
            let hits = store.search(&query, &[scope.as_str()], &types, limit)?;
            print!("{}", answer::render(hits));
        }
        Command::Get { id } => {
            let store = db::Store::open_default()?;
            match store.get(&id)? {
                Some(d) => {
                    println!("- {}", d.subject);
                    println!("  {} · {} · {}", d.date, d.author, d.source);
                    println!("  {}", d.body);
                    let commits = store.linked_commits(&id)?;
                    if !commits.is_empty() {
                        println!("\n  linked commits:");
                        for (hash, subj) in commits {
                            println!("    {} {subj}", &hash[..hash.len().min(8)]);
                        }
                    }
                }
                None => println!("no active decision with id {id}"),
            }
        }
        Command::Link {
            commit,
            decision,
            subject,
        } => {
            let store = db::Store::open_default()?;
            let subject = subject.unwrap_or_default();
            store.link_git(&decision, &commit, &subject)?;
            println!("linked {commit} -> {decision}");
        }
        Command::Import { file } => {
            let store = db::Store::open_default()?;
            let text = match file {
                Some(path) => std::fs::read_to_string(&path)
                    .with_context(|| format!("read {path}"))?,
                None => {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf)?;
                    buf
                }
            };
            let rows: Vec<store::ExternalDecision> = serde_json::from_str(&text)?;
            let n = store.import_external(&rows)?;
            println!("imported {n} decisions");
        }
        Command::Serve {} => mcp::serve()?,
    }
    Ok(())
}
