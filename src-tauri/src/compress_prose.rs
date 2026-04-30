//! Caveman-compress style prose compression for the Memory Optimizer.
//!
//! Ported from https://github.com/JuliusBrussee/caveman/tree/main/caveman-compress
//! (compress.py, validate.py, detect.py). Reuses Quill's existing AI client so
//! it inherits the same auth, rate-limit retry, and OAuth handling as the rest
//! of the Memory Optimizer — no shell-out to the `claude` CLI, no new env vars.
//!
//! For each scanned memory file:
//! 1. denylist sensitive filenames (.env, *.pem, *.key, credentials, tokens, ...)
//! 2. detect natural language (.md/.txt/.rst, or extensionless that scans as prose)
//! 3. ask the LLM to rewrite in caveman style
//! 4. validate (headings, code blocks, URLs, paths, bullets) against the original
//! 5. up to 2 cherry-pick fix retries via a targeted "fix only the listed errors" prompt
//! 6. write `<file>.original.md` backup, then overwrite original with the
//!    compressed version. On final failure, restore the backup and remove it.

use std::path::Path;

mod detect;
mod prompt;
mod validate;

pub use detect::is_sensitive_path;
pub use detect::should_compress;
pub use prompt::{build_compress_prompt, build_fix_prompt, strip_llm_wrapper};
pub use validate::validate as validate_pair;

pub const MAX_RETRIES: usize = 2;
pub const MAX_FILE_SIZE: u64 = 500_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressOutcome {
    /// File was rewritten and validated.
    Compressed,
    /// File looked like prose but the LLM output failed validation past the
    /// retry budget; original is restored.
    Failed { errors: Vec<String> },
    /// File was skipped without writing (sensitive, not prose, backup exists, ...).
    Skipped { reason: String },
}

/// Pluggable LLM call so the orchestrator (memory_optimizer) can reuse its
/// existing ai_client wiring while keeping this module testable.
pub type LlmCall<'a> =
    &'a (dyn Fn(String) -> futures::future::BoxFuture<'a, Result<String, String>> + Sync);

/// Compress a single file in place. Pure orchestrator: the actual LLM call
/// goes through the supplied closure so we do not hard-couple to a single
/// client implementation.
pub async fn compress_file(path: &Path, call_llm: LlmCall<'_>) -> Result<CompressOutcome, String> {
    if is_sensitive_path(path) {
        return Ok(CompressOutcome::Skipped {
            reason:
                "sensitive filename — refused (compression sends file contents to a third-party API)"
                    .to_string(),
        });
    }

    if !path.is_file() {
        return Ok(CompressOutcome::Skipped {
            reason: "not a regular file".into(),
        });
    }

    let metadata = std::fs::metadata(path).map_err(|e| format!("stat failed: {e}"))?;
    if metadata.len() > MAX_FILE_SIZE {
        return Ok(CompressOutcome::Skipped {
            reason: format!("file > {} bytes (max {})", metadata.len(), MAX_FILE_SIZE),
        });
    }

    if !should_compress(path) {
        return Ok(CompressOutcome::Skipped {
            reason: "not natural-language prose".into(),
        });
    }

    let original_text = std::fs::read_to_string(path).map_err(|e| format!("read failed: {e}"))?;

    let backup_path = backup_path_for(path);
    if backup_path.exists() {
        return Ok(CompressOutcome::Skipped {
            reason: format!(
                ".original.md backup already exists at {} (refusing to overwrite)",
                backup_path.display()
            ),
        });
    }

    // Step 1: compress
    let compressed_raw = call_llm(build_compress_prompt(&original_text)).await?;
    let mut compressed = strip_llm_wrapper(&compressed_raw).to_string();

    // Step 2: write backup + draft, then validate-with-retry
    std::fs::write(&backup_path, &original_text)
        .map_err(|e| format!("backup write failed: {e}"))?;
    std::fs::write(path, &compressed).map_err(|e| format!("draft write failed: {e}"))?;

    for attempt in 0..MAX_RETRIES {
        let result = validate_pair(&original_text, &compressed);
        if result.is_valid {
            return Ok(CompressOutcome::Compressed);
        }

        if attempt + 1 == MAX_RETRIES {
            // Restore original; remove backup so the workspace looks untouched.
            std::fs::write(path, &original_text).map_err(|e| format!("restore failed: {e}"))?;
            let _ = std::fs::remove_file(&backup_path);
            return Ok(CompressOutcome::Failed {
                errors: result.errors,
            });
        }

        let fix_raw = call_llm(build_fix_prompt(
            &original_text,
            &compressed,
            &result.errors,
        ))
        .await?;
        compressed = strip_llm_wrapper(&fix_raw).to_string();
        std::fs::write(path, &compressed).map_err(|e| format!("fix write failed: {e}"))?;
    }

    // unreachable: the loop returns on the last iteration
    Ok(CompressOutcome::Compressed)
}

fn backup_path_for(path: &Path) -> std::path::PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let backup_name = format!("{stem}.original.md");
    match path.parent() {
        Some(parent) => parent.join(backup_name),
        None => std::path::PathBuf::from(backup_name),
    }
}
