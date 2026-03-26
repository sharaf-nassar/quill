use serde::{Deserialize, Serialize};

// Payload received from hook scripts via HTTP API
#[derive(Deserialize, Clone, Debug)]
pub struct TokenReportPayload {
    pub session_id: String,
    pub hostname: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    #[serde(default)]
    pub cwd: Option<String>,
}

// Time-series point for token charts
#[derive(Serialize, Clone, Debug)]
pub struct TokenDataPoint {
    pub timestamp: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub total_tokens: i64,
}

// Aggregate stats for token stats panel
#[derive(Serialize, Clone, Debug)]
pub struct TokenStats {
    pub total_input: i64,
    pub total_output: i64,
    pub total_cache_creation: i64,
    pub total_cache_read: i64,
    pub total_tokens: i64,
    pub turn_count: i64,
    pub avg_input_per_turn: f64,
    pub avg_output_per_turn: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UsageBucket {
    pub label: String,
    pub utilization: f64,
    pub resets_at: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct UsageData {
    pub buckets: Vec<UsageBucket>,
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct DataPoint {
    pub timestamp: String,
    pub utilization: f64,
}

// Host-level token breakdown
#[derive(Serialize, Clone, Debug)]
pub struct HostBreakdown {
    pub hostname: String,
    pub total_tokens: i64,
    pub turn_count: i64,
    pub last_active: String,
}

// Per-project token totals (grouped by session cwd)
#[derive(Serialize, Clone, Debug)]
pub struct ProjectTokens {
    pub project: String,
    pub total_tokens: i64,
    pub session_count: i64,
}

// Aggregate session stats (unlimited, for analytics)
#[derive(Serialize, Clone, Debug)]
pub struct SessionStats {
    pub avg_duration_seconds: f64,
    pub avg_tokens: f64,
    pub session_count: i64,
    pub total_tokens: i64,
}

// Session-level token breakdown
#[derive(Serialize, Clone, Debug)]
pub struct SessionBreakdown {
    pub session_id: String,
    pub hostname: String,
    pub total_tokens: i64,
    pub turn_count: i64,
    pub first_seen: String,
    pub last_active: String,
    pub project: Option<String>,
}

// Project-level token breakdown (grouped by cwd + hostname)
#[derive(Serialize, Clone, Debug)]
pub struct ProjectBreakdown {
    pub project: String,
    pub hostname: String,
    pub total_tokens: i64,
    pub turn_count: i64,
    pub session_count: i64,
    pub last_active: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct BucketStats {
    pub label: String,
    pub current: f64,
    pub avg: f64,
    pub max: f64,
    pub min: f64,
    pub time_above_80: f64,
    pub trend: String,
    pub sample_count: i64,
}

// --- Learning system models ---

// Payload received from observation hook scripts via HTTP API
#[derive(Deserialize, Clone, Debug)]
pub struct ObservationPayload {
    pub session_id: String,
    pub hook_phase: String,
    pub tool_name: String,
    #[serde(default)]
    pub tool_input: Option<String>,
    #[serde(default)]
    pub tool_output: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

// Payload from session-end hook
#[derive(Deserialize, Clone, Debug)]
pub struct SessionEndPayload {
    pub session_id: String,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

// Payload to record a learning run result
#[derive(Deserialize, Clone, Debug)]
pub struct LearningRunPayload {
    pub trigger_mode: String,
    pub observations_analyzed: i64,
    pub rules_created: i64,
    #[serde(default)]
    pub rules_updated: i64,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub logs: Option<String>,
    #[serde(default)]
    pub phases: Option<String>,
}

// Payload to record learned rule metadata from /learn skill
#[derive(Deserialize, Clone, Debug)]
pub struct LearnedRulePayload {
    pub name: String,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    #[serde(default)]
    pub observation_count: i64,
    pub file_path: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub is_anti_pattern: bool,
    #[serde(default)]
    pub source: Option<String>,
}

fn default_confidence() -> f64 {
    0.5
}

// Learning run record returned to frontend/API
#[derive(Serialize, Clone, Debug)]
pub struct LearningRun {
    pub id: i64,
    pub trigger_mode: String,
    pub observations_analyzed: i64,
    pub rules_created: i64,
    pub rules_updated: i64,
    pub duration_ms: Option<i64>,
    pub status: String,
    pub error: Option<String>,
    pub logs: Option<String>,
    pub phases: Option<String>,
    pub created_at: String,
}

// Learned rule record returned to frontend
#[derive(Serialize, Clone, Debug)]
pub struct LearnedRule {
    pub name: String,
    pub domain: Option<String>,
    pub confidence: f64,
    pub observation_count: i64,
    pub file_path: String,
    pub created_at: String,
    pub updated_at: String,
    pub state: String,
    pub project: Option<String>,
    pub is_anti_pattern: bool,
    pub source: Option<String>,
}

// Tool frequency count for status strip
#[derive(Serialize, Clone, Debug)]
pub struct ToolCount {
    pub tool_name: String,
    pub count: i64,
}

// Learning settings
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LearningSettings {
    pub enabled: bool,
    pub trigger_mode: String,
    pub periodic_minutes: i64,
    pub min_observations: i64,
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f64,
}

fn default_min_confidence() -> f64 {
    0.95
}

impl Default for LearningSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            trigger_mode: "on-demand".to_string(),
            periodic_minutes: 180,
            min_observations: 50,
            min_confidence: 0.95,
        }
    }
}

// Learning status for GET /api/v1/learning/status
#[derive(Serialize, Clone, Debug)]
pub struct LearningStatus {
    pub observation_count: i64,
    pub unanalyzed_count: i64,
    pub rules_count: i64,
    pub last_run: Option<LearningRun>,
}

// --- Session indexing HTTP payloads ---

/// Notify that a session JSONL file has been created/updated
#[derive(Deserialize)]
pub struct SessionNotifyPayload {
    pub session_id: String,
    pub jsonl_path: String,
}

/// A single message pushed via the HTTP API
#[derive(Deserialize)]
pub struct SessionMessagePayload {
    pub uuid: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub msg_type: String,
    pub timestamp: String,
    pub content: String,
    pub role: String,
    #[serde(default)]
    pub tools_used: Vec<String>,
    #[serde(default)]
    pub files_modified: Vec<String>,
}

/// Batch of messages pushed via the HTTP API
#[derive(Deserialize)]
pub struct SessionMessagesPayload {
    pub host: String,
    pub session_id: String,
    pub project: String,
    #[serde(default)]
    pub git_branch: String,
    pub messages: Vec<SessionMessagePayload>,
}

// Haiku analysis output item (parsed from JSON)
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct AnalysisRule {
    pub name: String,
    pub domain: String,
    pub confidence: f64,
    pub content: String,
    #[serde(default)]
    pub is_anti_pattern: bool,
}

// Verdict on an existing rule from LLM analysis
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct RuleVerdict {
    pub name: String,
    pub verdict: String,
    #[serde(default = "default_verdict_strength")]
    pub strength: f64,
}

fn default_verdict_strength() -> f64 {
    0.5
}

// Top-level LLM analysis output (two-phase)
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct AnalysisOutput {
    #[serde(default)]
    pub new_rules: Vec<AnalysisRule>,
    #[serde(default)]
    pub verdicts: Vec<RuleVerdict>,
}

// Intermediate findings from a single analysis stream.
// Both observation and git streams produce this same format
// so the synthesis prompt can reason about them uniformly.
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct StreamFindings {
    #[serde(default)]
    pub patterns: Vec<StreamPattern>,
    #[serde(default)]
    pub verdicts: Vec<RuleVerdict>,
}

#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct StreamPattern {
    pub name: String,
    pub domain: String,
    pub description: String,
    pub evidence: String,
    pub confidence: f64,
    #[serde(default)]
    pub is_anti_pattern: bool,
}

// Cached git history snapshot, one per project.
// `created_at` is DB-populated via DEFAULT, not passed from Rust on insert.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct GitSnapshot {
    pub project: String,
    pub commit_hash: String,
    pub commit_count: i64,
    pub raw_data: String,
}

// Phase progress tracking for multi-stream learning runs
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct RunPhase {
    pub name: String,
    pub status: String,
    pub duration_ms: Option<i64>,
    pub findings_count: i64,
}

impl StreamFindings {
    /// Convert single-stream findings directly to AnalysisOutput.
    /// Used when only one stream produces findings (skip Sonnet).
    pub fn to_analysis_output(&self) -> AnalysisOutput {
        AnalysisOutput {
            new_rules: self
                .patterns
                .iter()
                .map(|p| AnalysisRule {
                    name: p.name.clone(),
                    domain: p.domain.clone(),
                    confidence: p.confidence,
                    content: format!("{}\n\nEvidence: {}", p.description, p.evidence),
                    is_anti_pattern: p.is_anti_pattern,
                })
                .collect(),
            verdicts: self.verdicts.clone(),
        }
    }
}

// Tagged learning log event for real-time frontend streaming
#[derive(Serialize, Clone, Debug)]
pub struct LearningLogEvent {
    pub run_id: i64,
    pub message: String,
}

// --- Memory optimizer models ---

/// Action types for memory optimization suggestions
#[allow(dead_code)]
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ActionType {
    Delete,
    Update,
    Merge,
    Create,
    Flag,
}

impl std::fmt::Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionType::Delete => write!(f, "delete"),
            ActionType::Update => write!(f, "update"),
            ActionType::Merge => write!(f, "merge"),
            ActionType::Create => write!(f, "create"),
            ActionType::Flag => write!(f, "flag"),
        }
    }
}

