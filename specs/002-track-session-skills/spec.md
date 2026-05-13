# Feature Specification: Skills Breakdown Tab

**Feature Branch**: `002-track-session-skills`
**Created**: 2026-05-12
**Status**: Draft
**Input**: User description: "$speckit-specify great. we want to build this as a new Skills tab in the analytics breakdown section [Image #1]. it should show the total skill usage counts per skill, should have an all time toggle to show all time counts instead of timeframe bound, and should have badges to filter for only codex or cc"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - View Skill Counts in Breakdown (Priority: P1)

Users can open a new Skills tab in the existing analytics breakdown section to see which skills were used most during the selected analytics timeframe.

**Why this priority**: This is the core value of the feature: make skill usage visible in the same place users already inspect sessions, projects, and hosts.

**Independent Test**: Can be tested by loading analytics data with known skill usage, selecting the Skills breakdown tab, and verifying that each skill appears with the correct timeframe-bound count.

**Acceptance Scenarios**:

1. **Given** the selected timeframe contains recognized skill usage, **When** the user selects the Skills breakdown tab, **Then** the breakdown lists one row per skill with the total use count for that timeframe.
2. **Given** the selected timeframe contains no recognized skill usage, **When** the user selects the Skills breakdown tab, **Then** the breakdown shows a clear empty state instead of stale or unrelated rows.
3. **Given** multiple skills have different usage counts, **When** the Skills tab is displayed, **Then** skills are ordered from highest count to lowest count.

---

### User Story 2 - Compare Timeframe and All-Time Skill Usage (Priority: P2)

Users can switch the Skills breakdown between the active analytics timeframe and all-time totals without changing the rest of the analytics view.

**Why this priority**: Timeframe counts answer "what happened recently", while all-time counts answer "what skills dominate my history"; both are needed for meaningful analytics.

**Independent Test**: Can be tested by using data where at least one skill has usage outside the selected timeframe, toggling all-time mode, and confirming only the Skills breakdown counts change.

**Acceptance Scenarios**:

1. **Given** the Skills tab is showing the selected timeframe, **When** the user enables all-time mode, **Then** the listed skill counts include recognized usage from all indexed history.
2. **Given** all-time mode is enabled, **When** the user disables it, **Then** the skill counts return to the selected analytics timeframe.
3. **Given** the user changes the analytics timeframe while all-time mode is enabled, **When** the Skills tab remains open, **Then** all-time counts stay active until the user disables the toggle.

---

### User Story 3 - Filter Skill Counts by Provider (Priority: P2)

Users can filter the Skills breakdown to all providers, Codex only, or Claude Code only using provider badges.

**Why this priority**: Skill usage differs by assistant surface, so provider filtering is necessary to compare Codex and Claude Code behavior without leaving the breakdown panel.

**Independent Test**: Can be tested by loading data with known Codex and Claude Code skill usage, applying each provider badge, and verifying that counts match the selected provider scope.

**Acceptance Scenarios**:

1. **Given** both Codex and Claude Code have recognized usage for the same skill, **When** the user selects the All provider badge, **Then** the skill row shows the combined count across both providers.
2. **Given** both providers have skill usage, **When** the user selects the Codex badge, **Then** only Codex skill usage contributes to the listed counts.
3. **Given** both providers have skill usage, **When** the user selects the Claude Code badge, **Then** only Claude Code skill usage contributes to the listed counts.
4. **Given** the active provider filter has no matching skill usage, **When** the Skills tab is displayed, **Then** the empty state names the active provider scope.

### Edge Cases

