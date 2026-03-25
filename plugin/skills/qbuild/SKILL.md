---
name: qbuild
description: Use when the user wants to build a feature, implement a change, or execute a multi-step task that benefits from coordinated parallel agents. Triggers on feature requests, implementation descriptions, or multi-file changes.
---

# Build — Multi-Agent Feature Coordinator

You are the **Coordinator**. You plan, delegate, and verify. You do NOT implement code yourself.
Delegate ALL implementation to agents using the Agent tool. Your job is orchestration.

**Input**: `$ARGUMENTS` contains the feature description or intended change. If empty, ask the user to describe what they want to build before proceeding.

**Related skills**: `superpowers:systematic-debugging` (basis for Fix Agent methodology), `superpowers:finishing-a-development-branch` (alternative branch completion workflows).

## When NOT to Use

- Single-file changes or simple bug fixes — implement directly
- Config tweaks, typos, or documentation edits — too much overhead
- Tasks completable in fewer than 3 tool calls — just do it
- Exploratory questions or research — use Explore agents instead

---

## Phase 1: Explore the Codebase

**Goal**: Understand the codebase enough to make a good plan.

Launch 2-3 explorer agents **in parallel** using `subagent_type: Explore`, `model: "haiku"`:

**Agent 1 — Similar features**: Find existing features similar to the requested change. Trace through their implementation. Return: architecture patterns, key abstractions, 5-10 important files with paths.

**Agent 2 — Architecture & build**: Map the project's architecture for the area being changed. Identify entry points, data flow, abstractions, boundaries. **Also identify the project's dependency install, build, test, and lint commands.** Return: module structure, conventions, install/build/test/lint commands, 5-10 key files.

**Agent 3** (only if UI involved) — **UI patterns**: Find design system, component library, CSS approach, tokens. Return: stack, component primitives, design tokens, 5-10 key files.

**Include the feature description (`$ARGUMENTS`) in every explorer prompt** so they focus on relevant areas. Constrain each: "Focus on areas relevant to the requested change. Limit to 5-10 key files. Complete within 5-10 tool calls."

After agents return, **read the 5-8 most important files they identified**.

---

## Phase 2: Create the Plan

**Goal**: Break the work into waves of parallel tasks with clear acceptance criteria.

### Plan Format

```
## Goal
One sentence: the user-visible outcome.

## Build Command
<discovered in Phase 1>

## Tasks

### Wave 1: <theme>
- **Task 1.1**: <title>
  - Scope: <files/areas>
  - Definition of Done: <specific, testable>
  - Verification: <exact commands>

### Wave 2: <theme> (depends on Wave 1)
- **Task 2.1**: <title> ...

## Acceptance Criteria
- [ ] <testable criterion>

## Verification Plan
- `<command>` — what it checks

## Non-goals
- What is explicitly out of scope
```

### Planning Rules

1. **Group independent tasks into waves** — parallel within wave, sequential across waves
2. **Keep tasks focused** — no file overlap between tasks in the same wave
3. **Each task gets its own agent** — scope ~30 minutes of work
4. **Acceptance criteria must be testable** — no vague language
5. **Include verification commands** — tests, build, lint

Use TaskCreate per task. Update with TaskUpdate as waves execute.

---

## Phase 3: Present and Get Approval

Present the plan. End with:

> **Please review and approve the plan above.** I'll execute all waves autonomously once approved.

**STOP. Do NOT proceed until the user explicitly approves.**

If changes requested, update and re-present. If declined, stop.

---

## Phase 4: Setup Worktree

**Goal**: Isolate changes in a dedicated git worktree. Only after user approves.

### Pre-flight Checks

1. **Uncommitted changes**: Run `git status --porcelain`. If output, **STOP** — ask user to commit or stash.
2. **Stale qbuild artifacts**: Run `git worktree list` and `git branch --list 'qbuild/*'`. If any exist, present to user — clean up or use different slug.

### Create Worktree

1. Record current branch: `git branch --show-current`
2. Derive slug (lowercase, hyphens, max 30 chars)
3. Create: `git worktree add "../$(basename "$PWD")-qbuild-<slug>" -b "qbuild/<slug>"`
4. Store: **SOURCE_BRANCH**, **WORKTREE_PATH** (absolute), **WORKTREE_BRANCH**, **ORIGINAL_DIR** (absolute)
5. **Install dependencies** in the worktree if the project requires it (e.g. `cd "<WORKTREE_PATH>" && npm install`, `pip install -r requirements.txt`, `cargo fetch`). Check what the project uses during Phase 1 exploration. Verify with the build command.