/// LLM response: a single optimization suggestion
#[allow(dead_code)]
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct OptimizationSuggestionLlm {
    pub action_type: ActionType,
    pub target_file: Option<String>,
    pub new_filename: Option<String>,
    pub reasoning: String,
    pub proposed_content: Option<String>,
    pub merge_sources: Option<Vec<String>>,
}

/// Top-level LLM analysis output for memory optimization
#[allow(dead_code)]
#[derive(Deserialize, Serialize, Clone, Debug, schemars::JsonSchema)]
pub struct OptimizationOutput {
    #[serde(default)]
    pub suggestions: Vec<OptimizationSuggestionLlm>,
}

/// A memory file record returned to frontend
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct MemoryFile {
    pub id: i64,
    pub project_path: String,
    pub file_path: String,
    pub file_name: String,
    pub content_hash: String,
    pub last_scanned_at: String,
    pub memory_type: Option<String>,
    pub description: Option<String>,
    pub content: String,
    pub changed_since_last_run: bool,
}

/// An optimization suggestion returned to frontend
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct OptimizationSuggestion {
    pub id: i64,
    pub run_id: i64,
    pub project_path: String,
    pub action_type: String,
    pub target_file: Option<String>,
    pub reasoning: String,
    pub proposed_content: Option<String>,
    pub merge_sources: Option<Vec<String>>,
    pub status: String,
    pub error: Option<String>,
    pub resolved_at: Option<String>,
    pub created_at: String,
    pub original_content: Option<String>,
    pub diff_summary: Option<String>,
    pub backup_data: Option<String>,
    pub group_id: Option<String>,
}

