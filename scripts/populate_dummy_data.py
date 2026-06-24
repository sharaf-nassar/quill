#!/usr/bin/env python3
"""
Seed Quill's SQLite DB with reproducible dummy data for screenshots.

Default usage (writes to your real Quill DB after backing it up):
    python3 scripts/populate_dummy_data.py

Sandboxed usage (writes only inside an arbitrary dir, no backup, no running-Quill guard):
    python3 scripts/populate_dummy_data.py \\
        --data-dir /tmp/quill-demo/data \\
        --rules-dir /tmp/quill-demo/rules \\
        --no-backup

The CLI surface is documented in
specs/001-marketing-site/contracts/seeder-cli.md.
"""

import argparse
import hashlib
import json
import os
import random
import shutil
import sqlite3
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path

DEFAULT_DATA_DIR = Path.home() / ".local" / "share" / "com.quilltoolkit.app"
DEFAULT_RULES_DIR = Path.home() / ".claude" / "rules" / "learned"
DEFAULT_PROJECTS_DIR = Path.home() / ".claude" / "projects"

# Module-level globals bound by main() after argparse so existing populate_* helpers
# can read them without threading a config object through every signature.
DB_PATH: Path = DEFAULT_DATA_DIR / "usage.db"
BAK_PATH: Path = DB_PATH.with_suffix(".db.bak")
PROJECTS_DIR: Path = DEFAULT_PROJECTS_DIR
QUIET: bool = False
NO_BACKUP: bool = False
USING_OVERRIDE: bool = False  # True when --data-dir was passed; skips running-Quill guard
SKIP_PROJECTS: bool = False   # True when --no-projects was passed


def log(msg: str = "") -> None:
    """Stage-progress output; suppressed under --quiet."""
    if not QUIET:
        print(msg)

HOSTNAMES = ["macbook-pro", "dev-server", "workstation"]
PROJECTS = [
	"/home/alex/quill",
	"/home/alex/gateway",
	"/home/alex/pipeline",
	"/home/alex/dashboard",
]
BUCKET_LABELS = ["5 hours", "7 days", "Sonnet", "Opus", "Code", "OAuth"]

TOOLS = [
	"Edit", "Write", "Read", "Bash", "Grep", "Glob",
	"WebSearch", "WebFetch", "Task", "TodoWrite",
]

HOOK_PHASES = ["PreToolUse", "PostToolUse", "Stop"]

NOW = datetime.now(timezone.utc)


def ts(dt: datetime) -> str:
	# Naive ISO (no offset). Use this for fields consumed by the
	# `src/utils/time.ts::timeAgo` helper which UNCONDITIONALLY appends "Z"
	# before parsing — a datetime with "+00:00" offset would produce
	# "...+00:00Z" which parses to NaN ("last NaNd ago").
	# Used by: learning_runs.created_at and any other field whose display
	# path goes through that timeAgo helper.
	return dt.replace(tzinfo=None).isoformat()


def ts_tz(dt: datetime) -> str:
	# Timezone-aware RFC3339 (`+00:00` suffix). Matches what production
	# code writes via `chrono::Utc::now().to_rfc3339()`.
	# Use this for fields whose READER calls `chrono::DateTime::parse_from_rfc3339`
	# in Rust (which rejects naive ISO and falls back to epoch 0). Examples:
	#   - response_times.timestamp (read by parse_ts_diff in get_llm_runtime_stats)
	#   - JSONL session timestamp field (Tantivy session indexer)
	dt_utc = dt if dt.tzinfo is not None else dt.replace(tzinfo=timezone.utc)
	return dt_utc.isoformat()


def rand_session() -> str:
	return str(uuid.UUID(int=random.getrandbits(128)))


def rand_hex(n: int = 40) -> str:
	return "".join(random.choices("0123456789abcdef", k=n))


def check_quill_not_running() -> None:
	"""Ensure Quill is not running — restoring over an active WAL connection corrupts the DB."""
	import subprocess
	result = subprocess.run(
		["pgrep", "-f", "quill"],
		capture_output=True, text=True,
	)
	if result.returncode == 0:
		pids = result.stdout.strip()
		print(f"\n  ERROR: Quill appears to be running (PIDs: {pids.replace(chr(10), ', ')})")
		print("  Stop Quill before running this script to avoid DB corruption.")
		print("  Kill it with: pkill -f quill")
		raise SystemExit(1)


def backup_db() -> None:
	if not DB_PATH.exists():
		print(f"  DB not found at {DB_PATH}, skipping backup.")
		return
	# Remove stale WAL/SHM files to avoid corruption on restore
	for suffix in ["-wal", "-shm"]:
		wal = DB_PATH.with_name(DB_PATH.name + suffix)
		if wal.exists():
			wal.unlink()
			print(f"  Removed stale {wal.name}")
	shutil.copy2(DB_PATH, BAK_PATH)
	print(f"  Backed up DB to {BAK_PATH}")


