use crate::integrations::IntegrationProvider;
use crate::models::{
    IndicatorMetric, ProviderStatus, StatusIndicatorState, UsageBucket, UsageData,
};
use chrono::{DateTime, Local, Timelike};
use std::cmp::Ordering;

pub const INDICATOR_UPDATED_EVENT: &str = "indicator-updated";

fn provider_label(provider: IntegrationProvider) -> &'static str {
    match provider {
        IntegrationProvider::Claude => "Claude",
        IntegrationProvider::Codex => "Codex",
        IntegrationProvider::MiniMax => "MiniMax",
    }
}

fn format_utilization(utilization: f64) -> String {
    format!("{utilization:.0}%")
}

fn minimax_model_label(label: &str) -> Option<String> {
    label.split_once(" (").map(|(model, _)| model.to_string())
}

fn to_indicator_metric(
    provider: IntegrationProvider,
    bucket: &UsageBucket,
    model_label: Option<String>,
) -> IndicatorMetric {
    IndicatorMetric {
        provider,
        key: bucket.key.clone(),
        label: bucket.label.clone(),
        model_label,
        utilization: bucket.utilization,
        resets_at: bucket.resets_at.clone(),
        display_reset_time: format_local_reset_time(bucket.resets_at.as_deref()),
    }
}

fn find_bucket_by_key<'a>(buckets: &'a [UsageBucket], key: &str) -> Option<&'a UsageBucket> {
    buckets.iter().find(|bucket| bucket.key == key)
}

fn find_codex_base_bucket<'a>(buckets: &'a [UsageBucket], prefix: &str) -> Option<&'a UsageBucket> {
    buckets.iter().find(|bucket| bucket.key.starts_with(prefix))
}

fn compare_utilization_desc(left: &&UsageBucket, right: &&UsageBucket) -> Ordering {
    left.utilization
        .partial_cmp(&right.utilization)
        .unwrap_or(Ordering::Equal)
        .then_with(|| left.key.cmp(&right.key))
}

fn provider_has_metrics(provider: IntegrationProvider, usage: &UsageData) -> bool {
    let buckets = usage
        .buckets
        .iter()
        .filter(|bucket| bucket.provider == provider)
        .cloned()
        .collect::<Vec<_>>();
    let (short_window, weekly_window) = resolve_metrics_for_provider(provider, &buckets);
    short_window.is_some() || weekly_window.is_some()
}

fn provider_is_enabled(provider: IntegrationProvider, statuses: &[ProviderStatus]) -> bool {
    statuses
        .iter()
        .find(|status| status.provider == provider)
        .is_some_and(|status| status.enabled)
}

fn resolve_metrics_for_provider(
    provider: IntegrationProvider,
    buckets: &[UsageBucket],
) -> (Option<IndicatorMetric>, Option<IndicatorMetric>) {
    match provider {
        IntegrationProvider::Claude => resolve_claude_metrics(buckets),
        IntegrationProvider::Codex => resolve_codex_metrics(buckets),
        IntegrationProvider::MiniMax => resolve_minimax_metrics(buckets),
    }
}

fn build_title_text(
    provider: Option<IntegrationProvider>,
    short_window: Option<&IndicatorMetric>,
    weekly_window: Option<&IndicatorMetric>,
) -> String {
    match provider {
        Some(provider) if short_window.is_none() && weekly_window.is_none() => {
            format!("{} indicator data unavailable", provider_label(provider))
        }
        Some(_) => {
            let now_text = short_window
                .map(|metric| format_utilization(metric.utilization))
                .unwrap_or_else(|| "--".to_string());
            let reset_text = short_window
                .and_then(|metric| metric.display_reset_time.clone())
                .unwrap_or_else(|| "--".to_string());
            let week_text = weekly_window
                .map(|metric| format_utilization(metric.utilization))
                .unwrap_or_else(|| "--".to_string());
            format!("{now_text} · {reset_text} · {week_text}")
        }
        None => "Indicator state unavailable".to_string(),
    }
}

fn build_warning(
    configured_provider: Option<IntegrationProvider>,
    resolved_provider: Option<IntegrationProvider>,
    resolved_provider_error: Option<&str>,
    short_window: Option<&IndicatorMetric>,
    weekly_window: Option<&IndicatorMetric>,
) -> Option<String> {
    let mut warnings = Vec::new();

    if let (Some(configured), Some(resolved)) = (configured_provider, resolved_provider)
        && configured != resolved
    {
        warnings.push(format!(
            "{} is unavailable; showing {} instead.",
            provider_label(configured),
            provider_label(resolved)
        ));
    }

    if let Some(message) = resolved_provider_error
        && (short_window.is_some() || weekly_window.is_some())
    {
        warnings.push(format!(
            "Showing cached data after refresh failed: {message}"
        ));
    }

    match (
        short_window.is_some(),
        weekly_window.is_some(),
        warnings.is_empty(),
    ) {
        (false, false, true) => {
            warnings.push("No indicator usage windows are available.".to_string())
        }
        (false, true, true) => warnings.push("Short-window usage is unavailable.".to_string()),
        (true, false, true) => warnings.push("Weekly usage is unavailable.".to_string()),
        _ => {}
    }

    if warnings.is_empty() {
        None
    } else {
        Some(warnings.join(" "))
    }
}

