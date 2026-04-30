//! Prompt builders ported verbatim from caveman-compress compress.py
//! (`build_compress_prompt`, `build_fix_prompt`, `strip_llm_wrapper`).

/// Strip an outer ```markdown ... ``` fence the model sometimes wraps the
/// whole output in. Inner fenced code blocks are left untouched.
pub fn strip_llm_wrapper(text: &str) -> &str {
    let trimmed = text.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() < 6 {
        return text;
    }

    let fence_char = match bytes[0] {
        b'`' => '`',
        b'~' => '~',
        _ => return text,
    };

    // Count opening fence length
    let mut open_len = 0usize;
    while open_len < bytes.len() && bytes[open_len] as char == fence_char {
        open_len += 1;
    }
    if open_len < 3 {
        return text;
    }

    // Find end of first line (the opening fence may have an info string).
    let Some(first_newline) = trimmed.find('\n') else {
        return text;
    };
    let body_start = first_newline + 1;

    // Look for a matching closing fence at the very end (allowing trailing whitespace).
    let close_marker: String = std::iter::repeat_n(fence_char, open_len).collect();
    let body = &trimmed[body_start..];
    let body_trimmed = body.trim_end();
    if let Some(stripped) = body_trimmed.strip_suffix(close_marker.as_str()) {
        let cleaned = stripped.trim_end_matches('\n');
        return cleaned;
    }
    text
}

pub fn build_compress_prompt(original: &str) -> String {
    format!(
        "Compress this markdown into caveman format.\n\
\n\
STRICT RULES:\n\
- Do NOT modify anything inside ``` code blocks\n\
- Do NOT modify anything inside inline backticks\n\
- Preserve ALL URLs exactly\n\
- Preserve ALL headings exactly\n\
- Preserve file paths and commands\n\
- Return ONLY the compressed markdown body — do NOT wrap the entire output in a ```markdown fence or any other fence. Inner code blocks from the original stay as-is; do not add a new outer fence around the whole file.\n\
\n\
Only compress natural language.\n\
\n\
TEXT:\n\
{original}\n"
    )
}

pub fn build_fix_prompt(original: &str, compressed: &str, errors: &[String]) -> String {
    let errors_str = errors
        .iter()
        .map(|e| format!("- {e}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You are fixing a caveman-compressed markdown file. Specific validation errors were found.\n\
\n\
CRITICAL RULES:\n\
- DO NOT recompress or rephrase the file\n\
- ONLY fix the listed errors — leave everything else exactly as-is\n\
- The ORIGINAL is provided as reference only (to restore missing content)\n\
- Preserve caveman style in all untouched sections\n\
\n\
ERRORS TO FIX:\n\
{errors_str}\n\
\n\
HOW TO FIX:\n\
- Missing URL: find it in ORIGINAL, restore it exactly where it belongs in COMPRESSED\n\
- Code block mismatch: find the exact code block in ORIGINAL, restore it in COMPRESSED\n\
- Heading mismatch: restore the exact heading text from ORIGINAL into COMPRESSED\n\
- Do not touch any section not mentioned in the errors\n\
\n\
ORIGINAL (reference only):\n\
{original}\n\
\n\
COMPRESSED (fix this):\n\
{compressed}\n\
\n\
Return ONLY the fixed compressed file. No explanation.\n"
    )
}
