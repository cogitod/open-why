use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use open_why::{
    answer, db, embed, mcp, miner, store, CurrentRecordResolution, RankExplanation, Record,
};

/// Ask why a decision was made, with its evidence.
#[derive(Parser)]
#[command(
    name = "why",
    version,
    about = "Ask why a decision was made, with its evidence."
)]
struct Cli {
    /// The question to answer. Bare `why "..."` asks directly; no subcommand needed.
    #[arg(value_name = "QUESTION", num_args = 1.., trailing_var_arg = true)]
    question: Vec<String>,

    /// Repo path or git URL for a bare question (default: current directory)
    #[arg(long, value_name = "REPO")]
    repo: Option<String>,

    /// Number of results for a bare question
    #[arg(long, default_value_t = 5)]
    limit: usize,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Index a repository's decision history (commits + ADRs) into the store
    Init {
        /// Path or git URL (default: current directory)
        repo: Option<String>,
    },
    /// Capture a decision into the store (idempotent; supersedes optional)
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
        /// Include superseded decisions (historical mode)
        #[arg(long)]
        historical: bool,
        /// Show per-result ranking components (similarity, importance, recency, RRF)
        #[arg(long)]
        explain: bool,
        /// Also show near-miss candidates that just missed the top-N
        #[arg(long)]
        explain_drops: bool,
    },
    /// Fetch one decision by id
    Get {
        id: String,
        /// Reach past supersession and print the full chain
        #[arg(long)]
        historical: bool,
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
    /// Download the local embedding model so no env var is needed
    FetchModel {},
    /// Record retrieval feedback on a decision (closes the usage→quality loop)
    Feedback {
        id: String,
        /// Mark the decision helpful (raises its effectiveness)
        #[arg(long)]
        helpful: bool,
        /// Mark the decision not helpful (lowers its effectiveness)
        #[arg(long)]
        not_helpful: bool,
    },
    /// Run as an MCP stdio server
    Serve {},
    /// Run a long-lived MCP server shared by every `why serve` on this machine (for a
    /// supervisor like launchd; plain `serve` connects to it automatically when present)
    ServeDaemon {},
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => ask_bare(&cli),
        Some(cmd) => match cmd {
            Command::Init { repo } => {
                let repo = miner::resolve_repo(repo)?;
                let decisions = miner::mine(&repo)?;
                let store = db::Store::open_default()?;
                let scope = store::scope_for(&repo);
                store.import_decisions(&scope, &decisions)?;
                println!("indexed {} decisions (scope: {scope})", decisions.len());
                Ok(())
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
                    Some(id) if !id.is_empty() => store.capture_external(
                        &d,
                        &scope,
                        id,
                        valid_from.as_deref(),
                        fact_key.as_deref(),
                        supersedes.as_deref(),
                    ),
                    _ => store.capture(&d, &scope, supersedes.as_deref()),
                };
                let id = result?;
                println!("captured decision {id} (scope: {scope})");
                Ok(())
            }
            Command::Search {
                query,
                limit,
                scope,
                types,
                historical,
                explain,
                explain_drops,
            } => {
                let store = db::Store::open_default()?;
                let scope = scope.unwrap_or_else(|| "global".to_string());
                if explain_drops {
                    let (results, drops) = store.search_records_drops(
                        &query,
                        &[scope.as_str()],
                        &types,
                        limit,
                        historical,
                        5,
                    )?;
                    print!("{}", render_explain(results, true));
                    if !drops.is_empty() {
                        println!("--- near-miss (did not make the top-N) ---");
                        print!("{}", render_explain(drops, true));
                    }
                } else if explain {
                    let hits = store.search_records_explain(
                        &query,
                        &[scope.as_str()],
                        &types,
                        limit,
                        historical,
                    )?;
                    print!("{}", render_explain(hits, false));
                } else if historical {
                    let hits = store.search_records_with(
                        &query,
                        &[scope.as_str()],
                        &types,
                        limit,
                        true,
                    )?;
                    print!("{}", answer::render_records(hits));
                } else {
                    let hits = store.search_records(&query, &[scope.as_str()], &types, limit)?;
                    print!("{}", answer::render_records(hits));
                }
                Ok(())
            }
            Command::Get { id, historical } => {
                let store = db::Store::open_default()?;
                if historical {
                    let chain = store.supersession_chain(&id, 20)?;
                    if chain.is_empty() {
                        println!("no decision with id {id}");
                    } else {
                        for (i, r) in chain.iter().enumerate() {
                            let label = if i + 1 == chain.len() {
                                if r.superseded_by.is_some() {
                                    "superseded → (successor not in store)"
                                } else {
                                    "current"
                                }
                            } else if i == 0 {
                                "oldest"
                            } else {
                                "→"
                            };
                            println!("- {} [{label}] {}", r.title, r.date);
                            println!("  {} · {}", r.kind, r.source);
                        }
                    }
                } else {
                    match store.get_current_evidence(&id)? {
                        CurrentRecordResolution::Ok {
                            requested_id,
                            current_id,
                            record,
                            git_refs,
                            supersession_chain,
                            as_of,
                            ..
                        } => {
                            println!("- {} [{}]", record.title, current_id);
                            println!("  requested: {requested_id} · current as of {as_of}");
                            println!("  {} · {} · {}", record.date, record.author, record.source);
                            println!("  {}", record.content);
                            if supersession_chain.len() > 1 {
                                println!("\n  supersession: {}", supersession_chain.join(" -> "));
                            }
                            if !git_refs.is_empty() {
                                println!("\n  linked commits:");
                                for git_ref in git_refs {
                                    println!(
                                        "    {} {}",
                                        &git_ref.commit_hash[..git_ref.commit_hash.len().min(8)],
                                        git_ref.commit_subject
                                    );
                                }
                            }
                        }
                        CurrentRecordResolution::Error { code, message, .. } => {
                            println!("{code:?}: {message}")
                        }
                    }
                }
                Ok(())
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
                Ok(())
            }
            Command::Import { file } => {
                let store = db::Store::open_default()?;
                let text = match file {
                    Some(path) => {
                        std::fs::read_to_string(&path).with_context(|| format!("read {path}"))?
                    }
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
                Ok(())
            }
            Command::FetchModel {} => {
                let dir = embed::fetch_model()?;
                println!("model ready at {}", dir.display());
                Ok(())
            }
            Command::Feedback {
                id,
                helpful,
                not_helpful,
            } => {
                if helpful == not_helpful {
                    anyhow::bail!("pass exactly one of --helpful or --not-helpful");
                }
                let store = db::Store::open_default()?;
                match store.feedback(&id, helpful)? {
                    Some(eff) => println!(
                        "recorded {} feedback on {id}: effectiveness now {eff:.3}",
                        if helpful { "helpful" } else { "not-helpful" }
                    ),
                    None => println!("no active decision with id {id}"),
                }
                Ok(())
            }
            Command::Serve {} => mcp::serve(),
            Command::ServeDaemon {} => mcp::serve_daemon(),
        },
    }
}

