use crate::integrations::IntegrationProvider;
use crate::models::{CpaAccountHealth, CpaPoolAggregate, UsageBucket, UsageSource};
use chrono::DateTime;
use std::collections::BTreeMap;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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

    let quota_accounts = provider_accounts
        .iter()
        .copied()
        .filter(|account| {
            account.is_quota_readable()
                && !account.buckets.iter().flatten().any(|bucket| {
                    bucket.utilization >= 100.0
                        && is_account_window(
                            provider,
                            &account_window_key(bucket, &account.health.auth_index),
                        )
                })
        })
        .collect::<Vec<_>>();
    let healthy = quota_accounts
        .iter()
        .filter(|account| account.is_healthy())
        .count();
    let use_readable_fallback = healthy == 0;
    let mut means = BTreeMap::<String, (UsageBucket, f64, usize)>::new();
    for account in quota_accounts
        .into_iter()
        .filter(|account| account.is_healthy() || use_readable_fallback)
    {
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
        && (key.starts_with("codex_primary_") || key.starts_with("codex_secondary_"))
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

pub(crate) fn is_account_window(provider: IntegrationProvider, key: &str) -> bool {
    match provider {
        IntegrationProvider::Claude => matches!(key, "five_hour" | "seven_day"),
        IntegrationProvider::Codex => key
            .strip_prefix("codex_")
            .and_then(|tail| tail.strip_suffix('m'))
            .is_some_and(|minutes| minutes.parse::<u32>().is_ok()),
        _ => false,
    }
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
                quota: Default::default(),
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

    // @lat: [[cpa-tests#CPA Regression Tests#Scoped Codex quota and credits]]
    #[test]
    fn cpa_scoped_codex_exhaustion_never_excludes_the_account_pool() {
        let make_bucket = |key, value| {
            let mut bucket = bucket("a", key, value);
            bucket.provider = IntegrationProvider::Codex;
            bucket
        };
        let mut snapshot = account(
            "a",
            "ready",
            false,
            false,
            false,
            Some(vec![
                make_bucket("codex_300m", 25.0),
                make_bucket("codex_scope_61_300m", 100.0),
                make_bucket("codex_scope_62_300m", 50.0),
            ]),
        );
        snapshot.health.provider = "codex".into();
        let pools = compute_cpa_pools(&[snapshot]);
        assert_eq!(pools[0].healthy, 1);
        assert_eq!(pools[0].buckets.len(), 3);
        assert!(
            pools[0]
                .buckets
                .iter()
                .any(|bucket| bucket.key == "cpa/pool/codex_300m" && bucket.utilization == 25.0)
        );
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
                Some(vec![bucket("a", "five_hour", 40.0)]),
            ),
            account(
                "b",
                "cooling",
                false,
                false,
                false,
                Some(vec![bucket("b", "five_hour", 100.0)]),
            ),
        ];

        let pool = &compute_cpa_pools(&accounts)[0];
        assert_eq!((pool.healthy, pool.total), (0, 2));
        assert_eq!(pool.buckets[0].utilization, 40.0);
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
                Some(vec![bucket("a", "five_hour", 40.0)]),
            ),
            account(
                "b",
                "error",
                false,
                false,
                false,
                Some(vec![bucket("b", "five_hour", 100.0)]),
            ),
        ];

        let pool = &compute_cpa_pools(&accounts)[0];
        assert_eq!((pool.healthy, pool.total), (0, 2));
        assert_eq!(
            pool.buckets.first().map(|bucket| bucket.utilization),
            Some(40.0)
        );
    }

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#Exhausted account exclusion]]
    #[test]
    fn excludes_exhausted_accounts_from_every_window_and_reset() {
        for provider in [IntegrationProvider::Claude, IntegrationProvider::Codex] {
            let (short_window, weekly_window) = if provider == IntegrationProvider::Claude {
                ("five_hour", "seven_day")
            } else {
                ("codex_300m", "codex_10080m")
            };
            for status in ["active", "ready", "cooling", "error"] {
                let mut accounts = [
                    account(
                        "usable",
                        status,
                        false,
                        false,
                        false,
                        Some(vec![
                            bucket("usable", short_window, 13.0),
                            bucket("usable", weekly_window, 21.0),
                        ]),
                    ),
                    account(
                        "maxed",
                        status,
                        false,
                        false,
                        false,
                        Some(vec![
                            bucket("maxed", short_window, 0.0),
                            bucket("maxed", weekly_window, 100.0),
                        ]),
                    ),
                    account("unavailable", status, false, true, false, None),
                ];
                for account in &mut accounts {
                    account.health.provider = provider.as_str().to_string();
                    for bucket in account.buckets.iter_mut().flatten() {
                        bucket.provider = provider;
                        bucket.resets_at = Some(
                            if account.health.auth_index == "usable" {
                                "2026-09-10T00:00:00Z"
                            } else {
                                "2026-09-01T00:00:00Z"
                            }
                            .to_string(),
                        );
                    }
                }
                let pool = &compute_cpa_pools(&accounts)[0];
                assert_eq!(pool.total, 3);
                assert_eq!(pool.healthy, usize::from(is_usable_account_status(status)));
                assert_eq!(pool.buckets.len(), 2);
                for bucket in &pool.buckets {
                    assert_eq!(
                        bucket.utilization,
                        if bucket.key.ends_with(short_window) {
                            13.0
                        } else {
                            21.0
                        }
                    );
                    assert_eq!(bucket.resets_at.as_deref(), Some("2026-09-10T00:00:00Z"));
                }
            }
        }
    }

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#Scoped Claude limits retain account totals]]
    #[test]
    fn scoped_claude_exhaustion_preserves_general_totals() {
        for scoped_window in [
            "weekly_scoped_fable",
            "weekly_scoped_opus",
            "seven_day_sonnet",
            "seven_day_opus",
            "seven_day_cowork",
            "seven_day_oauth_apps",
        ] {
            for status in ["ready", "cooling", "error"] {
                let accounts = [("a", 2.0, 53.0), ("b", 5.0, 79.0)].map(|(id, short, weekly)| {
                    account(
                        id,
                        status,
                        false,
                        false,
                        false,
                        Some(vec![
                            bucket(id, "five_hour", short),
                            bucket(id, "seven_day", weekly),
                            bucket(id, scoped_window, 100.0),
                        ]),
                    )
                });
                let pool = &compute_cpa_pools(&accounts)[0];
                assert_eq!(
                    (pool.healthy, pool.total),
                    (if status == "ready" { 2 } else { 0 }, 2)
                );
                assert_eq!(pool.buckets.len(), 3);
                for (window, expected) in [
                    ("five_hour", 3.5),
                    ("seven_day", 66.0),
                    (scoped_window, 100.0),
                ] {
                    let bucket = pool
                        .buckets
                        .iter()
                        .find(|bucket| bucket.key == format!("cpa/pool/{window}"))
                        .unwrap();
                    assert_eq!(bucket.utilization, expected);
                    assert_eq!(bucket.resets_at.as_deref(), Some("reset-a"));
                }
            }
        }
    }

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#Entirely exhausted pool]]
    #[test]
    fn entirely_exhausted_pool_has_no_numeric_buckets() {
        for status in ["ready", "cooling", "error"] {
            let accounts = [account(
                "maxed",
                status,
                false,
                false,
                false,
                Some(vec![
                    bucket("maxed", "five_hour", 0.0),
                    bucket("maxed", "seven_day", 100.0),
                ]),
            )];
            let pool = &compute_cpa_pools(&accounts)[0];
            assert_eq!((pool.healthy, pool.total), (0, 1));
            assert!(pool.buckets.is_empty());
        }
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
