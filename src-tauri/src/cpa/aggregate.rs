use crate::integrations::IntegrationProvider;
use crate::models::{CpaAccountHealth, CpaPoolAggregate, UsageBucket, UsageSource};
use chrono::DateTime;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(crate) struct CpaAccountSnapshot {
    pub health: CpaAccountHealth,
    pub buckets: Option<Vec<UsageBucket>>,
}

impl CpaAccountSnapshot {
    pub(crate) fn is_healthy(&self) -> bool {
        is_usable_account_status(&self.health.status) && self.is_quota_readable()
    }

    pub(crate) fn is_quota_readable(&self) -> bool {
        !self.health.disabled && !self.health.unavailable
    }
}

pub(crate) fn is_usable_account_status(status: &str) -> bool {
    let status = status.trim();
    status.eq_ignore_ascii_case("active") || status.eq_ignore_ascii_case("ready")
}

// @lat: [[features#Features#Live Usage View#CPA Pool Aggregation]]
pub(crate) fn compute_cpa_pools(accounts: &[CpaAccountSnapshot]) -> Vec<CpaPoolAggregate> {
    // Pi has transcript usage only and no quota pool in v1.
    [IntegrationProvider::Claude, IntegrationProvider::Codex]
        .into_iter()
        .filter_map(|provider| compute_provider_pool(accounts, provider))
        .collect()
}

fn compute_provider_pool(
    accounts: &[CpaAccountSnapshot],
    provider: IntegrationProvider,
) -> Option<CpaPoolAggregate> {
    let provider_accounts = accounts
        .iter()
        .filter(|account| account.health.provider == provider.as_str())
        .collect::<Vec<_>>();
    if provider_accounts.is_empty() {
        return None;
    }

    let healthy = provider_accounts
        .iter()
        .filter(|account| account.is_healthy())
        .count();
    let use_readable_fallback = healthy == 0;
    let mut means = BTreeMap::<String, (UsageBucket, f64, usize)>::new();
    for account in provider_accounts.iter().copied().filter(|account| {
        account.is_healthy() || use_readable_fallback && account.is_quota_readable()
    }) {
        let Some(buckets) = account.buckets.as_ref() else {
            continue;
        };
        for bucket in buckets {
            let window_key = account_window_key(bucket, &account.health.auth_index);
            match means.get_mut(&window_key) {
                Some((aggregate, sum, count)) => {
                    *sum += bucket.utilization;
                    *count += 1;
                    aggregate.resets_at =
                        earliest_reset(aggregate.resets_at.as_deref(), bucket.resets_at.as_deref());
                }
                None => {
                    let mut aggregate = bucket.clone();
                    aggregate.key = format!("cpa/pool/{window_key}");
                    aggregate.source = UsageSource::Cpa;
                    aggregate.account_id = None;
                    aggregate.account_label = None;
                    means.insert(window_key, (aggregate, bucket.utilization, 1));
                }
            }
        }
    }

    let mut buckets = means
        .into_values()
        .map(|(mut bucket, sum, count)| {
            bucket.utilization = sum / count as f64;
            bucket
        })
        .collect::<Vec<_>>();
    buckets.sort_by(|left, right| {
        left.sort_order
            .cmp(&right.sort_order)
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.key.cmp(&right.key))
    });
    Some(CpaPoolAggregate {
        provider,
        healthy,
        total: provider_accounts.len(),
        buckets,
    })
}

fn account_window_key(bucket: &UsageBucket, auth_index: &str) -> String {
    let key = bucket
        .key
        .strip_prefix(&format!("cpa/{auth_index}/"))
        .unwrap_or(&bucket.key)
        .to_string();
    if bucket.provider == IntegrationProvider::Codex
        && let Some(minutes) = key
            .rsplit('_')
            .next()
            .and_then(|part| part.strip_suffix('m'))
            .and_then(|part| part.parse::<u32>().ok())
    {
        return format!("codex_{minutes}m");
    }
    key
}

