#!/usr/bin/env python3
"""
Seed Quill's SQLite DB with reproducible dummy data for screenshots.

Usage:
    python3 scripts/populate_dummy_data.py

The real DB is backed up to usage.db.bak before any changes.
Restore command is printed at the end.
"""

import hashlib
import json
import os
import random
import shutil
import sqlite3
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path

random.seed(42)

DB_PATH = Path.home() / ".local" / "share" / "com.quilltoolkit.app" / "usage.db"
BAK_PATH = DB_PATH.with_suffix(".db.bak")

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
	return dt.isoformat()


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
	""")


def clear_tables(conn: sqlite3.Connection) -> None:
	tables = [
		"usage_snapshots", "usage_hourly", "token_snapshots", "token_hourly",
		"settings", "observations", "learning_runs", "learned_rules",
		"schema_version", "observation_summaries", "tool_actions",
		"memory_files", "optimization_runs", "optimization_suggestions",
		"git_snapshots", "response_times",
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

LEARNED_DIR = Path.home() / ".claude" / "rules" / "learned"


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
	for i in range(50):
		t = NOW - timedelta(hours=random.randint(0, 72))
		session_id = random.choice(sessions)
		tool_name = random.choice(TOOLS)
		category = tool_category_map.get(tool_name, "command")
		project = random.choice(PROJECTS)
		file_path = f"{project}/src/module_{random.randint(1, 10)}.py"
		message_id = rand_session()
		summary = f"{tool_name} on {os.path.basename(file_path)}"
		full_input = json.dumps({"file_path": file_path, "command": "build"})
		full_output = json.dumps({"result": "ok", "lines_changed": random.randint(1, 50)})
		rows.append((
			message_id, session_id, tool_name, category,
			file_path, summary, full_input, full_output, ts(t),
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
			ts_val = ts(t)
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


# ── Main ──────────────────────────────────────────────────────────────────────

def main() -> None:
	print(f"\nQuill Dummy Data Seeder")
	print(f"DB path: {DB_PATH}")
	print()

	print("Step 0: Checking Quill is not running...")
	check_quill_not_running()
	print("  OK — no Quill process found.")
	print()

	print("Step 1: Backing up database...")
	backup_db()
	print()

	DB_PATH.parent.mkdir(parents=True, exist_ok=True)
	conn = sqlite3.connect(str(DB_PATH))
	conn.execute("PRAGMA foreign_keys = OFF")

	try:
		print("Step 2: Ensuring schema exists...")
		ensure_schema(conn)
		print()

		print("Step 3: Clearing existing data...")
		clear_tables(conn)
		print()

		print("Step 4: Populating tables...")
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
		print()

		conn.commit()
		print("Done. All 16 tables populated.")
	finally:
		conn.execute("PRAGMA foreign_keys = ON")
		conn.close()

	print()
	print("─" * 60)
	if BAK_PATH.exists():
		print("To restore the original DB, STOP QUILL FIRST then run:")
		print(f"  pkill -f quill; sleep 1; cp {BAK_PATH} {DB_PATH}")
	else:
		print("No backup was created (DB did not exist before seeding).")
	print("─" * 60)


if __name__ == "__main__":
	main()