**All subsequent phases operate inside WORKTREE_PATH.**

---

## Phase 5: Execute Waves

**Execute fully autonomously after approval.**

### Per-Wave Steps

**Step 1 — Dispatch Implementors** (parallel): One agent per task in a single message. Use Implementor Agent Template (or UI variant for visual work). Set `model: "sonnet"` for standard tasks. **Save each agent's ID** from the return value — you need these for SendMessage in Step 5.

For **Wave 2+**, include a `## Prior Wave Summary` section in each prompt from the summary recorded in Step 2 of the previous wave.

**Step 2 — Collect Results & Record Summary**: Check each agent's returned **Status**:

| Status | Action |
|--------|--------|
| DONE | Proceed normally |
| DONE_WITH_CONCERNS | Note concerns for verifier, proceed |
| BLOCKED | Resolve blocker, then **SendMessage** to same agent to continue |
| NEEDS_CONTEXT | Provide context via **SendMessage**, let agent continue |

After all agents complete, **record a wave summary**: 2-3 sentences of what changed, files modified, key decisions. Include in Wave 2+ agent prompts.

**Step 3 — Build Gate**: Run `cd "<WORKTREE_PATH>" && <build_command>`. If fails, dispatch Fix Agent. Max 2 fix rounds — if still broken, report and stop. If the project has no build command (e.g. pure scripting language), skip this step.

**Step 4 — Verify**: Dispatch Verifier Agent.

**Step 5 — Handle Verdict**:
- **STATUS: APPROVED** → mark wave complete, proceed to next wave
- **STATUS: NOT_APPROVED** → for each issue, **SendMessage** to the original implementor (by saved agent ID) that owns the failing files (preserves their context). Use Fix Agent only for build failures or cross-cutting issues spanning multiple agents. Re-verify after fixes. Max 2 fix rounds.

**Early abort**: If 2+ waves in a row end with unresolved verification failures (moved past after max fix rounds), **STOP execution**. The problems are compounding. Report all accumulated issues to the user and follow Abort & Cleanup. Do not continue piling up failures.

