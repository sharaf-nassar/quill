use super::aggregate::CpaAccountSnapshot;
use super::client::{CpaAuthFile, CpaClient, CpaError};
use super::quota::{fetch_claude_usage, fetch_codex_usage};
use crate::integrations::IntegrationProvider;
use crate::models::CpaAccountHealth;
use std::cmp::Ordering;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

pub(crate) const CPA_ACCOUNT_LIMIT: usize = 16;
pub(crate) const CPA_MAX_CONCURRENCY: usize = 3;
pub(crate) const CPA_LAUNCH_STAGGER: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WindowSmokeGates {
    pub claude: bool,
    pub codex: bool,
}

#[derive(Clone, Debug)]
struct WindowCall {
    account_position: usize,
    provider: IntegrationProvider,
    auth_index: String,
    chatgpt_account_id: Option<String>,
}

pub(crate) async fn poll_account_snapshots(
    client: &CpaClient,
    mut auth_files: Vec<CpaAuthFile>,
    smoke: WindowSmokeGates,
) -> Vec<CpaAccountSnapshot> {
    auth_files.sort_by(|left, right| compare_auth_indexes(&left.auth_index, &right.auth_index));
    let mut snapshots = auth_files
        .iter()
        .map(auth_file_snapshot)
        .collect::<Vec<_>>();
    let calls = window_calls(&snapshots, &auth_files, smoke);
    let results = schedule_window_calls(calls, CPA_LAUNCH_STAGGER, {
        let client = client.clone();
        move |call| {
            let client = client.clone();
            async move {
                let result = match call.provider {
                    IntegrationProvider::Claude => {
                        fetch_claude_usage(&client, &call.auth_index).await
                    }
                    IntegrationProvider::Codex => match call.chatgpt_account_id.as_deref() {
                        Some(account_id) => {
                            fetch_codex_usage(&client, &call.auth_index, account_id).await
                        }
                        None => Err(CpaError::AccountCall {
                            auth_index: call.auth_index.clone(),
                            status_code: None,
                        }),
                    },
                    IntegrationProvider::MiniMax => unreachable!("CPA window plan is native-only"),
                };
                (call.account_position, result)
            }
        }
    })
    .await;

    for (account_position, result) in results {
        let Ok(mut buckets) = result else {
            continue;
        };
        let label = snapshots[account_position].health.label.clone();
        for bucket in &mut buckets {
            bucket.account_label = Some(label.clone());
        }
        snapshots[account_position].buckets = Some(buckets);
    }
    snapshots
}

fn auth_file_snapshot(auth_file: &CpaAuthFile) -> CpaAccountSnapshot {
    CpaAccountSnapshot {
        health: CpaAccountHealth {
            provider: auth_file.provider.trim().to_ascii_lowercase(),
            auth_index: auth_file.auth_index.clone(),
            label: account_label(auth_file),
            status: auth_file.status.trim().to_ascii_lowercase(),
            status_message: auth_file.status_message.clone(),
            disabled: auth_file.disabled,
            unavailable: auth_file.unavailable,
            runtime_only: auth_file.runtime_only,
        },
        buckets: None,
    }
}

fn account_label(auth_file: &CpaAuthFile) -> String {
    auth_file
        .email
        .as_ref()
        .or(auth_file.label.as_ref())
        .or(auth_file.account.as_ref())
        .or(auth_file.name.as_ref())
        .cloned()
        .unwrap_or_else(|| format!("Account {}", auth_file.auth_index))
}

fn compare_auth_indexes(left: &str, right: &str) -> Ordering {
    match (left.parse::<u64>(), right.parse::<u64>()) {
        (Ok(left_number), Ok(right_number)) => {
            left_number.cmp(&right_number).then_with(|| left.cmp(right))
        }
        _ => left.cmp(right),
    }
}

fn window_calls(
    snapshots: &[CpaAccountSnapshot],
    auth_files: &[CpaAuthFile],
    smoke: WindowSmokeGates,
) -> Vec<WindowCall> {
    snapshots
        .iter()
        .zip(auth_files)
        .enumerate()
        .filter(|(_, (snapshot, _))| snapshot.is_healthy())
        .filter_map(|(account_position, (snapshot, auth_file))| {
            let provider = match snapshot.health.provider.as_str() {
                "claude" if smoke.claude => IntegrationProvider::Claude,
                "codex" if smoke.codex => IntegrationProvider::Codex,
                _ => return None,
            };
            Some(WindowCall {
                account_position,
                provider,
                auth_index: snapshot.health.auth_index.clone(),
                chatgpt_account_id: auth_file.chatgpt_account_id.clone(),
            })
        })
        .take(CPA_ACCOUNT_LIMIT)
        .collect()
}

