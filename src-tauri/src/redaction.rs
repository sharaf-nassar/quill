//! Shared secret/PII redaction boundary for the learning pipeline.
//!
//! Feature 005 (Learning System Hardening), finding C-1 / contract
//! `specs/005-learning-system-hardening/contracts/redaction.md`, research
//! decision R-1 (Decision 2 detector layers, Decision 3 ordering).
//!
//! This is the single canonical definition of "what counts as a secret/PII"
//! and the only entry point every inference input and the capture path must
//! route through. [`redact`] applies a layered detector, in order:
//!
//! 1. **Anchored credential patterns** — PEM private keys, JWTs, provider key
//!    prefixes (`sk-ant-`, `sk-`, `vck_`, `gh[posru]_`, `github_pat_`,
//!    `xox[baprs]-`, `AKIA`, `AIza`, `rk_live_`), `Authorization: Bearer` /
//!    bare bearer tokens, and `KEY=VALUE` / `KEY: VALUE` where the key matches
//!    `SECRET|TOKEN|KEY|PASSWORD|PASSWD|API|CREDENTIAL` (broadened with the
//!    `compress_prose/detect.rs` `SENSITIVE_NAME_TOKENS` list:
//!    `ACCESSKEY`/`PRIVATEKEY`/`PASSWD`/`CREDENTIAL`). Generalized from the
//!    legacy `prompt_utils::redact_secrets`; the proven anchored shapes are
//!    kept verbatim. Only the secret *value* is masked; the key/frame stays.
//! 2. **Connection-string / URL userinfo** — `scheme://user:password@host`
//!    masks only the `:password@` segment; scheme, host, and path survive.
//! 3. **Shannon-entropy fallback** — whitespace/quote/`=`/`:`-delimited tokens
//!    of length ≥ 24 over a base64 / base64url / hex charset whose Shannon
//!    entropy over bytes is ≥ 4.0 bits/char are masked. Guardrails skip
//!    file-path-looking tokens, git-hash-length hex (7/8/40/64) adjacent to
//!    commit/sha context, and pure-decimal tokens to avoid false positives.
//! 4. **Email** — masks the local-part, keeps `@domain`.
//!
//! ## Invariants (enforced by `mod tests`)
//!
//! - **Idempotent** — `redact(redact(x)) == redact(x)`. The single mask token
//!   [`MASK`] (`‹redacted›`) is constructed so re-running every layer over
//!   already-masked text is a fixed point: it has no provider prefix, no
//!   `://…@` userinfo, is < 24 chars and outside the base64/hex charset, and
//!   contains no `@`, so no layer re-triggers on it (and any layer that *does*
//!   span a `MASK` value simply rewrites it to `MASK`).
//! - **Structure-preserving** (FR-006) — key names, URL scheme/host, the
//!   `@domain` of an email, and the literal `Bearer ` frame are retained;
//!   only the secret value itself is replaced.
//! - **Order** — this function performs NO compression, truncation, or
//!   prompt-sanitization. Per R-1 Decision 3 it MUST run *before* any lossy
//!   `compress_observation` / `safe_truncate` / `sanitize_for_prompt` at every
//!   call site (truncation-first could split a secret so an anchored regex
//!   misses it). Call-site adoption (feature 005 US1) is complete:
//!   [`redact`] is wired into `server.rs`, `storage.rs`, `learning.rs`,
//!   `git_analysis.rs`, and `memory_optimizer.rs`.

/// Idempotent mask token. Chosen so it matches none of the detector layers
/// (kept stable from the legacy `prompt_utils::redact_secrets`).
pub const MASK: &str = "\u{2039}redacted\u{203a}"; // ‹redacted›