def ensure_schema(conn: sqlite3.Connection) -> None:
	"""Run the same migrations the Rust app would run so all tables exist."""
	conn.executescript("""
		PRAGMA journal_mode=WAL;
		PRAGMA busy_timeout=5000;

		-- NOTE: provider + bucket_key are added by Rust migration 14 when this
		-- table is opened by the app. We create them up-front (with the same
		-- shape migration 14 produces) so the seeder can write Codex rows that
		-- survive: migration 14's RENAME/backfill only runs when the columns are
		-- ABSENT, and it would otherwise force every row to provider='claude'.
		-- get_latest_usage_buckets() reads the latest row per (provider,
		-- bucket_key), so both columns must carry real values here.
		CREATE TABLE IF NOT EXISTS usage_snapshots (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			timestamp TEXT NOT NULL,
			provider TEXT NOT NULL DEFAULT 'claude',
			bucket_key TEXT NOT NULL,
			bucket_label TEXT NOT NULL,
			utilization REAL NOT NULL,
			resets_at TEXT,
			created_at TEXT DEFAULT (datetime('now'))
		);

		CREATE TABLE IF NOT EXISTS usage_hourly (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			hour TEXT NOT NULL,
			provider TEXT NOT NULL DEFAULT 'claude',
			bucket_key TEXT NOT NULL,
			bucket_label TEXT NOT NULL,
			avg_utilization REAL NOT NULL,
			max_utilization REAL NOT NULL,
			min_utilization REAL NOT NULL,
			sample_count INTEGER NOT NULL,
			UNIQUE(hour, provider, bucket_key)
		);

		-- token_snapshots: `cwd` (mig 1) + `provider` (mig 12) folded in.
		CREATE TABLE IF NOT EXISTS token_snapshots (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			session_id TEXT NOT NULL,
			hostname TEXT NOT NULL DEFAULT 'local',
			timestamp TEXT NOT NULL,
			input_tokens INTEGER NOT NULL,
			output_tokens INTEGER NOT NULL,
			cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
			cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
			cwd TEXT DEFAULT NULL,
			created_at TEXT DEFAULT (datetime('now')),
			provider TEXT NOT NULL DEFAULT 'claude'
		);

		-- token_hourly: rebuilt by mig 12 with `provider` and UNIQUE(hour,
		-- provider, hostname).
		CREATE TABLE IF NOT EXISTS token_hourly (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			hour TEXT NOT NULL,
			provider TEXT NOT NULL DEFAULT 'claude',
			hostname TEXT NOT NULL DEFAULT 'local',
			total_input INTEGER NOT NULL,
			total_output INTEGER NOT NULL,
			total_cache_creation INTEGER NOT NULL DEFAULT 0,
			total_cache_read INTEGER NOT NULL DEFAULT 0,
			turn_count INTEGER NOT NULL,
			UNIQUE(hour, hostname, provider)
		);

		CREATE TABLE IF NOT EXISTS settings (
			key TEXT PRIMARY KEY,
			value TEXT NOT NULL
		);

		-- observations: `provider` added by mig 12.
		CREATE TABLE IF NOT EXISTS observations (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			session_id TEXT NOT NULL,
			timestamp TEXT NOT NULL,
			hook_phase TEXT NOT NULL,
			tool_name TEXT NOT NULL,
			tool_input TEXT,
			tool_output TEXT,
			cwd TEXT,
			created_at TEXT DEFAULT (datetime('now')),
			provider TEXT NOT NULL DEFAULT 'claude'
		);

		-- learning_runs: + logs (mig 5), phases (mig 9), provider_scope (mig 12),
		-- inference_metadata (mig 24).
		CREATE TABLE IF NOT EXISTS learning_runs (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			trigger_mode TEXT NOT NULL,
			observations_analyzed INTEGER NOT NULL DEFAULT 0,
			rules_created INTEGER NOT NULL DEFAULT 0,
			rules_updated INTEGER NOT NULL DEFAULT 0,
			duration_ms INTEGER,
			status TEXT NOT NULL DEFAULT 'running',
			error TEXT,
			created_at TEXT DEFAULT (datetime('now')),
			logs TEXT DEFAULT NULL,
			phases TEXT DEFAULT NULL,
			provider_scope TEXT NOT NULL DEFAULT '["claude"]',
			inference_metadata TEXT DEFAULT NULL
		);

		-- learned_rules: TRUE v27 shape. Base cols + source (mig 4-ish),
		-- content (mig 11), provider_scope (mig 12), content_hash (mig 15), and
		-- the six migration-25 provenance/lifecycle columns.
		CREATE TABLE IF NOT EXISTS learned_rules (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			name TEXT NOT NULL UNIQUE,
			domain TEXT,
			confidence REAL NOT NULL DEFAULT 0.5,
			observation_count INTEGER NOT NULL DEFAULT 0,
			file_path TEXT NOT NULL,
			created_at TEXT DEFAULT (datetime('now')),
			updated_at TEXT DEFAULT (datetime('now')),
			source TEXT DEFAULT 'observations',
			alpha REAL NOT NULL DEFAULT 1.0,
			beta_param REAL NOT NULL DEFAULT 1.0,
			last_evidence_at TEXT DEFAULT NULL,
			state TEXT NOT NULL DEFAULT 'emerging',
			project TEXT DEFAULT NULL,
			is_anti_pattern INTEGER NOT NULL DEFAULT 0,
			confirmed_projects TEXT DEFAULT NULL,
			content TEXT DEFAULT NULL,
			provider_scope TEXT NOT NULL DEFAULT '["claude"]',
			content_hash TEXT DEFAULT NULL,
			lifecycle TEXT NOT NULL DEFAULT 'candidate',
			origin_run_id INTEGER,
			origin_model TEXT,
			origin_at TEXT,
			current_version INTEGER NOT NULL DEFAULT 1,
			superseded_by TEXT
		);

		CREATE TABLE IF NOT EXISTS schema_version (
			version INTEGER PRIMARY KEY
		);

		-- observation_summaries: rebuilt by mig 12 with `provider` and
		-- UNIQUE(period, provider, project).
		CREATE TABLE IF NOT EXISTS observation_summaries (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			period TEXT NOT NULL,
			provider TEXT NOT NULL DEFAULT 'claude',
			project TEXT,
			tool_counts TEXT NOT NULL,
			error_count INTEGER NOT NULL DEFAULT 0,
			total_observations INTEGER NOT NULL DEFAULT 0,
			created_at TEXT DEFAULT (datetime('now')),
			UNIQUE(period, provider, project)
		);

		-- tool_actions: `provider` added by mig 12.
		CREATE TABLE IF NOT EXISTS tool_actions (
			id            INTEGER PRIMARY KEY AUTOINCREMENT,
			message_id    TEXT NOT NULL,
			session_id    TEXT NOT NULL,
			tool_name     TEXT NOT NULL,
			category      TEXT NOT NULL,
			file_path     TEXT,
			summary       TEXT NOT NULL,
			full_input    TEXT,
			full_output   TEXT,
			timestamp     TEXT NOT NULL,
			provider      TEXT NOT NULL DEFAULT 'claude'
		);

		CREATE TABLE IF NOT EXISTS memory_files (
			id              INTEGER PRIMARY KEY AUTOINCREMENT,
			project_path    TEXT NOT NULL,
			file_path       TEXT NOT NULL,
			content_hash    TEXT NOT NULL,
			last_scanned_at TEXT NOT NULL,
			UNIQUE(project_path, file_path)
		);

		-- optimization_runs: + provider_scope (mig 12), inference_metadata (mig 24).
		CREATE TABLE IF NOT EXISTS optimization_runs (
			id                  INTEGER PRIMARY KEY AUTOINCREMENT,
			project_path        TEXT NOT NULL,
			trigger             TEXT NOT NULL,
			memories_scanned    INTEGER NOT NULL DEFAULT 0,
			suggestions_created INTEGER NOT NULL DEFAULT 0,
			context_sources     TEXT NOT NULL DEFAULT '{}',
			status              TEXT NOT NULL DEFAULT 'running',
			error               TEXT,
			started_at          TEXT NOT NULL,
			completed_at        TEXT,
			provider_scope      TEXT NOT NULL DEFAULT '["claude"]',
			inference_metadata  TEXT DEFAULT NULL
		);

		CREATE TABLE IF NOT EXISTS optimization_suggestions (
			id               INTEGER PRIMARY KEY AUTOINCREMENT,
			run_id           INTEGER NOT NULL REFERENCES optimization_runs(id),
			project_path     TEXT NOT NULL,
			action_type      TEXT NOT NULL,
			target_file      TEXT,
			reasoning        TEXT NOT NULL,
			proposed_content TEXT,
			merge_sources    TEXT,
			status           TEXT NOT NULL DEFAULT 'pending',
			error            TEXT,
			resolved_at      TEXT,
			created_at       TEXT NOT NULL,
			original_content TEXT,
			diff_summary     TEXT,
			backup_data      TEXT,
			group_id         TEXT,
			provider_scope   TEXT NOT NULL DEFAULT '["claude"]'
		);

		CREATE TABLE IF NOT EXISTS git_snapshots (
			id           INTEGER PRIMARY KEY AUTOINCREMENT,
			project      TEXT NOT NULL UNIQUE,
			commit_hash  TEXT NOT NULL,
			commit_count INTEGER NOT NULL,
			raw_data     TEXT NOT NULL,
			created_at   TEXT DEFAULT (datetime('now'))
		);

		-- response_times: rebuilt by mig 12 with `provider` and
		-- UNIQUE(provider, session_id, timestamp).
		CREATE TABLE IF NOT EXISTS response_times (
			id           INTEGER PRIMARY KEY AUTOINCREMENT,
			provider     TEXT NOT NULL DEFAULT 'claude',
			session_id   TEXT NOT NULL,
			timestamp    TEXT NOT NULL,
			response_secs REAL,
			idle_secs    REAL,
			created_at   TEXT DEFAULT (datetime('now')),
			UNIQUE(provider, session_id, timestamp)
		);

		CREATE TABLE IF NOT EXISTS context_savings_events (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			event_id TEXT NOT NULL UNIQUE,
			schema_version INTEGER NOT NULL,
			provider TEXT NOT NULL,
			session_id TEXT,
			hostname TEXT NOT NULL,
			cwd TEXT,
			timestamp TEXT NOT NULL,
			event_type TEXT NOT NULL,
			source TEXT NOT NULL,
			decision TEXT NOT NULL,
			category TEXT NOT NULL DEFAULT 'unknown',
			reason TEXT,
			delivered INTEGER NOT NULL,
			indexed_bytes INTEGER,
			returned_bytes INTEGER,
			input_bytes INTEGER,
			tokens_indexed_est INTEGER,
			tokens_returned_est INTEGER,
			tokens_saved_est INTEGER,
			tokens_preserved_est INTEGER,
			estimate_method TEXT,
			estimate_confidence REAL,
			source_ref TEXT,
			snapshot_ref TEXT,
			metadata_json TEXT,
			created_at TEXT NOT NULL DEFAULT (datetime('now'))
		);

		-- session_events: the per-JSONL-line timeline feeding get_llm_runtime_stats
		-- (feature 008). Mirrors Rust migration 26 exactly (table + indexes) so
		-- the app's `CREATE TABLE IF NOT EXISTS` becomes a no-op. The LLM RUNTIME
		-- card reads this table EXCLUSIVELY; without rows it shows "no data".
		CREATE TABLE IF NOT EXISTS session_events (
			provider     TEXT NOT NULL,
			session_id   TEXT NOT NULL,
			agent_id     TEXT,
			is_sidechain INTEGER NOT NULL DEFAULT 0,
			timestamp    TEXT NOT NULL,
			kind         TEXT NOT NULL,
			uuid         TEXT,
			parent_uuid  TEXT
		);
		CREATE UNIQUE INDEX IF NOT EXISTS uidx_se_identity
			ON session_events(provider, session_id, COALESCE(agent_id, ''), timestamp, kind);
		CREATE INDEX IF NOT EXISTS idx_se_timestamp
			ON session_events(timestamp);
		CREATE INDEX IF NOT EXISTS idx_se_chain
			ON session_events(provider, session_id, agent_id, timestamp);
		CREATE INDEX IF NOT EXISTS idx_se_provider_session_sidechain
			ON session_events(provider, session_id, is_sidechain, timestamp);

		-- ── Tables created by migrations 21/25/27 ──────────────────────────────
		-- The app records schema_version up to the latest (27), so it runs ZERO
		-- migrations against this DB. Every table a migration would otherwise
		-- CREATE must therefore exist here in final shape, or the app's queries
		-- against them fail. Shapes copied verbatim from src-tauri/src/storage.rs.

		-- skill_usages: mig 21 (table) + mig 22 (cwd, hostname).
		CREATE TABLE IF NOT EXISTS skill_usages (
			id          INTEGER PRIMARY KEY AUTOINCREMENT,
			provider    TEXT NOT NULL,
			session_id  TEXT NOT NULL,
			message_id  TEXT NOT NULL,
			skill_name  TEXT NOT NULL,
			skill_path  TEXT NOT NULL,
			timestamp   TEXT NOT NULL,
			tool_name   TEXT,
			created_at  TEXT DEFAULT (datetime('now')),
			cwd         TEXT,
			hostname    TEXT,
			UNIQUE(provider, session_id, message_id, skill_name, skill_path, timestamp)
		);
		CREATE INDEX IF NOT EXISTS idx_skill_usages_provider_ts
			ON skill_usages(provider, timestamp);
		CREATE INDEX IF NOT EXISTS idx_skill_usages_provider_session
			ON skill_usages(provider, session_id);
		CREATE INDEX IF NOT EXISTS idx_skill_usages_skill_ts
			ON skill_usages(skill_name, timestamp);
		CREATE INDEX IF NOT EXISTS idx_skill_usages_skill_cwd
			ON skill_usages(skill_name, cwd);

		-- rule_versions: mig 25 (append-only rule content history).
		CREATE TABLE IF NOT EXISTS rule_versions (
			id              INTEGER PRIMARY KEY AUTOINCREMENT,
			rule_id         INTEGER NOT NULL
								REFERENCES learned_rules(id) ON DELETE CASCADE,
			version         INTEGER NOT NULL,
			content         TEXT NOT NULL,
			content_hash    TEXT NOT NULL,
			domain          TEXT,
			is_anti_pattern INTEGER NOT NULL DEFAULT 0,
			provider_scope  TEXT NOT NULL DEFAULT '["claude"]',
			source          TEXT,
			run_id          INTEGER,
			change_kind     TEXT NOT NULL,
			rolled_back_from INTEGER,
			author          TEXT NOT NULL DEFAULT 'system',
			created_at      TEXT NOT NULL DEFAULT (datetime('now')),
			UNIQUE(rule_id, version)
		);
		CREATE INDEX IF NOT EXISTS idx_rule_versions_rule_version
			ON rule_versions(rule_id, version DESC);

		-- rule_evidence_citations: mig 25 (grounding snapshot; no FK on obs).
		CREATE TABLE IF NOT EXISTS rule_evidence_citations (
			id             INTEGER PRIMARY KEY AUTOINCREMENT,
			rule_id        INTEGER NOT NULL
							   REFERENCES learned_rules(id) ON DELETE CASCADE,
			run_id         INTEGER,
			rule_version   INTEGER,
			observation_id INTEGER,
			provider       TEXT,
			session_id     TEXT,
			cwd            TEXT,
			tool_name      TEXT,
			evidence_ts    TEXT,
			snippet        TEXT,
			kind           TEXT,
			ref_id         TEXT,
			created_at     TEXT NOT NULL DEFAULT (datetime('now'))
		);
		CREATE INDEX IF NOT EXISTS idx_rule_evidence_rule_version
			ON rule_evidence_citations(rule_id, rule_version);
		CREATE INDEX IF NOT EXISTS idx_rule_evidence_run
			ON rule_evidence_citations(run_id);
		CREATE INDEX IF NOT EXISTS idx_rule_evidence_observation
			ON rule_evidence_citations(observation_id);

		-- rule_tombstones: mig 25 (durable suppression, name-keyed).
		CREATE TABLE IF NOT EXISTS rule_tombstones (
			rule_name         TEXT PRIMARY KEY,
			rule_id           INTEGER,
			tombstoned_at     TEXT NOT NULL DEFAULT (datetime('now')),
			tombstoned_by     TEXT NOT NULL,
			reason            TEXT,
			last_content_hash TEXT,
			reactivated_at    TEXT,
			reactivated_by    TEXT
		);
		CREATE INDEX IF NOT EXISTS idx_rule_tombstones_reactivated
			ON rule_tombstones(reactivated_at);

		-- operator_feedback: mig 25 (primary outcome signal).
		CREATE TABLE IF NOT EXISTS operator_feedback (
			id                INTEGER PRIMARY KEY AUTOINCREMENT,
			rule_name         TEXT NOT NULL,
			actor             TEXT NOT NULL DEFAULT 'operator',
			feedback          TEXT NOT NULL,
			note              TEXT,
			rule_content_hash TEXT,
			created_at        TEXT NOT NULL DEFAULT (datetime('now')),
			updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
			UNIQUE(rule_name, actor)
		);

		-- evaluation_results: mig 25 (counterfactual verdicts).
		CREATE TABLE IF NOT EXISTS evaluation_results (
			id                 INTEGER PRIMARY KEY AUTOINCREMENT,
			rule_name          TEXT NOT NULL,
			learning_run_id    INTEGER REFERENCES learning_runs(id),
			replay_set_version TEXT,
			judge_model        TEXT,
			evaluated_at       TEXT NOT NULL DEFAULT (datetime('now')),
			verdict            TEXT,
			delta              REAL,
			regression         INTEGER NOT NULL DEFAULT 0,
			negative_transfer  INTEGER NOT NULL DEFAULT 0,
			judge_uncalibrated INTEGER NOT NULL DEFAULT 0,
			replay_set_stale   INTEGER NOT NULL DEFAULT 0,
			agreement_score    REAL,
			rationale          TEXT,
			per_case_json      TEXT
		);
		CREATE INDEX IF NOT EXISTS idx_evaluation_results_rule_evaluated
			ON evaluation_results(rule_name, evaluated_at DESC);

		-- reviewer_overrides: mig 25 (audited regression overrides).
		CREATE TABLE IF NOT EXISTS reviewer_overrides (
			id                 INTEGER PRIMARY KEY AUTOINCREMENT,
			rule_name          TEXT NOT NULL,
			replay_set_version TEXT,
			overridden_by      TEXT,
			reason             TEXT NOT NULL,
			overridden_at      TEXT NOT NULL DEFAULT (datetime('now'))
		);

		-- hook_invocations: mig 27 (hooks-breakdown tab source).
		CREATE TABLE IF NOT EXISTS hook_invocations (
			provider           TEXT NOT NULL,
			session_id         TEXT NOT NULL,
			agent_id           TEXT,
			is_sidechain       INTEGER NOT NULL DEFAULT 0,
			timestamp          TEXT NOT NULL,
			hook_event         TEXT NOT NULL,
			hook_matcher       TEXT,
			tool_name          TEXT,
			hook_identity      TEXT NOT NULL,
			script_command_raw TEXT,
			exit_code          INTEGER,
			duration_ms        INTEGER,
			cwd                TEXT,
			hostname           TEXT,
			message_id         TEXT
		);
		CREATE UNIQUE INDEX IF NOT EXISTS uidx_hook_invocations_identity
			ON hook_invocations(provider, session_id, COALESCE(agent_id, ''),
								timestamp, hook_identity);
		CREATE INDEX IF NOT EXISTS idx_hook_invocations_provider_ts
			ON hook_invocations(provider, timestamp);
		CREATE INDEX IF NOT EXISTS idx_hook_invocations_provider_session
			ON hook_invocations(provider, session_id);
		CREATE INDEX IF NOT EXISTS idx_hook_invocations_identity_ts
			ON hook_invocations(hook_identity, timestamp);
		CREATE INDEX IF NOT EXISTS idx_hook_invocations_identity_cwd
			ON hook_invocations(hook_identity, cwd);
	""")


