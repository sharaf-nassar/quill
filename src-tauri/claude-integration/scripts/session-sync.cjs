#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");
const os = require("os");
const crypto = require("crypto");
const https = require("https");
const http = require("http");
const LOCAL_TIMEOUT_MS = 1500;
const REMOTE_TIMEOUT_MS = 2000;
// Must match MAX_MESSAGES_PER_REQUEST in src-tauri/src/server.rs.
const MAX_MESSAGES_PER_REQUEST = 500;
// Must match MAX_STRING_LEN in src-tauri/src/server.rs (byte length).
const MAX_STRING_LEN = 256;
// Must match MAX_CWD_LEN in src-tauri/src/server.rs (byte length).
const MAX_CWD_LEN = 4096;
// Must match MAX_CONTENT_LEN in src-tauri/src/server.rs (byte length).
const MAX_CONTENT_LEN = 1000000;
const CONTENT_TRUNCATION_MARKER = "\n[quill: content truncated]";
const UNKNOWN_PROJECT = "unknown-project";
const UNKNOWN_HOST = "unknown-host";
const MAX_ERROR_DETAIL_LEN = 512;
const SESSION_LEASE_STALE_MS = 10000;
const REMOTE_ASSISTANT_TOOL_USE_TYPE = "assistant_tool_use";
const LEASE_OWNER_FILE = "owner";
const LEASE_HEARTBEAT_FILE = "lease";
const LEASE_CANDIDATE_PREFIX = "owner-";
// Rejections produced by validate_session_messages_payload in
// src-tauri/src/server.rs before it inspects any individual message. These
// describe the request envelope, so no amount of bisecting can isolate a
// culprit row and dropping records would only destroy good data.
const ENVELOPE_REJECTIONS = [
  "Invalid session_id",
  "Invalid host",
  "Invalid project",
  "Invalid cwd",
  "No messages provided",
  "Too many messages",
];
// server.rs answers a failed payload validation, and only a failed payload
// validation, with 400. Every other permanent status is a transport or
// deployment problem (wrong URL, expired secret, a proxy in the way) that says
// nothing about the records, so records are never dropped for one.
const ROW_REJECTION_STATUS = 400;

// Hook stderr is diagnostic only: it is surfaced to the user by Claude Code but
// never changes the hook's exit status, so logging here cannot break a session.
function logSync(message) {
  try {
    console.error(`session-sync: ${message}`);
  } catch (_) {
    // A closed or broken stderr must never abort a hook.
  }
}

function logSyncDebug(message) {
  if (!process.env.QUILL_DEBUG) return;
  logSync(message);
}

function describeError(err) {
  if (err instanceof Error && typeof err.message === "string") return err.message;
  try {
    return String(err);
  } catch (_) {
    return "unknown error";
  }
}

function isLocal(urlStr) {
  return urlStr.includes("localhost") || urlStr.includes("127.0.0.1") || urlStr.includes("[::1]");
}

// 4xx means the server will reject an identical retry forever. 408 and 429 are
// the two exceptions: both are explicit invitations to send the same bytes
// again later.
function isPermanentStatus(status) {
  return status >= 400 && status < 500 && status !== 408 && status !== 429;
}

class SyncRequestError extends Error {
  constructor(status, detail, permanent) {
    super(detail ? `server returned ${status}: ${detail}` : `server returned ${status}`);
    this.name = "SyncRequestError";
    this.status = status;
    this.detail = detail;
    this.permanent = permanent;
  }
}

function isEnvelopeRejection(detail) {
  if (typeof detail !== "string" || detail.length === 0) return false;
  return ENVELOPE_REJECTIONS.some((known) => detail.includes(known));
}