/// Redact recognized secrets and PII from `input`, returning a new `String`.
///
/// Applies the layered detector documented at the module level, in order
/// (anchored credentials → URL userinfo → Shannon-entropy fallback → email
/// local-part). Idempotent and structure-preserving. Performs no
/// compression/truncation — it is designed to run *before* compression at
/// call sites (see module docs / R-1 Decision 3).
pub fn redact(input: &str) -> String {
    use regex::Regex;
    use std::sync::LazyLock;

    // ---- Layer 1: anchored credential patterns -------------------------
    // High-precision, kept verbatim from the proven legacy detector. MASK
    // contains none of these shapes, so re-running is a no-op.
    static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        vec![
            // PEM private-key blocks (multi-line).
            Regex::new(
                r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            )
            .unwrap(),
            // JWTs (header.payload.signature, base64url segments).
            Regex::new(r"\beyJ[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}\b").unwrap(),
            // Provider key prefixes
            // (Anthropic/OpenAI/Vercel/GitHub/Slack/AWS/Google/Stripe).
            Regex::new(
                r"\b(sk-ant-[A-Za-z0-9_-]{16,}|sk-[A-Za-z0-9]{20,}|vck_[A-Za-z0-9]{16,}|gh[posru]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{20,}|rk_live_[0-9A-Za-z]{16,})\b",
            )
            .unwrap(),
            // Authorization: Bearer <token> / bare bearer <token>. The
            // literal `Bearer ` frame (capture 1) is preserved.
            Regex::new(r"(?i)(authorization\s*:\s*bearer\s+|\bbearer\s+)[A-Za-z0-9._\-]{10,}")
                .unwrap(),
        ]
    });

    // Sensitive KEY=VALUE / KEY: VALUE — preserve the key + separator, mask
    // the value. Key alternation broadened with the
    // `compress_prose/detect.rs` `SENSITIVE_NAME_TOKENS` list
    // (`ACCESSKEY`/`PRIVATEKEY`/`PASSWD`/`CREDENTIAL` in addition to the
    // legacy `SECRET|TOKEN|KEY|PASSWORD|API`).
    //
    // The credential core must appear as its own underscore-delimited
    // *segment* of the key, not as an arbitrary substring: each
    // sensitive word is bounded by start-or-`_` on the left and
    // end-or-`_` on the right. This is the broadening fix over the legacy
    // `[A-Z0-9_]*KEY[A-Z0-9_]*` — that form also matched `MONKEY=`
    // (`KEY` glued inside `MONKEY`). Now `API_KEY`/`ACCESS_KEY`/`DB_TOKEN`
    // /`APIKEY` match but `MONKEY` / `TURKEY` do not.
    static ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?im)\b((?:[A-Z0-9]+_)*(?:SECRET|TOKEN|PASSWORD|PASSWD|CREDENTIAL|ACCESSKEY|PRIVATEKEY|APIKEY|API|KEY)(?:_[A-Z0-9]+)*)\s*([=:])\s*"?[^\s"']{3,}"?"#,
        )
        .unwrap()
    });

    // Authorization-header capture group preserved by index (group 1 is the
    // `Bearer ` / `authorization: bearer ` frame for the 4th pattern only).
    let bearer_re = &PATTERNS[3];

    let mut out = input.to_string();
    for (i, re) in PATTERNS.iter().enumerate() {
        if i == 3 {
            // Preserve the `Bearer `/`Authorization: Bearer ` frame.
            out = bearer_re
                .replace_all(&out, |c: &regex::Captures| format!("{}{MASK}", &c[1]))
                .into_owned();
        } else {
            out = re.replace_all(&out, MASK).into_owned();
        }
    }
    out = ASSIGN
        .replace_all(&out, |c: &regex::Captures| {
            // Idempotence: if the value is already exactly MASK, re-emit the
            // whole match unchanged (do not re-insert a normalizing space, so
            // `KEY=‹redacted›` is a true fixed point — not `KEY= ‹redacted›`).
            let whole = &c[0];
            let val = whole[c[1].len()..].trim_start_matches(['=', ':']).trim();
            if val == MASK {
                whole.to_string()
            } else {
                format!("{}{} {MASK}", &c[1], &c[2])
            }
        })
        .into_owned();

    // ---- Layer 2: connection-string / URL userinfo ---------------------
    // scheme://user:password@host → scheme://user:‹redacted›@host. Only the
    // `:password` between userinfo and `@` is masked; scheme/user/host/path
    // are preserved. The password class excludes `@`, `/`, whitespace, and
    // quotes so the match cannot run past the authority. A bare `://user@`
    // (no password) is intentionally left untouched.
    static URL_USERINFO: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"([a-zA-Z][a-zA-Z0-9+.\-]*://[^\s/:@'"]+):[^\s/@'"]+@"#).unwrap()
    });
    out = URL_USERINFO
        .replace_all(&out, |c: &regex::Captures| format!("{}:{MASK}@", &c[1]))
        .into_owned();

    // ---- Layer 3: Shannon-entropy fallback -----------------------------
    // Split on whitespace, quotes, `=`, `:` and inspect each long, high-
    // entropy base64/base64url/hex blob. Guardrails (below) drop the
    // structured-but-not-secret cases that would otherwise false-positive.
    out = redact_high_entropy_tokens(&out);

    // ---- Layer 4: email ------------------------------------------------
    // local-part@domain → ‹redacted›@domain. Domain (incl. TLD) survives so
    // the "an address was here" signal is rule-neutral but the identity is
    // gone. The MASK token contains no `@`, so this never re-triggers.
    static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"[A-Za-z0-9._%+\-]+@([A-Za-z0-9.\-]+\.[A-Za-z]{2,})"#).unwrap()
    });
    out = EMAIL
        .replace_all(&out, |c: &regex::Captures| format!("{MASK}@{}", &c[1]))
        .into_owned();

    out
}