def clear_tables(conn: sqlite3.Connection) -> None:
	tables = [
		"usage_snapshots", "usage_hourly", "token_snapshots", "token_hourly",
		"settings", "observations", "learning_runs", "learned_rules",
		"schema_version", "observation_summaries", "tool_actions",
		"memory_files", "optimization_runs", "optimization_suggestions",
		"git_snapshots", "response_times", "context_savings_events",
		"session_events", "skill_usages", "hook_invocations",
		"rule_versions", "rule_evidence_citations", "rule_tombstones",
		"operator_feedback", "evaluation_results", "reviewer_overrides",
	]
	for tbl in tables:
		conn.execute(f"DELETE FROM {tbl}")


# ── 1. usage_snapshots ────────────────────────────────────────────────────────

def populate_usage_snapshots(conn: sqlite3.Connection) -> None:
	# Live rate-limit bars come from get_latest_usage_buckets(), which keeps the
	# MAX(timestamp) row per (provider, bucket_key). We seed both providers so
	# the Claude AND Codex bars render, and we land a deterministic, non-trivial
	# utilization on the most-recent row of each bucket so no bar reads ~0%.
	#
	# Claude bucket_key values mirror Rust migration 14's CASE mapping; Codex
	# keys mirror fetcher.rs::parse_codex_rate_limits ("{scope}_{minutes}m").
	claude_buckets = [
		("five_hour", "5 hours"),
		("seven_day", "7 days"),
		("seven_day_sonnet", "Sonnet"),
		("seven_day_opus", "Opus"),
		("seven_day_cowork", "Code"),
		("seven_day_oauth_apps", "OAuth"),
	]
	codex_buckets = [
		("primary_300m", "5 hours"),
		("secondary_10080m", "7 days"),
	]
	# Final "current" utilization per bucket_key on the app 0..100 PERCENT scale
	# (utilization is rendered directly as "N%"; 0..1 fractions show as ~0%). Short windows run hot,
	# weekly windows higher, model/other buckets moderate — so the bars read as
	# an actively-used account rather than an idle one.
	current_util = {
		"five_hour": 42.0,
		"seven_day": 58.0,
		"seven_day_sonnet": 31.0,
		"seven_day_opus": 19.0,
		"seven_day_cowork": 27.0,
		"seven_day_oauth_apps": 14.0,
		"primary_300m": 48.0,
		"secondary_10080m": 63.0,
	}
	all_buckets = [("claude", k, l) for k, l in claude_buckets] + [
		("codex", k, l) for k, l in codex_buckets
	]

	rows = []
	start = NOW - timedelta(days=7)
	t = start
	while t < NOW:
		resets_at = (t + timedelta(hours=5)).isoformat()
		for provider, key, label in all_buckets:
			# Wander around each bucket's target so the history sparkline looks
			# organic but trends toward the current value.
			target = current_util[key]
			utilization = round(min(97.0, max(3.0, random.gauss(target, 12.0))), 2)
			rows.append((ts(t), provider, key, label, utilization, resets_at))
		t += timedelta(minutes=5)

	# Final snapshot at exactly NOW per bucket — this is the row the live bars
	# read. Pin it to the deterministic target so the demo is reproducible.
	# ts_tz (RFC3339 w/ offset) on the latest row so the Rust recent-snapshot
	# check (parse_from_rfc3339) parses it and serves bars from the DB (Path A).
	latest_ts = NOW + timedelta(minutes=30)
	resets_at_now = (latest_ts + timedelta(hours=5)).isoformat()
	for provider, key, label in all_buckets:
		rows.append((ts_tz(latest_ts), provider, key, label, current_util[key], resets_at_now))

	conn.executemany(
		"INSERT INTO usage_snapshots "
		"(timestamp, provider, bucket_key, bucket_label, utilization, resets_at) "
		"VALUES (?, ?, ?, ?, ?, ?)",
		rows,
	)
	print(f"  usage_snapshots: {len(rows)} rows (claude + codex buckets)")


# ── 2. usage_hourly ───────────────────────────────────────────────────────────

def populate_usage_hourly(conn: sqlite3.Connection) -> None:
	# Same (provider, bucket_key) shape as usage_snapshots so the Analytics
	# hourly rollups have data for both providers. UNIQUE is (hour, provider,
	# bucket_key) post-migration-14, so each tuple is distinct.
	buckets = [
		("claude", "five_hour", "5 hours"),
		("claude", "seven_day", "7 days"),
		("claude", "seven_day_sonnet", "Sonnet"),
		("claude", "seven_day_opus", "Opus"),
		("claude", "seven_day_cowork", "Code"),
		("claude", "seven_day_oauth_apps", "OAuth"),
		("codex", "primary_300m", "5 hours"),
		("codex", "secondary_10080m", "7 days"),
	]
	rows = []
	start = (NOW - timedelta(days=7)).replace(minute=0, second=0, microsecond=0)
	hour = start
	while hour <= NOW:
		hour_str = hour.strftime("%Y-%m-%dT%H:00:00+00:00")
		for provider, key, label in buckets:
			samples = [random.uniform(5.0, 95.0) for _ in range(12)]
			rows.append((
				hour_str, provider, key, label,
				round(sum(samples) / len(samples), 4),
				round(max(samples), 4),
				round(min(samples), 4),
				len(samples),
			))
		hour += timedelta(hours=1)

	conn.executemany(
		"INSERT OR IGNORE INTO usage_hourly "
		"(hour, provider, bucket_key, bucket_label, avg_utilization, max_utilization, min_utilization, sample_count) "
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
		rows,
	)
	print(f"  usage_hourly: {len(rows)} rows")


# ── 3. token_snapshots ────────────────────────────────────────────────────────

def populate_token_snapshots(conn: sqlite3.Connection) -> list[tuple[str, str]]:
	"""Return list of (session_id, hostname) for reuse.

	Drives the token CHARTS (30D/7D ranges) and the LIVE 6h summary. The Live
	summary (useLiveSummaryData.ts) and get_session_breakdown derive
	`last_active`/`project` from token_snapshots, then filter client-side to the
	rolling 6h window — so we seed BOTH a 30-day historical spread AND a
	dedicated recent cluster (0.5-5.5h old) across distinct projects.
	"""
	HISTORICAL_WINDOW_DAYS = 30

	sessions = []
	for _ in range(40):
		sessions.append((rand_session(), random.choice(HOSTNAMES), random.choice(PROJECTS)))

	rows = []
	start = NOW - timedelta(days=HISTORICAL_WINDOW_DAYS)
	span_hours = HISTORICAL_WINDOW_DAYS * 24
	for session_id, hostname, project in sessions:
		# Spread session starts across the full 30-day window so the 30D chart
		# is filled rather than clumped in the last week.
		t = start + timedelta(hours=random.randint(0, span_hours - 4))
		num_turns = random.randint(3, 30)
		for _ in range(num_turns):
			rows.append((
				session_id, hostname, ts(t),
				random.randint(500, 8000),
				random.randint(200, 3000),
				random.randint(0, 2000),
				random.randint(0, 5000),
				project,
			))
			t += timedelta(minutes=random.randint(1, 15))

	# RECENT cluster: 7 sessions inside the rolling 6h window, each on a
	# DISTINCT project, so the Live summary shows non-zero Sessions / Projects /
	# Tokens and the 1H/24H analytics ranges are populated. Projects cycle
	# through the fictional PROJECTS list (more sessions than projects is fine —
	# the live "Projects" count is over distinct cwd values that are recent).
	# These start 70-330 min ago (mostly in the 1h..6h band) so they do NOT
	# dominate the last-hour token total that the efficiency card divides by.
	recent_sessions = []
	for idx in range(7):
		session_id = rand_session()
		hostname = random.choice(HOSTNAMES)
		project = PROJECTS[idx % len(PROJECTS)]
		recent_sessions.append((session_id, hostname, project))
		start_minutes_ago = random.randint(70, 330)
		t = NOW - timedelta(minutes=start_minutes_ago)
		num_turns = random.randint(4, 12)
		for _ in range(num_turns):
			if t >= NOW - timedelta(minutes=62):
				break
			rows.append((
				session_id, hostname, ts(t),
				random.randint(800, 9000),
				random.randint(400, 3500),
				random.randint(0, 2500),
				random.randint(0, 6000),
				project,
			))
			t += timedelta(minutes=random.randint(1, 8))

	# LAST-HOUR micro-cluster: the EFFICIENCY card is tokens / lines-changed over
	# the default 1h range. With ~90 changed lines in the last hour (seeded in
	# populate_tool_actions), a ~18k-token budget here lands efficiency at
	# ~200 tokens/line. Two small sessions on distinct projects keep the live
	# Projects count and 1H token range populated without swamping the ratio.
	LAST_HOUR_TOKEN_BUDGET = 18_000
	emitted = 0
	for idx in range(2):
		session_id = rand_session()
		hostname = random.choice(HOSTNAMES)
		project = PROJECTS[idx % len(PROJECTS)]
		recent_sessions.append((session_id, hostname, project))
		t = NOW - timedelta(minutes=random.randint(45, 55))
		# ~5 modest turns per session; per-turn tokens sized to the budget.
		for _ in range(5):
			if t >= NOW:
				break
			inp = random.randint(700, 1300)
			out = random.randint(300, 700)
			cc = random.randint(0, 400)
			cr = random.randint(0, 600)
			emitted += inp + out + cc + cr
			rows.append((session_id, hostname, ts(t), inp, out, cc, cr, project))
			t += timedelta(minutes=random.randint(1, 4))
			if emitted >= LAST_HOUR_TOKEN_BUDGET:
				break
		if emitted >= LAST_HOUR_TOKEN_BUDGET:
			break

	conn.executemany(
		"INSERT INTO token_snapshots "
		"(session_id, hostname, timestamp, input_tokens, output_tokens, "
		" cache_creation_input_tokens, cache_read_input_tokens, cwd) "
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
		rows,
	)
	print(f"  token_snapshots: {len(rows)} rows ({len(sessions)} historical + {len(recent_sessions)} recent sessions)")
	return [(s, h) for s, h, _ in sessions] + [(s, h) for s, h, _ in recent_sessions]


