use rig::client::CompletionClient;
use rig::completion::TypedPrompt;
use rig::providers::anthropic;
use serde::de::DeserializeOwned;
use std::time::Duration;

use crate::config;
use crate::models::AnalysisOutput;

pub const MODEL_HAIKU: &str = "claude-haiku-4-5-20251001";
pub const MODEL_SONNET: &str = "claude-sonnet-4-5-20250929";

const MAX_RATE_LIMIT_RETRIES: usize = 2;
const FALLBACK_RATE_LIMIT_DELAY_SECS: u64 = 60;
const MAX_RATE_LIMIT_DELAY_SECS: u64 = 300;

/// Build an Anthropic client that authenticates with an OAuth Bearer token.
///
/// Claude Code stores an OAuth access token (`sk-ant-oat01-…`) which must be
/// sent as `Authorization: Bearer <token>` with the beta header
/// `anthropic-beta: oauth-2025-04-20`.  Rig's built-in Anthropic provider
/// hardcodes `x-api-key`, so we use a reqwest-middleware layer to swap the
/// header on every outgoing request.
fn build_oauth_client(
    token: &str,
) -> Result<anthropic::Client<reqwest_middleware::ClientWithMiddleware>, String> {
    let mw_client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
        .with(AnthropicRateLimitMiddleware)
        .with(OAuthHeaderMiddleware)
        .build();

    anthropic::Client::builder()
        .http_client(mw_client)
        .api_key(token)
        .anthropic_beta("oauth-2025-04-20")
        .build()
        .map_err(|e| format!("Failed to build Anthropic client: {e}"))
}

/// Analyze observations using the Anthropic API via Rig.
pub async fn analyze_observations(prompt: &str, model: &str) -> Result<AnalysisOutput, String> {
    let token = config::read_access_token()?;
    let client = build_oauth_client(&token)?;

    let agent = client
        .agent(model)
        .preamble(
            "You are a behavioral pattern analyzer for Claude Code tool-use observations. \
             Respond with structured JSON matching the provided schema.",
        )
        .max_tokens(4096)
        .build();

    let result: AnalysisOutput = agent
        .prompt_typed(prompt)
        .await
        .map_err(|e| format!("Anthropic API error: {e}"))?;

    Ok(result)
}

/// Generic typed analysis using the Anthropic API via Rig.
/// Like `analyze_observations` but accepts any JsonSchema-compatible output type.
pub async fn analyze_typed<T>(
    prompt: &str,
    preamble: &str,
    model: &str,
    max_tokens: u64,
) -> Result<T, String>
where
    T: DeserializeOwned + schemars::JsonSchema + Send + Sync + 'static,
{
    let token = config::read_access_token()?;
    let client = build_oauth_client(&token)?;

    let agent = client
        .agent(model)
        .preamble(preamble)
        .max_tokens(max_tokens)
        .build();

    let result: T = agent
        .prompt_typed(prompt)
        .await
        .map_err(|e| format!("Anthropic API error: {e}"))?;

    Ok(result)
}

// ---------------------------------------------------------------------------
// Middleware: retry short Anthropic 429s, then swap OAuth auth headers.
// ---------------------------------------------------------------------------

struct AnthropicRateLimitMiddleware;

#[derive(Debug)]
struct AnthropicRateLimitError(String);

impl std::fmt::Display for AnthropicRateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AnthropicRateLimitError {}

fn retry_after_delay(response: &reqwest::Response, attempt: usize) -> Duration {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or_else(|| {
            Duration::from_secs(FALLBACK_RATE_LIMIT_DELAY_SECS * (attempt as u64 + 1))
        })
}

impl reqwest_middleware::Middleware for AnthropicRateLimitMiddleware {
    fn handle<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        req: reqwest::Request,
        extensions: &'life1 mut http::Extensions,
        next: reqwest_middleware::Next<'life2>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = reqwest_middleware::Result<reqwest::Response>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let request_template = req.try_clone();
            let mut request = req;
            let mut attempt = 0usize;

            loop {
                let response = next.clone().run(request, extensions).await?;
                if response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
                    return Ok(response);
                }

                let delay = retry_after_delay(&response, attempt);
                if delay.as_secs() > MAX_RATE_LIMIT_DELAY_SECS {
                    return Err(reqwest_middleware::Error::middleware(
                        AnthropicRateLimitError(format!(
                            "Anthropic rate limit exceeded; retry after {} seconds before running analysis again.",
                            delay.as_secs()
                        )),
                    ));
                }

                if attempt >= MAX_RATE_LIMIT_RETRIES {
                    return Err(reqwest_middleware::Error::middleware(
                        AnthropicRateLimitError(format!(
                            "Anthropic rate limit still active after {MAX_RATE_LIMIT_RETRIES} retries; wait before running analysis again."
                        )),
                    ));
                }

                let Some(next_request) = request_template
                    .as_ref()
                    .and_then(|template| template.try_clone())
                else {
                    return Ok(response);
                };

                attempt += 1;
                log::warn!(
                    "Anthropic rate limited; retrying in {}s (attempt {}/{})",
                    delay.as_secs(),
                    attempt,
                    MAX_RATE_LIMIT_RETRIES
                );
                tokio::time::sleep(delay).await;
                request = next_request;
            }
        })
    }
}

struct OAuthHeaderMiddleware;

impl reqwest_middleware::Middleware for OAuthHeaderMiddleware {
    fn handle<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        mut req: reqwest::Request,
        extensions: &'life1 mut http::Extensions,
        next: reqwest_middleware::Next<'life2>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = reqwest_middleware::Result<reqwest::Response>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        // Rig sets the OAuth token as x-api-key; move it to Authorization: Bearer.
        if let Some(key) = req.headers_mut().remove("x-api-key") {
            let bearer = format!("Bearer {}", key.to_str().unwrap_or_default());
            if let Ok(val) = http::HeaderValue::from_str(&bearer) {
                req.headers_mut().insert(http::header::AUTHORIZATION, val);
            }
        }
        Box::pin(async move { next.run(req, extensions).await })
    }
}
