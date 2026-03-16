/// Redact potential credentials from tool output.
///
/// Inspired by ZeroClaw's credential sanitization: detect token/key/password patterns,
/// preserve first 4 characters, replace rest with REDACTED.
pub fn redact_credentials(input: &str) -> String {
    use std::sync::LazyLock;

    static PATTERNS: LazyLock<Vec<regex_lite::Regex>> = LazyLock::new(|| {
        [
            // API keys: sk-..., xai-..., etc.
            r#"(?i)\b(sk-[a-zA-Z0-9]{4})[a-zA-Z0-9_-]{20,}\b"#,
            // Bearer tokens
            r#"(?i)(Bearer\s+[a-zA-Z0-9]{4})[a-zA-Z0-9_.\-/+=]{20,}"#,
            // Generic key=value patterns
            r#"(?i)((?:api[_-]?key|secret|token|password|authorization)\s*[=:]\s*['"]?[a-zA-Z0-9]{4})[a-zA-Z0-9_.\-/+=]{12,}"#,
        ]
        .iter()
        .filter_map(|p| regex_lite::Regex::new(p).ok())
        .collect()
    });

    let mut result = input.to_string();
    for pattern in PATTERNS.iter() {
        result = pattern.replace_all(&result, "${1}[REDACTED]").to_string();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_api_key() {
        let input = "Using key: sk-abcd1234567890abcdef1234567890";
        let output = redact_credentials(input);
        assert!(output.contains("sk-abcd"));
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("1234567890abcdef"));
    }

    #[test]
    fn test_no_false_positive_on_short_strings() {
        let input = "The key is short";
        assert_eq!(redact_credentials(input), input);
    }
}
