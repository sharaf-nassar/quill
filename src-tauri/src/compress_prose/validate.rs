//! Caveman-compress validator port (validate.py).
//!
//! Verifies that a compressed markdown file preserves the original's
//! structural invariants. Errors are blocking; warnings are best-effort.

use std::collections::BTreeSet;

#[derive(Debug, Default, Clone)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationResult {
    fn new() -> Self {
        Self {
            is_valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn add_error(&mut self, msg: String) {
        self.is_valid = false;
        self.errors.push(msg);
    }

    fn add_warning(&mut self, msg: String) {
        self.warnings.push(msg);
    }
}

pub fn validate(original: &str, compressed: &str) -> ValidationResult {
    let mut result = ValidationResult::new();
    validate_headings(original, compressed, &mut result);
    validate_code_blocks(original, compressed, &mut result);
    validate_urls(original, compressed, &mut result);
    validate_paths(original, compressed, &mut result);
    validate_bullets(original, compressed, &mut result);
    result
}

fn extract_headings(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('#') {
            let mut level = 1usize;
            let mut chars = rest.chars().peekable();
            while chars.peek() == Some(&'#') {
                level += 1;
                chars.next();
                if level >= 6 {
                    break;
                }
            }
            let after_hashes: String = chars.collect();
            if !after_hashes.starts_with(' ') && !after_hashes.starts_with('\t') {
                continue;
            }
            out.push((level, after_hashes.trim().to_string()));
        }
    }
    out
}

fn validate_headings(orig: &str, comp: &str, result: &mut ValidationResult) {
    let h1 = extract_headings(orig);
    let h2 = extract_headings(comp);
    if h1.len() != h2.len() {
        result.add_error(format!(
            "Heading count mismatch: {} vs {}",
            h1.len(),
            h2.len()
        ));
    }
    if h1 != h2 {
        result.add_warning("Heading text/order changed".into());
    }
}

/// Line-based fenced code block extractor matching caveman-compress's
/// CommonMark-aware behaviour (variable-length fences, ``` and ~~~).
fn extract_code_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut i = 0usize;
    while i < lines.len() {
        let Some((fence_char, fence_len, indent)) = parse_fence_open(lines[i]) else {
            i += 1;
            continue;
        };
        let mut block_lines: Vec<&str> = vec![lines[i]];
        i += 1;
        let mut closed = false;
        while i < lines.len() {
            if let Some((c, l, ind)) = parse_fence_open(lines[i])
                && c == fence_char
                && l >= fence_len
                && ind == indent
                && fence_info(lines[i]).is_empty()
            {
                block_lines.push(lines[i]);
                closed = true;
                i += 1;
                break;
            }
            block_lines.push(lines[i]);
            i += 1;
        }
        if closed {
            blocks.push(block_lines.join("\n"));
        }
    }
    blocks
}

fn parse_fence_open(line: &str) -> Option<(char, usize, usize)> {
    let stripped = line.trim_start_matches(' ');
    let indent = line.len() - stripped.len();
    if indent > 3 {
        return None;
    }
    let mut chars = stripped.chars();
    let first = chars.next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let mut count = 1usize;
    for c in chars {
        if c == first {
            count += 1;
        } else {
            break;
        }
    }
    if count < 3 {
        return None;
    }
    Some((first, count, indent))
}

fn fence_info(line: &str) -> &str {
    let stripped = line.trim_start_matches(' ');
    let first = stripped.chars().next().unwrap_or(' ');
    let rest = stripped.trim_start_matches(first);
    rest.trim()
}

fn validate_code_blocks(orig: &str, comp: &str, result: &mut ValidationResult) {
    let c1 = extract_code_blocks(orig);
    let c2 = extract_code_blocks(comp);
    if c1 != c2 {
        result.add_error("Code blocks not preserved exactly".into());
    }
}

fn extract_urls(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    loop {
        let Some(http_pos) = rest.find("http") else {
            break;
        };
        let after_http = &rest[http_pos..];
        let prefix_len = if after_http.starts_with("https://") {
            8
        } else if after_http.starts_with("http://") {
            7
        } else {
            rest = &rest[http_pos + 4..];
            continue;
        };
        let url_start = http_pos;
        let mut end = url_start + prefix_len;
        for (idx, ch) in after_http[prefix_len..].char_indices() {
            if ch.is_whitespace() || ch == ')' {
                end = url_start + prefix_len + idx;
                break;
            }
            end = url_start + prefix_len + idx + ch.len_utf8();
        }
        out.insert(rest[url_start..end].to_string());
        if end >= rest.len() {
            break;
        }
        rest = &rest[end..];
    }
    out
}

fn validate_urls(orig: &str, comp: &str, result: &mut ValidationResult) {
    let u1 = extract_urls(orig);
    let u2 = extract_urls(comp);
    if u1 != u2 {
        let lost: Vec<&str> = u1.difference(&u2).map(|s| s.as_str()).collect();
        let added: Vec<&str> = u2.difference(&u1).map(|s| s.as_str()).collect();
        result.add_error(format!("URL mismatch: lost={lost:?}, added={added:?}"));
    }
}

fn extract_paths(text: &str) -> BTreeSet<String> {
    // Conservative approximation of caveman-compress's PATH_REGEX. Matches
    // tokens that contain a '/' (and aren't URLs) or start with `./`, `../`,
    // `/`. The original Python regex is fairly loose; ours mirrors the spirit
    // but avoids false matches inside URLs.
    let mut out = BTreeSet::new();
    for raw in text.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ',' | ';'))
    {
        let token = raw.trim_matches(|c: char| {
            matches!(
                c,
                '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '.' | ',' | ';' | ':' | '!' | '?'
            )
        });
        if token.is_empty() {
            continue;
        }
        if token.starts_with("http://") || token.starts_with("https://") {
            continue;
        }
        if token.starts_with("./") || token.starts_with("../") || token.starts_with('/') {
            out.insert(token.to_string());
            continue;
        }
        if token.contains('/')
            && token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | '~'))
        {
            out.insert(token.to_string());
        }
    }
    out
}

fn validate_paths(orig: &str, comp: &str, result: &mut ValidationResult) {
    let p1 = extract_paths(orig);
    let p2 = extract_paths(comp);
    if p1 != p2 {
        let lost: Vec<&str> = p1.difference(&p2).map(|s| s.as_str()).collect();
        let added: Vec<&str> = p2.difference(&p1).map(|s| s.as_str()).collect();
        result.add_warning(format!("Path mismatch: lost={lost:?}, added={added:?}"));
    }
}

fn count_bullets(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let l = line.trim_start();
            (l.starts_with("- ") || l.starts_with("* ") || l.starts_with("+ "))
                && !l.starts_with("---")
        })
        .count()
}

fn validate_bullets(orig: &str, comp: &str, result: &mut ValidationResult) {
    let b1 = count_bullets(orig);
    let b2 = count_bullets(comp);
    if b1 == 0 {
        return;
    }
    let diff = (b1 as f64 - b2 as f64).abs() / b1 as f64;
    if diff > 0.15 {
        result.add_warning(format!("Bullet count changed too much: {b1} -> {b2}"));
    }
}