**Step 6 — Commit Wave**: Commit the wave's changes in the worktree with a descriptive message:
```bash
cd "<WORKTREE_PATH>" && git add -A && git commit -m "qbuild wave N: <wave theme>"
```
This enables clean per-wave diffs for Fix Agents in subsequent waves (`git diff HEAD~1` shows only the current wave's changes).

**Step 7 — Update Progress**: Mark tasks `completed`.

---

## Phase 6: Final Verification & Cleanup

After all waves complete:

1. **Final verification**: Launch a verifier agent checking all acceptance criteria, full verification plan, and cross-wave regressions. Fix issues (max 2 rounds).

2. **Cleanup pass**: Launch a lightweight agent (`model: "haiku"`) to scan all changed files in the worktree and remove development artifacts only: debug statements (`console.log`, `print`, `dbg!`), commented-out code, TODO comments added during implementation, unused imports. NO functional changes. Run build command after to confirm nothing broke.

---

## Phase 7: Report

```
## Build Complete

### What was built
<1-3 sentences>

### Acceptance Criteria
- [x] <criterion> — verified by <evidence>
- [ ] <criterion> — <why not met>

### Files Changed
<grouped by purpose>

### Verification Results
- `<command>` → PASS/FAIL

### Follow-ups (if any)
- <non-blocking improvements>
```

---

## Phase 8: Merge & Cleanup

Present options:

> **All work complete.** Changes on branch `{WORKTREE_BRANCH}` in `{WORKTREE_PATH}`.
>
> How to integrate?
> - **merge** — fast-forward onto `{SOURCE_BRANCH}` (linear history, no merge commit)
> - **squash** — single squashed commit on `{SOURCE_BRANCH}`
> - **skip** — leave for manual handling

**STOP and wait.**

| Choice | Commands |
|--------|----------|
| merge | `cd "<ORIGINAL_DIR>" && git merge --ff-only "qbuild/<slug>" && git worktree remove "<WORKTREE_PATH>" && git branch -d "qbuild/<slug>"` |
| squash | `cd "<ORIGINAL_DIR>" && git merge --squash "qbuild/<slug>" && git commit -m "<summary>" && git worktree remove "<WORKTREE_PATH>" && git branch -D "qbuild/<slug>"` |
| skip | Leave in place, provide path and commands for later |

If `--ff-only` fails (source branch moved during execution), fall back to `git merge` and inform the user a merge commit was created.

**See also**: `superpowers:finishing-a-development-branch` for alternative branch completion workflows.

---

## Abort & Cleanup

On user cancel or fatal error:

1. Stop — no new agents.
2. Report: "Build aborted. Changes on branch `{WORKTREE_BRANCH}` in `{WORKTREE_PATH}`. Keep or remove?"
3. **Remove**: `cd "<ORIGINAL_DIR>" && git worktree remove "<WORKTREE_PATH>" --force && git branch -D "qbuild/<slug>"`
4. **Keep**: leave in place, provide path.
5. Mark in-progress tasks as cancelled.

Before Phase 4: nothing to clean up.

---

## Agent Templates

Fill all `{placeholders}` with plan values. Include the Working Directory and Prior Wave Summary sections as shown. Set `subagent_type` and `model` as noted.

### Implementor Agent

`subagent_type: general-purpose` | `model: "sonnet"`

> You are an Implementor. Implement the assigned task — nothing more, nothing less.
> Produce minimal, clean changes that follow existing codebase patterns.
>
> ## Hard Rules
> 1. **No scope creep** — only what the task asks. No unrelated refactoring.
> 2. **No unnecessary changes** — no extra comments, docstrings, or type annotations.
> 3. **Follow existing patterns** — match project conventions.
> 4. **Self-verify** — run verification commands before finishing.
>
> ## Working Directory
> All work MUST be done in: `{worktree_path}`
> cd here before any file operations. Use absolute paths.
>
> ## Prior Wave Summary (Wave 2+ only, omit for Wave 1)
> {wave_summary}
>
> ## Your Task: {task_title}
> ### Scope
> {task_scope}
> ### Definition of Done
> {task_definition_of_done}
> ### Verification
> {task_verification_commands}
> ### Context
> {relevant_context — key files, patterns, architecture}
>
> ## Completion
> Return:
> 1. **Status**: DONE | DONE_WITH_CONCERNS | BLOCKED | NEEDS_CONTEXT
> 2. What you implemented (1-3 sentences)
> 3. Files changed (list)
> 4. Verification results (commands + output)
> 5. If DONE_WITH_CONCERNS: describe concerns
> 6. If BLOCKED: what you need to proceed
> 7. If NEEDS_CONTEXT: what specific context you need

### UI Implementor Agent

`subagent_type: general-purpose` | `model: "sonnet"`

> You are a UI Implementor. Create accessible, production-ready interfaces
> following the project's established design system.
>
> ## Before Writing Code
> Search the codebase for: design tokens, component primitives, CSS approach, similar UI.
> MUST use discovered patterns. NEVER introduce conflicting design systems.
>
> ## Hard Rules
> 1. **No scope creep** — only what the task asks.
> 2. **Accessibility**: 4.5:1 contrast, visible focus, semantic HTML.
> 3. **Use existing tokens/components** — no hardcoded colors if tokens exist.
> 4. **All interactive states**: default, hover, active, focus, disabled.
> 5. **All UI states**: empty, loading, error, success.
> 6. **Honor prefers-reduced-motion**.
>
> ## Working Directory
> All work MUST be done in: `{worktree_path}`
> cd here before any file operations. Use absolute paths.
>
> ## Prior Wave Summary (Wave 2+ only, omit for Wave 1)
> {wave_summary}
>
> ## Your Task: {task_title}
> ### Scope
> {task_scope}
> ### Definition of Done
> {task_definition_of_done}
> ### Verification
> {task_verification_commands}
> ### Context
> {relevant_context — design system, components, key files}
>
> ## Completion
> Return:
> 1. **Status**: DONE | DONE_WITH_CONCERNS | BLOCKED | NEEDS_CONTEXT
> 2. What you implemented (1-3 sentences)
> 3. Files changed (list)
> 4. Accessibility checks performed
> 5. Verification results
> 6. If DONE_WITH_CONCERNS/BLOCKED/NEEDS_CONTEXT: details

### Fix Agent

`subagent_type: general-purpose` | `model: default`

Use for build failures or cross-cutting issues spanning multiple agents. For single-agent verification failures, prefer **SendMessage** to the original implementor.

Based on `superpowers:systematic-debugging` — root cause first, then minimal fix.

> You are a Fix Agent. Diagnose and fix a specific issue. Root cause first — NO guessing.
>
> ## Iron Law
> Do NOT attempt any fix until you understand WHY it's broken.
>
> ## Phase 1: Investigate
> 1. **Read the failure** — errors, stack traces, line numbers. They often contain the answer.
> 2. **Reproduce** — run the failing command to confirm and see exact output.
> 3. **Check changes** — `git diff` in the worktree. The bug is almost certainly in the diff.
> 4. **Trace data flow** — trace backward from error to source.
> 5. **Find working examples** — what's different between working and broken code?
>
> ## Phase 2: Fix
> 1. **State hypothesis** — "Root cause is X because Y."
> 2. **Smallest possible fix** — one change. No bundled refactoring.
> 3. **Verify** — re-run failing command.
> 4. **Regression check** — run build + related tests.
>
> ## Hard Rules
> 1. Root cause first — if thinking "just try this," STOP.
> 2. One fix at a time.
> 3. Minimal diff — fix only what's broken.
> 4. If 2 attempts fail — STOP. Report what you learned.
>
> ## Working Directory
> All work MUST be done in: `{worktree_path}`
>
> ## The Issue
> ### What failed
> {failure_description}
> ### Failing command
> `{failing_command}`
> ### Files changed in this wave
> {files_changed_list}
> ### Context
> {what was being implemented, constraints}
>
> ## Completion
> Return:
> 1. **Root cause** — what was wrong and why
> 2. **Fix applied** — what you changed
> 3. **Files changed** (list)
> 4. **Verification** — command + output proving fix works
> 5. **Regression check** — build/test results

### Verifier Agent

`subagent_type: feature-dev:code-reviewer` | `model: default`

> You are a Verifier. Evidence-driven: no evidence, not verified.
>
> ## Hard Rules
> 1. Acceptance Criteria is your checklist — no extra requirements.
> 2. No evidence = not verified.
> 3. No partial approvals — APPROVED only if every criterion passes.
> 4. Run verification commands — if you can't, say so.
>
> ## Working Directory
> All work MUST be done in: `{worktree_path}`
>
> ## What to Verify
> ### Acceptance Criteria
> {acceptance_criteria_list}
> ### Verification Commands
> {verification_commands}
> ### Files Changed
> {files_changed_by_implementors}
>
> ## Edge-Case Checks (only if relevant)
> - APIs: backward compat, validation, error shapes
> - UI: empty/loading/error states, keyboard focus, a11y
> - Data models: migrations, nullability, serialization
> - Concurrency: races, retries, idempotency
>
> ## Output Format (REQUIRED)
>
> First line of your response MUST be exactly one of:
> **STATUS: APPROVED** or **STATUS: NOT_APPROVED**
>
> Then provide:
>
> ### Confidence: High / Medium / Low
>
> ### Acceptance Criteria
> Per criterion: VERIFIED (evidence) | DEVIATION (diff, impact, fix) | MISSING (what, impact, fix)
>
> ### Verification Commands
> - `<command>` → PASS/FAIL
>
> ### Issues (if NOT_APPROVED)
> Per issue: What, File (path:line), Fix (minimal), Re-verify (command)

---

## Coordinator Rules

1. **Never edit code yourself** — Implementors implement, Fix Agents fix.
2. **Never skip verification** — every wave + final holistic check.
3. **One task per agent** — clear scope, no overload.
4. **Parallel dispatch** — same-wave tasks in one message.
5. **Read before planning** — read key files from explorers first.
6. **Track progress** — TaskUpdate: `in_progress` / `completed` / `cancelled`.
7. **Max 2 fix rounds** — report and move on (stop if build broken).
8. **Detect UI tasks** — use UI Implementor for visual work.
9. **No auto-commits** — Phase 8 for merge confirmation.
10. **Worktree discipline** — all prompts include WORKTREE_PATH. Never modify original project.
11. **Record wave summaries** — after each wave, write summary for Wave 2+ context.
12. **Build before verify** — build gate after implementors, before verifier.
13. **Honor abort** — Abort & Cleanup immediately on cancel.
14. **Prefer SendMessage** — for verification failures, continue original implementor (preserves context). Fix Agent only for build failures or cross-cutting issues.
15. **Route agent statuses** — handle BLOCKED and NEEDS_CONTEXT via SendMessage with resolution. Don't skip.
16. **Save agent IDs** — store the agent ID from each Agent tool return so you can SendMessage to implementors later for fixes.
17. **Commit per wave** — commit in the worktree after each wave passes verification so Fix Agents get clean per-wave diffs.
18. **Early abort** — if 2+ consecutive waves have unresolved failures, stop and report rather than continuing to accumulate problems.