# ── 4. token_hourly ───────────────────────────────────────────────────────────

def populate_token_hourly(conn: sqlite3.Connection) -> None:
	rows = []
	start = (NOW - timedelta(days=7)).replace(minute=0, second=0, microsecond=0)
	hour = start
	while hour <= NOW:
		hour_str = hour.strftime("%Y-%m-%dT%H:00:00+00:00")
		for hostname in HOSTNAMES:
			rows.append((
				hour_str, hostname,
				random.randint(5000, 80000),
				random.randint(2000, 30000),
				random.randint(0, 10000),
				random.randint(0, 20000),
				random.randint(1, 15),
			))
		hour += timedelta(hours=1)

	conn.executemany(
		"INSERT OR IGNORE INTO token_hourly "
		"(hour, hostname, total_input, total_output, total_cache_creation, total_cache_read, turn_count) "
		"VALUES (?, ?, ?, ?, ?, ?, ?)",
		rows,
	)
	print(f"  token_hourly: {len(rows)} rows")


# ── 5. settings ───────────────────────────────────────────────────────────────

def populate_settings(conn: sqlite3.Connection) -> None:
	# integration.providers.v1 is deserialized by manager.rs::load_saved_statuses
	# into Vec<ProviderStatus> (serde rename_all = "camelCase"). Seeding it makes
	# the demo self-rendering: claude + codex enabled so both providers' rate
	# bars, live summaries, and provider toggles populate without manual setup.
	# setupState uses the snake_case serde variant "installed".
	verified_at = ts_tz(NOW - timedelta(minutes=4))
	provider_statuses = [
		{
			"provider": "claude",
			"detectedCli": True,
			"detectedHome": True,
			"enabled": True,
			"setupState": "installed",
			"userHasMadeChoice": True,
			"lastError": None,
			"lastVerifiedAt": verified_at,
		},
		{
			"provider": "codex",
			"detectedCli": True,
			"detectedHome": True,
			"enabled": True,
			"setupState": "installed",
			"userHasMadeChoice": True,
			"lastError": None,
			"lastVerifiedAt": verified_at,
		},
	]

	settings = [
		("learning.enabled", "true"),
		("learning.trigger_mode", "periodic"),
		("learning.periodic_minutes", "180"),
		("learning.min_observations", "50"),
		("learning.min_confidence", "0.95"),
		("app.theme", "dark"),
		# Marketing screenshots are taken at a roomy window size.
		("app.window_width", "1280"),
		("app.window_height", "800"),
		# Enable brevity so the demo reflects the recommended profile.
		("feature.brevity.enabled", "true"),
		# Suppress the AppImage first-run desktop-integration prompt (modal dialog).
		("appimage.integration", "declined"),
		# Self-render claude + codex as installed/enabled.
		("integration.providers.v1", json.dumps(provider_statuses)),
	]
	conn.executemany(
		"INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)",
		settings,
	)
	print(f"  settings: {len(settings)} rows")


# ── 6. observations ───────────────────────────────────────────────────────────

def populate_observations(conn: sqlite3.Connection) -> None:
	rows = []
	start = NOW - timedelta(days=3)
	for i in range(500):
		t = start + timedelta(seconds=i * 60 + random.randint(0, 30))
		session_id = rand_session() if i % 25 == 0 else rows[-1][0] if rows else rand_session()
		tool = random.choice(TOOLS)
		project = random.choice(PROJECTS)
		tool_input = json.dumps({
			"file_path": f"{project}/src/main.py",
			"old_string": "# old code",
			"new_string": "# new code",
		}) if tool in ("Edit", "Write") else json.dumps({"command": "cargo build"})
		rows.append((
			session_id,
			ts(t),
			random.choice(HOOK_PHASES),
			tool,
			tool_input,
			project,
		))

	conn.executemany(
		"INSERT INTO observations "
		"(session_id, timestamp, hook_phase, tool_name, tool_input, cwd) "
		"VALUES (?, ?, ?, ?, ?, ?)",
		rows,
	)
	print(f"  observations: {len(rows)} rows")


# ── 7. learning_runs ──────────────────────────────────────────────────────────

def populate_learning_runs(conn: sqlite3.Connection) -> list[int]:
	run_data = [
		("periodic", 120, 3, 1, 4200, "completed", None,
		 '["Analyzing observations","Generating rules","Saving rules"]',
		 '[{"name":"observations","status":"completed"},{"name":"git","status":"completed"}]'),
		("periodic", 85, 2, 0, 3100, "completed", None,
		 '["Analyzing observations","Generating rules","Saving rules"]',
		 '[{"name":"observations","status":"completed"}]'),
		("on-demand", 210, 5, 2, 8900, "completed", None,
		 '["Analyzing observations","Generating rules","Saving rules","Validating"]',
		 '[{"name":"observations","status":"completed"},{"name":"git","status":"completed"},{"name":"memory","status":"completed"}]'),
		("periodic", 60, 1, 1, 2800, "completed", None,
		 '["Analyzing observations","Generating rules"]',
		 '[{"name":"observations","status":"completed"}]'),
		("on-demand", 0, 0, 0, 500, "failed", "Not enough observations (need 50, have 0)",
		 '["Analyzing observations"]',
		 '[{"name":"observations","status":"failed"}]'),
		("periodic", 150, 4, 3, 6500, "completed", None,
		 '["Analyzing observations","Generating rules","Saving rules","Validating"]',
		 '[{"name":"observations","status":"completed"},{"name":"git","status":"completed"}]'),
	]
	ids = []
	for i, (trigger, analyzed, created, updated, duration, status, error, logs, phases) in enumerate(run_data):
		created_at = ts(NOW - timedelta(hours=(len(run_data) - i) * 18))
		cursor = conn.execute(
			"INSERT INTO learning_runs "
			"(trigger_mode, observations_analyzed, rules_created, rules_updated, "
			" duration_ms, status, error, created_at, logs, phases) "
			"VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
			(trigger, analyzed, created, updated, duration, status, error, created_at, logs, phases),
		)
		ids.append(cursor.lastrowid)
	print(f"  learning_runs: {len(run_data)} rows")
	return ids


# ── 8. learned_rules ──────────────────────────────────────────────────────────

RULE_DEFS = [
	{
		"name": "prefer-immutable-updates",
		"domain": "coding-style",
		"confidence": 0.92,
		"observation_count": 87,
		"source": "observations",
		"state": "confirmed",
		"content": "# Prefer Immutable Updates\n\nAlways create new objects instead of mutating existing ones.\nUse spread operators, Object.assign(), or immutable helpers.\n",
	},
	{
		"name": "use-async-await",
		"domain": "coding-style",
		"confidence": 0.88,
		"observation_count": 64,
		"source": "observations",
		"state": "confirmed",
		"content": "# Use Async/Await\n\nPrefer async/await over .then() chains for Promise handling.\nThis improves readability and error handling.\n",
	},
	{
		"name": "small-focused-functions",
		"domain": "coding-style",
		"confidence": 0.79,
		"observation_count": 45,
		"source": "observations",
		"state": "emerging",
		"content": "# Small Focused Functions\n\nKeep functions under 50 lines. Extract helpers for complex logic.\nOne function, one responsibility.\n",
	},
	{
		"name": "validate-inputs-at-boundary",
		"domain": "security",
		"confidence": 0.95,
		"observation_count": 103,
		"source": "observations",
		"state": "confirmed",
		"content": "# Validate Inputs at Boundaries\n\nAlways validate user input and external data at system entry points.\nUse schema-based validation (e.g., Zod for TypeScript).\n",
	},
	{
		"name": "native-python-types",
		"domain": "coding-style",
		"confidence": 0.83,
		"observation_count": 58,
		"source": "git",
		"state": "emerging",
		"content": "# Native Python Types\n\nUse native Python 3.10+ type annotations.\nPrefer `list[str]` over `List[str]`, `str | None` over `Optional[str]`.\n",
	},
	{
		"name": "avoid-blanket-exceptions",
		"domain": "error-handling",
		"confidence": 0.91,
		"observation_count": 72,
		"source": "observations",
		"state": "confirmed",
		"content": "# Avoid Blanket Exception Catches\n\nNever catch bare `except Exception`. Always catch specific exceptions.\nLet unexpected errors bubble up for better debugging.\n",
	},
	{
		"name": "tabs-over-spaces",
		"domain": "formatting",
		"confidence": 0.97,
		"observation_count": 200,
		"source": "observations",
		"state": "confirmed",
		"content": "# Tabs Over Spaces\n\nUse tabs for indentation, not spaces.\nConfigure your editor and linter accordingly.\n",
	},
	{
		"name": "no-console-log-in-production",
		"domain": "coding-style",
		"confidence": 0.71,
		"observation_count": 38,
		"source": "observations",
		"state": "emerging",
		"is_anti_pattern": True,
		"content": "# No console.log in Production\n\nRemove console.log statements before committing.\nUse proper logging libraries with log levels instead.\n",
	},
]

LEARNED_DIR: Path = DEFAULT_RULES_DIR