/// Shannon entropy in bits/char over the raw bytes of `s`.
///
/// Hand-computed (no external crate): `-Σ p_i · log2(p_i)` where `p_i` is the
/// empirical frequency of byte value `i`. Random base64 trends toward ~6
/// bits/char; English prose and structured identifiers stay well under the
/// 4.0-bit gate used by [`redact`].
fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for &b in s.as_bytes() {
        counts[b as usize] += 1;
    }
    let len = s.len() as f64;
    let mut entropy = 0.0_f64;
    for &count in counts.iter() {
        if count == 0 {
            continue;
        }
        let p = count as f64 / len;
        entropy -= p * p.log2();
    }
    entropy
}

/// True if every char is in the base64, base64url, or hex alphabet (a single
/// blob may legitimately mix these; we accept the union plus `=` padding).
fn is_base64_or_hex_charset(token: &str) -> bool {
    token.chars().all(|c| {
        c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '-' || c == '_' || c == '='
    })
}

/// True if every char is a hex digit (`0-9a-fA-F`).
fn is_pure_hex(token: &str) -> bool {
    !token.is_empty() && token.chars().all(|c| c.is_ascii_hexdigit())
}

/// Guardrail discriminating a high-entropy *secret blob* from a long
/// structured *identifier* (e.g. a camelCase symbol name).
///
/// Random base64/base64url/hex credential material essentially always
/// contains at least one digit or a base64 symbol (`+ / - _ =`); a token
/// that is *pure ASCII letters* is overwhelmingly an identifier or a word
/// run, never a generated secret. Requiring a non-letter rejects long
/// `thisIsACamelCaseName`-style false positives without weakening real
/// secret detection (Layer 1's anchored prefixes already cover the rare
/// all-letter API key).
fn has_secret_like_charmix(token: &str) -> bool {
    token
        .chars()
        .any(|c| c.is_ascii_digit() || matches!(c, '+' | '/' | '-' | '_' | '='))
}

/// Guardrail: looks like a filesystem path (so not a bare secret blob).
/// Containing a path separator, a `~` home prefix, or a dotted extension-ish
/// tail are all strong "this is a path/filename" tells.
fn looks_like_path(token: &str) -> bool {
    token.contains('/') || token.contains('\\') || token.starts_with('~') || token.starts_with('.')
}

/// Guardrail: a hex string of a length git uses for object names
/// (abbreviated 7/8, or full SHA-1 40 / SHA-256 64) sitting next to
/// commit/sha/hash wording. Such tokens are high-entropy but are not secrets.
fn is_git_hash_in_context(token: &str, before: &str) -> bool {
    let is_hex = !token.is_empty() && token.chars().all(|c| c.is_ascii_hexdigit());
    let git_len = matches!(token.len(), 7 | 8 | 40 | 64);
    if !(is_hex && git_len) {
        return false;
    }
    // Look at a short window of preceding context (case-insensitive) for
    // commit/sha/hash wording.
    let window = before
        .chars()
        .rev()
        .take(48)
        .collect::<String>()
        .to_ascii_lowercase();
    window.contains("commit")
        || window.contains("sha")
        || window.contains("hash")
        || window.contains("rev")
        || window.contains("git")
}

