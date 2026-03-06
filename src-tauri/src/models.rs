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

// Haiku analysis output item (parsed from JSON)
#[derive(Deserialize, Clone, Debug)]
pub struct AnalysisRule {
    pub name: String,
    pub domain: String,
    pub confidence: f64,
    pub content: String,
}

// Verdict on an existing rule from LLM analysis
#[derive(Deserialize, Clone, Debug)]
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
#[derive(Deserialize, Clone, Debug)]
pub struct AnalysisOutput {
    #[serde(default)]
    pub new_rules: Vec<AnalysisRule>,
    #[serde(default)]
    pub verdicts: Vec<RuleVerdict>,
}