def populate_learned_rules(conn: sqlite3.Connection) -> None:
	LEARNED_DIR.mkdir(parents=True, exist_ok=True)
	# A rule reads as "active" only when its .md sits in the provider-scope dir the
	# app scans in demo mode (resolve_rules_dir()/claude). Rules written to
	# LEARNED_DIR alone stay discovered candidates. Route confirmed rules into the
	# scope dir so the Learning view shows a populated ACTIVE RULES section rather
	# than an empty one.
	active_dir = LEARNED_DIR / "claude"
	active_dir.mkdir(parents=True, exist_ok=True)

	rows = []
	for i, rule in enumerate(RULE_DEFS):
		file_name = f"{rule['name']}.md"
		is_active = rule.get("state") == "confirmed"
		file_path = str((active_dir if is_active else LEARNED_DIR) / file_name)

		# Write the .md file
		with open(file_path, "w") as fh:
			fh.write(rule["content"])

		age_days = random.randint(1, 30)
		created_at = ts(NOW - timedelta(days=age_days))
		updated_at = ts(NOW - timedelta(days=random.randint(0, age_days)))
		last_evidence_at = updated_at

		confidence = rule["confidence"]
		alpha = confidence * 10
		beta = (1 - confidence) * 10

		rows.append((
			rule["name"],
			rule["domain"],
			confidence,
			rule["observation_count"],
			file_path,
			created_at,
			updated_at,
			rule["source"],
			alpha,
			beta,
			last_evidence_at,
			rule.get("state", "emerging"),
			None,  # project
			1 if rule.get("is_anti_pattern") else 0,
			None,  # confirmed_projects
		))

	conn.executemany(
		"INSERT OR IGNORE INTO learned_rules "
		"(name, domain, confidence, observation_count, file_path, "
		" created_at, updated_at, source, alpha, beta_param, last_evidence_at, "
		" state, project, is_anti_pattern, confirmed_projects) "
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		rows,
	)
	print(f"  learned_rules: {len(rows)} rows  ({len(rows)} .md files in {LEARNED_DIR})")


# ── 9. schema_version ─────────────────────────────────────────────────────────

def populate_schema_version(conn: sqlite3.Connection) -> None:
	# Record EVERY migration version up to the app's latest (27). The Rust
	# migration runner guards each block with `if current_version < N`, so
	# recording 1..27 makes the app run ZERO migrations against the seeded DB.
	# This is required because ensure_schema already builds every table in its
	# final post-migration shape — re-running ALTER ADD COLUMN migrations would
	# collide (e.g. "duplicate column name: cwd"). Bump this if storage.rs adds
	# a migration beyond 27 (search: INSERT INTO schema_version (version) VALUES).
	LATEST_SCHEMA_VERSION = 27
	for v in range(1, LATEST_SCHEMA_VERSION + 1):
		conn.execute("INSERT OR IGNORE INTO schema_version (version) VALUES (?)", (v,))
	print(f"  schema_version: versions 1-{LATEST_SCHEMA_VERSION}")


# ── 10. observation_summaries ─────────────────────────────────────────────────

def populate_observation_summaries(conn: sqlite3.Connection) -> None:
	periods = ["1h", "24h", "7d", "30d"]
	rows = []
	for period in periods:
		for project in PROJECTS + [None]:
			tool_counts = json.dumps({tool: random.randint(1, 50) for tool in TOOLS})
			total = random.randint(20, 300)
			rows.append((
				period,
				project,
				tool_counts,
				random.randint(0, 10),
				total,
			))

	conn.executemany(
		"INSERT OR IGNORE INTO observation_summaries "
		"(period, project, tool_counts, error_count, total_observations) "
		"VALUES (?, ?, ?, ?, ?)",
		rows,
	)
	print(f"  observation_summaries: {len(rows)} rows")


# ── 11. tool_actions ──────────────────────────────────────────────────────────

def populate_tool_actions(conn: sqlite3.Connection) -> None:
	"""Drive the code VELOCITY (lines/hr) and EFFICIENCY (tokens/line) cards.

	Both metrics (useCodeInsights.ts) read `total_changed` from
	get_code_stats_history(), which parses category='code_change' rows via
	storage.rs::parse_code_change: Edit counts new_string/old_string LINES;
	Write counts content LINES. The default NOW tab runs the 1h range, so
	velocity = (lines changed in the last hour). We therefore (a) spread ~80%
	Edit/Write actions with realistic MULTI-LINE snippets across the last 30
	days to fill the 24h/7d/30d ranges and the history chart, and (b) land a
	dedicated recent cluster summing to ~75-100 changed lines inside the last
	hour so the headline velocity reads ~75-100 lines/hr and efficiency lands
	~150-250 tokens/line against the recent token cluster.
	"""
	tool_category_map = {
		"Read": "file_read", "Glob": "search", "Grep": "search",
		"Bash": "command", "WebSearch": "web", "WebFetch": "web",
		"Task": "command", "TodoWrite": "command",
	}
	non_code_tools = ["Read", "Bash", "Grep", "Glob", "WebSearch", "WebFetch", "Task", "TodoWrite"]

	sessions = [rand_session() for _ in range(8)]

	# Varied fictional multi-line code bodies (8-30 lines each) used as Write
	# content and as Edit new_string. Counting lines on these is what feeds the
	# velocity/efficiency math. All identifiers are invented.
	code_blocks = [
		(
			"pub fn reconcile_buckets(snapshots: &[Snapshot]) -> Vec<Bucket> {\n"
			"    let mut by_key: HashMap<&str, Bucket> = HashMap::new();\n"
			"    for snap in snapshots {\n"
			"        let entry = by_key.entry(snap.key.as_str()).or_default();\n"
			"        if snap.timestamp > entry.latest {\n"
			"            entry.latest = snap.timestamp;\n"
			"            entry.utilization = snap.utilization;\n"
			"        }\n"
			"    }\n"
			"    let mut out: Vec<Bucket> = by_key.into_values().collect();\n"
			"    out.sort_by(|a, b| a.key.cmp(&b.key));\n"
			"    out\n"
			"}"
		),
		(
			"async function refreshLiveSummary(range) {\n"
			"  const cutoff = Date.now() - RANGE_HOURS[range] * HOUR_MS;\n"
			"  const sessions = await invoke('get_session_breakdown', { range });\n"
			"  const active = sessions.filter((s) => toMs(s.last_active) >= cutoff);\n"
			"  const projects = new Set(active.map((s) => s.project).filter(Boolean));\n"
			"  return {\n"
			"    sessionCount: active.length,\n"
			"    projectCount: projects.size,\n"
			"    tokens: active.reduce((sum, s) => sum + s.total_tokens, 0),\n"
			"  };\n"
			"}"
		),
		(
			"def merge_retention(preserved, retrieved):\n"
			"    pool = {}\n"
			"    for source in preserved:\n"
			"        pool.setdefault(source.ref, {'preserved': True, 'retrieved': False})\n"
			"    for source in retrieved:\n"
			"        slot = pool.setdefault(source.ref, {'preserved': False, 'retrieved': False})\n"
			"        slot['retrieved'] = True\n"
			"    reused = sum(1 for v in pool.values() if v['preserved'] and v['retrieved'])\n"
			"    total = sum(1 for v in pool.values() if v['preserved'])\n"
			"    ratio = reused / total if total else 0.0\n"
			"    return reused, total, ratio"
		),
		(
			"impl TurnWalker {\n"
			"    fn flush(&mut self, end_ms: f64) {\n"
			"        let dur = (end_ms - self.turn_start_ms).max(0.0);\n"
			"        if dur > 0.0 {\n"
			"            self.total += dur;\n"
			"            self.count += 1;\n"
			"            let bucket = ((self.turn_start_ms - self.from_ms) / self.bucket_ms) as usize;\n"
			"            self.buckets[bucket.min(6)] += dur;\n"
			"        }\n"
			"    }\n"
			"}"
		),
		(
			"export function buildSparkline(points, buckets) {\n"
			"  const span = points.length ? points[points.length - 1].t - points[0].t : 0;\n"
			"  if (span <= 0) return new Array(buckets).fill(0);\n"
			"  const width = span / buckets;\n"
			"  const out = new Array(buckets).fill(0);\n"
			"  for (const p of points) {\n"
			"    const idx = Math.min(buckets - 1, Math.floor((p.t - points[0].t) / width));\n"
			"    out[idx] += p.value;\n"
			"  }\n"
			"  return out;\n"
			"}"
		),
		(
			"def parse_codex_buckets(rate_limits):\n"
			"    buckets = []\n"
			"    for scope in ('primary', 'secondary'):\n"
			"        entry = rate_limits.get(scope)\n"
			"        if not entry:\n"
			"            continue\n"
			"        minutes = entry.get('window_minutes', 300 if scope == 'primary' else 10080)\n"
			"        buckets.append({\n"
			"            'key': f'{scope}_{minutes}m',\n"
			"            'label': window_label(minutes),\n"
			"            'utilization': entry.get('used_percent', 0.0) / 100.0,\n"
			"        })\n"
			"    return buckets"
		),
		(
			"fn classify_gap(prev_kind: Option<&str>, kind: &str, gap: f64) -> bool {\n"
			"    let tool_loop = matches!(prev_kind, Some(\"asst_tool_use\")) && kind == \"user_tool_result\";\n"
			"    if tool_loop {\n"
			"        gap <= TOOL_WAIT_MAX_SECS\n"
			"    } else {\n"
			"        gap <= IDLE_THRESHOLD_SECS\n"
			"    }\n"
			"}"
		),
		(
			"const useDebouncedValue = (value, delayMs) => {\n"
			"  const [debounced, setDebounced] = useState(value);\n"
			"  useEffect(() => {\n"
			"    const handle = setTimeout(() => setDebounced(value), delayMs);\n"
			"    return () => clearTimeout(handle);\n"
			"  }, [value, delayMs]);\n"
			"  return debounced;\n"
			"};"
		),
	]

	# Smaller before/after pairs for Edit churn (old_string -> new_string).
	edit_pairs = [
		(
			"let timeout = Duration::from_secs(5);\nclient.set_timeout(timeout);",
			"let timeout = Duration::from_secs(15);\nlet retry = Duration::from_millis(250);\nclient.set_timeout(timeout);\nclient.set_retry(retry);",
		),
		(
			"return rows.filter(r => r.active);",
			"return rows\n  .filter((r) => r.active && !r.archived)\n  .map((r) => normalizeRow(r));",
		),
		(
			"if err != nil {\n    return err\n}",
			"if err != nil {\n    log.Printf(\"reconcile failed: %v\", err)\n    return fmt.Errorf(\"reconcile: %w\", err)\n}",
		),
		(
			"export const LIMIT = 50;",
			"export const LIMIT = 200;\nexport const PAGE_SIZE = 25;\nexport const MAX_RETRIES = 3;",
		),
	]

	exts = ["rs", "ts", "tsx", "py", "go"]

	def code_change_row(t: datetime, prefer_write: bool | None = None) -> tuple:
		session_id = random.choice(sessions)
		project = random.choice(PROJECTS)
		message_id = rand_session()
		ext = random.choice(exts)
		file_path = f"{project}/src/module_{random.randint(1, 18)}.{ext}"
		is_write = random.random() < 0.45 if prefer_write is None else prefer_write
		if is_write:
			tool_name = "Write"
			content = random.choice(code_blocks)
			full_input = json.dumps({"file_path": file_path, "content": content})
		else:
			tool_name = "Edit"
			old_s, new_s = random.choice(edit_pairs)
			full_input = json.dumps({"file_path": file_path, "old_string": old_s, "new_string": new_s})
		full_output = json.dumps({"result": "ok"})
		summary = f"{tool_name} on {os.path.basename(file_path)}"
		return (
			message_id, session_id, tool_name, "code_change",
			file_path, summary, full_input, full_output, ts_tz(t),
		)

	def lines_changed(row: tuple) -> int:
		"""Mirror parse_code_change line-counting to budget the recent window."""
		payload = json.loads(row[6])
		if "content" in payload:
			return payload["content"].count("\n") + 1
		added = payload["new_string"].count("\n") + 1
		removed = payload["old_string"].count("\n") + 1
		return added + removed

	rows = []
	# ~300 total actions, ~80% Edit/Write code_change spread across 30 days.
	TOTAL = 300
	code_change_target = int(TOTAL * 0.80)
	for _ in range(code_change_target):
		t = NOW - timedelta(minutes=random.randint(90, 30 * 24 * 60))
		rows.append(code_change_row(t))

	# Remaining ~20% are non-code tools, also spread across 30 days.
	for _ in range(TOTAL - code_change_target):
		t = NOW - timedelta(minutes=random.randint(90, 30 * 24 * 60))
		session_id = random.choice(sessions)
		project = random.choice(PROJECTS)
		tool_name = random.choice(non_code_tools)
		file_path = f"{project}/src/module_{random.randint(1, 18)}.py"
		category = tool_category_map.get(tool_name, "command")
		full_input = json.dumps({"file_path": file_path, "command": "build"})
		full_output = json.dumps({"result": "ok", "lines_changed": random.randint(1, 50)})
		summary = f"{tool_name} on {os.path.basename(file_path)}"
		rows.append((
			rand_session(), session_id, tool_name, category,
			file_path, summary, full_input, full_output, ts_tz(t),
		))

	# RECENT 1h cluster: accumulate code_change rows until ~75-100 changed lines
	# land inside the last hour, so the default 1h velocity card reads in range.
	recent_lines = 0
	recent_count = 0
	while recent_lines < 88:
		minutes_ago = random.randint(2, 58)
		t = NOW - timedelta(minutes=minutes_ago)
		# Bias slightly toward Edit so churn stays granular and realistic.
		row = code_change_row(t, prefer_write=random.random() < 0.35)
		rows.append(row)
		recent_lines += lines_changed(row)
		recent_count += 1

	conn.executemany(
		"INSERT INTO tool_actions "
		"(message_id, session_id, tool_name, category, file_path, "
		" summary, full_input, full_output, timestamp) "
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
		rows,
	)
	print(
		f"  tool_actions: {len(rows)} rows "
		f"(~{code_change_target} code_change + {recent_count} recent, ~{recent_lines} lines in last 1h)"
	)


