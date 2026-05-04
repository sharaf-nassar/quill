use crate::config::http_client;
use serde::{Deserialize, Serialize};

const RELEASES_URL: &str = "https://api.github.com/repos/sharaf-nassar/quill/releases";
const USER_AGENT: &str = "quill-app";
const DEFAULT_LIMIT: u32 = 30;
const MAX_LIMIT: u32 = 100;

#[derive(Deserialize, Debug)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ReleaseNote {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub html_url: String,
    pub published_at: Option<String>,
}

fn clamp_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn normalize_release(raw: GithubRelease) -> Option<ReleaseNote> {
    if raw.draft || raw.prerelease {
        return None;
    }
    Some(ReleaseNote {
        tag_name: raw.tag_name,
        name: raw.name.filter(|s| !s.is_empty()),
        body: raw.body.filter(|s| !s.is_empty()),
        html_url: raw.html_url,
        published_at: raw.published_at,
    })
}

fn normalize_releases(raw: Vec<GithubRelease>) -> Vec<ReleaseNote> {
    raw.into_iter().filter_map(normalize_release).collect()
}

pub async fn fetch_release_notes(limit: Option<u32>) -> Result<Vec<ReleaseNote>, String> {
    let per_page = clamp_limit(limit);
    let url = format!("{RELEASES_URL}?per_page={per_page}");
    let response = http_client()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("Failed to reach GitHub releases API: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        let preview: String = detail.chars().take(180).collect();
        return Err(format!(
            "GitHub releases API returned {status}{}",
            if preview.is_empty() {
                String::new()
            } else {
                format!(": {preview}")
            }
        ));
    }

    let raw: Vec<GithubRelease> = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse GitHub releases response: {e}"))?;

    Ok(normalize_releases(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(tag: &str, draft: bool, prerelease: bool) -> GithubRelease {
        GithubRelease {
            tag_name: tag.to_string(),
            name: Some(format!("Quill {tag}")),
            body: Some("notes".to_string()),
            html_url: format!("https://github.com/sharaf-nassar/quill/releases/tag/{tag}"),
            published_at: Some("2026-04-01T00:00:00Z".to_string()),
            draft,
            prerelease,
        }
    }

    #[test]
    fn drops_draft_and_prerelease_entries() {
        let raw = vec![
            make("v0.3.25", false, false),
            make("v0.3.26-rc.1", false, true),
            make("v0.3.27", true, false),
            make("v0.3.24", false, false),
        ];
        let notes = normalize_releases(raw);
        let tags: Vec<&str> = notes.iter().map(|n| n.tag_name.as_str()).collect();
        assert_eq!(tags, vec!["v0.3.25", "v0.3.24"]);
    }

    #[test]
    fn empty_strings_become_none() {
        let raw = vec![GithubRelease {
            tag_name: "v0.3.25".to_string(),
            name: Some(String::new()),
            body: Some(String::new()),
            html_url: "https://example.invalid".to_string(),
            published_at: None,
            draft: false,
            prerelease: false,
        }];
        let notes = normalize_releases(raw);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].name.is_none());
        assert!(notes[0].body.is_none());
    }

    #[test]
    fn clamp_limit_respects_bounds() {
        assert_eq!(clamp_limit(None), DEFAULT_LIMIT);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(5)), 5);
        assert_eq!(clamp_limit(Some(MAX_LIMIT + 50)), MAX_LIMIT);
    }

    #[test]
    fn preserves_input_order() {
        let raw = vec![
            make("v3", false, false),
            make("v2", false, false),
            make("v1", false, false),
        ];
        let notes = normalize_releases(raw);
        let tags: Vec<&str> = notes.iter().map(|n| n.tag_name.as_str()).collect();
        assert_eq!(tags, vec!["v3", "v2", "v1"]);
    }
}
