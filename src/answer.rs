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

#[cfg(test)]
mod tests {
    use super::*;

    fn base_decision() -> Decision {
        Decision {
            sha: String::new(),
            author: "adrian".to_string(),
            date: "2026-01-01".to_string(),
            updated_at: String::new(),
            subject: "Use SQLite".to_string(),
            body: String::new(),
            source: "commit".to_string(),
            importance: 0.5,
            kind: "commit".to_string(),
            access_count: 0,
            effectiveness: 0.5,
            embedding: None,
        }
    }

    #[test]
    fn render_shortens_commit_sha_and_truncates_snippet_to_28_words() {
        let words: Vec<String> = (0..40).map(|i| format!("word{i}")).collect();
        let mut d = base_decision();
        d.sha = "0123456789abcdef".to_string();
        d.body = words.join(" ");
        let out = render(vec![d]);
        assert!(out.contains("commit 01234567"));
        assert!(!out.contains("0123456789abcdef"));
        let snippet_line = out.lines().nth(2).unwrap();
        assert_eq!(snippet_line.split_whitespace().count(), 28);
    }

    #[test]
    fn render_reports_unknown_location_for_empty_commit_sha() {
        let out = render(vec![base_decision()]);
        assert!(out.contains("unknown"));
    }

    #[test]
    fn render_uses_source_path_for_non_commit_kind() {
        let mut d = base_decision();
        d.kind = "adr".to_string();
        d.source = "docs/ADR-001.md".to_string();
        let out = render(vec![d]);
        assert!(out.contains("docs/ADR-001.md"));
    }

    fn base_record() -> Record {
        Record {
            id: "id-1".to_string(),
            kind: "fact".to_string(),
            title: "A fact".to_string(),
            content: String::new(),
            importance: 0.5,
            source: String::new(),
            author: "adrian".to_string(),
            date: "2026-01-01".to_string(),
            commit_sha: String::new(),
            scope: "global".to_string(),
            superseded_by: None,
            valid_from: None,
            valid_until: None,
            updated_at: String::new(),
            access_count: 0,
            effectiveness: 0.5,
            embedding: None,
        }
    }

    #[test]
    fn render_records_marks_superseded_rows_with_shortened_pointer() {
        let mut r = base_record();
        r.superseded_by = Some("abcdefghij".to_string());
        let out = render_records(vec![r]);
        assert!(out.contains("(superseded by abcdefgh)"));
    }

    #[test]
    fn render_records_marks_retired_without_pointer() {
        let mut r = base_record();
        r.valid_until = Some("2026-02-01".to_string());
        let out = render_records(vec![r]);
        assert!(out.contains("(superseded)"));
        assert!(!out.contains("(superseded by"));
    }

    #[test]
    fn render_records_has_no_marker_when_current() {
        let out = render_records(vec![base_record()]);
        assert!(!out.contains("superseded"));
    }
}