/// An optimization run record returned to frontend
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct OptimizationRun {
    pub id: i64,
    pub project_path: String,
    pub trigger: String,
    pub memories_scanned: i64,
    pub suggestions_created: i64,
    pub status: String,
    pub error: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
}

/// A known project for the memory optimizer
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct KnownProject {
    pub path: String,
    pub name: String,
    pub has_memories: bool,
    pub memory_count: i64,
    pub is_custom: bool,
}

/// Event payload for memory optimizer log messages
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct MemoryOptimizerLogEvent {
    pub message: String,
}

/// Event payload for memory optimizer status changes
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct MemoryOptimizerUpdatedEvent {
    pub run_id: i64,
    pub status: String,
}

/// Event payload for memory files changed
#[allow(dead_code)]
#[derive(Serialize, Clone, Debug)]
pub struct MemoryFilesUpdatedEvent {
    pub project_path: String,
}

// --- Code change stats models ---

/// Aggregate code change stats for a time range
#[derive(Serialize, Clone, Debug)]
pub struct CodeStats {
    pub lines_added: i64,
    pub lines_removed: i64,
    pub net_change: i64,
    pub session_count: i64,
    pub avg_per_session: f64,
    pub by_language: Vec<LanguageBreakdown>,
}

/// Per-language breakdown of code changes
#[derive(Serialize, Clone, Debug)]
pub struct LanguageBreakdown {
    pub language: String,
    pub lines: i64,
    pub percentage: f64,
}

/// Time-bucketed code change data point for charts
#[derive(Serialize, Clone, Debug)]
pub struct CodeStatsHistoryPoint {
    pub timestamp: String,
    pub lines_added: i64,
    pub lines_removed: i64,
    pub total_changed: i64,
}

/// Per-session code change stats
#[derive(Serialize, Clone, Debug)]
pub struct SessionCodeStats {
    pub lines_added: i64,
    pub lines_removed: i64,
    pub net_change: i64,
}

/// Aggregate response time stats for a time range
#[derive(Serialize, Clone, Debug)]
pub struct ResponseTimeStats {
    pub avg_response_secs: f64,
    pub peak_response_secs: f64,
    pub avg_idle_secs: f64,
    pub sample_count: i64,
    pub sparkline: Vec<f64>,
}
