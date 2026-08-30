use clap::Parser;
use open_why::{answer, miner};

/// Ask why a decision was made — with the evidence.
#[derive(Parser)]
#[command(name = "why", version, about = "Ask why a decision was made — with the evidence.")]
struct Cli {
    /// The question to answer (may contain spaces; quote it or type it plain)
    #[arg(required = true)]
    question: Vec<String>,
    /// Path or git URL (default: current directory)
    #[arg(long)]
    repo: Option<String>,
    /// Number of results to show
    #[arg(long, default_value_t = 5)]
    limit: usize,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let question = cli.question.join(" ");
    let repo = miner::resolve_repo(cli.repo)?;
    print!("{}", answer::ask(&question, &repo, cli.limit)?);
    Ok(())
}
