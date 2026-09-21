use super::aggregate::CpaAccountSnapshot;
use super::client::{CpaAuthFile, CpaClient, CpaError};
use super::quota::{QuotaReading, account_call_error, fetch_claude_usage, fetch_codex_usage};
use crate::integrations::{IntegrationProvider, cpa::CpaConnection};
use crate::models::{
    CpaAccountHealth, CpaModelAvailability, CpaQuotaState, ProviderErrorKind, UsageBucket,
    UsageProviderError, UsageSource,
};
use crate::storage::{Storage, cpa::STATE_SETTING};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;
use tokio::task::JoinSet;

pub(crate) const CPA_ACCOUNT_LIMIT: usize = 16;
pub(crate) const CPA_MAX_CONCURRENCY: usize = 3;
pub(crate) const CPA_LAUNCH_STAGGER: Duration = Duration::from_millis(250);
pub(crate) const PASSIVE_FRESH_SECS: i64 = 180;
const FULL_REFRESH_SECS: i64 = 15 * 60;

type Observations = Vec<(String, UsageBucket)>;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
enum Failure {
    Network,
    ManagementAuth,
    Credential,
    RateLimited,
    Invalid,
}

impl Failure {
    fn from_error(error: &CpaError) -> Self {
        match error {
            CpaError::Unauthorized | CpaError::Forbidden => Self::ManagementAuth,
            CpaError::Unreachable => Self::Network,
            CpaError::AccountCall {
                status_code: Some(401 | 403),
                ..
            } => Self::Credential,
            CpaError::AccountCall {
                status_code: Some(429),
                ..
            }
            | CpaError::ManagementCall {
                status_code: 429, ..
            } => Self::RateLimited,
            _ => Self::Invalid,
        }
    }

