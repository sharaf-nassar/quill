#!/usr/bin/env python3
"""
Seed Quill's SQLite DB with reproducible dummy data for screenshots.

Default usage (writes to your real Quill DB after backing it up):
    python3 scripts/populate_dummy_data.py

Sandboxed usage (writes only inside an arbitrary dir, no backup, no running-Quill guard):
    python3 scripts/populate_dummy_data.py \\
        --bin src-tauri/target/debug/quill \\
        --data-dir /tmp/quill-demo/data \\
        --rules-dir /tmp/quill-demo/rules \\
        --projects-dir /tmp/quill-demo/projects \\
        --codex-sessions-dir /tmp/quill-demo/codex-sessions \\
        --home-dir /tmp/quill-demo/home \\
        --no-backup

The CLI surface is documented in
specs/001-marketing-site/contracts/seeder-cli.md.
"""

import argparse
import hashlib
import json
import ntpath
import os
import random
import shutil
import sqlite3
import stat
import subprocess
import sys
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path

DEFAULT_DATA_DIR = Path.home() / ".local" / "share" / "com.quilltoolkit.app"
DEFAULT_RULES_DIR = Path.home() / ".claude" / "rules" / "learned"
DEFAULT_PROJECTS_DIR = Path.home() / ".claude" / "projects"
DEFAULT_CODEX_SESSIONS_DIR = Path.home() / ".codex" / "sessions"

# Module-level globals bound by main() after argparse so existing populate_* helpers
# can read them without threading a config object through every signature.
DB_PATH: Path = DEFAULT_DATA_DIR / "usage.db"
BAK_PATH: Path = DB_PATH.with_suffix(".db.bak")
PROJECTS_DIR: Path = DEFAULT_PROJECTS_DIR
CODEX_SESSIONS_DIR: Path | None = None
# Isolated HOME for the per-project memory documents; None leaves them unwritten.
MEMORY_HOME: Path | None = None
QUIET: bool = False
NO_BACKUP: bool = False
USING_OVERRIDE: bool = False  # True when --data-dir was passed; skips running-Quill guard
SKIP_PROJECTS: bool = False   # True when --no-projects was passed
MODEL_FIXTURE_MODE: bool = False


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


def resolve_quill_bin(configured: Path | None) -> Path:
	"""Find the executable that owns the production migration path."""
	if configured is not None:
		candidates = [configured]
	else:
		name = "quill.exe" if os.name == "nt" else "quill"
		found = shutil.which("quill")
		repo_root = Path(__file__).resolve().parents[1]
		candidates = ([Path(found)] if found else []) + [
			repo_root / "src-tauri" / "target" / "release" / name,
			repo_root / "src-tauri" / "target" / "debug" / name,
		]
	for candidate in candidates:
		candidate = candidate.expanduser().resolve()
		if candidate.is_file() and (os.name == "nt" or os.access(candidate, os.X_OK)):
			return candidate
	raise FileNotFoundError("Quill binary not found; pass --bin PATH or build Quill first")


def initialize_database(quill_bin: Path) -> None:
	"""Create or migrate usage.db through Quill's production Rust schema path."""
	subprocess.run([str(quill_bin), "--init-database", str(DB_PATH)], check=True)


def clear_tables(conn: sqlite3.Connection) -> None:
	tables = [
		"usage_snapshots", "usage_hourly", "token_snapshots", "token_hourly",
		"settings", "observations", "learning_runs", "learned_rules",
		"observation_summaries", "tool_actions",
		"memory_files", "optimization_runs", "optimization_suggestions",
		"git_snapshots", "response_times", "context_savings_events",
		"session_events", "skill_usages", "hook_invocations",
		"rule_versions", "rule_evidence_citations", "rule_tombstones",
		"operator_feedback", "evaluation_results", "reviewer_overrides",
		"model_usage_observations", "model_observation_sources",
		"model_backfill_state", "model_usage_hourly", "runtime_hourly",
		"runtime_turn_state",
	]
	for tbl in tables:
		conn.execute(f"DELETE FROM {tbl}")
	conn.execute(
		"""UPDATE rollup_meta
		SET rollup_generation = 0,
			model_backfill_status = 'pending',
			model_backfill_done_through_ms = NULL,
			runtime_backfill_status = 'pending',
			runtime_backfill_done_through_rowid = NULL
		WHERE id = 1"""
	)


# ── 1. usage_snapshots ────────────────────────────────────────────────────────