# ── 12. memory_files ──────────────────────────────────────────────────────────

def populate_memory_files(conn: sqlite3.Connection) -> None:
	memory_file_names = [
		"CLAUDE.md", "memory/MEMORY.md", ".claude/commands/deploy.md",
		".claude/commands/test.md", "docs/architecture.md",
	]
	rows = []
	for project in PROJECTS:
		for fname in memory_file_names:
			file_path = f"{project}/{fname}"
			content = f"# {fname}\n\nProject memory for {project}."
			content_hash = hashlib.sha256(content.encode()).hexdigest()
			last_scanned = ts(NOW - timedelta(hours=random.randint(0, 48)))
			rows.append((project, file_path, content_hash, last_scanned))

	conn.executemany(
		"INSERT OR IGNORE INTO memory_files "
		"(project_path, file_path, content_hash, last_scanned_at) "
		"VALUES (?, ?, ?, ?)",
		rows,
	)
	print(f"  memory_files: {len(rows)} rows")


# ── 13 + 14. optimization_runs + optimization_suggestions ────────────────────

def populate_optimization(conn: sqlite3.Connection) -> None:
	action_types = ["update_memory", "merge_memory", "create_memory", "delete_memory"]
	statuses = ["completed", "completed", "completed", "failed"]
	suggestion_statuses = ["pending", "applied", "dismissed", "pending"]

	run_rows = []
	for project in PROJECTS:
		for i in range(2):
			started = NOW - timedelta(hours=random.randint(1, 120))
			completed = started + timedelta(seconds=random.randint(5, 60))
			context_sources = json.dumps({
				"session_history": random.randint(5, 30),
				"git_analysis": random.randint(1, 10),
			})
			run_rows.append((
				project,
				random.choice(["manual", "periodic", "post-session"]),
				random.randint(3, 12),
				random.randint(0, 5),
				context_sources,
				random.choice(statuses),
				None,
				ts(started),
				ts(completed),
			))

	run_ids = []
	for row in run_rows:
		cursor = conn.execute(
			"INSERT INTO optimization_runs "
			"(project_path, trigger, memories_scanned, suggestions_created, "
			" context_sources, status, error, started_at, completed_at) "
			"VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
			row,
		)
		run_ids.append((cursor.lastrowid, row[0]))

	print(f"  optimization_runs: {len(run_rows)} rows")

	sug_rows = []
	for run_id, project in run_ids:
		for _ in range(random.randint(0, 3)):
			fname = random.choice(["CLAUDE.md", "memory/MEMORY.md", "docs/notes.md"])
			target = f"{project}/{fname}"
			original = "# Old content\n\nSome notes here."
			proposed = "# Updated content\n\nImproved notes here."
			diff = "@@ -1,2 +1,2 @@\n-# Old content\n+# Updated content"
			created_at = ts(NOW - timedelta(hours=random.randint(0, 48)))
			sug_rows.append((
				run_id, project,
				random.choice(action_types),
				target,
				"Consolidate duplicate memory entries for clarity.",
				proposed,
				random.choice(suggestion_statuses),
				created_at,
				original,
				diff,
				json.dumps({"original_path": target, "original_content": original}),
				rand_hex(8),
			))

	conn.executemany(
		"INSERT INTO optimization_suggestions "
		"(run_id, project_path, action_type, target_file, reasoning, proposed_content, "
		" status, created_at, original_content, diff_summary, backup_data, group_id) "
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		sug_rows,
	)
	print(f"  optimization_suggestions: {len(sug_rows)} rows")


# ── 15. git_snapshots ─────────────────────────────────────────────────────────

def populate_git_snapshots(conn: sqlite3.Connection) -> None:
	rows = []
	for project in PROJECTS:
		commits = []
		for i in range(random.randint(10, 30)):
			t = NOW - timedelta(days=random.randint(0, 90))
			commits.append({
				"hash": rand_hex(40),
				"message": random.choice([
					"feat: add new feature",
					"fix: resolve bug in parser",
					"refactor: extract utility functions",
					"docs: update readme",
					"chore: bump dependencies",
					"perf: optimize database queries",
				]),
				"author": random.choice(["Alex Smith", "Jordan Lee", "Casey Brown"]),
				"timestamp": ts(t),
			})
		raw_data = json.dumps({"commits": commits, "branches": ["main", "develop"]})
		rows.append((
			project,
			commits[0]["hash"],
			len(commits),
			raw_data,
			ts(NOW - timedelta(hours=random.randint(0, 24))),
		))

	conn.executemany(
		"INSERT OR IGNORE INTO git_snapshots "
		"(project, commit_hash, commit_count, raw_data, created_at) "
		"VALUES (?, ?, ?, ?, ?)",
		rows,
	)
	print(f"  git_snapshots: {len(rows)} rows")


# ── 16. response_times ────────────────────────────────────────────────────────

def populate_response_times(conn: sqlite3.Connection) -> None:
	sessions = [rand_session() for _ in range(8)]
	rows = []
	seen: set[tuple[str, str]] = set()

	for session_id in sessions:
		start = NOW - timedelta(hours=random.randint(1, 48))
		num_turns = random.randint(5, 25)
		t = start
		for _ in range(num_turns):
			# response_times.timestamp is read by parse_ts_diff (chrono::parse_from_rfc3339);
			# must be tz-aware or LLM RUNTIME aggregations silently return 0.
			ts_val = ts_tz(t)
			key = (session_id, ts_val)
			if key not in seen:
				seen.add(key)
				rows.append((
					session_id,
					ts_val,
					round(random.uniform(0.5, 45.0), 2),
					round(random.uniform(10.0, 600.0), 2),
				))
			t += timedelta(seconds=random.randint(30, 900))

	conn.executemany(
		"INSERT OR IGNORE INTO response_times "
		"(session_id, timestamp, response_secs, idle_secs) "
		"VALUES (?, ?, ?, ?)",
		rows,
	)
	print(f"  response_times: {len(rows)} rows")


# ── 17. context_savings_events ────────────────────────────────────────────────