/// Apply the Layer-3 entropy fallback over `input`, returning a new string.
///
/// Tokens are the maximal runs between the delimiter set
/// {whitespace, `"`, `'`, `=`, `:`}; everything else (including `@`, `/`,
/// `.`, `,`, brackets) stays inside a token so URLs/paths are caught by the
/// path guardrail rather than being shredded into fragments. The delimiters
/// themselves are re-emitted verbatim so structure is byte-preserved.
fn redact_high_entropy_tokens(input: &str) -> String {
    // Length floor for any entropy-detected token (R-1 Decision 2: ≥24).
    const MIN_LEN: usize = 24;
    // Shannon-entropy gate for *mixed* base64/base64url blobs (R-1's tunable
    // 4.0 bits/char knob — the dominant quality lever).
    const ENTROPY_BITS: f64 = 4.0;
    // A *pure-hex* blob can never reach 4.0 bits/char (16-symbol alphabet ⇒
    // theoretical max = log2(16) = 4.0, real strings sit below it), so the
    // entropy gate is unreachable for hex by construction. R-1 Decision 2
    // gates hex on *charset + length + git-hash/path* instead; this is the
    // hex length floor (covers leaked HMACs / hex API tokens while the
    // git-hash-in-context guardrail still exempts real 7/8/40/64 SHAs).
    const MIN_HEX_LEN: usize = 32;

    let mut out = String::with_capacity(input.len());
    let mut token = String::new();
    // Byte offset where the current token started (for preceding-context).
    let mut token_start = 0usize;

    let flush = |token: &mut String, token_start: usize, input: &str, out: &mut String| {
        if token.is_empty() {
            return;
        }
        let t = token.as_str();
        let before = &input[..token_start];
        let base_ok = t.len() >= MIN_LEN
                && is_base64_or_hex_charset(t)
                && !looks_like_path(t)
                && !t.chars().all(|c| c.is_ascii_digit()) // pure decimal
                && !is_git_hash_in_context(t, before);
        let is_secretish = base_ok
            && if is_pure_hex(t) {
                // Hex path: structural gate (entropy unreachable for hex).
                // The git-hash guardrail above already removed contextual
                // SHAs; a long bare hex blob is a credential.
                t.len() >= MIN_HEX_LEN
            } else {
                // Mixed base64/base64url path: entropy gate + charmix
                // guardrail (rejects long pure-letter identifiers).
                has_secret_like_charmix(t) && shannon_entropy(t) >= ENTROPY_BITS
            };
        if is_secretish {
            out.push_str(MASK);
        } else {
            out.push_str(t);
        }
        token.clear();
    };

    for (idx, ch) in input.char_indices() {
        let is_delim = ch.is_whitespace() || ch == '"' || ch == '\'' || ch == '=' || ch == ':';
        if is_delim {
            flush(&mut token, token_start, input, &mut out);
            out.push(ch);
        } else {
            if token.is_empty() {
                token_start = idx;
            }
            token.push(ch);
        }
    }
    flush(&mut token, token_start, input, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Layer 1: anchored credential patterns -------------------------

    // @lat: anchored-cred-layer
    #[test]
    fn masks_anthropic_provider_key() {
        let s = "export ANTHROPIC_API=sk-ant-api03-AbCdEf0123456789ZyXwVu_secretpart";
        let r = redact(s);
        assert!(!r.contains("AbCdEf0123456789ZyXwVu"), "secret leaked: {r}");
        assert!(r.contains(MASK), "no mask emitted: {r}");
    }

    #[test]
    fn masks_assorted_provider_prefixes() {
        for raw in [
            "sk-proj0123456789ABCDEFGHIJ",
            "vck_0123456789ABCDEFGHIJ",
            "ghp_0123456789ABCDEFGHIJ0123456789ABCD",
            "github_pat_0123456789ABCDEFGHIJ_abcdEFGH",
            "xoxb-0123456789-abcdefghij",
            "AKIAIOSFODNN7EXAMPLE",
            "AIzaSyA0123456789abcdefghijklmnopqrstuv",
            "rk_live_0123456789ABCDEFGHIJ",
        ] {
            let r = redact(&format!("value is {raw} ok"));
            assert!(!r.contains(raw), "leaked {raw} -> {r}");
            assert!(r.contains(MASK), "no mask for {raw} -> {r}");
            assert!(r.starts_with("value is "), "frame lost for {raw} -> {r}");
        }
    }

    #[test]
    fn masks_jwt_keeps_surrounding_prose() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let r = redact(&format!("token before {jwt} token after"));
        assert!(!r.contains(jwt), "jwt leaked: {r}");
        assert!(r.contains("token before "), "prefix lost: {r}");
        assert!(r.contains(" token after"), "suffix lost: {r}");
    }

    #[test]
    fn masks_pem_private_key_block() {
        // Split the PEM header tokens via concat! so the source bytes never
        // contain a contiguous blocklisted header — keeps the repo
        // detect-private-key pre-commit scanner meaningful for REAL keys.
        // The runtime value is byte-identical to a genuine PEM block.
        let pem = concat!(
            "-----BEGIN RSA ",
            "PRIVATE KEY-----\nMIIBduperSecretKeyMaterialLine1\nMoreBase64==\n-----END RSA ",
            "PRIVATE KEY-----"
        );
        let r = redact(&format!("key:\n{pem}\ndone"));
        assert!(!r.contains("duperSecretKeyMaterial"), "pem leaked: {r}");
        assert!(r.contains(MASK), "no mask: {r}");
        assert!(r.ends_with("done"), "trailing content lost: {r}");
    }

    // @lat: bearer-frame-preserved
    #[test]
    fn masks_bearer_token_but_keeps_frame() {
        let r = redact("Authorization: Bearer abc123DEF456ghi789JKL");
        assert!(!r.contains("abc123DEF456ghi789JKL"), "token leaked: {r}");
        assert!(
            r.to_lowercase().contains("bearer"),
            "Bearer frame lost: {r}"
        );
        assert!(r.contains(MASK), "no mask: {r}");

        let bare = redact("sent bearer myOpaqueSessionToken99 today");
        assert!(!bare.contains("myOpaqueSessionToken99"), "leaked: {bare}");
        assert!(bare.to_lowercase().contains("bearer"), "frame lost: {bare}");
    }

    // @lat: assign-key-preserved
    #[test]
    fn masks_key_value_assignment_preserving_key() {
        let r = redact("DATABASE_PASSWORD=hunter2hunter2hunter2");
        assert!(!r.contains("hunter2hunter2hunter2"), "value leaked: {r}");
        assert!(r.starts_with("DATABASE_PASSWORD"), "key lost: {r}");
        assert!(r.contains(MASK), "no mask: {r}");

        let colon = redact("api_secret: superSensitiveValue123");
        assert!(!colon.contains("superSensitiveValue123"), "leaked: {colon}");
        assert!(
            colon.to_lowercase().starts_with("api_secret"),
            "key lost: {colon}"
        );
    }

    #[test]
    fn masks_broadened_detect_tokens() {
        // ACCESSKEY / PRIVATEKEY / CREDENTIAL / PASSWD broadened from
        // compress_prose/detect.rs SENSITIVE_NAME_TOKENS.
        for line in [
            "AWS_ACCESS_KEY=AKIDEXAMPLE0123456789abc",
            "MY_PRIVATE_KEY=privatekeymaterial0123456789",
            "DB_CREDENTIAL=mongodbpass0123456789",
            "ROOT_PASSWD=correcthorsebatterystaple",
        ] {
            let r = redact(line);
            let key = line.split(['=', ':']).next().unwrap();
            assert!(r.starts_with(key), "key lost for {line} -> {r}");
            assert!(r.contains(MASK), "no mask for {line} -> {r}");
            let secret = line.split('=').nth(1).unwrap();
            assert!(!r.contains(secret), "secret leaked for {line} -> {r}");
        }
    }

    #[test]
    fn does_not_mask_innocuous_word_containing_key() {
        // `MONKEY` ends in `KEY` but is not a credential key; the value is
        // short prose and must survive untouched.
        let s = "MONKEY=banana";
        assert_eq!(redact(s), s, "false positive on MONKEY=banana");
    }

    // ---- Layer 2: connection-string / URL userinfo ---------------------

    // @lat: url-userinfo-layer
    #[test]
    fn masks_connection_string_password_only() {
        let r = redact("postgres://app_user:s3cr3tP@ssBADexample@db.internal:5432/prod");
        assert!(
            r.starts_with("postgres://app_user:"),
            "scheme/user lost: {r}"
        );
        assert!(r.contains("@db.internal:5432/prod"), "host/path lost: {r}");
        assert!(r.contains(MASK), "no mask: {r}");
        assert!(!r.contains("s3cr3tP"), "password leaked: {r}");
    }

    #[test]
    fn masks_redis_url_userinfo() {
        let r = redact("redis://default:abcdEFGHverysecret@cache.example.com:6379/0");
        assert!(r.starts_with("redis://default:"), "prefix lost: {r}");
        assert!(
            r.contains("@cache.example.com:6379/0"),
            "host/path lost: {r}"
        );
        assert!(!r.contains("abcdEFGHverysecret"), "password leaked: {r}");
    }

    #[test]
    fn leaves_plain_url_without_userinfo_intact() {
        let s = "see https://docs.example.com/path?q=1 for details";
        assert_eq!(redact(s), s, "plain URL must be untouched");
    }

    // ---- Layer 3: Shannon-entropy fallback -----------------------------

    // @lat: entropy-fallback-layer
    #[test]
    fn masks_high_entropy_unprefixed_secret() {
        // 32-char random-ish base64, no recognizable provider prefix, sitting
        // bare in prose — only the entropy layer can catch this.
        let secret = "Zk9pQ2xWd0h4N3RBb1JmU2VjcjN0S2V5";
        let r = redact(&format!("the value {secret} was used"));
        assert!(!r.contains(secret), "high-entropy secret leaked: {r}");
        assert!(r.contains(MASK), "no mask: {r}");
        assert!(r.starts_with("the value "), "prefix lost: {r}");
        assert!(r.ends_with(" was used"), "suffix lost: {r}");
    }

    #[test]
    fn masks_long_hex_token_without_git_context() {
        // 40 hex chars but NO commit/sha wording nearby → treated as a secret
        // (e.g. a leaked HMAC / API token), not a git hash.
        let tok = "0a1b2c3d4e5f60718293a4b5c6d7e8f901234567";
        let r = redact(&format!("secret material {tok} end"));
        assert!(!r.contains(tok), "hex secret leaked: {r}");
        assert!(r.contains(MASK), "no mask: {r}");
    }

    #[test]
    fn does_not_mask_git_sha_in_commit_context() {
        // 40-hex adjacent to "commit" wording → git hash guardrail, kept.
        let sha = "9fceb02d4e5f60718293a4b5c6d7e8f901234567";
        let s = format!("reverted commit {sha} cleanly");
        assert_eq!(redact(&s), s, "git SHA must not be masked: {sha}");

        let short = "fixed in commit 1a2b3c4 today";
        assert_eq!(redact(short), short, "abbreviated SHA must survive");
    }

    #[test]
    fn does_not_mask_long_camelcase_identifier() {
        // Long, but low-entropy structured identifier — must survive.
        let ident = "thisIsAVeryLongDescriptiveCamelCaseFunctionNameForHandlers";
        let s = format!("call {ident} here");
        assert_eq!(redact(&s), s, "camelCase identifier false-positived: {s}");
    }

    #[test]
    fn does_not_mask_file_path() {
        let p = "/home/user/projects/quill/src-tauri/src/redaction_module_file.rs";
        let s = format!("edited {p} now");
        assert_eq!(redact(&s), s, "file path false-positived: {s}");
    }

    #[test]
    fn does_not_mask_pure_decimal_or_prose() {
        let decimal = "the id 123456789012345678901234567890 is numeric";
        assert_eq!(redact(decimal), decimal, "decimal false-positived");

        let prose = "The quick brown fox jumps over the lazy dog while refactoring the parser.";
        assert_eq!(redact(prose), prose, "ordinary prose false-positived");
    }

    // ---- Layer 4: email ------------------------------------------------

    // @lat: email-layer
    #[test]
    fn masks_email_local_part_keeps_domain() {
        let r = redact("contact alice.bob+test@corp.example.com for access");
        assert!(!r.contains("alice.bob+test"), "local-part leaked: {r}");
        assert!(r.contains("@corp.example.com"), "domain lost: {r}");
        assert!(r.contains(MASK), "no mask: {r}");
        assert!(r.starts_with("contact "), "prefix lost: {r}");
    }

    // ---- Invariants ----------------------------------------------------

    // @lat: idempotent-invariant
    #[test]
    fn redact_is_idempotent_over_seeded_corpus() {
        for case in seeded_corpus() {
            let once = redact(case);
            let twice = redact(&once);
            assert_eq!(once, twice, "not idempotent for input: {case}");
        }
    }

    // @lat: structure-preserving-invariant
    #[test]
    fn structure_is_preserved_across_corpus() {
        // The bundle of secrets below must all be masked while every
        // structural frame (key, scheme/host, @domain, Bearer) survives.
        let blob = "DATABASE_URL=postgres://svc:topSecretPw99XYZ@db.host:5432/app\n\
             Authorization: Bearer eyJabc.eyJdef.sigZ9_part\n\
             API_KEY=sk-ant-api03-LongLivedSecretValue000111\n\
             owner alice@team.example.org pushed";
        let r = redact(blob);
        // Frames preserved.
        assert!(r.contains("DATABASE_URL="), "key frame lost: {r}");
        assert!(r.contains("postgres://svc:"), "scheme/user lost: {r}");
        assert!(r.contains("@db.host:5432/app"), "host/path lost: {r}");
        assert!(r.to_lowercase().contains("bearer "), "Bearer lost: {r}");
        assert!(r.contains("API_KEY="), "API_KEY frame lost: {r}");
        assert!(r.contains("@team.example.org"), "email domain lost: {r}");
        // Secrets gone.
        assert!(!r.contains("topSecretPw99XYZ"), "db pw leaked: {r}");
        assert!(!r.contains("LongLivedSecretValue"), "api key leaked: {r}");
        assert!(!r.contains("alice@"), "email local-part leaked: {r}");
        // Idempotent on this composite too.
        assert_eq!(redact(&r), r, "composite not idempotent");
    }

    #[test]
    fn mask_token_itself_is_inert() {
        // The mask token must not re-trigger any layer.
        assert_eq!(redact(MASK), MASK, "MASK re-triggered a layer");
        let framed = format!("KEY={MASK} url://u:{MASK}@h {MASK}@d.com");
        assert_eq!(redact(&framed), framed, "framed MASK not a fixed point");
    }

    #[test]
    fn empty_and_clean_inputs_unchanged() {
        assert_eq!(redact(""), "");
        let clean = "Refactored handler to return Result; added 3 unit tests.";
        assert_eq!(redact(clean), clean);
    }

    /// Seeded corpus exercised by the idempotence test — one real-looking
    /// representative per detector layer plus the negative cases.
    fn seeded_corpus() -> Vec<&'static str> {
        vec![
            // Layer 1
            "export KEY=sk-ant-api03-AbCdEf0123456789ZyXwVu_tail",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6",
            // concat! splits the header tokens so source bytes contain no
            // contiguous blocklisted PEM header (detect-private-key stays
            // meaningful); runtime value is a real PEM block.
            concat!(
                "-----BEGIN ",
                "PRIVATE KEY-----\nABCdefSecret==\n-----END ",
                "PRIVATE KEY-----"
            ),
            "Authorization: Bearer abc123DEF456ghi789JKL000",
            "DB_PASSWORD=correcthorsebatterystaple",
            // Layer 2
            "postgres://u:p4ssw0rdSecret@host:5432/db",
            // Layer 3
            "blob Zk9pQ2xWd0h4N3RBb1JmU2VjcjN0S2V5 end",
            "0a1b2c3d4e5f60718293a4b5c6d7e8f901234567",
            // Layer 4
            "ping alice.bob+x@corp.example.com please",
            // Negatives
            "reverted commit 9fceb02d4e5f60718293a4b5c6d7e8f901234567 ok",
            "thisIsAVeryLongDescriptiveCamelCaseIdentifierName",
            "/home/user/projects/quill/src/some_long_module.rs",
            "The quick brown fox refactors the lazy parser today.",
            "MONKEY=banana",
        ]
    }
}