fn build_status(
    resolved_provider: Option<IntegrationProvider>,
    warning: Option<&String>,
    short_window: Option<&IndicatorMetric>,
    weekly_window: Option<&IndicatorMetric>,
) -> String {
    if resolved_provider.is_none() || (short_window.is_none() && weekly_window.is_none()) {
        return "unavailable".to_string();
    }

    if warning.is_some() {
        return "degraded".to_string();
    }

    "ready".to_string()
}

fn resolve_claude_metrics(
    buckets: &[UsageBucket],
) -> (Option<IndicatorMetric>, Option<IndicatorMetric>) {
    let short_window = find_bucket_by_key(buckets, "five_hour")
        .map(|bucket| to_indicator_metric(IntegrationProvider::Claude, bucket, None));
    let weekly_window = find_bucket_by_key(buckets, "seven_day")
        .map(|bucket| to_indicator_metric(IntegrationProvider::Claude, bucket, None));
    (short_window, weekly_window)
}

fn resolve_codex_metrics(
    buckets: &[UsageBucket],
) -> (Option<IndicatorMetric>, Option<IndicatorMetric>) {
    let short_window = find_codex_base_bucket(buckets, "primary_")
        .map(|bucket| to_indicator_metric(IntegrationProvider::Codex, bucket, None));
    let weekly_window = find_codex_base_bucket(buckets, "secondary_")
        .map(|bucket| to_indicator_metric(IntegrationProvider::Codex, bucket, None));
    (short_window, weekly_window)
}

fn resolve_minimax_metrics(
    buckets: &[UsageBucket],
) -> (Option<IndicatorMetric>, Option<IndicatorMetric>) {
    let short_window = buckets
        .iter()
        .filter(|bucket| bucket.key.ends_with("_5h"))
        .max_by(compare_utilization_desc)
        .map(|bucket| {
            to_indicator_metric(
                IntegrationProvider::MiniMax,
                bucket,
                minimax_model_label(&bucket.label),
            )
        });
    let weekly_window = buckets
        .iter()
        .filter(|bucket| bucket.key.ends_with("_weekly"))
        .max_by(compare_utilization_desc)
        .map(|bucket| {
            to_indicator_metric(
                IntegrationProvider::MiniMax,
                bucket,
                minimax_model_label(&bucket.label),
            )
        });
    (short_window, weekly_window)
}

fn format_local_reset_time(resets_at: Option<&str>) -> Option<String> {
    let parsed = resets_at
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|timestamp| timestamp.with_timezone(&Local));

    parsed.map(|timestamp| {
        if timestamp.minute() == 0 {
            timestamp.format("%-I%P").to_string()
        } else {
            timestamp.format("%-I:%M%P").to_string()
        }
    })
}

fn resolve_provider_choice(
    configured: Option<IntegrationProvider>,
    statuses: &[ProviderStatus],
    usage: &UsageData,
) -> (Option<IntegrationProvider>, Option<IntegrationProvider>) {
    if let Some(provider) = configured
        && provider_is_enabled(provider, statuses)
        && provider_has_metrics(provider, usage)
    {
        return (configured, Some(provider));
    }

    let resolved = statuses
        .iter()
        .filter(|status| status.enabled)
        .map(|status| status.provider)
        .find(|provider| provider_has_metrics(*provider, usage));

    (configured, resolved)
}

fn resolved_provider_error(
    provider: Option<IntegrationProvider>,
    usage: &UsageData,
) -> Option<&str> {
    let provider = provider?;
    usage
        .provider_errors
        .iter()
        .find(|provider_error| provider_error.provider == provider)
        .map(|provider_error| provider_error.message.as_str())
}

pub fn resolve_indicator_state(
    configured_provider: Option<IntegrationProvider>,
    statuses: &[ProviderStatus],
    usage: &UsageData,
) -> StatusIndicatorState {
    let (configured_primary_provider, resolved_primary_provider) =
        resolve_provider_choice(configured_provider, statuses, usage);
    let provider_buckets = resolved_primary_provider
        .map(|provider| {
            usage
                .buckets
                .iter()
                .filter(|bucket| bucket.provider == provider)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let (short_window, weekly_window) = resolved_primary_provider
        .map(|provider| resolve_metrics_for_provider(provider, &provider_buckets))
        .unwrap_or((None, None));
    let provider_error = resolved_provider_error(resolved_primary_provider, usage);
    let warning = build_warning(
        configured_primary_provider,
        resolved_primary_provider,
        provider_error,
        short_window.as_ref(),
        weekly_window.as_ref(),
    );
    let status = build_status(
        resolved_primary_provider,
        warning.as_ref(),
        short_window.as_ref(),
        weekly_window.as_ref(),
    );
    StatusIndicatorState {
        configured_primary_provider,
        resolved_primary_provider,
        status,
        title_text: build_title_text(
            resolved_primary_provider,
            short_window.as_ref(),
            weekly_window.as_ref(),
        ),
        warning,
        updated_at: None,
        short_window,
        weekly_window,
    }
}
