use crate::{
    db, miner,
    store::{Decision, Record},
};
use anyhow::Result;
use std::path::Path;

pub fn ask(question: &str, repo: &Path, limit: usize) -> Result<String> {
    let store = db::Store::open_default()?;
    let scope = crate::store::scope_for(repo);
    if store.count_for_scope(&scope)? == 0 {
        let mined = miner::mine(repo)?;
        store.import_decisions(&scope, &mined)?;
    }
    let hits = store.search(question, &[scope.as_str(), "global"], &[], limit)?;
    if hits.is_empty() {
        return Ok(format!("no decision found for: \"{question}\""));
    }
    Ok(render(hits))
}

pub fn render(decisions: Vec<Decision>) -> String {
    let mut out = String::new();
    for d in decisions {
        let location = if d.kind == "commit" {
            if d.sha.is_empty() {
                "unknown".to_string()
            } else {
                format!("commit {}", &d.sha[..d.sha.len().min(8)])
            }
        } else {
            d.source.clone()
        };
        out.push_str(&format!("- {}\n", d.subject));
        out.push_str(&format!("  {} · {} · {}\n", d.date, d.author, location));
        let snippet: String = d
            .body
            .split_whitespace()
            .take(28)
            .collect::<Vec<_>>()
            .join(" ");
        if !snippet.is_empty() {
            out.push_str(&format!("  {snippet}\n"));
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Render full records (id + temporal window), marking superseded rows so a historical
/// search can show which answers have been retired and where they point.
pub fn render_records(records: Vec<Record>) -> String {
    let mut out = String::new();
    for r in records {
        let superseded = r.superseded_by.is_some() || r.valid_until.is_some();
        let marker = if superseded {
            match &r.superseded_by {
                Some(next) if !next.is_empty() => {
                    format!(" (superseded by {})", &next[..next.len().min(8)])
                }
                _ => " (superseded)".to_string(),
            }
        } else {
            String::new()
        };
        out.push_str(&format!("- {}{}\n", r.title, marker));
        out.push_str(&format!("  {} · {} · {}\n", r.date, r.author, r.source));
        let snippet: String = r
            .content
            .split_whitespace()
            .take(28)
            .collect::<Vec<_>>()
            .join(" ");
        if !snippet.is_empty() {
            out.push_str(&format!("  {snippet}\n"));
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}
