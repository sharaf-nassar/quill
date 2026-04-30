//! File-type and sensitive-filename detection. Mirrors caveman-compress
//! detect.py + the `is_sensitive_path` heuristic from compress.py.

use std::path::Path;

const COMPRESSIBLE_EXTENSIONS: &[&str] = &["md", "txt", "markdown", "rst"];

const SKIP_EXTENSIONS: &[&str] = &[
    "py",
    "js",
    "ts",
    "tsx",
    "jsx",
    "json",
    "yaml",
    "yml",
    "toml",
    "env",
    "lock",
    "css",
    "scss",
    "html",
    "xml",
    "sql",
    "sh",
    "bash",
    "zsh",
    "go",
    "rs",
    "java",
    "c",
    "cpp",
    "h",
    "hpp",
    "rb",
    "php",
    "swift",
    "kt",
    "lua",
    "dockerfile",
    "makefile",
    "csv",
    "ini",
    "cfg",
];

const SENSITIVE_PATH_COMPONENTS: &[&str] = &[".ssh", ".aws", ".gnupg", ".kube", ".docker"];

const SENSITIVE_NAME_TOKENS: &[&str] = &[
    "secret",
    "credential",
    "password",
    "passwd",
    "apikey",
    "accesskey",
    "token",
    "privatekey",
];

const SENSITIVE_EXACT_BASENAMES: &[&str] = &[".netrc", "authorized_keys", "known_hosts"];

const SENSITIVE_BASENAME_PREFIXES: &[&str] = &[
    ".env",
    "credentials",
    "credential",
    "secrets",
    "secret",
    "passwords",
    "password",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
];

const SENSITIVE_EXTENSIONS: &[&str] = &[
    "pem", "key", "p12", "pfx", "crt", "cer", "jks", "keystore", "asc", "gpg",
];

/// Returns true for filenames that almost certainly hold secrets or PII.
/// Compressing them would ship the raw bytes to the Anthropic API.
pub fn is_sensitive_path(path: &Path) -> bool {
    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
        let lname = name.to_ascii_lowercase();
        if SENSITIVE_EXACT_BASENAMES
            .iter()
            .any(|exact| lname == *exact)
        {
            return true;
        }
        if SENSITIVE_BASENAME_PREFIXES
            .iter()
            .any(|prefix| lname.starts_with(prefix))
        {
            return true;
        }
    }

    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        let lext = ext.to_ascii_lowercase();
        if SENSITIVE_EXTENSIONS.iter().any(|e| *e == lext) {
            return true;
        }
    }

    for component in path.components() {
        if let Some(s) = component.as_os_str().to_str() {
            let lower = s.to_ascii_lowercase();
            if SENSITIVE_PATH_COMPONENTS.contains(&lower.as_str()) {
                return true;
            }
        }
    }

    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
        let normalized: String = name
            .chars()
            .filter_map(|c| match c {
                '_' | '-' | ' ' | '.' => None,
                c => Some(c.to_ascii_lowercase()),
            })
            .collect();
        if SENSITIVE_NAME_TOKENS
            .iter()
            .any(|tok| normalized.contains(tok))
        {
            return true;
        }
    }

    false
}

/// Heuristic mirror of caveman-compress detect.detect_file_type().
fn detect_file_type(path: &Path) -> &'static str {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        let lext = ext.to_ascii_lowercase();
        if COMPRESSIBLE_EXTENSIONS.iter().any(|e| *e == lext) {
            return "natural_language";
        }
        if SKIP_EXTENSIONS.iter().any(|e| *e == lext) {
            return "code";
        }
        return "unknown";
    }

    // Extensionless: read up to 50 lines and decide. Conservative: if we cannot
    // read the file, treat it as unknown (which compresses to a `Skipped`).
    let Ok(text) = std::fs::read_to_string(path) else {
        return "unknown";
    };
    let lines: Vec<&str> = text.lines().take(50).collect();
    let non_empty = lines.iter().filter(|l| !l.trim().is_empty()).count();
    if non_empty == 0 {
        return "natural_language";
    }
    let code_lines = lines
        .iter()
        .filter(|l| !l.trim().is_empty() && looks_like_code(l))
        .count();
    if code_lines as f64 / non_empty as f64 > 0.4 {
        return "code";
    }
    "natural_language"
}

fn looks_like_code(line: &str) -> bool {
    let l = line.trim_start();
    l.starts_with("import ")
        || l.starts_with("from ")
        || l.starts_with("require(")
        || l.starts_with("const ")
        || l.starts_with("let ")
        || l.starts_with("var ")
        || l.starts_with("def ")
        || l.starts_with("class ")
        || l.starts_with("function ")
        || l.starts_with("async function ")
        || l.starts_with("export ")
        || l.starts_with('@')
        || l.ends_with("};")
        || l.ends_with("});")
        || l.ends_with(']')
        || l.ends_with(';')
}

pub fn should_compress(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    if let Some(name) = path.file_name().and_then(|s| s.to_str())
        && name.ends_with(".original.md")
    {
        return false;
    }
    detect_file_type(path) == "natural_language"
}