- Historical sessions may lack reliable skill-use records; ambiguous activity is not counted as a named skill.
- A skill may be used multiple times in a single session; each distinct recognized use increments that skill's count.
- The same skill name may appear under both providers; All view combines counts by skill name, while provider-filtered views show only the selected provider's contribution.
- Skill names may be long, scoped, or contain separators; rows must remain readable without overlapping count columns.
- Several skills may have identical counts; tie ordering must remain stable and predictable.
- The user may switch between Sessions, Projects, Hosts, and Skills repeatedly; each tab must show data for its own active filters without leaking rows from another tab.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST add a Skills option to the existing analytics breakdown selector alongside Sessions, Projects, and Hosts.
- **FR-002**: System MUST keep the existing default breakdown selection unchanged when users first open analytics.
- **FR-003**: System MUST show one row per skill in the Skills breakdown, including the skill display name and total recognized use count.
- **FR-004**: A skill use count MUST represent each distinct recognized use of a skill; repeated uses in the same session MUST increase the count.
- **FR-005**: System MUST base named skill counts only on records that include a reliable skill identity and provider.
- **FR-006**: System MUST exclude ambiguous or unidentified skill activity from named skill totals rather than estimating a skill name.
- **FR-007**: System MUST use the active analytics timeframe for Skills counts by default.
- **FR-008**: System MUST provide an all-time toggle inside the Skills breakdown controls.
- **FR-009**: When all-time mode is enabled, System MUST calculate Skills counts across all indexed history while still respecting the active provider filter.
- **FR-010**: When all-time mode is disabled, System MUST calculate Skills counts only within the selected analytics timeframe.
- **FR-011**: System MUST provide provider badges for All, Codex, and Claude Code in the Skills breakdown.
- **FR-012**: System MUST make the active provider badge visually distinct from inactive provider badges.
- **FR-013**: Selecting the Codex badge MUST limit Skills rows and counts to recognized Codex skill usage.
- **FR-014**: Selecting the Claude Code badge MUST limit Skills rows and counts to recognized Claude Code skill usage.
- **FR-015**: Selecting the All badge MUST combine recognized skill usage from Codex and Claude Code.
- **FR-016**: System MUST sort Skills rows by total use count descending, with stable alphabetical ordering for ties.
- **FR-017**: System MUST show an empty state when the active time scope and provider filter contain no recognized skill usage.
- **FR-018**: System MUST update the Skills breakdown when the active timeframe, all-time toggle, provider badge, or indexed analytics data changes.
- **FR-019**: System MUST preserve existing Sessions, Projects, and Hosts breakdown behavior when the Skills tab is inactive.
- **FR-020**: System MUST keep Skills rows visually consistent with the existing breakdown panel density, scrolling, and count formatting.

### Key Entities *(include if feature involves data)*

- **Skill Use**: A distinct recognized use of a named skill in a session, associated with a provider and occurrence time.
- **Skill Aggregate**: A per-skill total derived from Skill Use records for the active time scope and provider filter.
- **Time Scope**: The selection that determines whether Skills counts are bound to the active analytics timeframe or all indexed history.
- **Provider Filter**: The selected provider scope for Skills counts: All, Codex, or Claude Code.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Given a validation dataset with known skill usage, displayed Skills counts match expected totals for each provider filter and time scope.
- **SC-002**: Users can identify the top five skills for the active scope from the analytics breakdown without navigating away from the analytics view.
- **SC-003**: Switching between timeframe-bound and all-time Skills counts updates the list within one second on datasets comparable to existing analytics payloads.
- **SC-004**: Switching between All, Codex, and Claude Code provider badges updates Skills counts without changing the selected Sessions, Projects, or Hosts breakdown state.
- **SC-005**: Existing Sessions, Projects, and Hosts breakdown output remains unchanged when the Skills tab is not selected.

## Assumptions

- The Skills tab belongs to the breakdown panel in the current analytics Now view, matching the location shown in the provided screenshot.
- The initial provider filter for Skills is All.
- The all-time toggle is off by default so Skills follows the active analytics timeframe initially.
- "CC" means Claude Code, and "Codex" means Codex sessions.
- Reliable skill usage may not exist for every historical session; the feature reports recognized skill usage and does not infer missing records.