fn ask_bare(cli: &Cli) -> Result<()> {
    let question = cli.question.join(" ").trim().to_string();
    if question.is_empty() {
        Cli::command().print_help()?;
        println!();
        return Ok(());
    }
    let repo = miner::resolve_repo(cli.repo.clone())?;
    print!("{}", answer::ask(&question, &repo, cli.limit)?);
    Ok(())
}

/// Render records with their ranking explanation. `numbered` adds the final rank position, used
/// to show near-miss drops relative to the kept results.
fn render_explain(pairs: Vec<(Record, RankExplanation)>, numbered: bool) -> String {
    let mut out = String::new();
    for (idx, (r, e)) in pairs.into_iter().enumerate() {
        let prefix = if numbered {
            format!("#{} ", idx + 1)
        } else {
            String::new()
        };
        out.push_str(&format!("- {}{}\n", prefix, r.title));
        out.push_str(&format!("  {} · {} · {}\n", r.date, r.author, r.source));
        let sem = e
            .semantic_rank
            .map(|r| format!("sem#{r}"))
            .unwrap_or_else(|| "sem=n/a".to_string());
        let lex = e
            .lexical_rank
            .map(|r| format!("lex#{r}"))
            .unwrap_or_else(|| "lex=n/a".to_string());
        out.push_str(&format!(
            "  sim={:.3} imp={:.2} eff={:.2} age={:.0}d dec={:.2} hyb={:.3} {sem} {lex} rrf={:.4}\n",
            e.similarity, e.importance, e.effectiveness, e.age_days, e.recency_decay, e.hybrid_score, e.rrf_score
        ));
        out.push('\n');
    }
    out.trim_end().to_string()
}
