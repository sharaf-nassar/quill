//! Shared "brevity" managed instruction block, ported from caveman-compress
//! (https://github.com/JuliusBrussee/caveman/tree/main/caveman-compress).
//!
//! Both Claude Code and Codex write the same caveman-style instruction block
//! into their managed memory file (`~/.claude/CLAUDE.md` or
//! `~/.codex/AGENTS.md`). The block uses a single shared marker pair so that
//! when both files canonicalize to the same path (the common AGENTS.md ->
//! CLAUDE.md symlink case) only one block ever lives in the file.

use std::fs;
use std::path::{Path, PathBuf};

use crate::integrations::types::IntegrationProvider;

pub const BREVITY_INSTRUCTIONS: &str = "## Quill Brevity Profile\n\
\n\
Caveman compression style. Reduce input + output tokens. Preserve technical substance.\n\
\n\
- Drop articles, filler, hedging, pleasantries (`a`, `the`, `just`, `really`, `basically`, `you should`, `make sure to`).\n\
- Replace verbose phrasing: `in order to` -> `to`, `utilize` -> `use`, `the reason is because` -> `because`.\n\
- Fragments OK. Imperative mood. State the action; do not narrate.\n\
- Drop connective fluff (`however`, `furthermore`, `additionally`).\n\
- Merge bullets that say the same thing.\n\
- One example per pattern, not three.\n\
\n\
Preserve EXACTLY (never modify):\n\
\n\
- Code blocks (fenced ```` ``` ```` and indented).\n\
- Inline code in backticks.\n\
- URLs and markdown links.\n\
- File paths (`/src/...`, `./config.yaml`, `~/.claude/...`).\n\
- Commands (`npm install`, `git commit`, `cargo test`).\n\
- Library, API, protocol, project, and proper-noun names.\n\
- Numbers, versions, dates, env vars (`$HOME`, `NODE_ENV`).\n\
- Markdown heading text and bullet/numbering structure.\n\
\n\
Apply to your own prose responses. Do NOT rewrite user prompts, file contents, code, or tool output.\n";

pub const BREVITY_BLOCK_START: &str = "<!-- quill-managed:brevity:start -->";
pub const BREVITY_BLOCK_END: &str = "<!-- quill-managed:brevity:end -->";

pub fn target_path(provider: IntegrationProvider) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    match provider {
        IntegrationProvider::Claude => Some(home.join(".claude").join("CLAUDE.md")),
        IntegrationProvider::Codex => Some(home.join(".codex").join("AGENTS.md")),
        IntegrationProvider::MiniMax => None,
    }
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        path.parent()
            .and_then(|p| fs::canonicalize(p).ok())
            .and_then(|p| path.file_name().map(|n| p.join(n)))
            .unwrap_or_else(|| path.to_path_buf())
    })
}

/// Apply the brevity block to the given provider's instruction file.
///
/// `present` is the desired state for THIS provider's file.
/// `also_present_providers` lists OTHER providers whose files should also keep
/// the block — used to detect the AGENTS.md → CLAUDE.md symlink case where
/// stripping for one provider would clobber a block another provider still
/// wants.
pub fn apply_block(
    provider: IntegrationProvider,
    present: bool,
    also_present_providers: &[IntegrationProvider],
) -> Result<(), String> {
    if matches!(provider, IntegrationProvider::MiniMax) {
        return Err(
            "Brevity profile is not supported for MiniMax (no managed instruction file).".into(),
        );
    }
    let Some(path) = target_path(provider) else {
        return Err("Cannot determine brevity target path".into());
    };
    let effective_present = present || shares_canonical_path(provider, also_present_providers);
    write_brevity_block(&path, effective_present)
}

fn shares_canonical_path(
    provider: IntegrationProvider,
    also_present_providers: &[IntegrationProvider],
) -> bool {
    let Some(this_path) = target_path(provider) else {
        return false;
    };
    let this_canonical = canonical(&this_path);
    also_present_providers
        .iter()
        .filter(|other| **other != provider)
        .any(|other| {
            target_path(*other)
                .map(|p| canonical(&p) == this_canonical)
                .unwrap_or(false)
        })
}