function postJSON(config, endpoint, payload) {
  const body = JSON.stringify(payload);
  const url = new URL(`${config.url}${endpoint}`);
  const mod = url.protocol === "https:" ? https : http;
  const timeoutMs = isLocal(config.url) ? LOCAL_TIMEOUT_MS : REMOTE_TIMEOUT_MS;

  return new Promise((resolve, reject) => {
    let settled = false;
    let timer;
    const settle = (callback, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      callback(value);
    };

    const req = mod.request(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${config.secret}`,
        "Content-Length": Buffer.byteLength(body),
      },
    }, (res) => {
      const status = res.statusCode || 0;
      if (status >= 200 && status < 300) {
        res.resume();
        settle(resolve, status);
        return;
      }
      // The server answers a rejected batch with the exact validation message.
      // Capturing a bounded prefix of it is the only way callers can tell an
      // envelope problem apart from a single poisoned row.
      const permanent = isPermanentStatus(status);
      let detail = "";
      res.setEncoding("utf8");
      res.on("data", (piece) => {
        if (detail.length < MAX_ERROR_DETAIL_LEN) detail += piece;
      });
      const rejectWithDetail = () => settle(
        reject,
        new SyncRequestError(status, detail.slice(0, MAX_ERROR_DETAIL_LEN).trim(), permanent),
      );
      res.on("error", rejectWithDetail);
      res.on("aborted", rejectWithDetail);
      res.on("end", rejectWithDetail);
    });

    req.on("error", (err) => settle(reject, err));
    timer = setTimeout(() => {
      req.destroy(new Error(`timed out after ${timeoutMs}ms`));
    }, timeoutMs);
    timer.unref?.();
    req.end(body);
  });
}

function daysInMonth(year, month) {
  const lengths = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  if (month !== 2) return lengths[month - 1];
  const isLeapYear = (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
  return isLeapYear ? 29 : 28;
}

// Mirrors chrono's parse_from_rfc3339, which src-tauri/src/server.rs runs over
// every message timestamp: a `T`, `t` or space separator, an optional
// fractional part, a mandatory `Z`/`z` or `+HH:MM` offset, and real calendar
// dates. Leap seconds (`:60`) are accepted, as chrono accepts them.
const RFC3339_PATTERN =
  /^(\d{4})-(\d{2})-(\d{2})[Tt ](\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:[Zz]|[+-](\d{2}):(\d{2}))$/;

function isRfc3339(value) {
  const match = RFC3339_PATTERN.exec(value);
  if (match === null) return false;
  const [, year, month, day, hour, minute, second, offsetHour, offsetMinute] = match;
  const monthValue = Number(month);
  if (monthValue < 1 || monthValue > 12) return false;
  const dayValue = Number(day);
  if (dayValue < 1 || dayValue > daysInMonth(Number(year), monthValue)) return false;
  if (Number(hour) > 23 || Number(minute) > 59 || Number(second) > 60) return false;
  if (offsetHour !== undefined && (Number(offsetHour) > 23 || Number(offsetMinute) > 59)) {
    return false;
  }
  return true;
}

// Returns a timestamp the server will accept, repairing near-misses through
// Date rather than surrendering the record. Returns null only when the value
// carries no recoverable instant at all.
function normalizeTimestamp(raw) {
  const value = typeof raw === "string" ? raw.trim() : "";
  if (value.length > 0 && value.length <= MAX_STRING_LEN && isRfc3339(value)) return value;
  const repaired = new Date(value);
  const repairedMs = repaired.getTime();
  if (!Number.isFinite(repairedMs)) return null;
  return repaired.toISOString();
}

// server.rs rejects the whole batch when any content exceeds MAX_CONTENT_LEN
// bytes, so clamp on a UTF-8 boundary rather than shipping a doomed request.
function clampContent(text) {
  const source = typeof text === "string" ? text : "";
  if (Buffer.byteLength(source, "utf8") <= MAX_CONTENT_LEN) return source;
  const buffer = Buffer.from(source, "utf8");
  let end = MAX_CONTENT_LEN - Buffer.byteLength(CONTENT_TRUNCATION_MARKER, "utf8");
  while (end > 0 && (buffer[end] & 0xc0) === 0x80) end -= 1;
  return `${buffer.subarray(0, end).toString("utf8")}${CONTENT_TRUNCATION_MARKER}`;
}

function fitsStringLimit(value) {
  return Buffer.byteLength(value, "utf8") <= MAX_STRING_LEN;
}

function buildMessage(
  entry,
  sourceTrackingKey,
  lineOrdinal,
  role,
  content,
  toolsUsed,
  hasToolUse,
  eventKinds,
  identity,
) {
  const uuid = typeof entry.uuid === "string" ? entry.uuid.trim() : "";
  const timestamp = normalizeTimestamp(entry.timestamp);
  if (timestamp === null) return null;
  const nativeUuid = uuid ? `claude:native:${uuid}` : "";
  const parentUuid = typeof entry.parentUuid === "string" && entry.parentUuid.trim()
    ? `claude:native:${entry.parentUuid.trim()}`
    : null;
  return {
    uuid: nativeUuid && fitsStringLimit(nativeUuid)
      ? nativeUuid
      : `claude:fallback:${sourceTrackingKey}:${lineOrdinal}`,
    type: role === "assistant" && hasToolUse
      ? REMOTE_ASSISTANT_TOOL_USE_TYPE
      : role,
    timestamp,
    content: clampContent(content),
    role,
    tools_used: toolsUsed,
    files_modified: [],
    event_kinds: eventKinds,
    ...(identity === null ? {} : {
      chain_id: identity.chainId,
      parent_chain_id: identity.parentChainId,
      agent_id: identity.agentId,
      is_sidechain: identity.isSidechain,
    }),
    parent_uuid: parentUuid !== null && fitsStringLimit(parentUuid) ? parentUuid : null,
  };
}

function nativeMessageIdentity(entry) {
  const rootSessionId = typeof entry.sessionId === "string"
    ? entry.sessionId.trim()
    : "";
  const agentId = typeof entry.agentId === "string"
    ? entry.agentId.trim()
    : "";
  if (entry.isSidechain === true) {
    if (!rootSessionId || !agentId) return { invalid: true };
    return {
      invalid: false,
      rootSessionId,
      chainId: agentId,
      parentChainId: rootSessionId,
      agentId,
      isSidechain: true,
    };
  }
  if (agentId) return { invalid: true };
  if (!rootSessionId) return null;
  return {
    invalid: false,
    rootSessionId,
    chainId: rootSessionId,
    parentChainId: null,
    agentId: null,
    isSidechain: false,
  };
}

function extractMessages(lines, sourceTrackingKey, firstLineOrdinal) {
  const pendingMessages = [];

  for (const [lineOffset, line] of lines.entries()) {
    let entry;
    try {
      entry = JSON.parse(line);
    } catch (_) {
      continue;
    }

    if (!entry.type || entry.type === "system" || entry.isMeta) continue;

    const role = entry.type === "human" || entry.type === "user"
      ? "user"
      : entry.type === "assistant"
        ? "assistant"
        : null;
    if (!role) continue;

    const content = entry.message?.content;
    const contentIsString = typeof content === "string";
    const blocks = Array.isArray(content) ? content : [];
    const textParts = contentIsString
      ? [content]
      : blocks
          .filter((block) => block.type === "text" && !block.isMeta)
          .map((block) => block.text)
          .filter((text) => typeof text === "string" && text.length > 0);
    const hasNonemptyText = contentIsString || textParts.some((text) => text.trim().length > 0);
    const hasToolResult = blocks.some((block) => block.type === "tool_result");
    const hasToolUse = blocks.some((block) => block.type === "tool_use");
    const hasThinking = blocks.some((block) => block.type === "thinking");
    const toolsUsed = blocks
      .filter((block) => block.type === "tool_use")
      .map((block) => block.name)
      .filter((name) => typeof name === "string" && name.length > 0);

    const isRuntimeRecord = role === "user"
      ? hasNonemptyText || hasToolResult
      : hasNonemptyText || hasToolUse || hasThinking;
    if (!isRuntimeRecord) continue;
    const eventKinds = role === "user"
      ? [
          ...(hasToolResult ? ["user_tool_result"] : []),
          ...(hasNonemptyText ? ["user_text"] : []),
        ]
      : [
          ...(hasThinking ? ["asst_thinking"] : []),
          ...(hasNonemptyText ? ["asst_text"] : []),
          ...(hasToolUse ? ["asst_tool_use"] : []),
        ];

    const identity = nativeMessageIdentity(entry);
    if (identity?.invalid) {
      // Everything already collected is unambiguous, so hand it back and stop
      // at this line instead of discarding a whole batch for one bad row.
      return {
        pendingMessages,
        invalidIdentityOrdinal: firstLineOrdinal + lineOffset,
      };
    }

    const message = buildMessage(
      entry,
      sourceTrackingKey,
      firstLineOrdinal + lineOffset,
      role,
      textParts.join("\n"),
      toolsUsed,
      hasToolUse,
      eventKinds,
      identity,
    );
    if (message === null) {
      // Never coerce the raw value: a JSON object carrying its own `toString`
      // key throws on conversion, and a diagnostic must not abort the run.
      const shownTimestamp = typeof entry.timestamp === "string"
        ? entry.timestamp.slice(0, 64)
        : `<${typeof entry.timestamp}>`;
      logSync(
        `dropping transcript line ${firstLineOrdinal + lineOffset}: `
        + `unusable timestamp ${shownTimestamp}`,
      );
      continue;
    }
    pendingMessages.push({
      message,
      sourceLineOrdinal: firstLineOrdinal + lineOffset,
      rootSessionId: identity?.rootSessionId ?? null,
      nativeIdentityKey: identity === null
        ? null
        : [
            identity.rootSessionId,
            identity.chainId,
            identity.parentChainId ?? "",
            identity.agentId ?? "",
            identity.isSidechain ? "sidechain" : "parent",
          ].join("\0"),
      nativeIsSidechain: identity?.isSidechain === true,
    });
  }

  return { pendingMessages, invalidIdentityOrdinal: null };
}

// Longest leading run of pending records that carries a single native identity
// and never mixes native sidechain rows with unattributed ones. Rows past the
// returned length cannot be attributed without guessing, so they are withheld
// rather than rewritten as parent activity.
function homogeneousIdentityPrefix(pendingMessages) {
  let identityKey = null;
  let sawSidechain = false;
  let sawUnattributed = false;

  for (const [index, pending] of pendingMessages.entries()) {
    const key = pending.nativeIdentityKey;
    if (key !== null && identityKey !== null && key !== identityKey) {
      return { length: index, reason: "batch spans more than one native identity" };
    }
    const nextSawSidechain = sawSidechain || pending.nativeIsSidechain === true;
    const nextSawUnattributed = sawUnattributed || key === null;
    if (nextSawSidechain && nextSawUnattributed) {
      return {
        length: index,
        reason: "batch mixes native sidechain rows with unattributed rows",
      };
    }
    identityKey = key === null ? identityKey : key;
    sawSidechain = nextSawSidechain;
    sawUnattributed = nextSawUnattributed;
  }

  return { length: pendingMessages.length, reason: null };
}

function newLeaseToken() {
  return crypto.randomBytes(16).toString("hex");
}

function ensureLeaseRoot(lockRoot) {
  try {
    fs.mkdirSync(lockRoot, { recursive: true, mode: 0o700 });
  } catch (err) {
    if (err.code !== "EEXIST") throw err;
  }
}

function readLeaseCandidate(candidatePath) {
  let directoryStats;
  try {
    directoryStats = fs.statSync(candidatePath);
  } catch (_) {
    return null;
  }

  let token = null;
  try {
    token = fs.readFileSync(path.join(candidatePath, LEASE_OWNER_FILE), "utf8");
  } catch (_) {
    // An interrupted creator ages from its owner-specific directory.
  }

  let updatedAtMs = directoryStats.mtimeMs;
  try {
    updatedAtMs = fs.statSync(
      path.join(candidatePath, LEASE_HEARTBEAT_FILE),
    ).mtimeMs;
  } catch (_) {
    // An interrupted creator may not have opened its heartbeat yet.
  }

  return {
    candidatePath,
    token,
    updatedAtMs,
    directoryDev: directoryStats.dev,
    directoryIno: directoryStats.ino,
  };
}

function isStaleLease(snapshot) {
  return snapshot !== null
    && Date.now() - snapshot.updatedAtMs > SESSION_LEASE_STALE_MS;
}

function sameLeaseCandidate(left, right) {
  return left !== null
    && right !== null
    && left.token === right.token
    && left.directoryDev === right.directoryDev
    && left.directoryIno === right.directoryIno;
}

function removeLeaseCandidate(expected) {
  try {
    const current = readLeaseCandidate(expected.candidatePath);
    if (sameLeaseCandidate(current, expected)) {
      fs.rmSync(expected.candidatePath, { recursive: true, force: true });
      return true;
    }
  } catch (_) {
    // Windows may retain a stale directory until its dead owner closes handles.
  }
  return false;
}

function listActiveLeaseCandidates(lockRoot) {
  ensureLeaseRoot(lockRoot);
  let entries;
  try {
    entries = fs.readdirSync(lockRoot, { withFileTypes: true });
  } catch (_) {
    return [];
  }

  const active = [];
  for (const entry of entries) {
    if (!entry.isDirectory() || !entry.name.startsWith(LEASE_CANDIDATE_PREFIX)) {
      continue;
    }
    const candidate = readLeaseCandidate(path.join(lockRoot, entry.name));
    if (candidate === null) continue;
    if (isStaleLease(candidate)) {
      const verified = readLeaseCandidate(candidate.candidatePath);
      if (sameLeaseCandidate(candidate, verified) && isStaleLease(verified)) {
        removeLeaseCandidate(verified);
      }
      continue;
    }
    active.push(candidate);
  }
  active.sort((left, right) => String(left.token).localeCompare(String(right.token)));
  return active;
}

function createLeaseCandidate(lockRoot) {
  ensureLeaseRoot(lockRoot);
  const token = newLeaseToken();
  const candidatePath = path.join(
    lockRoot,
    `${LEASE_CANDIDATE_PREFIX}${token}`,
  );
  let leaseFd;
  try {
    fs.mkdirSync(candidatePath, { mode: 0o700 });

    fs.writeFileSync(path.join(candidatePath, LEASE_OWNER_FILE), token, {
      encoding: "utf8",
      flag: "wx",
      mode: 0o600,
    });
    leaseFd = fs.openSync(
      path.join(candidatePath, LEASE_HEARTBEAT_FILE),
      "wx+",
      0o600,
    );
    const now = new Date();
    fs.futimesSync(leaseFd, now, now);
    const snapshot = readLeaseCandidate(candidatePath);
    if (snapshot === null) throw new Error("lease candidate disappeared");
    return { lockRoot, token, leaseFd, ...snapshot };
  } catch (err) {
    if (leaseFd !== undefined) {
      try { fs.closeSync(leaseFd); } catch (_) {}
    }
    try {
      fs.rmSync(candidatePath, { recursive: true, force: true });
    } catch (_) {
      // Candidate is owner-specific; leaving it cannot affect another owner.
    }
    throw err;
  }
}

function electLeaseCandidate(lease) {
  if (!renewSessionLease(lease)) return false;
  const active = listActiveLeaseCandidates(lease.lockRoot);
  return active.length === 1 && sameLeaseCandidate(active[0], lease);
}

function acquireSessionLease(lockRoot) {
  if (listActiveLeaseCandidates(lockRoot).length > 0) return null;
  const lease = createLeaseCandidate(lockRoot);
  if (electLeaseCandidate(lease)) return lease;
  releaseSessionLease(lease);
  return null;
}

function renewSessionLease(lease) {
  try {
    const priorHeartbeat = fs.fstatSync(lease.leaseFd);
    if (Date.now() - priorHeartbeat.mtimeMs > SESSION_LEASE_STALE_MS) {
      return false;
    }
    const now = new Date();
    fs.futimesSync(lease.leaseFd, now, now);
    return sameLeaseCandidate(readLeaseCandidate(lease.candidatePath), lease);
  } catch (_) {
    return false;
  }
}

function releaseSessionLease(lease) {
  try {
    fs.closeSync(lease.leaseFd);
  } catch (_) {
    // The descriptor may already be closed after an interrupted send.
  }
  removeLeaseCandidate(lease);
}

// The transcript is appended to while it is being read, so the final line may
// be a fragment that is not yet terminated. Report whether the buffer ended on
// a newline: without that bit a complete line and an in-progress one are
// indistinguishable, and acknowledging the fragment loses it forever.
function splitSourceLines(transcript) {
  const lines = transcript.split("\n");
  const endsWithNewline = lines.at(-1) === "";
  if (endsWithNewline) lines.pop();
  return { lines, endsWithNewline };
}

// Highest line count that may be checkpointed: an unterminated trailing line is
// never acknowledgeable, because more bytes for it are still coming.
function acknowledgeableLineCount({ lines, endsWithNewline }) {
  return Math.max(0, lines.length - (endsWithNewline ? 0 : 1));
}

function boundedLabel(value, fallback) {
  const trimmed = typeof value === "string" ? value.trim() : "";
  if (trimmed.length === 0) return fallback;
  return fitsStringLimit(trimmed) ? trimmed : trimmed.slice(0, 64);
}

// server.rs rejects the whole batch when project is empty, which happens for a
// missing cwd or a cwd of "/". A placeholder keeps the stream flowing.
function deriveProject(cwd) {
  return boundedLabel(path.basename(typeof cwd === "string" ? cwd : ""), UNKNOWN_PROJECT);
}

function deriveHost() {
  let hostname = "";
  try {
    hostname = os.hostname();
  } catch (_) {
    // A host without a resolvable name still deserves to sync.
  }
  return boundedLabel(hostname, UNKNOWN_HOST);
}

function deriveCwd(cwd) {
  if (typeof cwd !== "string") return null;
  if (cwd.trim().length === 0) return null;
  return Buffer.byteLength(cwd, "utf8") <= MAX_CWD_LEN ? cwd : null;
}

function cleanupOrphanedCursorTemps(trackingFile) {
  const directory = path.dirname(trackingFile);
  const prefix = `${path.basename(trackingFile)}.tmp-`;
  let entries;
  try {
    entries = fs.readdirSync(directory, { withFileTypes: true });
  } catch (_) {
    return;
  }
  for (const entry of entries) {
    if (!entry.isFile() || !entry.name.startsWith(prefix)) continue;
    try {
      fs.unlinkSync(path.join(directory, entry.name));
    } catch (_) {
      // A stale temp is harmless; the next exclusive owner retries cleanup.
    }
  }
}

function persistCursor(trackingFile, cursor, ownerToken) {
  const tempPath = `${trackingFile}.tmp-${ownerToken}`;
  const buffer = Buffer.from(String(cursor));
  let cursorFd;
  try {
    cursorFd = fs.openSync(tempPath, "wx", 0o600);
    let written = 0;
    while (written < buffer.length) {
      written += fs.writeSync(
        cursorFd,
        buffer,
        written,
        buffer.length - written,
      );
    }
    fs.fsyncSync(cursorFd);
    fs.closeSync(cursorFd);
    cursorFd = undefined;
    fs.renameSync(tempPath, trackingFile);

    try {
      const directoryFd = fs.openSync(path.dirname(trackingFile), "r");
      try {
        fs.fsyncSync(directoryFd);
      } finally {
        fs.closeSync(directoryFd);
      }
    } catch (_) {
      // Some platforms do not support fsync on directory descriptors.
    }
  } catch (err) {
    if (cursorFd !== undefined) {
      try { fs.closeSync(cursorFd); } catch (_) {}
    }
    try { fs.unlinkSync(tempPath); } catch (_) {}
    throw err;
  }
}

// Posts a batch, isolating poison rows by bisection. server.rs validates the
// whole payload before touching storage, so one permanently invalid record
// otherwise rejects every retry of the identical batch forever. Halving the
// batch on a permanent rejection narrows the culprit to a single record in
// O(log n) requests; that record is dropped and loudly logged so the rest of
// the stream keeps flowing.
//
// Returns { sent, dropped }. `sent: false` means the caller must leave the
// cursor untouched and retry on a later hook fire.
async function sendChunkWithBisect(config, envelope, chunk, lease) {
  if (chunk.length === 0) return { sent: true, dropped: [] };
  if (!renewSessionLease(lease)) return { sent: false, dropped: [] };

  try {
    await postJSON(config, "/api/v1/sessions/messages", {
      ...envelope,
      messages: chunk.map(({ message }) => message),
    });
    return { sent: true, dropped: [] };
  } catch (err) {
    if (err?.permanent !== true) {
      logSyncDebug(`retryable send failure for ${chunk.length} record(s): ${describeError(err)}`);
      return { sent: false, dropped: [] };
    }
    if (err.status !== ROW_REJECTION_STATUS || isEnvelopeRejection(err.detail)) {
      // Nothing in the batch is at fault, so dropping records would destroy
      // good data. Hold the cursor and surface the reason instead.
      logSync(
        `server rejected the request envelope for session ${envelope.session_id} `
        + `(${err.message}); no records acknowledged`,
      );
      return { sent: false, dropped: [] };
    }
    if (chunk.length === 1) {
      const [poisoned] = chunk;
      logSync(
        `dropping unacceptable record ${poisoned.message.uuid} at transcript line `
        + `${poisoned.sourceLineOrdinal} of session ${envelope.session_id}: ${err.message}`,
      );
      return { sent: true, dropped: [poisoned] };
    }

    const midpoint = Math.floor(chunk.length / 2);
    const head = await sendChunkWithBisect(config, envelope, chunk.slice(0, midpoint), lease);
    if (!head.sent) return head;
    const tail = await sendChunkWithBisect(config, envelope, chunk.slice(midpoint), lease);
    return { sent: tail.sent, dropped: [...head.dropped, ...tail.dropped] };
  }
}

async function main() {
  try {
    const raw = fs.readFileSync(0, "utf8");
    const input = JSON.parse(raw);

    if (input.stop_hook_active) return;

    const sessionId = input.session_id;
    const transcriptPath = input.transcript_path;
    if (!sessionId || !transcriptPath) return;

    const configPath = path.join(
      process.env.HOME || process.env.USERPROFILE,
      ".config",
      "quill",
      "config.json"
    );
    let config;
    try {
      config = JSON.parse(fs.readFileSync(configPath, "utf8"));
    } catch (err) {
      // An absent config simply means Quill is not set up on this machine;
      // anything else is a real misconfiguration worth reporting every time.
      if (err?.code !== "ENOENT") {
        logSync(`unusable config at ${configPath}: ${describeError(err)}`);
      }
      return;
    }
    if (typeof config?.url !== "string" || config.url.length === 0) {
      logSync(`config at ${configPath} has no url; nothing to sync`);
      return;
    }
    try {
      // Fail here rather than once per request: a malformed url makes every
      // send throw a bare TypeError that looks like a transient network fault.
      new URL(config.url);
    } catch (_) {
      logSync(`config at ${configPath} has a malformed url: ${config.url}`);
      return;
    }

    if (isLocal(config.url)) {
      // LOCAL: full-transcript reindexing is expensive, so only do it on Stop.
      if (input.hook_event_name !== "Stop") return;
      try {
        await postJSON(config, "/api/v1/sessions/notify", {
          provider: "claude",
          session_id: sessionId,
          jsonl_path: transcriptPath,
        });
      } catch (err) {
        // A closed desktop app is the routine case here, so only a rejection
        // the server would repeat forever is worth reporting every time.
        if (err?.permanent === true) {
          logSync(`local notify rejected: ${describeError(err)}`);
        } else {
          logSyncDebug(`local notify failed: ${describeError(err)}`);
        }
      }
      return;
    }

    // REMOTE: read JSONL, extract new messages, send them
    const sourceTrackingKey = crypto
      .createHash("sha256")
      .update(`${sessionId}\0${path.resolve(transcriptPath)}`)
      .digest("hex")
      .slice(0, 24);
    const trackingFile = path.join(
      os.tmpdir(),
      `.quill-sync-${sessionId}-${sourceTrackingKey}`,
    );
    const lockRoot = `${trackingFile}.leases`;
    const lease = acquireSessionLease(lockRoot);
    if (!lease) return;
    cleanupOrphanedCursorTemps(trackingFile);

    try {
      let lastSent = 0;
      try {
        lastSent = parseInt(fs.readFileSync(trackingFile, "utf8"), 10) || 0;
      } catch (_) {
        // No tracking file yet
      }

      let transcript;
      try {
        transcript = fs.readFileSync(transcriptPath, "utf8");
      } catch (_) {
        return;
      }

      const sourceLines = splitSourceLines(transcript);
      // Only complete lines may be parsed or acknowledged. A trailing fragment
      // is left for a later hook fire, when its remaining bytes have landed.
      const acknowledgeableEof = acknowledgeableLineCount(sourceLines);
      if (acknowledgeableEof <= lastSent) return;

      const newLines = sourceLines.lines.slice(lastSent, acknowledgeableEof);
      const { pendingMessages, invalidIdentityOrdinal } = extractMessages(
        newLines,
        sourceTrackingKey,
        lastSent,
      );

      // Highest line count this run may checkpoint. Each identity guard below
      // lowers it to the line where attribution became ambiguous, so the
      // unambiguous prefix is still delivered and acknowledged.
      let cursorCeiling = acknowledgeableEof;

      // Native sidechain rows without an agent/root identity cannot be
      // attributed safely, so they are withheld rather than acknowledged or
      // silently rewritten as parent activity.
      if (invalidIdentityOrdinal !== null) {
        cursorCeiling = Math.min(cursorCeiling, invalidIdentityOrdinal);
        logSync(
          `session ${sessionId}: transcript line ${invalidIdentityOrdinal} carries an `
          + "unattributable native sidechain identity; withholding it and every later line",
        );
      }

      const identityPrefix = homogeneousIdentityPrefix(pendingMessages);
      const sendableMessages = identityPrefix.length === pendingMessages.length
        ? pendingMessages
        : pendingMessages.slice(0, identityPrefix.length);
      if (identityPrefix.reason !== null) {
        const boundaryOrdinal = pendingMessages[identityPrefix.length].sourceLineOrdinal;
        cursorCeiling = Math.min(cursorCeiling, boundaryOrdinal);
        logSync(
          `session ${sessionId}: ${identityPrefix.reason} at transcript line `
          + `${boundaryOrdinal}; sending the ${identityPrefix.length} record(s) before it `
          + "and withholding the rest",
        );
      }

      if (sendableMessages.length === 0) {
        // Lines with no runtime records need no server acknowledgement.
        if (cursorCeiling <= lastSent) return;
        if (!renewSessionLease(lease)) return;
        persistCursor(trackingFile, cursorCeiling, lease.token);
        return;
      }

      const envelope = {
        provider: "claude",
        host: deriveHost(),
        session_id: sessionId,
        project: deriveProject(input.cwd),
        cwd: deriveCwd(input.cwd),
        git_branch: input.git_branch || null,
      };

      let offset = 0;
      while (offset < sendableMessages.length) {
        let chunkEnd = Math.min(
          offset + MAX_MESSAGES_PER_REQUEST,
          sendableMessages.length,
        );
        // Keep a normal Claude user/assistant turn in one transaction when a
        // 500-row boundary would otherwise split it. This preserves response
        // timing while retaining the same hard request limit.
        if (
          chunkEnd < sendableMessages.length
          && sendableMessages[chunkEnd - 1].message.role === "user"
          && sendableMessages[chunkEnd].message.role === "assistant"
        ) {
          chunkEnd -= 1;
        }
        const chunk = sendableMessages.slice(offset, chunkEnd);
        const nativeRoots = new Set(
          chunk.map(({ rootSessionId }) => rootSessionId).filter(Boolean),
        );
        if (nativeRoots.size > 1) {
          logSync(
            `session ${sessionId}: chunk at transcript line `
            + `${chunk[0].sourceLineOrdinal} spans more than one native root session; `
            + "withholding it",
          );
          return;
        }
        const rootSessionId = nativeRoots.values().next().value ?? sessionId;
        const { sent } = await sendChunkWithBisect(
          config,
          { ...envelope, session_id: rootSessionId },
          chunk,
          lease,
        );
        if (!sent) return;
        if (!renewSessionLease(lease)) return;
        persistCursor(
          trackingFile,
          sendableMessages[chunkEnd]?.sourceLineOrdinal ?? cursorCeiling,
          lease.token,
        );
        offset = chunkEnd;
      }
    } finally {
      releaseSessionLease(lease);
    }
  } catch (err) {
    logSync(`unhandled error: ${describeError(err)}`);
  }
}

void main();
