/// Sanitize text for safe embedding in an LLM prompt.
/// Strips characters that could be used for prompt injection.
pub fn sanitize_for_prompt(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '[' | ']' | '{' | '}' | '`' => ' ',
            '\n' | '\r' => ' ',
            _ => c,
        })
        .collect()
}

/// Escape content for safe embedding in structured XML prompts.
/// Replaces & with &amp; and < with &lt; universally to prevent content
/// from breaking out of XML wrapper tags. The wrapper tags themselves
/// are added by build_prompt AFTER escaping, so they remain valid.
/// Markdown structure (brackets, braces, backticks, newlines) is preserved.
///
/// NOTE: This replaces `sanitize_for_prompt` for the memory optimizer.
/// The learning system still uses `sanitize_for_prompt` which strips
/// brackets/backticks/newlines — a separate migration.
pub fn escape_for_prompt(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;")
}

/// Truncate a string at a valid UTF-8 char boundary.
pub fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Smart observation compression: extracts key signals instead of naive truncation.
/// Prioritizes: error messages > file paths > tool outcomes > general content.
pub fn compress_observation(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return sanitize_for_prompt(text);
    }

    let mut signals: Vec<&str> = Vec::new();
    let mut remaining_budget = max_len;

    // Extract error lines (highest priority)
    for line in text.lines() {
        let lower = line.to_lowercase();
        if lower.contains("error") || lower.contains("failed") || lower.contains("panic") {
            let trimmed = line.trim();
            if !trimmed.is_empty() && trimmed.len() <= remaining_budget {
                signals.push(trimmed);
                remaining_budget = remaining_budget.saturating_sub(trimmed.len() + 2);
            }
        }
    }

    // Extract file paths (second priority)
    for line in text.lines() {
        if remaining_budget < 20 {
            break;
        }
        let trimmed = line.trim();
        if (trimmed.contains('/') || trimmed.contains('\\'))
            && (trimmed.ends_with(".rs")
                || trimmed.ends_with(".ts")
                || trimmed.ends_with(".tsx")
                || trimmed.ends_with(".js")
                || trimmed.ends_with(".py")
                || trimmed.contains("file_path"))
            && !signals.contains(&trimmed)
            && trimmed.len() <= remaining_budget
        {
            signals.push(trimmed);
            remaining_budget = remaining_budget.saturating_sub(trimmed.len() + 2);
        }
    }

    // Fill remainder with truncated content (UTF-8 safe)
    if remaining_budget > 50 {
        let truncated = safe_truncate(text, remaining_budget);
        let result = format!("{} ... {}", signals.join(" | "), truncated);
        return sanitize_for_prompt(safe_truncate(&result, max_len));
    }

    let joined = signals.join(" | ");
    sanitize_for_prompt(safe_truncate(&joined, max_len))
}

/// Mask secrets/credentials in text before it is sent to the inference
/// subprocess (FR-012, Clarification Q1 = B). This is SEPARATE from and
/// ADDITIONAL to `sanitize_for_prompt` (which defends against prompt
/// injection, not data exposure).
///
/// Contract:
/// - Mask API keys, access/refresh/bearer tokens, `.env`-style
///   `KEY=value` assignments, and other recognizable credentials with a
///   single fixed masking token (so the pattern "an auth value was here"
///   survives — rule-neutral — while the literal secret does not).
/// - Preserve all surrounding behavioral/semantic text unchanged.
/// - Be idempotent: `redact_secrets(redact_secrets(x)) == redact_secrets(x)`
///   (the MASK token matches none of the patterns below, so re-running
///   over already-redacted text is a no-op).
// Superseded by the canonical `crate::redaction::redact` (R-1); its sole
// runtime caller (Stream C, learning.rs) was migrated by feature-005 T018.
// Retained as the proven anchored-shape reference and re-exercised by the
// redaction-adoption tests (T012); allow until its retirement task lands.
#[allow(dead_code)]
pub fn redact_secrets(input: &str) -> String {
    use regex::Regex;
    use std::sync::LazyLock;

    const MASK: &str = "\u{2039}redacted\u{203a}";

    // High-precision anchored patterns: mask the secret VALUE, leave
    // structure intact so behavioral patterns survive (rule-neutral,
    // SC-006). MASK contains none of the matched shapes → idempotent.
    static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        vec![
            // PEM private-key blocks (multi-line).
            Regex::new(
                r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            )
            .unwrap(),
            // JWTs.
            Regex::new(r"\beyJ[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}\b").unwrap(),
            // Provider key prefixes (Anthropic/OpenAI/Vercel/GitHub/Slack/AWS/Google/Stripe).
            Regex::new(
                r"\b(sk-ant-[A-Za-z0-9_-]{16,}|sk-[A-Za-z0-9]{20,}|vck_[A-Za-z0-9]{16,}|gh[posru]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{20,}|rk_live_[0-9A-Za-z]{16,})\b",
            )
            .unwrap(),
            // Authorization: Bearer <token>.
            Regex::new(r"(?i)(authorization\s*:\s*bearer\s+|\bbearer\s+)[A-Za-z0-9._\-]{10,}")
                .unwrap(),
        ]
    });
    // Sensitive KEY=VALUE / KEY: VALUE — preserve the key, mask the value.
    static ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?im)\b([A-Z0-9_]*(?:SECRET|TOKEN|KEY|PASSWORD|PASSWD|API|CREDENTIAL)[A-Z0-9_]*)\s*([=:])\s*"?[^\s"']{3,}"?"#,
        )
        .unwrap()
    });

    let mut out = input.to_string();
    for re in PATTERNS.iter() {
        out = re.replace_all(&out, MASK).into_owned();
    }
    ASSIGN
        .replace_all(&out, |c: &regex::Captures| {
            format!("{}{} {MASK}", &c[1], &c[2])
        })
        .into_owned()
}

/// Returns true if the name is safe for use as a memory filename.
/// Allows lowercase ASCII letters, digits, hyphens, and underscores.
#[allow(dead_code)] // Used by memory_optimizer.rs in upcoming tasks
pub fn is_safe_memory_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && !name.starts_with('-')
        && !name.starts_with('_')
}