fn write_brevity_block(path: &Path, present: bool) -> Result<(), String> {
    let original = match fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(format!("Failed to read {}: {err}", path.display())),
    };

    if original.is_none() && !present {
        return Ok(());
    }

    let content = original.clone().unwrap_or_default();
    let stripped = strip_block_pair(&content, BREVITY_BLOCK_START, BREVITY_BLOCK_END);
    let updated = if present {
        let block = format!(
            "{BREVITY_BLOCK_START}\n{}\n{BREVITY_BLOCK_END}",
            BREVITY_INSTRUCTIONS.trim()
        );
        if stripped.trim().is_empty() {
            format!("{block}\n")
        } else {
            format!("{}\n\n{block}\n", stripped.trim_end_matches('\n'))
        }
    } else {
        stripped
    };

    if Some(&updated) == original.as_ref() {
        return Ok(());
    }

    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
    }
    fs::write(path, &updated)
        .map_err(|err| format!("Failed to write {}: {err}", path.display()))?;
    log::info!(
        "{} Quill brevity section in {}",
        if present { "Wrote" } else { "Removed" },
        path.display()
    );
    Ok(())
}

fn strip_block_pair(content: &str, start: &str, end: &str) -> String {
    let mut result = content.to_string();
    while let Some(s) = result.find(start) {
        let Some(e) = result[s..].find(end).map(|pos| s + pos) else {
            break;
        };
        let block_end = e + end.len();
        let mut left = s;
        while left > 0 && result.as_bytes()[left - 1] == b'\n' {
            left -= 1;
        }
        let mut right = block_end;
        while right < result.len() && result.as_bytes()[right] == b'\n' {
            right += 1;
        }
        let has_left = left > 0;
        let has_right = right < result.len();
        let replacement = if has_left && has_right {
            "\n\n"
        } else if has_left || has_right {
            "\n"
        } else {
            ""
        };
        result.replace_range(left..right, replacement);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    fn run(input: &str, present: bool) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        fs::write(&path, input).unwrap();
        write_brevity_block(&path, present).unwrap();
        fs::read_to_string(&path).unwrap()
    }

    #[test]
    fn replaces_existing_block_in_place() {
        let input =
            format!("header\n\n{BREVITY_BLOCK_START}\nstale\n{BREVITY_BLOCK_END}\n\nfooter\n");
        let out = run(&input, true);
        assert_eq!(count(&out, BREVITY_BLOCK_START), 1);
        assert!(out.contains(BREVITY_INSTRUCTIONS.trim()));
        assert!(!out.contains("stale"));
        assert!(out.contains("header") && out.contains("footer"));
    }

    #[test]
    fn idempotent_when_already_present() {
        let input = "intro\n";
        let first = run(input, true);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        fs::write(&path, &first).unwrap();
        write_brevity_block(&path, true).unwrap();
        let second = fs::read_to_string(&path).unwrap();
        assert_eq!(first, second, "second apply must be a no-op");
    }

    #[test]
    fn strips_block_when_not_present() {
        let input = format!("{BREVITY_BLOCK_START}\nbody\n{BREVITY_BLOCK_END}\n");
        let out = run(&input, false);
        assert_eq!(count(&out, BREVITY_BLOCK_START), 0);
        assert_eq!(count(&out, BREVITY_BLOCK_END), 0);
    }

    #[test]
    fn creates_file_when_missing_and_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("CLAUDE.md");
        write_brevity_block(&path, true).unwrap();
        let out = fs::read_to_string(&path).unwrap();
        assert!(out.contains(BREVITY_BLOCK_START));
        assert!(out.contains(BREVITY_BLOCK_END));
    }

    #[test]
    fn no_op_when_missing_and_not_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        write_brevity_block(&path, false).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn preserves_other_managed_blocks() {
        let input =
            "<!-- quill-managed:claude:start -->\nmain\n<!-- quill-managed:claude:end -->\n";
        let out = run(input, true);
        assert!(out.contains("<!-- quill-managed:claude:start -->"));
        assert!(out.contains("main"));
        assert!(out.contains("<!-- quill-managed:claude:end -->"));
        assert_eq!(count(&out, BREVITY_BLOCK_START), 1);
    }
}
