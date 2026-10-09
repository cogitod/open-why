//! Reject recognizable credentials before ingestion. This is not a confidentiality classifier.
use regex::RegexSet;
use serde::Serialize;
use serde_json::Value;
use std::sync::OnceLock;

/// Rejection deliberately carries neither the matched value nor surrounding text.
#[derive(Debug, Clone, Copy)]
pub struct SensitiveDataRejected;

impl std::fmt::Display for SensitiveDataRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("input rejected by sensitive-data policy; remove credentials before retrying")
    }
}
impl std::error::Error for SensitiveDataRejected {}

fn patterns() -> &'static RegexSet {
    static PATTERNS: OnceLock<RegexSet> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        RegexSet::new([
            r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----",
            r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b",
            r"\bgh[pousr]_[A-Za-z0-9]{36,}\b",
            r"\bgithub_pat_[A-Za-z0-9_]{20,}\b",
            r"\bsk-(?:(?:ant|proj|svcacct)-)?[A-Za-z0-9_-]{20,}\b",
            r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b",
            r"\b[rs]k_live_[A-Za-z0-9]{16,}\b",
            r"\bAIza[A-Za-z0-9_-]{35}\b",
            r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
            r#"(?i)\bauthorization[ \t"']*[:=][ \t"']*(?:bearer|basic)[ \t]+[a-z0-9_./+=-]+"#,
            r"(?i)\b[a-z][a-z0-9+.-]*://[^\s/@:]+:[^\s/@]+@",
            r"(?i)\bhttps?://[^\s/@]+@",
            r#"(?i)\b(?:[a-z0-9]+[_-])*(?:password|passwd|api[_-]?key|api[_-]?token|access[_-]?token|auth[_-]?token|client[_-]?secret)[ \t"']*[:=][ \t"']*[.]*[a-z0-9_/+=-][a-z0-9_/+=.-]*"#,
        ]).expect("constant credential patterns must compile")
    })
}

/// Inspect a raw string without printing or retaining matches.
pub fn check_text(text: &str) -> Result<(), SensitiveDataRejected> {
    if patterns().is_match(text) {
        Err(SensitiveDataRejected)
    } else {
        // Tags may themselves be JSON text. Decode those strings before checking
        // them; otherwise an escaped credential could reach the embedder.
        if matches!(
            text.trim_start().as_bytes().first(),
            Some(b'[' | b'{' | b'"')
        ) {
            if let Ok(value) = serde_json::from_str::<Value>(text) {
                check_value(&value)?;
            }
        }
        Ok(())
    }
}

/// Inspect decoded strings, so JSON escaping cannot conceal a credential.
pub(crate) fn check_value(value: &Value) -> Result<(), SensitiveDataRejected> {
    match value {
        Value::String(text) => check_text(text),
        Value::Array(values) => values.iter().try_for_each(check_value),
        Value::Object(values) => values.iter().try_for_each(|(key, value)| {
            check_text(key)?;
            // Preserve the field/value relationship after JSON Unicode decoding.
            if let Value::String(text) = value {
                check_text(&format!("{key}={text}"))?;
            }
            check_value(value)
        }),
        _ => Ok(()),
    }
}

pub(crate) fn check<T: Serialize + ?Sized>(input: &T) -> anyhow::Result<()> {
    check_value(&serde_json::to_value(input)?)?;
    Ok(())
}