async fn schedule_window_calls<T, F, Fut, R>(
    calls: Vec<T>,
    launch_stagger: Duration,
    fetch: F,
) -> Vec<R>
where
    T: Send + 'static,
    F: Fn(T) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    let semaphore = Arc::new(Semaphore::new(CPA_MAX_CONCURRENCY));
    let mut in_flight = JoinSet::new();
    let mut results = Vec::new();
    let mut launched = false;
    for call in calls {
        if in_flight.len() == CPA_MAX_CONCURRENCY
            && let Some(result) = in_flight.join_next().await
        {
            results.push(result.expect("CPA window task must not panic"));
        }
        if launched {
            tokio::time::sleep(launch_stagger).await;
        }
        let semaphore = Arc::clone(&semaphore);
        let fetch = fetch.clone();
        in_flight.spawn(async move {
            let permit = semaphore
                .acquire_owned()
                .await
                .expect("CPA scheduler semaphore remains open");
            let result = fetch(call).await;
            drop(permit);
            result
        });
        launched = true;
    }
    while let Some(result) = in_flight.join_next().await {
        results.push(result.expect("CPA window task must not panic"));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    fn auth_file(auth_index: &str, provider: &str) -> CpaAuthFile {
        CpaAuthFile {
            auth_index: auth_index.to_string(),
            provider: provider.to_string(),
            name: None,
            email: None,
            label: None,
            account: None,
            status: "ready".to_string(),
            status_message: None,
            disabled: false,
            unavailable: false,
            runtime_only: false,
            chatgpt_account_id: Some("account-id".to_string()),
        }
    }

    // @lat: [[features#Features#Live Usage View#CPA Poll Scheduling#Smoke verdict gate]]
    #[test]
    fn false_or_absent_smoke_verdict_suppresses_window_calls() {
        let files = [auth_file("1", "claude"), auth_file("2", "codex")];
        let snapshots = files.iter().map(auth_file_snapshot).collect::<Vec<_>>();
        assert!(window_calls(&snapshots, &files, WindowSmokeGates::default()).is_empty());
        assert!(
            window_calls(
                &snapshots,
                &files,
                WindowSmokeGates {
                    claude: false,
                    codex: false,
                },
            )
            .is_empty()
        );
    }

    // @lat: [[features#Features#Live Usage View#CPA Poll Scheduling#Bounded staggered fan-out]]
    #[tokio::test]
    async fn twelve_account_stub_fanout_stays_bounded_and_inside_budget() {
        let calls = (0..12).collect::<Vec<_>>();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let launch_times = Arc::new(Mutex::new(Vec::new()));
        let started = Instant::now();
        let results = schedule_window_calls(calls, CPA_LAUNCH_STAGGER, {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            let launch_times = Arc::clone(&launch_times);
            move |call| {
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                let launch_times = Arc::clone(&launch_times);
                async move {
                    launch_times
                        .lock()
                        .expect("launch times lock")
                        .push(Instant::now());
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    call
                }
            }
        })
        .await;

        assert_eq!(results.len(), 12);
        assert!(peak.load(Ordering::SeqCst) <= CPA_MAX_CONCURRENCY);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            launch_times
                .lock()
                .expect("launch times lock")
                .windows(2)
                .all(|times| times[1].duration_since(times[0]) >= Duration::from_millis(200))
        );
    }

    #[test]
    // @lat: [[features#Features#Live Usage View#CPA Poll Scheduling#Deterministic account cap]]
    fn deterministic_limit_uses_first_sixteen_auth_indexes() {
        let mut files = (0..20)
            .rev()
            .map(|index| auth_file(&index.to_string(), "claude"))
            .collect::<Vec<_>>();
        files.sort_by(|left, right| compare_auth_indexes(&left.auth_index, &right.auth_index));
        let snapshots = files.iter().map(auth_file_snapshot).collect::<Vec<_>>();
        let calls = window_calls(
            &snapshots,
            &files,
            WindowSmokeGates {
                claude: true,
                codex: false,
            },
        );
        let indexes = calls
            .iter()
            .map(|call| call.auth_index.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            indexes,
            (0..16).map(|index| index.to_string()).collect::<Vec<_>>()
        );
    }
}