fn earliest_reset(current: Option<&str>, candidate: Option<&str>) -> Option<String> {
    match (current, candidate) {
        (None, None) => None,
        (Some(reset), None) | (None, Some(reset)) => Some(reset.to_string()),
        (Some(current), Some(candidate)) => {
            let current_time = DateTime::parse_from_rfc3339(current).ok();
            let candidate_time = DateTime::parse_from_rfc3339(candidate).ok();
            match (current_time, candidate_time) {
                (Some(current_time), Some(candidate_time)) if candidate_time < current_time => {
                    Some(candidate.to_string())
                }
                (None, Some(_)) => Some(candidate.to_string()),
                _ => Some(current.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(
        auth_index: &str,
        status: &str,
        disabled: bool,
        unavailable: bool,
        runtime_only: bool,
        buckets: Option<Vec<UsageBucket>>,
    ) -> CpaAccountSnapshot {
        CpaAccountSnapshot {
            health: CpaAccountHealth {
                provider: "claude".to_string(),
                auth_index: auth_index.to_string(),
                label: auth_index.to_string(),
                status: status.to_string(),
                status_message: None,
                disabled,
                unavailable,
                runtime_only,
            },
            buckets,
        }
    }

    fn bucket(auth_index: &str, window: &str, utilization: f64) -> UsageBucket {
        UsageBucket {
            provider: IntegrationProvider::Claude,
            key: format!("cpa/{auth_index}/{window}"),
            label: window.to_string(),
            utilization,
            resets_at: Some(format!("reset-{auth_index}")),
            sort_order: 0,
            source: UsageSource::Cpa,
            account_id: Some(auth_index.to_string()),
            account_label: Some(auth_index.to_string()),
        }
    }

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#Usable account mean]]
    #[test]
    fn averages_utilization_and_uses_earliest_reset() {
        let accounts = [
            account(
                "a",
                "ready",
                false,
                false,
                false,
                Some(vec![bucket("a", "5h", 41.0)]),
            ),
            account(
                "b",
                "ready",
                false,
                false,
                false,
                Some(vec![bucket("b", "5h", 82.0)]),
            ),
        ];

        let pools = compute_cpa_pools(&accounts);
        assert_eq!(pools[0].buckets[0].utilization, 61.5);
        assert_eq!(pools[0].buckets[0].resets_at.as_deref(), Some("reset-a"));
    }

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#Health denominator with unusable exclusions]]
    #[test]
    fn keeps_health_count_but_excludes_unusable_accounts() {
        let accounts = [
            account(
                "ready",
                "ready",
                false,
                false,
                false,
                Some(vec![
                    bucket("ready", "five_hour", 13.0),
                    bucket("ready", "seven_day", 21.0),
                ]),
            ),
            account(
                "disabled",
                "ready",
                true,
                false,
                false,
                Some(vec![
                    bucket("disabled", "five_hour", 99.0),
                    bucket("disabled", "seven_day", 99.0),
                ]),
            ),
            account(
                "unavailable",
                "ready",
                false,
                true,
                false,
                Some(vec![
                    bucket("unavailable", "five_hour", 98.0),
                    bucket("unavailable", "seven_day", 98.0),
                ]),
            ),
            account(
                "cooling",
                "cooling",
                false,
                false,
                false,
                Some(vec![
                    bucket("cooling", "five_hour", 0.0),
                    bucket("cooling", "seven_day", 100.0),
                ]),
            ),
        ];

        let pool = &compute_cpa_pools(&accounts)[0];
        assert_eq!((pool.healthy, pool.total), (1, 4));
        assert_eq!(pool.buckets.len(), 2);
        assert_eq!(pool.buckets[0].utilization, 13.0);
        assert_eq!(pool.buckets[1].utilization, 21.0);
    }

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#All-cooling fallback]]
    #[test]
    fn averages_cooling_accounts_when_none_are_active() {
        let accounts = [
            account(
                "a",
                "cooling",
                false,
                false,
                false,
                Some(vec![bucket("a", "5h", 40.0)]),
            ),
            account(
                "b",
                "cooling",
                false,
                false,
                false,
                Some(vec![bucket("b", "5h", 100.0)]),
            ),
        ];

        let pool = &compute_cpa_pools(&accounts)[0];
        assert_eq!((pool.healthy, pool.total), (0, 2));
        assert_eq!(pool.buckets[0].utilization, 70.0);
    }

    #[test]
    fn averages_readable_error_accounts_when_none_are_healthy() {
        let accounts = [
            account(
                "a",
                "error",
                false,
                false,
                false,
                Some(vec![bucket("a", "5h", 40.0)]),
            ),
            account(
                "b",
                "error",
                false,
                false,
                false,
                Some(vec![bucket("b", "5h", 100.0)]),
            ),
        ];

        let pool = &compute_cpa_pools(&accounts)[0];
        assert_eq!((pool.healthy, pool.total), (0, 2));
        assert_eq!(
            pool.buckets.first().map(|bucket| bucket.utilization),
            Some(70.0)
        );
    }

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#Usable lifecycle compatibility]]
    #[test]
    fn accepts_active_and_ready_but_rejects_other_lifecycle_states() {
        for status in ["active", "ACTIVE", "ready", "READY"] {
            assert!(account(status, status, false, false, false, None).is_healthy());
        }

        for status in [
            "cooling",
            "degraded",
            "error",
            "unknown",
            "pending",
            "refreshing",
            "disabled",
        ] {
            assert!(!account(status, status, false, false, false, None).is_healthy());
        }

        assert!(!account("disabled", "active", true, false, false, None).is_healthy());
        assert!(!account("unavailable", "ready", false, true, false, None).is_healthy());
    }

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#Missing account buckets stay gaps]]
    #[test]
    fn excludes_missing_account_bucket_from_mean() {
        let accounts = [
            account("missing", "ready", false, false, false, None),
            account(
                "present",
                "ready",
                false,
                false,
                false,
                Some(vec![bucket("present", "5h", 63.0)]),
            ),
        ];

        let pool = &compute_cpa_pools(&accounts)[0];
        assert_eq!((pool.healthy, pool.total), (2, 2));
        assert_eq!(pool.buckets[0].utilization, 63.0);
    }

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#All healthy buckets missing]]
    #[test]
    fn all_healthy_missing_has_no_numeric_bucket() {
        let pool = &compute_cpa_pools(&[account("missing", "ready", false, false, false, None)])[0];
        assert_eq!((pool.healthy, pool.total), (1, 1));
        assert!(pool.buckets.is_empty());
    }

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#Empty pool]]
    #[test]
    fn empty_input_has_no_pool() {
        assert!(compute_cpa_pools(&[]).is_empty());
    }

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#Runtime-only accounts included]]
    #[test]
    fn runtime_only_account_is_included() {
        let pool = &compute_cpa_pools(&[account(
            "runtime",
            "ready",
            false,
            false,
            true,
            Some(vec![bucket("runtime", "5h", 57.0)]),
        )])[0];
        assert_eq!((pool.healthy, pool.total), (1, 1));
        assert_eq!(pool.buckets[0].utilization, 57.0);
    }
}