def populate_context_savings_events(conn: sqlite3.Connection) -> None:
	"""Populate the four context-savings categories so the Context tab renders.

	Categories (closed taxonomy):
	  - preservation: large content kept out of the LLM transcript via MCP store
	  - retrieval: LLM pulled preserved content back via quill_get_context_source
	  - routing: router/capture guidance text injected into the transcript
	  - telemetry: hook observations recording session activity (no transcript cost)

	PRESERVED ("X% reused · A/B sources") comes from CONTEXT_SAVINGS_RETENTION_SQL,
	which links preservation and retrieval rows by a SHARED `source_ref` (the
	display `source` string is NOT the key). The denominator B = distinct
	source_refs that were PRESERVED in-window; the numerator A = source_refs that
	were BOTH preserved AND retrieved in-window. The old seed gave preservation a
	unique per-row source_ref and retrieval `None`, so nothing ever linked -> 0%.

	Fix: a fixed pool of ~25 source IDs. Preservation events draw from the WHOLE
	pool; retrieval events draw only from a ~40% subset, so retention lands
	~40% reused with ~25 preserved sources. The Context/Now tabs default to the
	1h range, so the SQL window is the last hour -> we concentrate full pool
	coverage inside the last hour and add a 7-day spread for the wider ranges.
	ROUTING COST sums routing `input_bytes`, so routing rows carry 200-2000 bytes.
	"""
	# Stable pool of (display label, source_ref). Fictional, varied.
	source_labels = [
		"docs/quill-internal-runbook.md",
		"https://docs.example.com/api-reference",
		"npm run build (output)",
		"rg 'thread-safety' src/",
		"tests/integration/load_test.log",
		"SELECT * FROM users WHERE last_seen > ...",
		"https://research.example.com/throughput-benchmarks",
		"cargo bench (output)",
		"docs/architecture-decisions.md",
		"k6 run scripts/loadtest.js (output)",
		"docs/migration-playbook.md",
		"https://docs.example.com/webhooks",
		"pnpm test --filter analytics (output)",
		"rg 'TODO' src-tauri/",
		"tests/e2e/checkout.spec.ts",
		"SELECT count(*) FROM sessions GROUP BY day",
		"https://blog.example.com/rust-async-pitfalls",
		"cargo clippy --all-targets (output)",
		"docs/security-threat-model.md",
		"terraform plan (output)",
		"https://docs.example.com/rate-limits",
		"git log --stat v1.4.0..HEAD",
		"tests/fixtures/large_payload.json",
		"docs/onboarding-checklist.md",
		"kubectl describe pod api-gateway (output)",
	]
	SOURCE_POOL_SIZE = len(source_labels)  # 25
	source_refs = [f"src-{i:03d}-{rand_hex(6)}" for i in range(SOURCE_POOL_SIZE)]
	pool = list(zip(source_labels, source_refs))

	# ~40% of the pool is "reusable": retrieval events only ever cite these, so
	# the retention ratio settles near 40% (10 of 25). Index 0..9.
	REUSABLE = 10
	reusable_pool = pool[:REUSABLE]

	rows: list[tuple] = []

	def make_row(category: str, when: datetime, source_pair: tuple[str, str] | None) -> None:
		if category == "preservation":
			event_type = random.choice(["mcp.index", "mcp.fetch", "mcp.execute"])
			decision = "indexed"
			indexed_b = random.randint(8_000, 350_000)
			returned_b = 0
			input_b = 0
			tok_indexed = indexed_b // 4
			tok_returned = 0
			tok_saved = tok_indexed
			tok_preserved = tok_indexed
		elif category == "retrieval":
			event_type = random.choice(["context.get_source", "compaction.snapshot.read"])
			decision = "returned"
			indexed_b = 0
			returned_b = random.randint(2_000, 60_000)
			input_b = 0
			tok_indexed = 0
			tok_returned = returned_b // 4
			tok_saved = tok_returned
			tok_preserved = 0
		elif category == "routing":
			event_type = "router.guidance"
			decision = "injected"
			indexed_b = 0
			# ROUTING COST headline = SUM(COALESCE(tokens_returned_est,
			# (returned_bytes+3)/4)) over routing rows (CATEGORY_TOTALS_SQL). It
			# does NOT read input_bytes, so the injected-guidance size lives in
			# returned_bytes/tokens_returned_est to make the card show tokens.
			returned_b = random.randint(2_000, 6_000)
			# input_bytes still drives the per-event "Input N bytes" subtitle and
			# the recent-events trailing metric in ContextSavingsTab.
			input_b = random.randint(200, 2_000)
			tok_indexed = 0
			tok_returned = returned_b // 4
			tok_saved = 0
			tok_preserved = 0
		else:  # telemetry
			event_type = random.choice(["capture.event", "capture.snapshot"])
			decision = "observed"
			indexed_b = 0
			returned_b = 0
			input_b = 0
			tok_indexed = 0
			tok_returned = 0
			tok_saved = 0
			tok_preserved = 0

		source_label = source_pair[0] if source_pair else random.choice(source_labels)
		source_ref = source_pair[1] if source_pair else None
		snapshot_ref = (
			f"snapshot:{rand_hex(8)}" if event_type == "compaction.snapshot.read" else None
		)
		rows.append((
			str(uuid.UUID(int=random.getrandbits(128))),
			1,
			random.choice(["claude", "codex"]),
			rand_session(),
			random.choice(HOSTNAMES),
			random.choice(PROJECTS),
			ts(when),
			event_type,
			source_label,
			decision,
			category,
			f"auto-{category}",
			1,
			indexed_b,
			returned_b,
			input_b,
			tok_indexed,
			tok_returned,
			tok_saved,
			tok_preserved,
			"byte_div_4",
			0.92,
			source_ref,
			snapshot_ref,
			None,
		))

	def recent_when() -> datetime:
		# Inside the last hour (the default analytics window).
		return NOW - timedelta(minutes=random.uniform(1, 58))

	def spread_when() -> datetime:
		# 1h..7d ago, to fill the 24h/7d ranges.
		return NOW - timedelta(hours=random.uniform(1, 7 * 24))

	# --- Recent (last-hour) cluster: this is what the default 1h cards read. ---
	# Every pool source gets at least one preservation event in-window so
	# B (sources_preserved) ~= 25.
	for source_pair in pool:
		make_row("preservation", recent_when(), source_pair)
	# Each reusable source also gets a retrieval event in-window so
	# A (reused) ~= 10 -> ~40% reused.
	for source_pair in reusable_pool:
		make_row("retrieval", recent_when(), source_pair)
	# Recent routing + telemetry so ROUTING COST and telemetry counts are live.
	for _ in range(12):
		make_row("routing", recent_when(), None)
	for _ in range(4):
		make_row("telemetry", recent_when(), None)

	# --- Historical spread to ~300 total, distribution ~60/20/15/5. ---
	TOTAL = 300
	remaining = TOTAL - len(rows)
	weighted = (
		["preservation"] * 60 + ["retrieval"] * 20 + ["routing"] * 15 + ["telemetry"] * 5
	)
	for _ in range(max(0, remaining)):
		category = random.choice(weighted)
		if category == "preservation":
			pair = random.choice(pool)
		elif category == "retrieval":
			# Keep retrieval confined to the reusable subset so the ratio holds.
			pair = random.choice(reusable_pool)
		else:
			pair = None
		make_row(category, spread_when(), pair)

	conn.executemany(
		"INSERT OR IGNORE INTO context_savings_events ("
		"event_id, schema_version, provider, session_id, hostname, cwd, timestamp, "
		"event_type, source, decision, category, reason, delivered, "
		"indexed_bytes, returned_bytes, input_bytes, "
		"tokens_indexed_est, tokens_returned_est, tokens_saved_est, tokens_preserved_est, "
		"estimate_method, estimate_confidence, source_ref, snapshot_ref, metadata_json"
		") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		rows,
	)
	print(
		f"  context_savings_events: {len(rows)} rows "
		f"(~{SOURCE_POOL_SIZE} sources, {REUSABLE} reusable -> ~{round(REUSABLE / SOURCE_POOL_SIZE * 100)}% reused)"
	)


# ── 17b. session_events ───────────────────────────────────────────────────────

def populate_session_events(conn: sqlite3.Connection) -> None:
	"""Seed the timeline that drives the LLM RUNTIME card.

	get_llm_runtime_stats reads `session_events` EXCLUSIVELY (no other table),
	so without rows the card shows "no data". A "logical turn" is a contiguous
	run of events on a chain (provider, session_id, agent_id) where every gap is
	<= 300s (IDLE_THRESHOLD), except an `asst_tool_use` -> `user_tool_result`
	gap which may stretch up to 6h (tool-loop). A gap over threshold ends the
	turn and starts a new one. `session_count` is distinct (provider,
	session_id); turn duration = last_event - first_event of the turn.

	The card defaults to the 1h range, so headline Sessions/Turns come from the
	last-hour cluster (we make ~9 sessions active with several short turns each,
	avg ~2-4 min). A 30-day spread (~40 sessions, ~5 turns each) makes the
	7d/30d ranges climb into the ~150-250 turn band. Kinds use the real
	SessionEventKind strings (sessions.rs): user_text, asst_text, asst_thinking,
	asst_tool_use, user_tool_result. No asst_start/asst_end kind exists.
	"""
	IDLE_THRESHOLD = 300  # seconds; matches storage.rs

	def emit_turn(rows: list, provider: str, session_id: str, start: datetime) -> datetime:
		"""Append one turn's events (2-5 tool-loop steps) and return its end time.

		Internal gaps stay <= IDLE_THRESHOLD so the whole run is one turn. The
		turn opens with assistant text/thinking, then alternates tool_use ->
		tool_result, and closes with assistant text.
		"""
		t = start
		# Opening assistant activity.
		parent = None
		first_uuid = "ev_" + rand_hex(16)
		rows.append((provider, session_id, None, 0, ts_tz(t), "asst_text", first_uuid, parent))
		parent = first_uuid
		if random.random() < 0.4:
			t += timedelta(seconds=random.randint(6, 25))
			u = "ev_" + rand_hex(16)
			rows.append((provider, session_id, None, 0, ts_tz(t), "asst_thinking", u, parent))
			parent = u

		# 1-3 tool steps with short gaps keep each turn ~2-5 min (gaps stay well
		# under the 300s idle threshold so the run is a single logical turn).
		steps = random.randint(1, 3)
		for _ in range(steps):
			# asst_tool_use
			t += timedelta(seconds=random.randint(8, 35))
			u_use = "ev_" + rand_hex(16)
			rows.append((provider, session_id, None, 0, ts_tz(t), "asst_tool_use", u_use, parent))
			parent = u_use
			# user_tool_result — tool-loop gap, kept modest for realistic timing.
			t += timedelta(seconds=random.randint(12, 70))
			u_res = "ev_" + rand_hex(16)
			rows.append((provider, session_id, None, 0, ts_tz(t), "user_tool_result", u_res, parent))
			parent = u_res

		# Closing assistant text.
		t += timedelta(seconds=random.randint(6, 30))
		u_end = "ev_" + rand_hex(16)
		rows.append((provider, session_id, None, 0, ts_tz(t), "asst_text", u_end, parent))
		return t

	def emit_session(rows: list, provider: str, session_id: str, start: datetime,
	                 num_turns: int, max_end: datetime | None) -> None:
		t = start
		for _ in range(num_turns):
			if max_end is not None and t >= max_end:
				break
			end = emit_turn(rows, provider, session_id, t)
			# Idle gap over the threshold => the next run is a NEW turn.
			t = end + timedelta(seconds=random.randint(IDLE_THRESHOLD + 20, IDLE_THRESHOLD + 600))

	rows: list[tuple] = []

	# --- Recent cluster: ~9 sessions active inside the last hour. Several short
	# turns each so the default 1h card shows a healthy session + turn count
	# (~8-10 sessions). Starts are staggered so they all overlap the 1h window.
	for _ in range(9):
		provider = "claude" if random.random() < 0.7 else "codex"
		session_id = rand_session()
		start = NOW - timedelta(minutes=random.randint(40, 56))
		# 3-5 turns, but emit_session stops once it would cross NOW.
		emit_session(rows, provider, session_id, start, random.randint(3, 5), NOW)

	# --- A few sessions in the last ~6 hours (for the 1h..24h ranges and the
	# "several sessions in the last 6 hours" requirement). Start >= ~66 min ago
	# and cap turns so they do NOT bleed into the 1h window and inflate its
	# session count beyond the intended ~8-10.
	for _ in range(8):
		provider = "claude" if random.random() < 0.7 else "codex"
		session_id = rand_session()
		start = NOW - timedelta(hours=random.uniform(1.2, 6.0))
		end_ceiling = NOW - timedelta(minutes=64)
		emit_session(rows, provider, session_id, start, random.randint(3, 6), end_ceiling)

	# --- 30-day historical spread so 7d/30d show ~150-250 turns total.
	for _ in range(40):
		provider = "claude" if random.random() < 0.65 else "codex"
		session_id = rand_session()
		start = NOW - timedelta(hours=random.uniform(6.0, 30 * 24))
		emit_session(rows, provider, session_id, start, random.randint(3, 8), None)

	conn.executemany(
		"INSERT OR IGNORE INTO session_events "
		"(provider, session_id, agent_id, is_sidechain, timestamp, kind, uuid, parent_uuid) "
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
		rows,
	)
	# Distinct sessions for the operator's sanity check.
	distinct_sessions = len({(r[0], r[1]) for r in rows})
	print(f"  session_events: {len(rows)} rows across {distinct_sessions} sessions")


