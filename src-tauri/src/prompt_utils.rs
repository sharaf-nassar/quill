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