def populate_usage_snapshots(conn: sqlite3.Connection) -> None:
	# Live rate-limit bars come from get_latest_usage_buckets(), which keeps the
	# MAX(timestamp) row per (provider, bucket_key). We seed both providers so
	# the Claude AND Codex bars render, and we land a deterministic, non-trivial
	# utilization on the most-recent row of each bucket so no bar reads ~0%.
	#
	# Claude bucket_key values mirror Rust migration 14's CASE mapping; Codex
	# keys mirror fetcher.rs::parse_codex_rate_limits ("{scope}_{minutes}m").
	#
	# THREE Claude buckets, not six. The 360px LIMITS row lays one fixed-width
	# cell per window beside the provider name, and the mockup's composition is
	# three (specs/018-widget-ui-redesign/mockup.tpl.html). A real account only
	# reports the model/OAuth buckets it actually has, so seeding the plan pair
	# plus one model window is both truthful and photographable; six overflow
	# the row.
	claude_buckets = [
		("five_hour", "5 hours"),
		("seven_day", "7 days"),
		("seven_day_opus", "Opus"),
	]
	codex_buckets = [
		("primary_300m", "5 hours"),
		("secondary_10080m", "7 days"),
	]
	# Final "current" utilization per bucket_key on the app 0..100 PERCENT scale
	# (utilization is rendered directly as "N%"; 0..1 fractions show as ~0%).
	# The rolling window runs hottest and the model window coolest, so the row
	# reads as an actively-used account rather than an idle one. Exactly one
	# cell lands in the amber band (>=50) and none in the red (>=80), so the
	# published shot shows the severity meter working without reading alarmed.
	current_util = {
		"five_hour": 62.0,
		"seven_day": 31.0,
		"seven_day_opus": 18.0,
		"primary_300m": 44.0,
		"secondary_10080m": 26.0,
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
		("claude", "seven_day_opus", "Opus"),
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

def populate_token_snapshots(conn: sqlite3.Connection) -> list[tuple[str, str, datetime, datetime]]:
	"""Seed the token corpus behind the usage chart, live summary and breakdown.

	Returns one `(session_id, provider, first_seen, last_seen)` per session so
	[[scripts/populate_dummy_data.py#populate_response_times]] can put this
	corpus's turns on this corpus's sessions.

	Drives the Usage chart ranges and live summary. The live
	summary (useLiveSummaryData.ts) and get_session_breakdown derive
	`last_active`/`project` from token_snapshots, then filter client-side to the
	rolling 6h window — so we seed BOTH a 30-day historical spread AND a
	dedicated recent cluster (0.5-5.5h old) across distinct projects.

	Every row carries a `provider` so session and host breakdowns keep producer
	identity. Claude leads the mix because it also owns the sub-agent rows below.

	A slice of the recent Claude sessions is written as sub-agent rows
	(`is_sidechain = 1` with an `agent_id`), which is what makes the Usage
	view's session breakdown show a rolled-up sub-agent count instead of a flat
	list — the same signal `get_session_breakdown` reads for `has_subagents`.
	"""
	HISTORICAL_WINDOW_DAYS = 30
	# Weighted so both series are legible at every range while Claude stays the
	# dominant one, matching the provider mix the rest of the dataset seeds.
	def pick_provider() -> str:
		return "claude" if random.random() < 0.62 else "codex"

	sessions = []
	for _ in range(40):
		sessions.append((
			rand_session(), random.choice(HOSTNAMES),
			random.choice(PROJECTS), pick_provider(),
		))

	rows = []
	start = NOW - timedelta(days=HISTORICAL_WINDOW_DAYS)
	span_hours = HISTORICAL_WINDOW_DAYS * 24
	for session_id, hostname, project, provider in sessions:
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
				project, provider, 0, None,
			))
			t += timedelta(minutes=random.randint(1, 15))

	# RECENT cluster: 7 sessions inside the rolling 6h window, each on a
	# DISTINCT project, so the Live summary shows non-zero Sessions / Projects /
	# Tokens and the 1H/24H analytics ranges are populated. Projects cycle
	# through the fictional PROJECTS list (more sessions than projects is fine —
	# the live "Projects" count is over distinct cwd values that are recent).
	# Providers alternate so the 1H and 6H ranges carry BOTH series, not just
	# the 30D one. These start 70-330 min ago (mostly in the 1h..6h band) so
	# they do NOT dominate the last-hour token total that the efficiency card
	# divides by.
	recent_sessions = []
	for idx in range(7):
		session_id = rand_session()
		hostname = random.choice(HOSTNAMES)
		project = PROJECTS[idx % len(PROJECTS)]
		provider = "claude" if idx % 3 != 2 else "codex"
		recent_sessions.append((session_id, hostname, project, provider))
		start_minutes_ago = random.randint(70, 330)
		t = NOW - timedelta(minutes=start_minutes_ago)
		num_turns = random.randint(4, 12)
		# Two of the Claude sessions delegate to a sub-agent; its turns are
		# tagged so the breakdown rolls them into the parent row.
		agent_id = f"demo-agent-{idx}" if provider == "claude" and idx < 2 else None
		for turn in range(num_turns):
			if t >= NOW - timedelta(minutes=62):
				break
			# Sub-agent work is a contiguous middle stretch of the session, the
			# way a delegated task actually lands in a transcript.
			sidechain = agent_id is not None and 1 <= turn <= 3
			rows.append((
				session_id, hostname, ts(t),
				random.randint(800, 9000),
				random.randint(400, 3500),
				random.randint(0, 2500),
				random.randint(0, 6000),
				project, provider,
				1 if sidechain else 0,
				agent_id if sidechain else None,
			))
			t += timedelta(minutes=random.randint(1, 8))

	# LAST-HOUR micro-cluster: the EFFICIENCY card is tokens / lines-changed over
	# the default 1h range. With ~90 changed lines in the last hour (seeded in
	# populate_tool_actions), a ~18k-token budget here lands efficiency at
	# ~200 tokens/line. Two small sessions on distinct projects — one per
	# provider — keep the live Projects count and the 1H token range populated
	# for both series without swamping the ratio.
	LAST_HOUR_TOKEN_BUDGET = 18_000
	emitted = 0
	for idx in range(2):
		session_id = rand_session()
		hostname = random.choice(HOSTNAMES)
		project = PROJECTS[idx % len(PROJECTS)]
		provider = "claude" if idx == 0 else "codex"
		recent_sessions.append((session_id, hostname, project, provider))
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
			rows.append((
				session_id, hostname, ts(t), inp, out, cc, cr,
				project, provider, 0, None,
			))
			t += timedelta(minutes=random.randint(1, 4))
			if emitted >= LAST_HOUR_TOKEN_BUDGET:
				break
		if emitted >= LAST_HOUR_TOKEN_BUDGET:
			break

	conn.executemany(
		"INSERT INTO token_snapshots "
		"(session_id, hostname, timestamp, input_tokens, output_tokens, "
		" cache_creation_input_tokens, cache_read_input_tokens, cwd, "
		" provider, is_sidechain, agent_id) "
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		rows,
	)
	providers = sorted({row[8] for row in rows})
	subagent_rows = sum(1 for row in rows if row[9] == 1)
	print(
		f"  token_snapshots: {len(rows)} rows ({len(sessions)} historical + "
		f"{len(recent_sessions)} recent sessions, providers: {', '.join(providers)}, "
		f"{subagent_rows} sub-agent rows)"
	)

	# Collapse the emitted rows into one activity window per session. Deriving
	# it here rather than tracking it alongside every append keeps the emitters
	# above single-purpose.
	windows: dict[str, tuple[str, datetime, datetime]] = {}
	for row in rows:
		session_id, moment, provider = row[0], datetime.fromisoformat(row[2]), row[8]
		moment = moment.replace(tzinfo=timezone.utc)
		known = windows.get(session_id)
		windows[session_id] = (
			(provider, moment, moment) if known is None
			else (known[0], min(known[1], moment), max(known[2], moment))
		)
	return [
		(session_id, provider, first_seen, last_seen)
		for session_id, (provider, first_seen, last_seen) in windows.items()
	]


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
	# bars, live summaries, and provider toggles populate without manual setup,
	# plus mini_max enabled-but-unconfigured so the SETUP row renders.
	# setupState uses the snake_case serde variants ("installed",
	# "not_installed"); merge_saved_statuses keeps `enabled` from this row and
	# re-derives the rest from live detection.
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
		# MiniMax is enabled but not installed, which is the exact state the
		# LIMITS row renders as SETUP (LimitsSection.emptyRowState: an enabled
		# provider with no buckets and a missing/not_installed setup state is
		# actionable, not broken). The marketing copy for the live section
		# describes that row, and a disabled provider gets no row at all.
		{
			"provider": "mini_max",
			"detectedCli": False,
			"detectedHome": False,
			"enabled": True,
			"setupState": "not_installed",
			"userHasMadeChoice": True,
			"lastError": None,
			"lastVerifiedAt": None,
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
		"scope": "shared",
		"domain": "coding-style",
		"confidence": 0.92,
		"observation_count": 87,
		"source": "observations",
		"state": "confirmed",
		"content": "# Prefer Immutable Updates\n\nAlways create new objects instead of mutating existing ones.\nUse spread operators, Object.assign(), or immutable helpers.\n",
	},
	{
		"name": "use-async-await",
		"scope": "claude",
		"domain": "coding-style",
		"confidence": 0.88,
		"observation_count": 64,
		"source": "observations",
		"state": "confirmed",
		"content": "# Use Async/Await\n\nPrefer async/await over .then() chains for Promise handling.\nThis improves readability and error handling.\n",
	},
	{
		"name": "small-focused-functions",
		"scope": "claude",
		"domain": "coding-style",
		"confidence": 0.79,
		"observation_count": 45,
		"source": "observations",
		"state": "emerging",
		"content": "# Small Focused Functions\n\nKeep functions under 50 lines. Extract helpers for complex logic.\nOne function, one responsibility.\n",
	},
	{
		"name": "validate-inputs-at-boundary",
		"scope": "shared",
		"domain": "security",
		"confidence": 0.95,
		"observation_count": 103,
		"source": "observations",
		"state": "confirmed",
		"content": "# Validate Inputs at Boundaries\n\nAlways validate user input and external data at system entry points.\nUse schema-based validation (e.g., Zod for TypeScript).\n",
	},
	{
		"name": "native-python-types",
		"scope": "codex",
		"domain": "coding-style",
		"confidence": 0.83,
		"observation_count": 58,
		"source": "git",
		"state": "emerging",
		"content": "# Native Python Types\n\nUse native Python 3.10+ type annotations.\nPrefer `list[str]` over `List[str]`, `str | None` over `Optional[str]`.\n",
	},
	{
		"name": "avoid-blanket-exceptions",
		"scope": "codex",
		"domain": "error-handling",
		"confidence": 0.91,
		"observation_count": 72,
		"source": "observations",
		"state": "confirmed",
		"content": "# Avoid Blanket Exception Catches\n\nNever catch bare `except Exception`. Always catch specific exceptions.\nLet unexpected errors bubble up for better debugging.\n",
	},
	{
		"name": "tabs-over-spaces",
		"scope": "claude",
		"domain": "formatting",
		"confidence": 0.97,
		"observation_count": 200,
		"source": "observations",
		"state": "confirmed",
		"content": "# Tabs Over Spaces\n\nUse tabs for indentation, not spaces.\nConfigure your editor and linter accordingly.\n",
	},
	{
		"name": "no-console-log-in-production",
		"scope": "claude",
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


# Rules root -> scope directory, and the `provider_scope` JSON each one means.
# Demo mode collapses every scope under the resolved rules root
# (learning.rs::learned_rules_dir_for_scope), and the reconciler infers a
# rule's scope from which of those three directories holds its `.md`
# (storage.rs::inferred_rule_provider_scope) — so the directory and the DB
# column have to agree or the next reconcile rewrites the row.
RULE_SCOPE_DIRS = {
	"claude": '["claude"]',
	"codex": '["codex"]',
	"shared": '["claude","codex"]',
}


def populate_learned_rules(conn: sqlite3.Connection) -> None:
	LEARNED_DIR.mkdir(parents=True, exist_ok=True)
	for scope in RULE_SCOPE_DIRS:
		(LEARNED_DIR / scope).mkdir(parents=True, exist_ok=True)

	# A rule reads as ACTIVE when it has a `file_path` and a non-terminal state
	# (types.ts::isActiveRule), and the app only ever finds a rule file inside
	# one of the three scope directories above. A confirmed rule therefore gets
	# its `.md` written into its scope directory; an emerging one gets NO file
	# and an empty `file_path`, which is exactly how the app stores a
	# discovered-but-unpromoted candidate. Writing candidate files flat in the
	# rules root would leave orphans the app never scans.
	rows = []
	written = 0
	for rule in RULE_DEFS:
		scope = rule["scope"]
		is_active = rule.get("state") == "confirmed"
		file_path = ""
		if is_active:
			path = LEARNED_DIR / scope / f"{rule['name']}.md"
			path.write_text(rule["content"])
			file_path = str(path)
			written += 1

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
			RULE_SCOPE_DIRS[scope],
			# An on-disk rule is `lifecycle='active'` (storage.rs promote path);
			# an unpromoted one stays a candidate. Storing the body for both is
			# what lets a discovered card expand without a file to read.
			"active" if is_active else "candidate",
			rule["content"],
			hashlib.sha256(rule["content"].encode()).hexdigest(),
		))

	conn.executemany(
		"INSERT OR IGNORE INTO learned_rules "
		"(name, domain, confidence, observation_count, file_path, "
		" created_at, updated_at, source, alpha, beta_param, last_evidence_at, "
		" state, project, is_anti_pattern, confirmed_projects, provider_scope, "
		" lifecycle, content, content_hash) "
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		rows,
	)
	print(
		f"  learned_rules: {len(rows)} rows  ({written} active .md files under "
		f"{LEARNED_DIR}/{{{','.join(RULE_SCOPE_DIRS)}}}, "
		f"{len(rows) - written} discovered candidates)"
	)


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
	get_code_stats_history(), which since migration 33 reads the STORED
	`lines_added` / `lines_removed` columns and only falls back to re-parsing
	`full_input` for legacy rows. Every code_change row therefore carries its
	own counts, computed exactly the way sessions.rs::count_code_change_lines
	does at ingest — otherwise TOK/LOC, LOC/HR and NET LINES read empty and the
	code-history readers report no changes.

	The default NOW tab runs the 1h range, so
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

	# (session_id, provider). Tool work is attributed the same way the token
	# corpus is, so code-history queries and the token series describe one
	# consistent two-provider workspace.
	sessions = [
		(rand_session(), "claude" if idx % 3 != 2 else "codex")
		for idx in range(8)
	]

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

	def action_key() -> str:
		"""A live row's identity within its session; mirrors a `tool_use_id`."""
		return f"toolu_{rand_hex(24)}"

	def code_change_row(t: datetime, prefer_write: bool | None = None) -> tuple:
		session_id, provider = random.choice(sessions)
		project = random.choice(PROJECTS)
		message_id = rand_session()
		ext = random.choice(exts)
		file_path = f"{project}/src/module_{random.randint(1, 18)}.{ext}"
		is_write = random.random() < 0.45 if prefer_write is None else prefer_write
		if is_write:
			tool_name = "Write"
			content = random.choice(code_blocks)
			full_input = json.dumps({"file_path": file_path, "content": content})
			# Mirrors sessions.rs::count_code_change_lines for Write: the whole
			# body is added and nothing is removed. `splitlines()` matches
			# Rust's `str::lines()` on a trailing newline, `count("\n") + 1`
			# does not.
			added, removed = len(content.splitlines()), 0
		else:
			tool_name = "Edit"
			old_s, new_s = random.choice(edit_pairs)
			full_input = json.dumps({"file_path": file_path, "old_string": old_s, "new_string": new_s})
			added, removed = len(new_s.splitlines()), len(old_s.splitlines())
		full_output = json.dumps({"result": "ok"})
		summary = f"{tool_name} on {os.path.basename(file_path)}"
		return (
			provider, action_key(), message_id, session_id, session_id,
			tool_name, "code_change", file_path, summary,
			full_input, full_output, ts_tz(t), added, removed,
		)

	rows = []
	# ~300 total actions, ~80% Edit/Write code_change spread across 30 days.
	TOTAL = 300
	code_change_target = int(TOTAL * 0.80)
	for _ in range(code_change_target):
		t = NOW - timedelta(minutes=random.randint(90, 30 * 24 * 60))
		rows.append(code_change_row(t))

	# Remaining ~20% are non-code tools, also spread across 30 days. A
	# non-code action changes nothing, so its line counts are 0/0 rather than
	# NULL — NULL is the legacy marker that sends the reader back to
	# re-parsing `full_input`.
	for _ in range(TOTAL - code_change_target):
		t = NOW - timedelta(minutes=random.randint(90, 30 * 24 * 60))
		session_id, provider = random.choice(sessions)
		project = random.choice(PROJECTS)
		tool_name = random.choice(non_code_tools)
		file_path = f"{project}/src/module_{random.randint(1, 18)}.py"
		category = tool_category_map.get(tool_name, "command")
		full_input = json.dumps({"file_path": file_path, "command": "build"})
		full_output = json.dumps({"result": "ok"})
		summary = f"{tool_name} on {os.path.basename(file_path)}"
		rows.append((
			provider, action_key(), rand_session(), session_id, session_id,
			tool_name, category, file_path, summary,
			full_input, full_output, ts_tz(t), 0, 0,
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
		recent_lines += row[12] + row[13]
		recent_count += 1

	conn.executemany(
		"INSERT INTO tool_actions "
		"(provider, action_key, message_id, session_id, chain_id, "
		" tool_name, category, file_path, summary, full_input, full_output, "
		" timestamp, lines_added, lines_removed) "
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		rows,
	)
	net_lines = sum(row[12] - row[13] for row in rows)
	print(
		f"  tool_actions: {len(rows)} rows "
		f"(~{code_change_target} code_change + {recent_count} recent, "
		f"~{recent_lines} lines in last 1h, {net_lines:+} net lines)"
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


# One memory document per seeded project. The Memories panel's "All Projects
# (N)" is the SUM of per-project context files, so one file each is what makes
# it read (4) — as long as the global CLAUDE.md that `claude_setup` recreates
# on every launch is removed, because that file is counted once per project.
# `type:` drives the badge and `description:` the one-line subtitle.
MEMORY_DOCS = [
	(
		"/home/alex/quill", "architecture.md", "context",
		"How the collector, store and widget split responsibility.",
		"# Architecture\n\nThe collector writes snapshots, the store owns\n"
		"aggregation, and the widget only reads. Nothing in the UI layer\n"
		"queries the database directly.\n",
	),
	(
		"/home/alex/gateway", "routing.md", "convention",
		"Route naming, versioning and deprecation rules.",
		"# Routing Conventions\n\nRoutes are versioned by prefix and never\n"
		"renamed in place. A retired route answers with a deprecation header\n"
		"for one release before it is removed.\n",
	),
	(
		"/home/alex/pipeline", "ingest.md", "context",
		"Batch sizes, retry policy and the replay window.",
		"# Ingest\n\nBatches cap at 500 records. A failed batch retries three\n"
		"times with backoff, then lands in the replay queue with its offset\n"
		"so a rerun is exact rather than approximate.\n",
	),
	(
		"/home/alex/dashboard", "components.md", "convention",
		"Component structure, prop shape and state boundaries.",
		"# Components\n\nOne component per file, props typed at the boundary,\n"
		"no state above the screen that owns it. Shared primitives live in\n"
		"the kit and never import from a screen.\n",
	),
]


def populate_memory_markdown() -> None:
	"""Write the per-project memory documents the Memories panel lists.

	memory_optimizer::memory_dir resolves these under `dirs::home_dir()` — NOT
	through QUILL_CLAUDE_PROJECTS_DIR — so a demo that isolates HOME has to be
	handed that HOME here or the panel photographs empty. The slug is the
	project path with `/` replaced by `-`, matching
	memory_optimizer::project_path_to_slug.

	Skipped unless --home-dir is passed, so a default run never writes
	fictional projects into the maintainer's real ~/.claude.
	"""
	if MEMORY_HOME is None:
		print("  memory docs: skipped (--home-dir not set)")
		return

	written = 0
	for project, file_name, memory_type, description, body in MEMORY_DOCS:
		slug = project.replace("/", "-")
		directory = MEMORY_HOME / ".claude" / "projects" / slug / "memory"
		directory.mkdir(parents=True, exist_ok=True)
		(directory / file_name).write_text(
			f"---\ntype: {memory_type}\ndescription: {description}\n---\n\n{body}"
		)
		written += 1

	print(f"  memory docs: {written} files under {MEMORY_HOME}/.claude/projects/")


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

def populate_response_times(
	conn: sqlite3.Connection,
	session_windows: list[tuple[str, str, datetime, datetime]],
) -> None:
	"""Seed per-turn latency for the SAME sessions the token corpus recorded.

	get_session_breakdown reads `turn_count` from here keyed by (provider,
	session_id), so turns on unrelated session ids leave every breakdown row in
	the Usage view reading "0 turns". Each session's turns are spread across
	its own token activity window, which also keeps `last_active` — a MAX over
	both tables — inside the window the tokens describe.

	A live row's chain is its own session (`chain_id = session_id`), the shape
	`uidx_rt_live` keys.
	"""
	rows = []
	seen: set[tuple[str, str, str]] = set()

	for session_id, provider, first_seen, last_seen in session_windows:
		span_secs = (last_seen - first_seen).total_seconds()
		# Roughly one turn per 10 minutes of session, floored at three so even
		# a short session reads as a conversation, capped so one long session
		# does not dominate the breakdown.
		turns = 1 if span_secs <= 0 else max(3, min(24, int(span_secs // 600) + 3))
		step = timedelta(seconds=span_secs / turns) if turns > 1 else timedelta(0)
		t = first_seen
		for _ in range(turns):
			# response_times.timestamp is read by parse_ts_diff (chrono::parse_from_rfc3339);
			# must be tz-aware or LLM RUNTIME aggregations silently return 0.
			ts_val = ts_tz(t)
			key = (provider, session_id, ts_val)
			if key not in seen:
				seen.add(key)
				rows.append((
					provider,
					session_id,
					session_id,
					ts_val,
					round(random.uniform(0.5, 45.0), 2),
					round(random.uniform(10.0, 600.0), 2),
				))
			t += step

	conn.executemany(
		"INSERT OR IGNORE INTO response_times "
		"(provider, session_id, chain_id, timestamp, response_secs, idle_secs) "
		"VALUES (?, ?, ?, ?, ?, ?)",
		rows,
	)
	print(f"  response_times: {len(rows)} rows across {len(session_windows)} sessions")


# ── 17. context_savings_events ────────────────────────────────────────────────

def populate_context_savings_events(conn: sqlite3.Connection) -> None:
	"""Populate the context-savings categories so the Context tab renders.

	Categories (closed taxonomy):
	  - preservation: large content kept out of the LLM transcript via MCP store
	  - retrieval: LLM pulled preserved content back via quill_get_context_source
	  - routing: router guidance text injected into the transcript

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
			event_type = "mcp.source_read"
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
		source_label = source_pair[0] if source_pair else random.choice(source_labels)
		source_ref = source_pair[1] if source_pair else None
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
	# Recent routing keeps ROUTING COST live.
	for _ in range(12):
		make_row("routing", recent_when(), None)

	# --- Historical spread to ~300 total, distribution ~65/20/15. ---
	TOTAL = 300
	remaining = TOTAL - len(rows)
	weighted = ["preservation"] * 65 + ["retrieval"] * 20 + ["routing"] * 15
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
		"estimate_method, estimate_confidence, source_ref, metadata_json"
		") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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

	# Migration 30 re-keyed this table on (source_key, event_key) and made
	# `chain_id` mandatory. These are live rows (source_key NULL), so the event
	# uuid is the event key and a parent chain is its own session — the same
	# shape `ingest_session_events` writes and `uidx_se_live` enforces.
	conn.executemany(
		"INSERT OR IGNORE INTO session_events "
		"(provider, event_key, session_id, chain_id, agent_id, is_sidechain, "
		" timestamp, kind, uuid, parent_uuid) "
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		[
			(provider, uuid_val, session_id, session_id, agent_id, is_sidechain,
			 timestamp, kind, uuid_val, parent_uuid)
			for provider, session_id, agent_id, is_sidechain, timestamp, kind,
			uuid_val, parent_uuid in rows
		],
	)
	# Distinct sessions for the operator's sanity check.
	distinct_sessions = len({(r[0], r[1]) for r in rows})
	print(f"  session_events: {len(rows)} rows across {distinct_sessions} sessions")


# ── 18. retained model-bearing JSONL files ────────────────────────────────────

DEMO_MODEL_HOSTNAME = "demo-workstation"
DEMO_JSONL_OWNER_FIELD = "_quillDemoFixture"
DEMO_JSONL_OWNER_VALUE = "populate_dummy_data.py:model-fixture:v1"


def datetime_to_ms(value: datetime) -> int:
	return int(value.timestamp() * 1000)


def configured_fixture_root(provider: str) -> Path:
	if provider == "claude":
		return PROJECTS_DIR
	if provider == "codex" and CODEX_SESSIONS_DIR is not None:
		return CODEX_SESSIONS_DIR
	raise ValueError(f"unsupported or unconfigured demo transcript provider: {provider}")


def path_is_link_or_junction(path: Path) -> bool:
	return path.is_symlink() or (
		hasattr(path, "is_junction") and path.is_junction()
	)


def ensure_configured_fixture_root(provider: str, root: Path) -> tuple[Path, Path]:
	lexical_root = Path(os.path.abspath(root.expanduser()))
	if os.path.lexists(lexical_root):
		if path_is_link_or_junction(lexical_root):
			raise ValueError(f"refusing symlinked or junction-backed {provider} fixture root")
		if not stat.S_ISDIR(lexical_root.lstat().st_mode):
			raise ValueError(f"configured {provider} fixture root is not a directory")
	else:
		lexical_root.mkdir(parents=True)
	canonical_root = lexical_root.resolve(strict=True)
	return lexical_root, canonical_root


def prepare_jsonl_fixture_target(provider: str, path: Path) -> Path:
	lexical_root, canonical_root = ensure_configured_fixture_root(
		provider,
		configured_fixture_root(provider),
	)
	target = Path(os.path.abspath(path.expanduser()))
	try:
		relative = target.relative_to(lexical_root)
	except ValueError as error:
		raise ValueError(f"{provider} fixture target is outside its configured root") from error
	if not relative.parts or target.suffix != ".jsonl":
		raise ValueError(f"{provider} fixture target must be a child JSONL path")

	parent = lexical_root
	for component in relative.parts[:-1]:
		parent /= component
		if os.path.lexists(parent):
			if path_is_link_or_junction(parent):
				raise ValueError(f"refusing symlinked or junction-backed {provider} fixture parent")
			if not stat.S_ISDIR(parent.lstat().st_mode):
				raise ValueError(f"{provider} fixture parent is not a directory")
		else:
			parent.mkdir()
		if canonical_path_inside(canonical_root, parent) is None:
			raise ValueError(f"{provider} fixture parent escaped its canonical root")

	if os.path.lexists(target):
		if path_is_link_or_junction(target):
			raise ValueError(f"refusing symlinked or junction-backed {provider} fixture target")
		if MODEL_FIXTURE_MODE:
			raise FileExistsError(
				f"refusing to overwrite existing {provider} fixture target: {target}"
			)
		if not stat.S_ISREG(target.lstat().st_mode):
			raise ValueError(f"legacy {provider} fixture target is not a regular file")
	return target


def safe_write_text(path: Path, contents: str, *, overwrite: bool) -> None:
	if os.name == "posix" and hasattr(os, "O_NOFOLLOW"):
		parent_flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
		parent_fd = os.open(path.parent, parent_flags)
		try:
			file_flags = os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW
			file_flags |= os.O_TRUNC if overwrite else os.O_EXCL
			file_fd = os.open(path.name, file_flags, 0o666, dir_fd=parent_fd)
			with os.fdopen(file_fd, "w", encoding="utf-8") as target:
				target.write(contents)
		finally:
			os.close(parent_fd)
	else:
		mode = "w" if overwrite else "x"
		with path.open(mode, encoding="utf-8") as target:
			target.write(contents)


def write_jsonl_fixture(provider: str, path: Path, records: list[dict | str]) -> None:
	lines = [record if isinstance(record, str) else json.dumps(record) for record in records]
	if MODEL_FIXTURE_MODE and lines:
		first_record = json.loads(lines[0])
		if not isinstance(first_record, dict):
			raise ValueError("demo JSONL ownership marker requires an object record")
		first_record[DEMO_JSONL_OWNER_FIELD] = DEMO_JSONL_OWNER_VALUE
		lines[0] = json.dumps(first_record)
	target = prepare_jsonl_fixture_target(provider, path)
	safe_write_text(
		target,
		"\n".join(lines) + "\n",
		overwrite=not MODEL_FIXTURE_MODE,
	)


def canonical_path_inside(root: Path, path: Path) -> Path | None:
	canonical_root = root.resolve(strict=True)
	canonical_path = path.resolve(strict=True)
	try:
		canonical_path.relative_to(canonical_root)
	except ValueError:
		return None
	return canonical_path


def discover_runtime_model_jsonls(provider: str, root: Path) -> dict[Path, Path]:
	"""Mirror provider transcript discovery and key by canonical source path."""
	root.mkdir(parents=True, exist_ok=True)
	candidates = []
	if provider == "claude":
		for project_dir in sorted(root.iterdir()):
			if not project_dir.is_dir():
				continue
			for entry in sorted(project_dir.iterdir()):
				if entry.is_file() and entry.suffix == ".jsonl":
					candidates.append(entry)
				elif entry.is_dir():
					subagents_dir = entry / "subagents"
					if subagents_dir.is_dir():
						candidates.extend(
							path for path in sorted(subagents_dir.iterdir())
							if path.is_file() and path.suffix == ".jsonl"
						)
	elif provider == "codex":
		def raise_walk_error(error: OSError) -> None:
			raise error

		for directory, subdirectories, filenames in os.walk(
			root,
			followlinks=False,
			onerror=raise_walk_error,
		):
			subdirectories.sort()
			for filename in sorted(filenames):
				if Path(filename).suffix == ".jsonl":
					candidates.append(Path(directory) / filename)
	else:
		raise ValueError(f"unsupported demo transcript provider: {provider}")

	discovered = {}
	for candidate in candidates:
		canonical = canonical_path_inside(root, candidate)
		if canonical is not None:
			discovered[canonical] = candidate
	return discovered


def is_seeder_owned_jsonl(path: Path) -> bool:
	try:
		with path.open("r", encoding="utf-8") as source:
			first_line = next((line for line in source if line.strip()), "")
		first_record = json.loads(first_line)
	except (OSError, UnicodeError, json.JSONDecodeError):
		return False
	return (
		isinstance(first_record, dict)
		and first_record.get(DEMO_JSONL_OWNER_FIELD) == DEMO_JSONL_OWNER_VALUE
	)


def cleanup_owned_model_jsonls() -> None:
	"""Remove only prior complete-mode fixtures, never unrelated transcripts."""
	if not MODEL_FIXTURE_MODE or CODEX_SESSIONS_DIR is None:
		return
	for provider, root, production_root in (
		("claude", PROJECTS_DIR, DEFAULT_PROJECTS_DIR),
		("codex", CODEX_SESSIONS_DIR, DEFAULT_CODEX_SESSIONS_DIR),
	):
		ensure_configured_fixture_root(provider, root)
		if paths_overlap(root, production_root):
			raise ValueError(f"refusing to clean marker-owned files in {provider} production paths")
		for candidate in discover_runtime_model_jsonls(provider, root).values():
			if is_seeder_owned_jsonl(candidate):
				candidate.unlink()


def rust_windows_canonical_path_text(canonical_path: str) -> str:
	"""Restore the verbatim path form returned by Rust canonicalize on Windows."""
	extended_prefix = "\\\\?\\"
	unc_prefix = "\\\\?\\UNC\\"
	if canonical_path.startswith(extended_prefix):
		return canonical_path
	if canonical_path.startswith("\\\\"):
		return unc_prefix + canonical_path[2:]
	drive, tail = ntpath.splitdrive(canonical_path)
	if len(drive) == 2 and drive[1] == ":" and tail.startswith("\\"):
		return extended_prefix + canonical_path
	raise ValueError("Windows canonical model source path is not absolute")


def rust_windows_source_key_hex(canonical_path: str) -> str:
	# sessions.rs iterates OsStr::encode_wide(), then hex-encodes each UTF-16
	# code unit's big-endian bytes. utf-16-be produces the identical byte stream.
	return rust_windows_canonical_path_text(canonical_path).encode(
		"utf-16-be",
		"surrogatepass",
	).hex()


def canonical_model_source_key(source_root_key: str, path: Path) -> str:
	canonical = path.resolve(strict=True)
	if os.name == "nt":
		return f"{source_root_key}:fs-windows:{rust_windows_source_key_hex(str(canonical))}"
	return f"{source_root_key}:fs-unix:{os.fsencode(canonical).hex()}"


def model_fixture_source(
	*,
	provider: str,
	source_root_key: str,
	path: Path,
	source_session_id: str,
	analytics_session_id: str,
	chain_id: str,
	parent_chain_id: str | None,
	agent_id: str | None,
	is_sidechain: bool,
	cwd: str,
	observations: list[dict],
	first_activity_at_ms: int | None = None,
) -> dict:
	activity = [observation["observed_at_ms"] for observation in observations]
	return {
		"provider": provider,
		"source_root_key": source_root_key,
		"path": path,
		"source_session_id": source_session_id,
		"analytics_session_id": analytics_session_id,
		"chain_id": chain_id,
		"parent_chain_id": parent_chain_id,
		"agent_id": agent_id,
		"is_sidechain": is_sidechain,
		"cwd": cwd,
		"hostname": DEMO_MODEL_HOSTNAME,
		"first_activity_at_ms": first_activity_at_ms if first_activity_at_ms is not None else min(activity),
		"last_activity_at_ms": max(activity),
		"observations": observations,
	}


def claude_fixture_observation(
	source_ordinal: int,
	record: dict,
	analytics_session_id: str,
	cwd: str,
) -> dict:
	message = record["message"]
	usage = message.get("usage", {})
	is_sidechain = record.get("isSidechain") is True
	agent_id = record.get("agentId") if is_sidechain else None
	source_session_id = record["sessionId"]
	model = message.get("model")
	return {
		"source_record_key": f"v1:claude_assistant:{source_ordinal}:0",
		"source_ordinal": source_ordinal,
		"observation_kind": "turn",
		"source_session_id": source_session_id,
		"analytics_session_id": analytics_session_id,
		"chain_id": agent_id if is_sidechain else source_session_id,
		"parent_chain_id": source_session_id if is_sidechain else None,
		"agent_id": agent_id,
		"turn_id": record.get("uuid"),
		"raw_model_id": model if isinstance(model, str) else None,
		"cwd": cwd,
		"hostname": DEMO_MODEL_HOSTNAME,
		"is_sidechain": is_sidechain,
		"observed_at_ms": datetime_to_ms(datetime.fromisoformat(record["timestamp"])),
		"input_tokens": usage.get("input_tokens"),
		"output_tokens": usage.get("output_tokens"),
		"cache_creation_tokens": usage.get("cache_creation_input_tokens"),
		"cache_read_tokens": usage.get("cache_read_input_tokens"),
		"model_evidence": "explicit" if isinstance(model, str) else "missing",
		"token_evidence": "direct" if usage else "unavailable",
	}


def write_claude_model_edge_fixtures() -> list[dict]:
	fixture_sources = []

	# One uncapped provider-qualified session. Raw IDs are generated data, not a
	# catalog: every valid string is retained exactly as written.
	scale_session_id = "demo-claude-model-scale-session"
	scale_cwd = PROJECTS[0]
	scale_project = PROJECTS_DIR / scale_cwd.replace("/", "-").lstrip("-")
	scale_path = scale_project / f"{scale_session_id}.jsonl"
	scale_records = []
	scale_observations = []
	scale_start = NOW - timedelta(minutes=25)
	for model_index in range(1001):
		observed_at = scale_start + timedelta(seconds=model_index)
		record = {
			"type": "assistant",
			"uuid": f"demo-scale-turn-{model_index:04d}",
			"sessionId": scale_session_id,
			"timestamp": ts_tz(observed_at),
			"cwd": scale_cwd,
			"gitBranch": "main",
			"message": {
				"role": "assistant",
				"model": f"demo/generated/model-{model_index:04d}",
				"usage": {
					"input_tokens": 2 + model_index % 7,
					"output_tokens": 1 + model_index % 5,
					"cache_creation_input_tokens": model_index % 3,
					"cache_read_input_tokens": model_index % 11,
				},
				"content": [{"type": "text", "text": "Generated model identity fixture."}],
			},
		}
		scale_records.append(record)
		scale_observations.append(claude_fixture_observation(
			model_index,
			record,
			scale_session_id,
			scale_cwd,
		))
	write_jsonl_fixture("claude", scale_path, scale_records)
	fixture_sources.append(model_fixture_source(
		provider="claude",
		source_root_key="claude:projects",
		path=scale_path,
		source_session_id=scale_session_id,
		analytics_session_id=scale_session_id,
		chain_id=scale_session_id,
		parent_chain_id=None,
		agent_id=None,
		is_sidechain=False,
		cwd=scale_cwd,
		observations=scale_observations,
	))

	# Interleaved parent/subagent timestamps include dynamic IDs, a missing-model
	# token turn, and a consecutive repeated model within each chain.
	chain_session_id = "demo-claude-chain-session"
	chain_agent_id = "demo-claude-agent-alpha"
	chain_cwd = PROJECTS[1]
	chain_project = PROJECTS_DIR / chain_cwd.replace("/", "-").lstrip("-")
	parent_path = chain_project / f"{chain_session_id}.jsonl"
	subagent_path = chain_project / chain_session_id / "subagents" / f"{chain_agent_id}.jsonl"

	def chain_record(offset_minutes: int, turn_id: str, model: str | None, sidechain: bool) -> dict:
		message = {
			"role": "assistant",
			"usage": {
				"input_tokens": 80,
				"output_tokens": 24,
				"cache_creation_input_tokens": 8,
				"cache_read_input_tokens": 32,
			},
			"content": [{"type": "text", "text": "Chain fixture turn."}],
		}
		if model is not None:
			message["model"] = model
		record = {
			"type": "assistant",
			"uuid": turn_id,
			"sessionId": chain_session_id,
			"timestamp": ts_tz(NOW - timedelta(minutes=offset_minutes)),
			"cwd": chain_cwd,
			"gitBranch": "main",
			"message": message,
		}
		if sidechain:
			record["isSidechain"] = True
			record["agentId"] = chain_agent_id
		return record

	parent_records = [
		chain_record(54, "demo-parent-turn-1", "demo/claude/parent-alpha", False),
		chain_record(50, "demo-parent-turn-2", None, False),
		chain_record(46, "demo-parent-turn-3", "demo/claude/parent-beta", False),
		chain_record(42, "demo-parent-turn-4", "demo/claude/parent-beta", False),
	]
	subagent_records = [
		chain_record(52, "demo-agent-turn-1", "demo/claude/agent-alpha", True),
		chain_record(48, "demo-agent-turn-2", "demo/claude/agent-alpha", True),
		chain_record(44, "demo-agent-turn-3", None, True),
		chain_record(40, "demo-agent-turn-4", "demo/claude/agent-beta", True),
	]
	for path, records, agent_id, is_sidechain in [
		(parent_path, parent_records, None, False),
		(subagent_path, subagent_records, chain_agent_id, True),
	]:
		write_jsonl_fixture("claude", path, records)
		observations = [
			claude_fixture_observation(index, record, chain_session_id, chain_cwd)
			for index, record in enumerate(records)
		]
		fixture_sources.append(model_fixture_source(
			provider="claude",
			source_root_key="claude:projects",
			path=path,
			source_session_id=chain_session_id,
			analytics_session_id=chain_session_id,
			chain_id=agent_id if is_sidechain else chain_session_id,
			parent_chain_id=chain_session_id if is_sidechain else None,
			agent_id=agent_id,
			is_sidechain=is_sidechain,
			cwd=chain_cwd,
			observations=observations,
		))

	return fixture_sources


def codex_turn_observation(
	source_ordinal: int,
	observed_at: datetime,
	source_session_id: str,
	analytics_session_id: str,
	parent_chain_id: str | None,
	cwd: str,
	turn_id: str,
	model: str | None,
) -> dict:
	return {
		"source_record_key": f"v1:codex_turn_context:{source_ordinal}:0",
		"source_ordinal": source_ordinal,
		"observation_kind": "turn",
		"source_session_id": source_session_id,
		"analytics_session_id": analytics_session_id,
		"chain_id": source_session_id,
		"parent_chain_id": parent_chain_id,
		"agent_id": None,
		"turn_id": turn_id,
		"raw_model_id": model,
		"cwd": cwd,
		"hostname": DEMO_MODEL_HOSTNAME,
		"is_sidechain": parent_chain_id is not None,
		"observed_at_ms": datetime_to_ms(observed_at),
		"input_tokens": None,
		"output_tokens": None,
		"cache_creation_tokens": None,
		"cache_read_tokens": None,
		"model_evidence": "explicit" if model is not None else "missing",
		"token_evidence": "unavailable",
	}


def codex_token_observation(
	source_ordinal: int,
	observed_at: datetime,
	source_session_id: str,
	analytics_session_id: str,
	parent_chain_id: str | None,
	cwd: str,
	deltas: tuple[int, int, int, int],
) -> dict:
	return {
		"source_record_key": f"v1:codex_token_count:{source_ordinal}:0",
		"source_ordinal": source_ordinal,
		"observation_kind": "token",
		"source_session_id": source_session_id,
		"analytics_session_id": analytics_session_id,
		"chain_id": source_session_id,
		"parent_chain_id": parent_chain_id,
		"agent_id": None,
		"turn_id": None,
		"raw_model_id": None,
		"cwd": cwd,
		"hostname": DEMO_MODEL_HOSTNAME,
		"is_sidechain": parent_chain_id is not None,
		"observed_at_ms": datetime_to_ms(observed_at),
		"input_tokens": deltas[0],
		"output_tokens": deltas[1],
		"cache_creation_tokens": deltas[2],
		"cache_read_tokens": deltas[3],
		"model_evidence": "missing",
		"token_evidence": "cumulative_delta",
	}


def populate_codex_session_jsonls() -> list[dict]:
	if not MODEL_FIXTURE_MODE or CODEX_SESSIONS_DIR is None:
		return []

	root_session_id = "demo-codex-chain-root"
	child_session_id = "demo-codex-chain-child"
	cwd = PROJECTS[2]
	day_dir = CODEX_SESSIONS_DIR / NOW.strftime("%Y/%m/%d")
	fixture_sources = []

	for session_id, parent_id, minute_offset in [
		(root_session_id, None, 38),
		(child_session_id, root_session_id, 37),
	]:
		start = NOW - timedelta(minutes=minute_offset)
		path = day_dir / f"rollout-{session_id}.jsonl"
		session_payload = {"id": session_id, "cwd": cwd}
		if parent_id is not None:
			session_payload["parent_thread_id"] = parent_id
		records = [{"timestamp": ts_tz(start), "type": "session_meta", "payload": session_payload}]
		observations = []
		cumulative = [0, 0, 0, 0]
		models = [
			f"demo/codex/dynamic-{0 if parent_id is None else 2}",
			None,
			f"demo/codex/dynamic-{1 if parent_id is None else 3}",
			f"demo/codex/dynamic-{1 if parent_id is None else 3}",
		]
		for turn_index, model in enumerate(models):
			turn_time = start + timedelta(minutes=turn_index * 2 + 1)
			turn_payload = {"turn_id": f"{session_id}-turn-{turn_index}"}
			if model is not None:
				turn_payload["model"] = model
			records.append({"timestamp": ts_tz(turn_time), "type": "turn_context", "payload": turn_payload})
			observations.append(codex_turn_observation(
				len(records) - 1,
				turn_time,
				session_id,
				root_session_id,
				parent_id,
				cwd,
				turn_payload["turn_id"],
				model,
			))

			delta = (55 + turn_index * 3, 18 + turn_index, 4 + turn_index, 20 + turn_index * 2)
			for dimension, value in enumerate(delta):
				cumulative[dimension] += value
			token_time = turn_time + timedelta(seconds=20)
			total_usage = {
				"input_tokens": cumulative[0] + cumulative[3],
				"output_tokens": cumulative[1],
				"cache_creation_tokens": cumulative[2],
				"cached_input_tokens": cumulative[3],
			}
			records.append({
				"timestamp": ts_tz(token_time),
				"type": "event_msg",
				"payload": {
					"type": "token_count",
					"info": {"total_token_usage": total_usage},
				},
			})
			observations.append(codex_token_observation(
				len(records) - 1,
				token_time,
				session_id,
				root_session_id,
				parent_id,
				cwd,
				delta,
			))

		write_jsonl_fixture("codex", path, records)
		fixture_sources.append(model_fixture_source(
			provider="codex",
			source_root_key="codex:sessions",
			path=path,
			source_session_id=session_id,
			analytics_session_id=root_session_id,
			chain_id=session_id,
			parent_chain_id=parent_id,
			agent_id=None,
			is_sidechain=parent_id is not None,
			cwd=cwd,
			observations=observations,
			first_activity_at_ms=datetime_to_ms(start),
		))

	print(f"  codex_session_jsonls: {len(fixture_sources)} files in {CODEX_SESSIONS_DIR}")
	return fixture_sources


def populate_model_analytics(conn: sqlite3.Connection, fixture_sources: list[dict]) -> None:
	state_updated_at_ms = datetime_to_ms(datetime.now(timezone.utc))
	if not MODEL_FIXTURE_MODE:
		conn.execute(
			"INSERT INTO model_backfill_state ("
			"id, generation, trigger, status, total_roots, completed_roots, failed_roots, "
			"inventory_complete, total_sources, source_total_published, processed_sources, "
			"failed_sources, skipped_sources, remaining_sources, observations_written, "
			"started_at_ms, updated_at_ms, finished_at_ms, last_error"
			") VALUES (1, 0, 'migration', 'pending', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "
			"NULL, ?, NULL, NULL)",
			(state_updated_at_ms,),
		)
		print("  model_analytics: pending (isolated Claude + Codex paths not supplied)")
		return

	configured_roots = {
		"claude": PROJECTS_DIR.resolve(strict=True),
		"codex": CODEX_SESSIONS_DIR.resolve(strict=True) if CODEX_SESSIONS_DIR else None,
	}
	expected_paths = {"claude": set(), "codex": set()}
	for source in fixture_sources:
		path = source["path"].resolve(strict=True)
		root = configured_roots[source["provider"]]
		if root is None or canonical_path_inside(root, path) is None:
			raise ValueError(f"model fixture source escaped configured {source['provider']} root")
		expected_paths[source["provider"]].add(path)
	discovered_paths = {
		provider: set(discover_runtime_model_jsonls(provider, root))
		for provider, root in configured_roots.items()
		if root is not None
	}
	completed_roots = sum(
		expected_paths[provider] == discovered_paths.get(provider, set())
		for provider in expected_paths
	)
	inventory_matches = completed_roots == len(expected_paths)
	backfill_started_at_ms = datetime_to_ms(datetime.now(timezone.utc))
	total_observations = 0
	for source in fixture_sources:
		path = source["path"].resolve(strict=True)
		contents = path.read_bytes()
		stat = path.stat()
		source_key = canonical_model_source_key(source["source_root_key"], path)
		content_sha256 = hashlib.sha256(contents).hexdigest()
		observation_count = len(source["observations"])
		source_success_at_ms = datetime_to_ms(datetime.now(timezone.utc))
		conn.execute(
			"INSERT INTO model_observation_sources ("
			"provider, source_key, source_root_key, source_path, source_session_id, "
			"analytics_session_id, chain_id, parent_chain_id, agent_id, is_sidechain, "
			"cwd, hostname, first_activity_at_ms, last_activity_at_ms, mtime_ns, size_bytes, "
			"content_sha256, last_error, suppressed_sha256, suppressed_at_ms, seen_generation, "
			"processing_status, observation_count, last_attempt_at_ms, last_success_at_ms"
			") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, "
			"NULL, 1, 'ok', ?, ?, ?)",
			(
				source["provider"], source_key, source["source_root_key"], str(path),
				source["source_session_id"], source["analytics_session_id"], source["chain_id"],
				source["parent_chain_id"], source["agent_id"], int(source["is_sidechain"]),
				source["cwd"], source["hostname"], source["first_activity_at_ms"],
				source["last_activity_at_ms"], stat.st_mtime_ns, stat.st_size, content_sha256,
				observation_count, source_success_at_ms, source_success_at_ms,
			),
		)
		for observation in source["observations"]:
			conn.execute(
				"INSERT INTO model_usage_observations ("
				"provider, source_key, source_record_key, source_ordinal, observation_kind, "
				"source_session_id, analytics_session_id, chain_id, parent_chain_id, agent_id, "
				"turn_id, raw_model_id, cwd, hostname, is_sidechain, observed_at_ms, input_tokens, "
				"output_tokens, cache_creation_tokens, cache_read_tokens, model_evidence, token_evidence"
				") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
				(
					source["provider"], source_key, observation["source_record_key"],
					observation["source_ordinal"], observation["observation_kind"],
					observation["source_session_id"], observation["analytics_session_id"],
					observation["chain_id"], observation["parent_chain_id"], observation["agent_id"],
					observation["turn_id"], observation["raw_model_id"], observation["cwd"],
					observation["hostname"], int(observation["is_sidechain"]),
					observation["observed_at_ms"], observation["input_tokens"],
					observation["output_tokens"], observation["cache_creation_tokens"],
					observation["cache_read_tokens"], observation["model_evidence"],
					observation["token_evidence"],
				),
			)
		total_observations += observation_count

	backfill_finished_at_ms = datetime_to_ms(datetime.now(timezone.utc))
	if inventory_matches:
		conn.execute(
			"INSERT INTO model_backfill_state ("
			"id, generation, trigger, status, total_roots, completed_roots, failed_roots, "
			"inventory_complete, total_sources, source_total_published, processed_sources, "
			"failed_sources, skipped_sources, remaining_sources, observations_written, "
			"started_at_ms, updated_at_ms, finished_at_ms, last_error"
			") VALUES (1, 1, 'reconcile', 'complete', 2, 2, 0, 1, ?, 1, ?, 0, 0, 0, ?, ?, ?, ?, NULL)",
			(
				len(fixture_sources), len(fixture_sources), total_observations,
				backfill_started_at_ms, backfill_finished_at_ms, backfill_finished_at_ms,
			),
		)
	else:
		# Runtime backfill owns unmarked/pre-existing sources. Keep this generation
		# pending instead of claiming a complete inventory the seeder did not write.
		conn.execute(
			"INSERT INTO model_backfill_state ("
			"id, generation, trigger, status, total_roots, completed_roots, failed_roots, "
			"inventory_complete, total_sources, source_total_published, processed_sources, "
			"failed_sources, skipped_sources, remaining_sources, observations_written, "
			"started_at_ms, updated_at_ms, finished_at_ms, last_error"
			") VALUES (1, 1, 'reconcile', 'pending', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "
			"NULL, ?, NULL, NULL)",
			(backfill_finished_at_ms,),
		)
	conn.execute(
		"INSERT OR REPLACE INTO settings (key, value) VALUES ('model_analytics.data_revision.v1', '1')"
	)
	if inventory_matches:
		print(
			f"  model_analytics: {total_observations} observations from "
			f"{len(fixture_sources)} sources; 2/2 roots complete"
		)
	else:
		discovered_count = sum(len(paths) for paths in discovered_paths.values())
		print(
			f"  model_analytics: pending runtime reconciliation; seeded "
			f"{len(fixture_sources)} of {discovered_count} retained sources"
		)


def restore_pending_model_state_after_failure() -> None:
	"""Best-effort singleton recovery without replacing the original failure."""
	recovery = None
	try:
		recovery = sqlite3.connect(str(DB_PATH))
		recovery.execute("BEGIN IMMEDIATE")
		recovery.execute(
			"INSERT INTO model_backfill_state ("
			"id, generation, trigger, status, total_roots, completed_roots, failed_roots, "
			"inventory_complete, total_sources, source_total_published, processed_sources, "
			"failed_sources, skipped_sources, remaining_sources, observations_written, "
			"started_at_ms, updated_at_ms, finished_at_ms, last_error"
			") VALUES (1, 0, 'migration', 'pending', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "
			"NULL, ?, NULL, NULL) "
			"ON CONFLICT(id) DO UPDATE SET "
			"generation=excluded.generation, trigger=excluded.trigger, status=excluded.status, "
			"total_roots=0, completed_roots=0, failed_roots=0, inventory_complete=0, "
			"total_sources=0, source_total_published=0, processed_sources=0, failed_sources=0, "
			"skipped_sources=0, remaining_sources=0, observations_written=0, started_at_ms=NULL, "
			"updated_at_ms=excluded.updated_at_ms, finished_at_ms=NULL, last_error=NULL",
			(datetime_to_ms(datetime.now(timezone.utc)),),
		)
		recovery.commit()
	except Exception as recovery_error:
		if recovery is not None:
			try:
				recovery.rollback()
			except Exception:
				pass
		try:
			print(
				f"WARNING: could not restore pending model backfill state: {recovery_error}",
				file=sys.stderr,
			)
		except Exception:
			pass
	finally:
		if recovery is not None:
			try:
				recovery.close()
			except Exception:
				pass


# ── 19. Claude session JSONL files ────────────────────────────────────────────

def populate_session_jsonls() -> list[dict]:
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
		return []

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

	ensure_configured_fixture_root("claude", PROJECTS_DIR)

	total_messages = 0
	total_files = 0
	fixture_sources = []
	for project_index, project_path in enumerate(PROJECTS):
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
			observations = []
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

				assistant_record = {
					"type": "assistant",
					"uuid": str(uuid.UUID(int=random.getrandbits(128))),
					"sessionId": session_id,
					"timestamp": ts_tz(turn_time + timedelta(seconds=random.randint(2, 30))),
					"cwd": project_path,
					"gitBranch": branch,
					"message": {
						"role": "assistant",
						"model": f"demo-claude-opaque-{project_index}-{session_idx}-{turn % 3}",
						"usage": {
							"input_tokens": 120 + turn * 7,
							"output_tokens": 36 + turn * 3,
							"cache_creation_input_tokens": 12 + turn,
							"cache_read_input_tokens": 48 + turn * 2,
						},
						"content": assistant_blocks,
					},
				}
				lines.append(json.dumps(assistant_record))
				observations.append(claude_fixture_observation(
					len(lines) - 1,
					assistant_record,
					session_id,
					project_path,
				))

			write_jsonl_fixture("claude", session_file, lines)
			fixture_sources.append(model_fixture_source(
				provider="claude",
				source_root_key="claude:projects",
				path=session_file,
				source_session_id=session_id,
				analytics_session_id=session_id,
				chain_id=session_id,
				parent_chain_id=None,
				agent_id=None,
				is_sidechain=False,
				cwd=project_path,
				observations=observations,
			))
			total_files += 1
			total_messages += len(lines)

	if MODEL_FIXTURE_MODE:
		edge_sources = write_claude_model_edge_fixtures()
		fixture_sources.extend(edge_sources)
		total_files += len(edge_sources)
		total_messages += sum(len(source["observations"]) for source in edge_sources)
	print(f"  session_jsonls: {total_files} files, {total_messages} messages in {PROJECTS_DIR}")
	return fixture_sources


# ── Main ──────────────────────────────────────────────────────────────────────

def paths_overlap(left: Path, right: Path) -> bool:
	left = left.expanduser().resolve()
	right = right.expanduser().resolve()
	return left == right or left in right.parents or right in left.parents

def parse_args() -> argparse.Namespace:
	parser = argparse.ArgumentParser(
		description="Seed Quill's SQLite DB with reproducible dummy data.",
	)
	parser.add_argument(
		"--data-dir", type=Path, default=None,
		help="Directory to write usage.db into. Default: platform app_data_dir for Quill.",
	)
	parser.add_argument(
		"--bin", type=Path, default=None,
		help="Quill executable used to initialize and migrate usage.db.",
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
		"--codex-sessions-dir", type=Path, default=None,
		help="Isolated directory to write fictional Codex session JSONL files into.",
	)
	parser.add_argument(
		"--home-dir", type=Path, default=None,
		help=(
			"Isolated HOME to write per-project memory documents into "
			"(<home>/.claude/projects/<slug>/memory/). Omit to skip them — the "
			"app resolves memory files from the real home directory."
		),
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
	args = parser.parse_args()
	if (
		args.data_dir is not None
		and args.projects_dir is not None
		and args.codex_sessions_dir is not None
		and not args.no_projects
	):
		for configured, production, label in (
			(args.projects_dir, DEFAULT_PROJECTS_DIR, "Claude projects"),
			(args.codex_sessions_dir, DEFAULT_CODEX_SESSIONS_DIR, "Codex sessions"),
		):
			if paths_overlap(configured, production):
				parser.error(
					f"isolated model fixtures cannot overlap the production {label} path"
				)
	return args


def main() -> None:
	global DB_PATH, BAK_PATH, LEARNED_DIR, PROJECTS_DIR, CODEX_SESSIONS_DIR
	global QUIET, NO_BACKUP, USING_OVERRIDE, SKIP_PROJECTS, MODEL_FIXTURE_MODE
	global MEMORY_HOME

	args = parse_args()

	QUIET = args.quiet
	NO_BACKUP = args.no_backup
	USING_OVERRIDE = args.data_dir is not None
	SKIP_PROJECTS = args.no_projects
	MODEL_FIXTURE_MODE = (
		USING_OVERRIDE
		and args.projects_dir is not None
		and args.codex_sessions_dir is not None
		and not SKIP_PROJECTS
	)

	data_dir = args.data_dir if args.data_dir is not None else DEFAULT_DATA_DIR
	rules_dir = args.rules_dir if args.rules_dir is not None else DEFAULT_RULES_DIR
	projects_dir = args.projects_dir if args.projects_dir is not None else DEFAULT_PROJECTS_DIR

	DB_PATH = data_dir / "usage.db"
	BAK_PATH = DB_PATH.with_suffix(".db.bak")
	LEARNED_DIR = rules_dir
	PROJECTS_DIR = projects_dir
	CODEX_SESSIONS_DIR = args.codex_sessions_dir
	MEMORY_HOME = args.home_dir
	quill_bin = resolve_quill_bin(args.bin)

	random.seed(args.seed)

	log(f"\nQuill Dummy Data Seeder")
	log(f"DB path:    {DB_PATH}")
	log(f"Quill bin:  {quill_bin}")
	log(f"Rules path: {LEARNED_DIR}")
	if CODEX_SESSIONS_DIR is not None:
		log(f"Codex path: {CODEX_SESSIONS_DIR}")
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
	log("Step 2: Initializing schema through Quill migrations...")
	initialize_database(quill_bin)
	log()

	conn = sqlite3.connect(str(DB_PATH))
	conn.execute("PRAGMA foreign_keys = OFF")

	try:
		log("Step 3: Clearing existing data...")
		clear_tables(conn)
		log()

		log("Step 4: Populating tables...")
		populate_usage_snapshots(conn)
		populate_usage_hourly(conn)
		session_windows = populate_token_snapshots(conn)
		populate_token_hourly(conn)
		populate_settings(conn)
		populate_observations(conn)
		populate_learning_runs(conn)
		populate_learned_rules(conn)
		populate_observation_summaries(conn)
		populate_tool_actions(conn)
		populate_memory_files(conn)
		populate_memory_markdown()
		populate_optimization(conn)
		populate_git_snapshots(conn)
		populate_response_times(conn, session_windows)
		populate_context_savings_events(conn)
		populate_session_events(conn)
		log()

		conn.commit()
		log("Done. Core analytics tables populated.")
	finally:
		conn.execute("PRAGMA foreign_keys = ON")
		conn.close()

	try:
		# Session JSONL files live outside SQLite. Every failure after the core DB
		# commit must restore migration-safe pending model state before bubbling.
		cleanup_owned_model_jsonls()
		fixture_sources = populate_session_jsonls()
		fixture_sources.extend(populate_codex_session_jsonls())

		# Model tables depend on exact retained-file fingerprints and canonical
		# source keys, so populate them only after both provider fixtures are on disk.
		model_conn = sqlite3.connect(str(DB_PATH))
		try:
			populate_model_analytics(model_conn, fixture_sources)
			model_conn.commit()
		except Exception:
			model_conn.rollback()
			raise
		finally:
			model_conn.close()
	except Exception:
		restore_pending_model_state_after_failure()
		raise

	# Final summary always prints (not gated by --quiet) so the maintainer always sees it.
	print()
	print("─" * 60)
	if USING_OVERRIDE:
		print(f"Sandbox seeded:")
		print(f"  data:     {DB_PATH}")
		print(f"  rules:    {LEARNED_DIR}")
		if not SKIP_PROJECTS:
			print(f"  projects: {PROJECTS_DIR}")
			if CODEX_SESSIONS_DIR is not None:
				print(f"  codex:    {CODEX_SESSIONS_DIR}")
		if MEMORY_HOME is not None:
			print(f"  home:     {MEMORY_HOME}")
	elif BAK_PATH.exists():
		print("To restore the original DB, STOP QUILL FIRST then run:")
		print(f"  pkill -f quill; sleep 1; cp {BAK_PATH} {DB_PATH}")
	else:
		print("No backup was created (DB did not exist before seeding).")
	print("─" * 60)


if __name__ == "__main__":
	main()
