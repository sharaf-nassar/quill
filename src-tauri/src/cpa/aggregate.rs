use crate::integrations::IntegrationProvider;
use crate::models::{CpaAccountHealth, CpaPoolAggregate, UsageBucket, UsageSource};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(crate) struct CpaAccountSnapshot {
    pub health: CpaAccountHealth,
    pub buckets: Option<Vec<UsageBucket>>,
}

impl CpaAccountSnapshot {
    pub(crate) fn is_healthy(&self) -> bool {
        self.health.status == "ready" && !self.health.disabled && !self.health.unavailable
    }
}

// @lat: [[features#Features#Live Usage View#CPA Pool Aggregation]]
pub(crate) fn compute_cpa_pools(accounts: &[CpaAccountSnapshot]) -> Vec<CpaPoolAggregate> {
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
    let mut maxima = BTreeMap::<String, UsageBucket>::new();
    for account in provider_accounts
        .iter()
        .copied()
        .filter(|account| account.is_healthy())
    {
        let Some(buckets) = account.buckets.as_ref() else {
            continue;
        };
        for bucket in buckets {
            let window_key = account_window_key(bucket, &account.health.auth_index);
            match maxima.get(&window_key) {
                Some(current) if current.utilization >= bucket.utilization => {}
                _ => {
                    let mut aggregate = bucket.clone();
                    aggregate.key = format!("cpa/pool/{window_key}");
                    aggregate.source = UsageSource::Cpa;
                    aggregate.account_id = None;
                    aggregate.account_label = None;
                    maxima.insert(window_key, aggregate);
                }
            }
        }
    }

    let mut buckets = maxima.into_values().collect::<Vec<_>>();
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
    bucket
        .key
        .strip_prefix(&format!("cpa/{auth_index}/"))
        .unwrap_or(&bucket.key)
        .to_string()
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

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#Worst-case healthy maximum]]
    #[test]
    fn uses_maximum_utilization_and_owning_reset() {
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
        assert_eq!(pools[0].buckets[0].utilization, 82.0);
        assert_eq!(pools[0].buckets[0].resets_at.as_deref(), Some("reset-b"));
    }

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#Full denominator with unhealthy exclusions]]
    #[test]
    fn excludes_unhealthy_math_but_keeps_full_denominator() {
        let accounts = [
            account(
                "ready",
                "ready",
                false,
                false,
                false,
                Some(vec![bucket("ready", "5h", 25.0)]),
            ),
            account(
                "disabled",
                "ready",
                true,
                false,
                false,
                Some(vec![bucket("disabled", "5h", 99.0)]),
            ),
            account(
                "unavailable",
                "ready",
                false,
                true,
                false,
                Some(vec![bucket("unavailable", "5h", 98.0)]),
            ),
            account(
                "cooling",
                "cooling",
                false,
                false,
                false,
                Some(vec![bucket("cooling", "5h", 97.0)]),
            ),
        ];

        let pool = &compute_cpa_pools(&accounts)[0];
        assert_eq!((pool.healthy, pool.total), (1, 4));
        assert_eq!(pool.buckets[0].utilization, 25.0);
    }

    // @lat: [[features#Features#Live Usage View#CPA Pool Aggregation#Missing account buckets stay gaps]]
    #[test]
    fn excludes_missing_account_bucket_from_maximum() {
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