# ── 18. claude session JSONL files ────────────────────────────────────────────

def populate_session_jsonls() -> None:
	"""Write fictional Claude-Code session JSONL files into PROJECTS_DIR.

	Tantivy's session indexer scans `<HOME>/.claude/projects/<project_dir>/*.jsonl`
	and indexes each message. Writing realistic-looking JSONL here lets the
	Session Search window in the demo Quill instance show populated results
	instead of an empty 'Refreshing session index...' placeholder.

	Each project directory name is the Claude-Code-style slug of the cwd
	(slashes converted to dashes). Each JSONL file is one session. Each line
	is a `{type:"user"|"assistant", message: {...}}` record per the format
	parsed in src-tauri/src/sessions.rs::extract_claude_messages_from_jsonl.
	"""
	if SKIP_PROJECTS:
		log("  session_jsonls: skipped (--no-projects)")
		return

	prompts = [
		"Why is the request handler timing out under load?",
		"Refactor the auth middleware to use the new token format.",
		"Add a flag to skip the cache when running benchmarks.",
		"Fix the off-by-one in the rate limit reset countdown.",
		"Explain why the migration is failing on the staging cluster.",
		"Rewrite this loop to be allocation-free.",
		"Add property tests for the date parser.",
		"Why does this query plan a sequential scan instead of using the index?",
	]
	assistant_replies = [
		"The handler is awaiting a single shared lock during peak traffic. Splitting it into a read-mostly RwLock should cut p99 substantially.",
		"I'll switch the verifier to the new ed25519 path and update the test fixtures. Three files affected.",
		"Adding a `--no-cache` flag and threading it through the bench harness now.",
		"The countdown computes `reset_at - now` but reset_at is end-exclusive in this provider's API; off-by-one fixed.",
		"Migration 0042 expects the old enum variant to exist. Staging was upgraded before the prep migration ran. Order fixed.",
		"Replacing the per-iteration String allocation with a SmallVec<u8; 64> buffer keeps the hot path entirely on the stack.",
		"Added 12 property tests covering leap years, DST boundaries, and timezone offsets at minute granularity.",
		"The composite index doesn't include the predicate column. Adding (project, created_at) drops the cost from 12k to 80.",
	]
	tools_used_pool = ["Edit", "Write", "Read", "Bash", "Grep", "Glob", "Task"]
	branches = ["main", "fix/auth-token-format", "perf/cache-skip-flag", "fix/rate-limit-off-by-one"]

	PROJECTS_DIR.mkdir(parents=True, exist_ok=True)

	total_messages = 0
	total_files = 0
	for project_path in PROJECTS_DIR.iterdir() if False else PROJECTS:
		# project_path is a string from the PROJECTS list (e.g. "/home/alex/projects/quill")
		# Claude-Code dir-naming convention replaces slashes with dashes.
		project_slug = project_path.replace("/", "-").lstrip("-")
		project_dir = PROJECTS_DIR / project_slug
		project_dir.mkdir(parents=True, exist_ok=True)

		# Two sessions per project, with a few exchanges each.
		for session_idx in range(2):
			session_id = rand_session()
			session_file = project_dir / f"{session_id}.jsonl"
			lines = []
			session_start = NOW - timedelta(hours=random.randint(1, 96))
			branch = random.choice(branches)
			turn_count = random.randint(2, 5)
			for turn in range(turn_count):
				turn_time = session_start + timedelta(minutes=turn * random.randint(2, 15))
				prompt = random.choice(prompts)
				reply = random.choice(assistant_replies)
				tools_picked = random.sample(tools_used_pool, k=random.randint(1, 3))

				# User message
				lines.append(json.dumps({
					"type": "user",
					"uuid": str(uuid.UUID(int=random.getrandbits(128))),
					"sessionId": session_id,
					"timestamp": ts_tz(turn_time),
					"cwd": project_path,
					"gitBranch": branch,
					"message": {
						"role": "user",
						"content": prompt,
					},
				}))

				# Assistant message with tool_use blocks
				assistant_blocks = [{"type": "text", "text": reply}]
				for tool in tools_picked:
					tool_id = "tu_" + rand_hex(16)
					if tool == "Bash":
						tool_input = {"command": random.choice([
							"cargo test --workspace",
							"npm run build",
							"git diff --stat HEAD~1",
						])}
					elif tool in ("Edit", "Write"):
						tool_input = {
							"file_path": f"{project_path}/src/{random.choice(['handler', 'auth', 'cache', 'parser'])}.rs",
							"old_string": "",
							"new_string": "",
						}
					elif tool == "Read":
						tool_input = {"file_path": f"{project_path}/{random.choice(['Cargo.toml', 'src/lib.rs', 'README.md'])}"}
					else:
						tool_input = {"pattern": "TODO"}
					assistant_blocks.append({
						"type": "tool_use",
						"id": tool_id,
						"name": tool,
						"input": tool_input,
					})

				lines.append(json.dumps({
					"type": "assistant",
					"uuid": str(uuid.UUID(int=random.getrandbits(128))),
					"sessionId": session_id,
					"timestamp": ts_tz(turn_time + timedelta(seconds=random.randint(2, 30))),
					"cwd": project_path,
					"gitBranch": branch,
					"message": {
						"role": "assistant",
						"content": assistant_blocks,
					},
				}))

			session_file.write_text("\n".join(lines) + "\n")
			total_files += 1
			total_messages += len(lines)

	print(f"  session_jsonls: {total_files} files, {total_messages} messages in {PROJECTS_DIR}")


# ── Main ──────────────────────────────────────────────────────────────────────

def parse_args() -> argparse.Namespace:
	parser = argparse.ArgumentParser(
		description="Seed Quill's SQLite DB with reproducible dummy data.",
	)
	parser.add_argument(
		"--data-dir", type=Path, default=None,
		help="Directory to write usage.db into. Default: platform app_data_dir for Quill.",
	)
	parser.add_argument(
		"--rules-dir", type=Path, default=None,
		help="Directory to write sample learned-rule .md files into. Default: ~/.claude/rules/learned/.",
	)
	parser.add_argument(
		"--projects-dir", type=Path, default=None,
		help="Directory to write fictional Claude session JSONL files into. Default: ~/.claude/projects/.",
	)
	parser.add_argument(
		"--no-projects", action="store_true",
		help="Skip writing session JSONL files (omits the Session Search demo data).",
	)
	parser.add_argument(
		"--no-backup", action="store_true",
		help="Skip the existing-DB backup step. Use this when seeding a fresh sandbox.",
	)
	parser.add_argument(
		"--seed", type=int, default=42,
		help="RNG seed for reproducibility (default: 42).",
	)
	parser.add_argument(
		"--quiet", action="store_true",
		help="Suppress per-step progress output; only emit the final summary.",
	)
	return parser.parse_args()


def main() -> None:
	global DB_PATH, BAK_PATH, LEARNED_DIR, PROJECTS_DIR, QUIET, NO_BACKUP, USING_OVERRIDE, SKIP_PROJECTS

	args = parse_args()

	QUIET = args.quiet
	NO_BACKUP = args.no_backup
	USING_OVERRIDE = args.data_dir is not None
	SKIP_PROJECTS = args.no_projects

	data_dir = args.data_dir if args.data_dir is not None else DEFAULT_DATA_DIR
	rules_dir = args.rules_dir if args.rules_dir is not None else DEFAULT_RULES_DIR
	projects_dir = args.projects_dir if args.projects_dir is not None else DEFAULT_PROJECTS_DIR

	DB_PATH = data_dir / "usage.db"
	BAK_PATH = DB_PATH.with_suffix(".db.bak")
	LEARNED_DIR = rules_dir
	PROJECTS_DIR = projects_dir

	random.seed(args.seed)

	log(f"\nQuill Dummy Data Seeder")
	log(f"DB path:    {DB_PATH}")
	log(f"Rules path: {LEARNED_DIR}")
	if USING_OVERRIDE:
		log("Mode:       sandbox (--data-dir override; running-Quill guard skipped)")
	log()

	if not USING_OVERRIDE:
		log("Step 0: Checking Quill is not running...")
		check_quill_not_running()
		log("  OK — no Quill process found.")
		log()

	if not NO_BACKUP:
		log("Step 1: Backing up database...")
		backup_db()
		log()
	else:
		log("Step 1: Backup skipped (--no-backup).")
		log()

	DB_PATH.parent.mkdir(parents=True, exist_ok=True)
	conn = sqlite3.connect(str(DB_PATH))
	conn.execute("PRAGMA foreign_keys = OFF")

	try:
		log("Step 2: Ensuring schema exists...")
		ensure_schema(conn)
		log()

		log("Step 3: Clearing existing data...")
		clear_tables(conn)
		log()

		log("Step 4: Populating tables...")
		populate_usage_snapshots(conn)
		populate_usage_hourly(conn)
		populate_token_snapshots(conn)
		populate_token_hourly(conn)
		populate_settings(conn)
		populate_observations(conn)
		populate_learning_runs(conn)
		populate_learned_rules(conn)
		populate_schema_version(conn)
		populate_observation_summaries(conn)
		populate_tool_actions(conn)
		populate_memory_files(conn)
		populate_optimization(conn)
		populate_git_snapshots(conn)
		populate_response_times(conn)
		populate_context_savings_events(conn)
		populate_session_events(conn)
		log()

		conn.commit()
		log("Done. All 18 tables populated.")
	finally:
		conn.execute("PRAGMA foreign_keys = ON")
		conn.close()

	# Session JSONL files (out of band — they live on the filesystem under
	# PROJECTS_DIR, not in the SQLite DB).
	populate_session_jsonls()

	# Final summary always prints (not gated by --quiet) so the maintainer always sees it.
	print()
	print("─" * 60)
	if USING_OVERRIDE:
		print(f"Sandbox seeded:")
		print(f"  data:     {DB_PATH}")
		print(f"  rules:    {LEARNED_DIR}")
		if not SKIP_PROJECTS:
			print(f"  projects: {PROJECTS_DIR}")
	elif BAK_PATH.exists():
		print("To restore the original DB, STOP QUILL FIRST then run:")
		print(f"  pkill -f quill; sleep 1; cp {BAK_PATH} {DB_PATH}")
	else:
		print("No backup was created (DB did not exist before seeding).")
	print("─" * 60)


if __name__ == "__main__":
	main()
