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
	"/home/alex/projects/quill",
	"/home/alex/projects/api-gateway",
	"/home/alex/projects/ml-pipeline",
	"/home/alex/projects/dashboard",
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

		CREATE TABLE IF NOT EXISTS usage_snapshots (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			timestamp TEXT NOT NULL,
			bucket_label TEXT NOT NULL,
			utilization REAL NOT NULL,
			resets_at TEXT,
			created_at TEXT DEFAULT (datetime('now'))
		);

		CREATE TABLE IF NOT EXISTS usage_hourly (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			hour TEXT NOT NULL,
			bucket_label TEXT NOT NULL,
			avg_utilization REAL NOT NULL,
			max_utilization REAL NOT NULL,
			min_utilization REAL NOT NULL,
			sample_count INTEGER NOT NULL,
			UNIQUE(hour, bucket_label)
		);

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
			created_at TEXT DEFAULT (datetime('now'))
		);

		CREATE TABLE IF NOT EXISTS token_hourly (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			hour TEXT NOT NULL,
			hostname TEXT NOT NULL DEFAULT 'local',
			total_input INTEGER NOT NULL,
			total_output INTEGER NOT NULL,
			total_cache_creation INTEGER NOT NULL DEFAULT 0,
			total_cache_read INTEGER NOT NULL DEFAULT 0,
			turn_count INTEGER NOT NULL,
			UNIQUE(hour, hostname)
		);

		CREATE TABLE IF NOT EXISTS settings (
			key TEXT PRIMARY KEY,
			value TEXT NOT NULL
		);

		CREATE TABLE IF NOT EXISTS observations (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			session_id TEXT NOT NULL,
			timestamp TEXT NOT NULL,
			hook_phase TEXT NOT NULL,
			tool_name TEXT NOT NULL,
			tool_input TEXT,
			tool_output TEXT,
			cwd TEXT,
			created_at TEXT DEFAULT (datetime('now'))
		);

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
			phases TEXT DEFAULT NULL
		);

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
			confirmed_projects TEXT DEFAULT NULL
		);

		CREATE TABLE IF NOT EXISTS schema_version (
			version INTEGER PRIMARY KEY
		);

		CREATE TABLE IF NOT EXISTS observation_summaries (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			period TEXT NOT NULL,
			project TEXT,
			tool_counts TEXT NOT NULL,
			error_count INTEGER NOT NULL DEFAULT 0,
			total_observations INTEGER NOT NULL DEFAULT 0,
			created_at TEXT DEFAULT (datetime('now')),
			UNIQUE(period, project)
		);

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
			timestamp     TEXT NOT NULL
		);

		CREATE TABLE IF NOT EXISTS memory_files (
			id              INTEGER PRIMARY KEY AUTOINCREMENT,
			project_path    TEXT NOT NULL,
			file_path       TEXT NOT NULL,
			content_hash    TEXT NOT NULL,
			last_scanned_at TEXT NOT NULL,
			UNIQUE(project_path, file_path)
		);

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
			completed_at        TEXT
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
			group_id         TEXT
		);

		CREATE TABLE IF NOT EXISTS git_snapshots (
			id           INTEGER PRIMARY KEY AUTOINCREMENT,
			project      TEXT NOT NULL UNIQUE,
			commit_hash  TEXT NOT NULL,
			commit_count INTEGER NOT NULL,
			raw_data     TEXT NOT NULL,
			created_at   TEXT DEFAULT (datetime('now'))
		);

		CREATE TABLE IF NOT EXISTS response_times (
			id           INTEGER PRIMARY KEY AUTOINCREMENT,
			session_id   TEXT NOT NULL,
			timestamp    TEXT NOT NULL,
			response_secs REAL,
			idle_secs    REAL,
			created_at   TEXT DEFAULT (datetime('now')),
			UNIQUE(session_id, timestamp)
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
	""")


def clear_tables(conn: sqlite3.Connection) -> None:
	tables = [
		"usage_snapshots", "usage_hourly", "token_snapshots", "token_hourly",
		"settings", "observations", "learning_runs", "learned_rules",
		"schema_version", "observation_summaries", "tool_actions",
		"memory_files", "optimization_runs", "optimization_suggestions",
		"git_snapshots", "response_times", "context_savings_events",
	]
	for tbl in tables:
		conn.execute(f"DELETE FROM {tbl}")


# ── 1. usage_snapshots ────────────────────────────────────────────────────────

def populate_usage_snapshots(conn: sqlite3.Connection) -> None:
	rows = []
	start = NOW - timedelta(days=7)
	t = start
	while t <= NOW:
		resets_at = (t + timedelta(hours=5)).isoformat()
		for label in BUCKET_LABELS:
			utilization = round(random.uniform(0.05, 0.95), 4)
			rows.append((ts(t), label, utilization, resets_at))
		t += timedelta(minutes=5)

	conn.executemany(
		"INSERT INTO usage_snapshots (timestamp, bucket_label, utilization, resets_at) "
		"VALUES (?, ?, ?, ?)",
		rows,
	)
	print(f"  usage_snapshots: {len(rows)} rows")


# ── 2. usage_hourly ───────────────────────────────────────────────────────────

def populate_usage_hourly(conn: sqlite3.Connection) -> None:
	rows = []
	start = (NOW - timedelta(days=7)).replace(minute=0, second=0, microsecond=0)
	hour = start
	while hour <= NOW:
		hour_str = hour.strftime("%Y-%m-%dT%H:00:00+00:00")
		for label in BUCKET_LABELS:
			samples = [random.uniform(0.05, 0.95) for _ in range(12)]
			rows.append((
				hour_str, label,
				round(sum(samples) / len(samples), 4),
				round(max(samples), 4),
				round(min(samples), 4),
				len(samples),
			))
		hour += timedelta(hours=1)

	conn.executemany(
		"INSERT OR IGNORE INTO usage_hourly "
		"(hour, bucket_label, avg_utilization, max_utilization, min_utilization, sample_count) "
		"VALUES (?, ?, ?, ?, ?, ?)",
		rows,
	)
	print(f"  usage_hourly: {len(rows)} rows")


# ── 3. token_snapshots ────────────────────────────────────────────────────────

def populate_token_snapshots(conn: sqlite3.Connection) -> list[tuple[str, str]]:
	"""Return list of (session_id, hostname) for reuse."""
	sessions = []
	for _ in range(20):
		sessions.append((rand_session(), random.choice(HOSTNAMES), random.choice(PROJECTS)))

	rows = []
	start = NOW - timedelta(days=7)
	for session_id, hostname, project in sessions:
		t = start + timedelta(hours=random.randint(0, 160))
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

	conn.executemany(
		"INSERT INTO token_snapshots "
		"(session_id, hostname, timestamp, input_tokens, output_tokens, "
		" cache_creation_input_tokens, cache_read_input_tokens, cwd) "
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
		rows,
	)
	print(f"  token_snapshots: {len(rows)} rows")
	return [(s, h) for s, h, _ in sessions]


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
	settings = [
		("learning.enabled", "true"),
		("learning.trigger_mode", "periodic"),
		("learning.periodic_minutes", "180"),
		("learning.min_observations", "50"),
		("learning.min_confidence", "0.95"),
		("app.theme", "dark"),
		("app.window_width", "280"),
		("app.window_height", "400"),
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
		"state": "confirmed",
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

	rows = []
	for i, rule in enumerate(RULE_DEFS):
		file_name = f"{rule['name']}.md"
		file_path = str(LEARNED_DIR / file_name)

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
	for v in range(1, 11):
		conn.execute("INSERT OR IGNORE INTO schema_version (version) VALUES (?)", (v,))
	print("  schema_version: versions 1-10")


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
	categories = ["file_edit", "file_read", "search", "command", "web"]
	tool_category_map = {
		"Edit": "file_edit", "Write": "file_edit",
		"Read": "file_read", "Glob": "search", "Grep": "search",
		"Bash": "command", "WebSearch": "web", "WebFetch": "web",
		"Task": "command", "TodoWrite": "command",
	}

	sessions = [rand_session() for _ in range(5)]
	rows = []
	# Sample line content for Edit/Write — keeps parse_code_change in
	# storage.rs::get_code_stats happy by counting lines from real strings.
	old_snippets = [
		"if let Err(e) = result {\n    return Err(e.into());\n}",
		"const TIMEOUT_MS: u64 = 5000;",
		"pub fn handle(&self) -> Result<(), Error> {\n    self.inner.lock().unwrap().handle()\n}",
		"return data.filter(x => x.active);",
		"// TODO: refactor",
	]
	new_snippets = [
		"result.map_err(|e| e.into())",
		"const TIMEOUT_MS: u64 = 10_000;\nconst RETRY_MS: u64 = 250;",
		"pub fn handle(&self) -> Result<(), Error> {\n    let guard = self.inner.lock()?;\n    guard.handle()\n}",
		"return data.filter(item => item.active && !item.archived).map(format_row);",
		"// Cleaned up in PR #142",
	]

	for i in range(50):
		t = NOW - timedelta(hours=random.randint(0, 72))
		session_id = random.choice(sessions)
		tool_name = random.choice(TOOLS)
		project = random.choice(PROJECTS)
		message_id = rand_session()

		# Edit/Write get category='code_change' with parseable input so
		# get_code_stats (storage.rs) computes added/removed LOC. Other tools
		# keep their category-map entry.
		if tool_name == "Edit":
			file_path = f"{project}/src/module_{random.randint(1, 10)}.rs"
			category = "code_change"
			old_s = random.choice(old_snippets)
			new_s = random.choice(new_snippets)
			full_input = json.dumps({"file_path": file_path, "old_string": old_s, "new_string": new_s})
			full_output = json.dumps({"result": "ok"})
		elif tool_name == "Write":
			file_path = f"{project}/src/module_{random.randint(1, 10)}.rs"
			category = "code_change"
			content = "\n".join(random.choices(new_snippets, k=random.randint(8, 25)))
			full_input = json.dumps({"file_path": file_path, "content": content})
			full_output = json.dumps({"result": "ok"})
		else:
			file_path = f"{project}/src/module_{random.randint(1, 10)}.py"
			category = tool_category_map.get(tool_name, "command")
			full_input = json.dumps({"file_path": file_path, "command": "build"})
			full_output = json.dumps({"result": "ok", "lines_changed": random.randint(1, 50)})

		summary = f"{tool_name} on {os.path.basename(file_path)}"
		rows.append((
			message_id, session_id, tool_name, category,
			file_path, summary, full_input, full_output, ts_tz(t),
		))

	conn.executemany(
		"INSERT INTO tool_actions "
		"(message_id, session_id, tool_name, category, file_path, "
		" summary, full_input, full_output, timestamp) "
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
		rows,
	)
	print(f"  tool_actions: {len(rows)} rows")


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
	"""
	preservation_sources = [
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
	]

	# Map event_type -> (category, decision, has_source_ref)
	event_type_specs = {
		"mcp.index": ("preservation", "indexed", True),
		"mcp.fetch": ("preservation", "indexed", True),
		"mcp.execute": ("preservation", "indexed", True),
		"context.get_source": ("retrieval", "returned", False),
		"compaction.snapshot.read": ("retrieval", "returned", False),
		"router.guidance": ("routing", "injected", False),
		"capture.event": ("telemetry", "observed", False),
		"capture.snapshot": ("telemetry", "observed", False),
	}

	rows = []
	for i in range(70):
		event_type, (category, decision, has_indexed) = random.choice(list(event_type_specs.items()))
		source_label = random.choice(preservation_sources)
		hours_ago = random.uniform(0, 7 * 24)
		when = NOW - timedelta(hours=hours_ago)

		if category == "preservation":
			indexed_b = random.randint(8_000, 350_000)
			returned_b = 0
			input_b = 0
			tok_indexed = indexed_b // 4
			tok_returned = 0
			tok_saved = tok_indexed
			tok_preserved = tok_indexed
		elif category == "retrieval":
			indexed_b = 0
			returned_b = random.randint(2_000, 60_000)
			input_b = 0
			tok_indexed = 0
			tok_returned = returned_b // 4
			tok_saved = tok_returned
			tok_preserved = 0
		elif category == "routing":
			indexed_b = 0
			returned_b = 0
			input_b = random.randint(150, 1_400)
			tok_indexed = 0
			tok_returned = 0
			tok_saved = 0
			tok_preserved = 0
		else:  # telemetry
			indexed_b = 0
			returned_b = 0
			input_b = 0
			tok_indexed = 0
			tok_returned = 0
			tok_saved = 0
			tok_preserved = 0

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
			f"source:{i+1}" if has_indexed else None,
			f"snapshot:{i+1}" if event_type == "compaction.snapshot.read" else None,
			None,
		))

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
	print(f"  context_savings_events: {len(rows)} rows across 4 categories")


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
		log()

		conn.commit()
		log("Done. All 17 tables populated.")
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
