//! Passive response headers are partial observations, never inventory-time samples.
use super::client::{CpaAuthFile, CpaQuotaObservation};
use super::quota::{QuotaReading, bucket, codex_window_label};
use crate::integrations::IntegrationProvider;
use chrono::{DateTime, TimeDelta, Utc};
use std::collections::BTreeMap;

pub(super) struct Observation {
    pub at: DateTime<Utc>,
    pub reading: QuotaReading,
}

pub(super) fn parse(file: &CpaAuthFile, now: DateTime<Utc>) -> Option<Observation> {
    parse_quota(&file.provider, &file.auth_index, file.quota.as_ref()?, now)
}

fn parse_quota(
    provider: &str,
    account: &str,
    quota: &CpaQuotaObservation,
    now: DateTime<Utc>,
) -> Option<Observation> {
    let at = DateTime::parse_from_rfc3339(quota.observed_at.as_deref()?)
        .ok()?
        .with_timezone(&Utc);
    if at > now || now.signed_duration_since(at).num_seconds() >= super::poll::PASSIVE_FRESH_SECS {
        return None;
    }
    let signals = quota
        .signals
        .iter()
        .map(|(key, value)| (key.to_ascii_lowercase(), value.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut reading = QuotaReading::default();
    match provider.to_ascii_lowercase().as_str() {
        "claude" => {
            for (prefix, key, label) in [
                ("5h", "five_hour", "5 hours"),
                ("7d", "seven_day", "7 days"),
            ] {
                let prefix = format!("anthropic-ratelimit-unified-{prefix}");
                let Some(used) = number(&signals, &format!("{prefix}-utilization")) else {
                    continue;
                };
                let Some(reset) = signals
                    .get(&format!("{prefix}-reset"))
                    .and_then(|s| timestamp(s))
                else {
                    continue;
                };
                if !(0.0..=1.0).contains(&used) {
                    continue;
                }
                reading.buckets.push(bucket(
                    IntegrationProvider::Claude,
                    account,
                    key,
                    label.into(),
                    used * 100.0,
                    Some(reset.to_rfc3339()),
                    0,
                ));
            }
        }
        "codex" => {
            // Active-Limit can select a non-default namespace on an error response.
            // Never relabel that limit as account-wide capacity. Unknown additional
            // and model namespaces retain the authoritative active-read fallback.
            if signals
                .get("x-codex-active-limit")
                .is_some_and(|limit| *limit != "codex")
            {
                return None;
            }
            for window in ["primary", "secondary"] {
                let prefix = format!("x-codex-{window}");
                let Some(used) = number(&signals, &format!("{prefix}-used-percent")) else {
                    continue;
                };
                let Some(minutes) = signals
                    .get(&format!("{prefix}-window-minutes"))
                    .and_then(|s| s.parse::<i64>().ok())
                    .filter(|n| *n > 0)
                else {
                    continue;
                };
                let reset = signals
                    .get(&format!("{prefix}-reset-at"))
                    .and_then(|s| timestamp(s))
                    .or_else(|| {
                        let seconds = signals
                            .get(&format!("{prefix}-reset-after-seconds"))?
                            .parse::<i64>()
                            .ok()
                            .filter(|seconds| *seconds >= 0)?;
                        at.checked_add_signed(TimeDelta::try_seconds(seconds)?)
                    });
                let Some(reset) = reset else { continue };
                if !(0.0..=100.0).contains(&used) {
                    continue;
                }
                reading.buckets.push(bucket(
                    IntegrationProvider::Codex,
                    account,
                    &format!("codex_{minutes}m"),
                    codex_window_label(minutes),
                    used,
                    Some(reset.to_rfc3339()),
                    0,
                ));
            }
        }
        _ => return None,
    }
    (!reading.buckets.is_empty()).then_some(Observation { at, reading })
}

fn number(signals: &BTreeMap<String, &str>, key: &str) -> Option<f64> {
    signals
        .get(key)?
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
}

fn timestamp(value: &str) -> Option<DateTime<Utc>> {
    value
        .parse::<i64>()
        .ok()
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    // @lat: [[cpa-tests#CPA Regression Tests#Passive observation truthfulness]]
    #[test]
    fn cpa_passive_units_age_and_scope_are_not_inferred() {
        let now = Utc::now();
        let mut quota = CpaQuotaObservation {
            observed_at: Some(now.to_rfc3339()),
            signals: BTreeMap::from([
                (
                    "Anthropic-Ratelimit-Unified-5h-Utilization".into(),
                    "0.42".into(),
                ),
                (
                    "Anthropic-Ratelimit-Unified-5h-Reset".into(),
                    (now.timestamp() + 3600).to_string(),
                ),
            ]),
        };
        let reading = parse_quota("claude", "a", &quota, now).unwrap();
        assert_eq!(reading.reading.buckets.len(), 1);
        assert_eq!(reading.reading.buckets[0].utilization, 42.0);
        quota.observed_at = Some((now - TimeDelta::seconds(180)).to_rfc3339());
        assert!(parse_quota("claude", "a", &quota, now).is_none());
        quota.observed_at = Some((now + TimeDelta::seconds(1)).to_rfc3339());
        assert!(parse_quota("claude", "a", &quota, now).is_none());
        quota.observed_at = None;
        assert!(parse_quota("claude", "a", &quota, now).is_none());
        quota.observed_at = Some(now.to_rfc3339());
        quota.signals = BTreeMap::from([
            ("X-Codex-Primary-Used-Percent".into(), "51".into()),
            ("X-Codex-Primary-Window-Minutes".into(), "300".into()),
            ("X-Codex-Primary-Reset-After-Seconds".into(), "60".into()),
        ]);
        let reading = parse_quota("codex", "a", &quota, now).unwrap();
        assert_eq!(reading.reading.buckets[0].utilization, 51.0);
        assert_eq!(
            reading.reading.buckets[0].resets_at,
            Some((now + TimeDelta::seconds(60)).to_rfc3339())
        );
        quota
            .signals
            .insert("X-Codex-Active-Limit".into(), "codex_other".into());
        assert!(parse_quota("codex", "a", &quota, now).is_none());
    }
}