    fn kind(self) -> ProviderErrorKind {
        match self {
            Self::Network => ProviderErrorKind::Network,
            Self::ManagementAuth => ProviderErrorKind::Auth,
            Self::Credential => ProviderErrorKind::Paused,
            Self::RateLimited => ProviderErrorKind::Stale,
            Self::Invalid => ProviderErrorKind::Server,
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::Network => "CPA quota request failed. Showing last observed data.",
            Self::ManagementAuth => {
                "CPA management access was rejected. Reconnect in Settings to resume polling."
            }
            Self::Credential => {
                "CPA account quota access is paused. Credential recovery remains with CPA."
            }
            Self::RateLimited => "CPA quota polling is rate limited. Showing last observed data.",
            Self::Invalid => {
                "CPA quota response was unavailable or invalid. Showing last observed data."
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AccountState {
    snapshot: CpaAccountSnapshot,
    #[serde(default)]
    observed: BTreeMap<String, DateTime<Utc>>,
    last_full_at: Option<DateTime<Utc>>,
    last_attempt_at: Option<DateTime<Utc>>,
    retry_at: Option<DateTime<Utc>>,
    failures: u32,
    failure: Option<Failure>,
}

impl AccountState {
    fn new(file: &CpaAuthFile) -> Self {
        Self {
            snapshot: auth_file_snapshot(file),
            observed: BTreeMap::new(),
            last_full_at: None,
            last_attempt_at: None,
            retry_at: None,
            failures: 0,
            failure: None,
        }
    }

    fn apply_full(
        &mut self,
        reading: QuotaReading,
        now: DateTime<Utc>,
        recorded: &mut Observations,
    ) {
        if reading.partial {
            self.apply_reading(reading, now, recorded);
            self.fail(&account_call_error(&self.snapshot.health.auth_index), now);
            return;
        }
        self.observed.clear();
        self.snapshot.health.quota.credits = reading.credits.clone();
        self.snapshot.buckets = Some(Vec::new());
        self.apply_reading(reading, now, recorded);
        self.last_full_at = Some(now);
        self.last_attempt_at = Some(now);
        self.retry_at = None;
        self.snapshot.health.quota.retry_at = None;
        self.snapshot.health.quota.message = None;
        self.failures = 0;
        self.failure = None;
    }

    fn apply_reading(
        &mut self,
        reading: QuotaReading,
        at: DateTime<Utc>,
        recorded: &mut Observations,
    ) {
        let buckets = self.snapshot.buckets.get_or_insert_with(Vec::new);
        for mut bucket in reading.buckets {
            if self.observed.get(&bucket.key).is_some_and(|old| *old >= at) {
                continue;
            }
            bucket.account_label = Some(self.snapshot.health.label.clone());
            self.observed.insert(bucket.key.clone(), at);
            buckets.retain(|old| old.key != bucket.key);
            buckets.push(bucket.clone());
            recorded.push((at.to_rfc3339(), bucket));
        }
        if reading.credits.is_some() {
            self.snapshot.health.quota.credits = reading.credits;
        }
    }

    fn fail(&mut self, error: &CpaError, now: DateTime<Utc>) {
        self.last_attempt_at = Some(now);
        self.failures = self.failures.saturating_add(1);
        self.failure = Some(Failure::from_error(error));
        self.retry_at = Some(retry_at(error, now, self.failures));
    }

    fn fully_observed(&self, now: DateTime<Utc>, max_age: i64) -> bool {
        self.snapshot.buckets.as_ref().is_some_and(|buckets| {
            !buckets.is_empty()
                && buckets.iter().all(|bucket| {
                    self.observed
                        .get(&bucket.key)
                        .is_some_and(|at| is_fresh(*at, now, max_age))
                })
        })
    }

    fn update_health(&mut self, now: DateTime<Utc>) {
        let quota = &mut self.snapshot.health.quota;
        quota.observed_at = self.observed.values().min().map(DateTime::to_rfc3339);
        quota.retry_at = self
            .retry_at
            .map(|time| time.to_rfc3339())
            .or_else(|| quota.retry_at.clone());
        if let Some(failure) = self.failure {
            quota.message = Some(failure.message().to_string());
        }
        let covered = self.fully_observed(now, FULL_REFRESH_SECS);
        let quota = &mut self.snapshot.health.quota;
        quota.state = if self.failure == Some(Failure::Credential) {
            CpaQuotaState::Paused
        } else if self.failure.is_some() || !covered || quota.message.is_some() {
            CpaQuotaState::Cached
        } else {
            CpaQuotaState::Live
        };
        if self.snapshot.buckets.as_ref().is_none_or(Vec::is_empty) && self.failure.is_none() {
            quota.state = CpaQuotaState::Unknown;
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct PollState {
    endpoint: String,
    accounts: BTreeMap<String, AccountState>,
    source_failure: Option<Failure>,
    source_failures: u32,
    source_retry_at: Option<DateTime<Utc>>,
}

pub(crate) struct CpaPollResult {
    pub snapshots: Vec<CpaAccountSnapshot>,
    pub errors: Vec<UsageProviderError>,
}

fn account_key(file: &CpaAuthFile) -> String {
    format!("{}/{}", file.provider.to_ascii_lowercase(), file.auth_index)
}

fn is_fresh(at: DateTime<Utc>, now: DateTime<Utc>, seconds: i64) -> bool {
    at <= now && now.signed_duration_since(at).num_seconds() < seconds
}

fn retry_at(error: &CpaError, now: DateTime<Utc>, failures: u32) -> DateTime<Utc> {
    let seconds = match error {
        CpaError::AccountCall {
            retry_after_secs: Some(seconds),
            ..
        }
        | CpaError::ManagementCall {
            retry_after_secs: Some(seconds),
            ..
        } => (*seconds).clamp(60, 365 * 86400) as i64,
        CpaError::AccountCall {
            status_code: Some(401 | 403 | 429),
            ..
        }
        | CpaError::ManagementCall {
            status_code: 429, ..
        } => 300,
        _ => crate::compute_network_backoff(failures).num_seconds(),
    };
    now + TimeDelta::seconds(seconds)
}

fn load_state(storage: &Storage, connection: &CpaConnection) -> Result<PollState, String> {
    if let Some(encoded) = storage.get_setting(STATE_SETTING)? {
        let state: PollState = serde_json::from_str(&encoded)
            .map_err(|_| "Stored CPA state is invalid. Reconnect CPA to recover.".to_string())?;
        return Ok(if state.endpoint == connection.base_url {
            state
        } else {
            PollState {
                endpoint: connection.base_url.clone(),
                ..Default::default()
            }
        });
    }
    // Import old caches once, preserving unknown observation age. The legacy
    // storage reader now returns a complete latest snapshot per account.
    let mut state = PollState {
        endpoint: connection.base_url.clone(),
        ..Default::default()
    };
    if let Some(encoded) = storage.get_setting("usage.cpa.last_accounts")? {
        let accounts: Vec<CpaAccountHealth> = serde_json::from_str(&encoded)
            .map_err(|_| "Stored CPA account inventory is invalid.".to_string())?;
        let buckets = storage.get_latest_cpa_usage_buckets()?;
        for mut health in accounts {
            health.quota.state = CpaQuotaState::Cached;
            let key = format!("{}/{}", health.provider, health.auth_index);
            let matching = buckets
                .iter()
                .filter(|bucket| {
                    bucket.provider.as_str() == health.provider
                        && bucket.account_id.as_deref() == Some(&health.auth_index)
                })
                .cloned()
                .collect::<Vec<_>>();
            state.accounts.insert(
                key,
                AccountState {
                    snapshot: CpaAccountSnapshot {
                        health,
                        buckets: (!matching.is_empty()).then_some(matching),
                    },
                    observed: BTreeMap::new(),
                    last_full_at: None,
                    last_attempt_at: None,
                    retry_at: None,
                    failures: 0,
                    failure: None,
                },
            );
        }
    }
    Ok(state)
}

fn result(state: &PollState, cached: bool) -> CpaPollResult {
    let mut errors = BTreeMap::<String, UsageProviderError>::new();
    let snapshots = state
        .accounts
        .values()
        .map(|account| {
            let mut snapshot = account.snapshot.clone();
            if cached && snapshot.health.quota.state == CpaQuotaState::Live {
                snapshot.health.quota.state = CpaQuotaState::Cached;
            }
            snapshot
        })
        .collect::<Vec<_>>();
    for provider in [IntegrationProvider::Claude, IntegrationProvider::Codex] {
        let accounts = state
            .accounts
            .values()
            .filter(|account| account.snapshot.health.provider == provider.as_str())
            .collect::<Vec<_>>();
        // Disabled/unavailable accounts retain local diagnostics but cannot
        // permanently degrade healthy siblings that are still being polled.
        let readable = accounts
            .iter()
            .copied()
            .filter(|account| account.snapshot.is_quota_readable())
            .collect::<Vec<_>>();
        let accounts = if readable.is_empty() {
            &accounts
        } else {
            &readable
        };
        let failure = state.source_failure.or_else(|| {
            accounts
                .iter()
                .filter_map(|account| account.failure)
                .min_by_key(|failure| match failure {
                    Failure::ManagementAuth => 0,
                    Failure::Network => 1,
                    Failure::RateLimited => 2,
                    Failure::Credential => 3,
                    Failure::Invalid => 4,
                })
        });
        if let Some(failure) = failure {
            errors.insert(
                provider.as_str().into(),
                UsageProviderError {
                    provider,
                    source: UsageSource::Cpa,
                    kind: failure.kind(),
                    message: failure.message().into(),
                },
            );
        } else if (cached && !accounts.is_empty())
            || accounts.iter().any(|account| {
                matches!(
                    account.snapshot.health.quota.state,
                    CpaQuotaState::Cached | CpaQuotaState::Unknown
                )
            })
        {
            errors.insert(
                provider.as_str().into(),
                UsageProviderError {
                    provider,
                    source: UsageSource::Cpa,
                    kind: ProviderErrorKind::Stale,
                    message: "Showing last observed CPA data; some account quotas are pending."
                        .into(),
                },
            );
        }
    }
    CpaPollResult {
        snapshots,
        errors: errors.into_values().collect(),
    }
}

pub(crate) fn cached(
    storage: &Storage,
    connection: &CpaConnection,
) -> Result<CpaPollResult, String> {
    Ok(result(&load_state(storage, connection)?, true))
}

pub(crate) async fn refresh(
    storage: &Storage,
    connection: &CpaConnection,
    force: bool,
) -> Result<CpaPollResult, String> {
    let mut state = load_state(storage, connection)?;
    let now = Utc::now();
    if state.source_failure == Some(Failure::ManagementAuth)
        || state.source_retry_at.is_some_and(|until| until > now)
    {
        return Ok(result(&state, true));
    }
    let client = CpaClient::new(&connection.base_url, &connection.management_key);
    let inventory = match &client {
        Ok(client) => client.auth_files().await,
        Err(error) => Err(error.clone()),
    };
    let mut observations = Vec::new();
    match inventory {
        Ok(files) => {
            state.source_failure = None;
            state.source_failures = 0;
            state.source_retry_at = None;
            observations = poll_account_snapshots(
                client.as_ref().expect("validated client"),
                files,
                &mut state,
                now,
                force,
            )
            .await;
        }
        Err(error) => {
            state.source_failures = state.source_failures.saturating_add(1);
            state.source_failure = Some(Failure::from_error(&error));
            state.source_retry_at = Some(retry_at(&error, now, state.source_failures));
        }
    }
    let encoded =
        serde_json::to_string(&state).map_err(|_| "Encode CPA state failed.".to_string())?;
    // Called only while the owner holds the shared usage refresh lock.
    storage.store_cpa_state(&encoded, &observations)?;
    Ok(result(&state, state.source_failure.is_some()))
}

#[derive(Clone)]
struct WindowCall {
    key: String,
    file: CpaAuthFile,
}

async fn poll_account_snapshots(
    client: &CpaClient,
    files: Vec<CpaAuthFile>,
    state: &mut PollState,
    now: DateTime<Utc>,
    force: bool,
) -> Observations {
    let mut previous = std::mem::take(&mut state.accounts);
    let mut observations = Vec::new();
    let mut calls = Vec::new();
    for file in files {
        let key = account_key(&file);
        let mut account = previous
            .remove(&key)
            .unwrap_or_else(|| AccountState::new(&file));
        let quota = std::mem::take(&mut account.snapshot.health.quota);
        account.snapshot.health = auth_file_snapshot(&file).health;
        account.snapshot.health.quota = quota;
        account.snapshot.health.quota.message = None;
        let credential_retry = file
            .cooldowns
            .iter()
            .filter(|cooldown| cooldown.scope == "credential")
            .filter_map(|cooldown| cooldown.retry_at.as_deref())
            .chain(
                file.next_retry_after
                    .as_deref()
                    .filter(|_| file.unavailable),
            )
            .filter_map(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|time| time.with_timezone(&Utc))
            .filter(|time| *time > now)
            .max();
        account.snapshot.health.quota.retry_at = credential_retry.map(|time| time.to_rfc3339());
        account.snapshot.health.quota.models = file
            .cooldowns
            .iter()
            .filter(|cooldown| cooldown.scope == "model")
            .filter_map(|cooldown| {
                let at = DateTime::parse_from_rfc3339(cooldown.retry_at.as_deref()?).ok()?;
                let model = cooldown.model_key.clone()?;
                (at > now).then(|| CpaModelAvailability {
                    model,
                    available: false,
                    available_at: Some(at.to_rfc3339()),
                })
            })
            .collect();
        let passive = super::observations::parse(&file, now);
        if let Some(passive) = passive
            && account.last_full_at.is_none_or(|full| passive.at > full)
        {
            account.apply_reading(passive.reading, passive.at, &mut observations);
        }
        let supported = matches!(
            file.provider.to_ascii_lowercase().as_str(),
            "claude" | "codex"
        );
        let eligible = supported && !file.disabled && !file.unavailable;
        let has_identity = !file.provider.eq_ignore_ascii_case("codex")
            || file
                .chatgpt_account_id
                .as_ref()
                .is_some_and(|id| !id.trim().is_empty());
        let cooling =
            account.retry_at.is_some_and(|until| until > now) || credential_retry.is_some();
        let needs_full = force
            || account
                .last_full_at
                .is_none_or(|at| !is_fresh(at, now, FULL_REFRESH_SECS));
        if eligible && !has_identity {
            account.snapshot.health.quota.message =
                Some("CPA did not provide the Codex account identity.".into());
            account.snapshot.health.quota.state = CpaQuotaState::Unknown;
        } else if eligible
            && !cooling
            && (needs_full || !account.fully_observed(now, PASSIVE_FRESH_SECS))
        {
            calls.push(WindowCall {
                key: key.clone(),
                file,
            });
        }
        state.accounts.insert(key, account);
    }
    prioritize_calls(&mut calls, state);
    let results = schedule_window_calls(
        calls,
        CPA_LAUNCH_STAGGER,
        {
            let client = client.clone();
            move |call: WindowCall| {
                let client = client.clone();
                async move {
                    let reading = match call.file.provider.to_ascii_lowercase().as_str() {
                        "claude" => fetch_claude_usage(&client, &call.file.auth_index)
                            .await
                            .map(|buckets| QuotaReading {
                                buckets,
                                ..Default::default()
                            }),
                        "codex" => match call.file.chatgpt_account_id.as_deref() {
                            Some(id) => fetch_codex_usage(&client, &call.file.auth_index, id).await,
                            None => Err(account_call_error(&call.file.auth_index)),
                        },
                        _ => unreachable!("only supported providers are scheduled"),
                    };
                    (call.key, reading, Utc::now())
                }
            }
        },
        |(_, reading, _)| reading.as_ref().err().is_some_and(is_management_failure),
    )
    .await;
    for (key, reading, observed_at) in results {
        let account = state
            .accounts
            .get_mut(&key)
            .expect("scheduled inventory account");
        match reading {
            Ok(reading) => {
                account.snapshot.health.quota.message = None;
                account.apply_full(reading, observed_at, &mut observations);
            }
            Err(error) => {
                if is_management_failure(&error) {
                    state.source_failures = state.source_failures.saturating_add(1);
                    state.source_failure = Some(Failure::from_error(&error));
                    state.source_retry_at =
                        Some(retry_at(&error, observed_at, state.source_failures));
                }
                account.fail(&error, observed_at);
            }
        }
    }
    for account in state.accounts.values_mut() {
        account.update_health(Utc::now());
    }
    observations
}

fn is_management_failure(error: &CpaError) -> bool {
    matches!(
        error,
        CpaError::Unauthorized
            | CpaError::Forbidden
            | CpaError::Unreachable
            | CpaError::ManagementCall { .. }
    )
}

fn prioritize_calls(calls: &mut Vec<WindowCall>, state: &PollState) {
    // Oldest attempted account first: bounded work without starving account 17+.
    calls.sort_by(|a, b| {
        state.accounts[&a.key]
            .last_attempt_at
            .cmp(&state.accounts[&b.key].last_attempt_at)
            .then_with(|| a.key.cmp(&b.key))
    });
    calls.truncate(CPA_ACCOUNT_LIMIT);
}

fn auth_file_snapshot(file: &CpaAuthFile) -> CpaAccountSnapshot {
    let status = file.status.trim().to_ascii_lowercase();
    CpaAccountSnapshot {
        health: CpaAccountHealth {
            provider: file.provider.trim().to_ascii_lowercase(),
            auth_index: file.auth_index.clone(),
            label: file
                .email
                .as_ref()
                .or(file.label.as_ref())
                .or(file.account.as_ref())
                .or(file.name.as_ref())
                .cloned()
                .unwrap_or_else(|| format!("Account {}", file.auth_index)),
            status: if super::aggregate::is_usable_account_status(&status) {
                "ready".into()
            } else {
                status
            },
            status_message: file.status_message.clone(),
            disabled: file.disabled,
            unavailable: file.unavailable,
            runtime_only: file.runtime_only,
            quota: Default::default(),
        },
        buckets: None,
    }
}

async fn schedule_window_calls<T, F, Fut, R>(
    calls: Vec<T>,
    stagger: Duration,
    fetch: F,
    stop: fn(&R) -> bool,
) -> Vec<R>
where
    T: Send + 'static,
    F: Fn(T) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    let mut in_flight = JoinSet::new();
    let mut results = Vec::new();
    let mut launched = false;
    for call in calls {
        if launched {
            tokio::time::sleep(stagger).await;
        }
        // Drain completed failures after the stagger, before launching siblings.
        while let Some(result) = in_flight.try_join_next() {
            let result = result.expect("CPA window task must not panic");
            let halted = stop(&result);
            results.push(result);
            if halted {
                in_flight.abort_all();
                return results;
            }
        }
        if in_flight.len() == CPA_MAX_CONCURRENCY
            && let Some(result) = in_flight.join_next().await
        {
            let result = result.expect("CPA window task must not panic");
            let halted = stop(&result);
            results.push(result);
            if halted {
                in_flight.abort_all();
                return results;
            }
        }
        let fetch = fetch.clone();
        in_flight.spawn(async move { fetch(call).await });
        launched = true;
    }
    while let Some(result) = in_flight.join_next().await {
        let result = result.expect("CPA window task must not panic");
        let halted = stop(&result);
        results.push(result);
        if halted {
            in_flight.abort_all();
            break;
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::super::quota::bucket;
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Instant;

    fn file(index: &str) -> CpaAuthFile {
        CpaAuthFile {
            auth_index: index.into(),
            provider: "claude".into(),
            name: None,
            email: None,
            label: None,
            account: None,
            status: "active".into(),
            status_message: None,
            disabled: false,
            unavailable: false,
            runtime_only: false,
            chatgpt_account_id: None,
            quota: None,
            model_quotas: BTreeMap::new(),
            cooldowns: Vec::new(),
            next_retry_after: None,
        }
    }

    fn reading(window: &str) -> QuotaReading {
        QuotaReading {
            buckets: vec![bucket(
                IntegrationProvider::Claude,
                "a",
                window,
                window.into(),
                42.0,
                None,
                0,
            )],
            ..Default::default()
        }
    }

    // @lat: [[cpa-tests#CPA Regression Tests#Authoritative snapshots and timestamps]]
    #[test]
    fn cpa_full_reads_replace_windows_and_failed_reads_preserve_observation_age() {
        let now = Utc::now();
        let mut account = AccountState::new(&file("a"));
        let mut recorded = Vec::new();
        account.apply_full(
            reading("retired"),
            now - TimeDelta::seconds(200),
            &mut recorded,
        );
        account.apply_full(reading("five_hour"), now, &mut recorded);
        account.fail(
            &CpaError::AccountCall {
                auth_index: "a".into(),
                status_code: Some(429),
                retry_after_secs: Some(600),
            },
            now,
        );
        account.update_health(now);
        assert_eq!(account.snapshot.buckets.as_ref().unwrap().len(), 1);
        assert!(
            account.snapshot.buckets.as_ref().unwrap()[0]
                .key
                .ends_with("/five_hour")
        );
        assert_eq!(
            account.snapshot.health.quota.observed_at,
            Some(now.to_rfc3339())
        );
        assert_eq!(account.snapshot.health.quota.state, CpaQuotaState::Cached);
        assert_eq!(account.retry_at, Some(now + TimeDelta::seconds(600)));
        account.apply_reading(reading("five_hour"), now, &mut recorded);
        assert_eq!(
            recorded.len(),
            2,
            "same passive observation is not a new historical sample"
        );
        account.apply_full(
            reading("seven_day"),
            now + TimeDelta::seconds(1),
            &mut recorded,
        );
        account.update_health(now + TimeDelta::seconds(1));
        assert!(account.failure.is_none());
        assert_eq!(account.snapshot.health.quota.state, CpaQuotaState::Live);
    }

    // @lat: [[cpa-tests#CPA Regression Tests#Partial scoped responses]]
    #[test]
    fn cpa_partial_reads_keep_old_scopes_cached_and_update_valid_windows() {
        let now = Utc::now();
        let mut account = AccountState::new(&file("a"));
        let mut recorded = Vec::new();
        account.apply_full(reading("scoped"), now, &mut recorded);
        let mut partial = reading("five_hour");
        partial.partial = true;
        account.apply_full(partial, now + TimeDelta::seconds(1), &mut recorded);
        account.update_health(now + TimeDelta::seconds(1));
        assert_eq!(account.snapshot.buckets.as_ref().unwrap().len(), 2);
        assert_eq!(account.last_full_at, Some(now));
        assert_eq!(account.snapshot.health.quota.state, CpaQuotaState::Cached);
        assert!(account.failure.is_some());
    }

    // @lat: [[cpa-tests#CPA Regression Tests#Inactive account diagnostics]]
    #[test]
    fn cpa_disabled_failed_accounts_do_not_degrade_healthy_siblings() {
        let now = Utc::now();
        let mut good = AccountState::new(&file("a"));
        good.apply_full(reading("five_hour"), now, &mut Vec::new());
        good.update_health(now);
        let mut bad = AccountState::new(&file("b"));
        bad.fail(
            &CpaError::AccountCall {
                auth_index: "b".into(),
                status_code: Some(401),
                retry_after_secs: None,
            },
            now,
        );
        bad.update_health(now);
        let mut state = PollState {
            accounts: BTreeMap::from([("claude/a".into(), good), ("claude/b".into(), bad)]),
            ..Default::default()
        };
        assert!(!result(&state, false).errors.is_empty());
        state
            .accounts
            .get_mut("claude/b")
            .unwrap()
            .snapshot
            .health
            .disabled = true;
        assert!(result(&state, false).errors.is_empty());
        assert_eq!(
            result(&state, false).snapshots[1].health.quota.state,
            CpaQuotaState::Paused
        );
    }

    // @lat: [[cpa-tests#CPA Regression Tests#Passive observation truthfulness]]
    #[test]
    fn cpa_partial_passive_coverage_does_not_skip_scoped_reads() {
        let now = Utc::now();
        let mut account = AccountState::new(&file("a"));
        let mut recorded = Vec::new();
        account.apply_full(
            reading("weekly_scoped_fable"),
            now - TimeDelta::seconds(181),
            &mut recorded,
        );
        account.apply_reading(reading("five_hour"), now, &mut recorded);
        assert!(!account.fully_observed(now, PASSIVE_FRESH_SECS));
    }

    // @lat: [[cpa-tests#CPA Regression Tests#Fair bounded scheduling]]
    #[test]
    fn cpa_oldest_attempt_rotation_reaches_accounts_beyond_sixteen() {
        let mut state = PollState::default();
        let all = (0..20)
            .map(|i| {
                let file = file(&i.to_string());
                let key = account_key(&file);
                state.accounts.insert(key.clone(), AccountState::new(&file));
                WindowCall { key, file }
            })
            .collect::<Vec<_>>();
        let mut calls = all.clone();
        prioritize_calls(&mut calls, &state);
        assert_eq!(calls.len(), 16);
        let attempted = calls
            .iter()
            .map(|call| call.key.clone())
            .collect::<Vec<_>>();
        for key in &attempted {
            state.accounts.get_mut(key).unwrap().last_attempt_at = Some(Utc::now());
        }
        let mut calls = all;
        prioritize_calls(&mut calls, &state);
        assert!(
            calls
                .iter()
                .take(4)
                .all(|call| !attempted.contains(&call.key))
        );
    }

    // @lat: [[cpa-tests#CPA Regression Tests#HTTP rejection persistence]]
    #[tokio::test]
    async fn cpa_http_rejections_persist_across_database_reopen_without_retries() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (status, kind) in [
            (401, ProviderErrorKind::Auth),
            (403, ProviderErrorKind::Auth),
            (429, ProviderErrorKind::Stale),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let connection = CpaConnection {
                base_url: format!("http://{}", listener.local_addr().unwrap()),
                management_key: "fixture-key".into(),
            };
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 1024];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        break;
                    }
                }
                socket.write_all(format!("HTTP/1.1 {status} Rejected\r\nContent-Length: 0\r\nRetry-After: 600\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            });
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("usage.db");
            let storage = crate::storage::cpa::test_storage_at(&path);
            storage
                .save_cpa_connection(&connection.base_url, &connection.management_key)
                .unwrap();
            assert_eq!(
                refresh(&storage, &connection, false).await.unwrap().errors[0].kind,
                kind
            );
            server.await.unwrap();
            drop(storage);
            let reopened = crate::storage::cpa::test_storage_at(&path);
            // The server is gone. Any accidental retry would replace this with Network.
            assert_eq!(
                refresh(&reopened, &connection, true).await.unwrap().errors[0].kind,
                kind
            );
        }
        assert!(is_management_failure(&CpaError::ManagementCall {
            status_code: 429,
            retry_after_secs: Some(60)
        }));
        assert!(!is_management_failure(&CpaError::AccountCall {
            auth_index: "a".into(),
            status_code: Some(429),
            retry_after_secs: Some(60)
        }));
    }

    // @lat: [[cpa-tests#CPA Regression Tests#Management authentication suppression]]
    #[tokio::test]
    async fn cpa_management_rejection_survives_restart_and_forced_refresh() {
        let storage = crate::storage::cpa::test_storage();
        let connection = CpaConnection {
            base_url: "http://127.0.0.1:1".into(),
            management_key: "test".into(),
        };
        let state = PollState {
            endpoint: connection.base_url.clone(),
            source_failure: Some(Failure::ManagementAuth),
            ..Default::default()
        };
        storage
            .store_cpa_state(&serde_json::to_string(&state).unwrap(), &[])
            .unwrap();
        for force in [false, true] {
            let result = refresh(&storage, &connection, force).await.unwrap();
            assert_eq!(result.errors[0].kind, ProviderErrorKind::Auth);
        }
        let different = CpaConnection {
            base_url: "http://127.0.0.1:2".into(),
            management_key: "test".into(),
        };
        assert!(
            load_state(&storage, &different)
                .unwrap()
                .source_failure
                .is_none()
        );
        storage
            .save_cpa_connection(&connection.base_url, "new")
            .unwrap();
        assert!(
            load_state(&storage, &connection)
                .unwrap()
                .source_failure
                .is_none()
        );
    }

    // @lat: [[cpa-tests#CPA Regression Tests#Fair bounded scheduling]]
    #[tokio::test]
    async fn cpa_fanout_is_staggered_bounded_and_stops_after_management_auth() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let launches = Arc::new(Mutex::new(Vec::new()));
        let started = Instant::now();
        let results = schedule_window_calls(
            (0..12).collect(),
            CPA_LAUNCH_STAGGER,
            {
                let active = active.clone();
                let peak = peak.clone();
                let launches = launches.clone();
                move |call| {
                    let active = active.clone();
                    let peak = peak.clone();
                    let launches = launches.clone();
                    async move {
                        launches.lock().unwrap().push(Instant::now());
                        peak.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        active.fetch_sub(1, Ordering::SeqCst);
                        call
                    }
                }
            },
            |_| false,
        )
        .await;
        assert_eq!(results.len(), 12);
        assert!(peak.load(Ordering::SeqCst) <= CPA_MAX_CONCURRENCY);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            launches
                .lock()
                .unwrap()
                .windows(2)
                .all(|pair| pair[1].duration_since(pair[0]) >= Duration::from_millis(200))
        );
        for error in [
            CpaError::Unauthorized,
            CpaError::Unreachable,
            CpaError::ManagementCall {
                status_code: 429,
                retry_after_secs: Some(60),
            },
        ] {
            let results = schedule_window_calls(
                (0..12).collect(),
                Duration::from_millis(20),
                move |call| {
                    let error = error.clone();
                    async move { (call, Err::<(), _>(error)) }
                },
                |(_, result)| result.as_ref().err().is_some_and(is_management_failure),
            )
            .await;
            assert_eq!(results.len(), 1);
        }
    }
}
