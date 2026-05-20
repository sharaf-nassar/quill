//! Counterfactual evaluation / frozen-replay harness for learned rules.
//!
//! Feature 005 (Learning System Hardening), finding C-4 / contract
//! `specs/005-learning-system-hardening/contracts/evaluation-harness.md`
//! (Decisions 1-6) and research.md R-4.
//!
//! For each frozen replay case the harness runs the candidate rule WITH and
//! WITHOUT through the local [`crate::cc_client`] path (pinned
//! [`Model::Sonnet46`], `N=3` majority/median), then asks a calibrated judge
//! for a typed [`EvalVerdict`]. A signed `delta` past a dead-band — or a
//! negative-transfer signal — is a regression. The judge is calibrated against
//! the maintainer-authored `expected_judgment` labels (Cohen's κ vs the frozen
//! set); below the agreement floor the verdict is advisory, not blocking. A
//! replay set whose judge model differs from the pinned baseline, or whose
//! `frozen_at` is older than `staleness_days`, is flagged stale (disclosed,
//! never auto-blocking).
//!
//! Implemented in User Story 4 (tasks T050/T051/T055). Persistence to
//! `evaluation_results` (T052) and the promotion-coupling gate (T053) are
//! now wired: [`run_evaluation`] / [`EvalOutcome`] are consumed by `lib.rs`
//! and [`EvalVerdictRow`] / [`RuleUnderTest`] by `storage.rs`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cc_client::{self, InvokeArgs, Model, Phase};

// ---------------------------------------------------------------------------
// Tunable constants (contract Decisions 3/4/6).
// ---------------------------------------------------------------------------

/// Signed-delta dead-band. `delta < -EPSILON` (quality dropped by more than
/// this) is a regression; `|delta| <= EPSILON` is within noise (no
/// regression). Contract Decision 3 (`≈0.05`).
pub const EPSILON: f64 = 0.05;

/// Paired WITH/WITHOUT + judge repetitions. The per-arm quality is the median
/// of `N` samples and the verdict is the majority across `N` judge calls;
/// majority/median dampens model non-determinism (contract Decision 3).
pub const N_REPEATS: usize = 3;

/// Cohen's κ floor for treating the judge as calibrated against the frozen
/// labels. Below this the harness still produces verdicts but marks them
/// advisory (`judge_uncalibrated = true`) — contract Decision 4 (`≈0.6`).
pub const KAPPA_FLOOR: f64 = 0.6;

/// Per-arm sample dispersion (max − min over the `N` quality samples) above
/// which a case is `inconclusive` rather than scored — "high per-arm variance
/// ⇒ inconclusive" (contract Decision 3).
pub const VARIANCE_INCONCLUSIVE: f64 = 0.34;

// ---------------------------------------------------------------------------
// Replay-set model (contract Decision 1 / data-model "Replay Set").
// ---------------------------------------------------------------------------

/// `manifest.json` at the root of the in-repo replay set. Mirrors the
/// finalized schema authored by T050.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReplayManifest {
    pub replay_set_version: i64,
    pub baseline_assistant_model: String,
    /// RFC3339 instant the set was frozen. Drives the age component of the
    /// staleness verdict.
    pub frozen_at: String,
    pub schema_version: i64,
    /// Age threshold for the staleness verdict (`≈90`, contract Decision 1).
    pub staleness_days: i64,
    /// Ordered list of case file stems (no `.json`).
    pub cases: Vec<String>,
}

/// One frozen, pre-redacted judgment case (`case_NNN_<slug>.json`).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReplayCase {
    /// Stem the case was loaded from (e.g. `case_001_absolute_paths_positive`).
    /// Populated by the loader, not present in the on-disk JSON.
    #[serde(default)]
    pub id: String,
    pub inputs: CaseInputs,
    pub rule_under_test: RuleUnderTest,
    pub expected_judgment: ExpectedJudgment,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CaseInputs {
    pub corpus: String,
    pub existing_rules_summary: String,
    pub provider_scope: Vec<String>,
    pub stream: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuleUnderTest {
    pub name: String,
    pub content: String,
    pub domain: String,
    pub claimed_confidence: f64,
}

/// Maintainer-authored ground-truth label for a case. Used only for
/// calibration (κ) — never fed back into the judge prompt.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExpectedJudgment {
    /// `helps` | `neutral` | `regresses`.
    pub verdict: String,
    pub rationale: String,
}

// ---------------------------------------------------------------------------
// Typed judge output (contract Decision 3) — `schemars::JsonSchema` so it can
// be the `T` of `cc_client::invoke_typed`.
// ---------------------------------------------------------------------------

/// Calibrated judge verdict for a single case (WITH vs WITHOUT). This is the
/// exact shape the judge model is asked to produce via [`invoke_typed`].
///
/// `regression` is derived deterministically by the harness from `delta` and
/// `negative_transfer` (see [`is_regression`]); the model populates the four
/// substantive fields and a free-text rationale.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct EvalVerdict {
    /// Quality of the WITH-rule arm in `[0,1]` (median over `N`).
    pub with_quality: f64,
    /// Quality of the WITHOUT-rule arm in `[0,1]` (median over `N`).
    pub without_quality: f64,
    /// `with_quality - without_quality`. Positive ⇒ the rule helped.
    pub delta: f64,
    /// Deterministic dead-band/negative-transfer regression flag.
    pub regression: bool,
    /// The rule helped the source archetype but would harm a different
    /// context (cross-domain misapplication).
    pub negative_transfer: bool,
    pub rationale: String,
}

// ---------------------------------------------------------------------------
// Result structs (contract Decision 5/6). T052 persists `EvalVerdictRow`;
// T053's promotion gate reads it. Defined here, written elsewhere.
// ---------------------------------------------------------------------------

/// Coarse verdict label persisted on the row and consulted by the promotion
/// gate. `Regresses` blocks `approve` unless an audited reviewer override
/// exists (FR-020); the soft labels only warn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalLabel {
    Helps,
    Neutral,
    Regresses,
    /// High per-arm variance — no trustworthy delta.
    Inconclusive,
}

impl EvalLabel {
    /// Stable lowercase string for the `evaluation_results.verdict` column.
    pub fn as_str(self) -> &'static str {
        match self {
            EvalLabel::Helps => "helps",
            EvalLabel::Neutral => "neutral",
            EvalLabel::Regresses => "regresses",
            EvalLabel::Inconclusive => "inconclusive",
        }
    }
}

/// Per-case evaluation outcome (one element of `EvalOutcome::cases`, serialized
/// into `evaluation_results.per_case_json`).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CaseResult {
    pub case_id: String,
    /// `None` when the case was `inconclusive` (no usable judge verdict).
    pub verdict: Option<EvalVerdict>,
    pub label: EvalLabel,
    /// Maintainer label for this case (carried for offline calibration audit).
    pub expected_verdict: String,
    pub tags: Vec<String>,
}

/// Calibration summary (contract Decision 4). `agreement` is raw accuracy of
/// the judge's coarse label vs the frozen `expected_judgment`; `kappa` is
/// chance-corrected Cohen's κ. Below [`KAPPA_FLOOR`] verdicts are advisory.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Calibration {
    pub scored_cases: usize,
    pub agreement: f64,
    pub kappa: f64,
    pub judge_uncalibrated: bool,
}

/// Staleness verdict (contract Decision 1 / FR-023). Stale iff the judge model
/// differs from the pinned baseline OR the set is older than
/// `staleness_days`. Disclosed; never auto-blocking.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvalStaleness {
    pub stale: bool,
    pub judge_model_mismatch: bool,
    pub age_exceeded: bool,
    pub age_days: i64,
    pub baseline_assistant_model: String,
    pub judge_model: String,
}

/// Rich result for one rule evaluated against the whole replay set. The
/// flattened scalar projection persisted by T052 is [`EvalVerdictRow`].
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvalOutcome {
    pub rule_name: String,
    /// Filled by the caller (T052) when an originating run id is known; the
    /// harness itself runs replay-set-only and leaves this `None`.
    pub learning_run_id: Option<i64>,
    pub replay_set_version: i64,
    pub judge_model: String,
    /// Aggregate verdict over all scored cases.
    pub verdict: EvalLabel,
    pub regression: bool,
    pub negative_transfer: bool,
    pub calibration: Calibration,
    pub staleness: EvalStaleness,
    pub cases: Vec<CaseResult>,
}

impl EvalOutcome {
    /// Scalar projection T052 will INSERT into `evaluation_results`
    /// (`per_case_json` is serialized separately from [`Self::cases`]).
    pub fn to_row(&self) -> EvalVerdictRow {
        EvalVerdictRow {
            rule_name: self.rule_name.clone(),
            learning_run_id: self.learning_run_id,
            replay_set_version: self.replay_set_version,
            judge_model: self.judge_model.clone(),
            verdict: self.verdict.as_str().to_string(),
            delta: aggregate_delta(&self.cases),
            regression: self.regression,
            negative_transfer: self.negative_transfer,
            judge_uncalibrated: self.calibration.judge_uncalibrated,
            replay_set_stale: self.staleness.stale,
            agreement_score: self.calibration.kappa,
        }
    }
}

/// Flattened scalar row matching the `evaluation_results` column set
/// (data-model.md). **Owned by T052** for persistence and by T053's
/// `latest_eval_verdict(...)` promotion gate — defined here so those tasks
/// build on a stable shape; this module never writes it to storage.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvalVerdictRow {
    pub rule_name: String,
    pub learning_run_id: Option<i64>,
    pub replay_set_version: i64,
    pub judge_model: String,
    pub verdict: String,
    pub delta: f64,
    pub regression: bool,
    pub negative_transfer: bool,
    pub judge_uncalibrated: bool,
    pub replay_set_stale: bool,
    pub agreement_score: f64,
}

/// Error surface for the harness (mirrors the plain-enum style of
/// [`cc_client::InferenceError`]; no new crates).
#[derive(Debug)]
#[non_exhaustive]
pub enum EvalError {
    /// Replay set directory / `manifest.json` / a case file is missing or
    /// unreadable.
    ReplaySetIo(String),
    /// `manifest.json` or a case file did not parse against the schema.
    ReplaySetParse(String),
    /// A WITH/WITHOUT/judge inference call failed (propagated).
    Inference(cc_client::InferenceError),
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::ReplaySetIo(m) => write!(f, "replay set io error: {m}"),
            EvalError::ReplaySetParse(m) => write!(f, "replay set parse error: {m}"),
            EvalError::Inference(e) => write!(f, "inference error during evaluation: {e}"),
        }
    }
}

impl std::error::Error for EvalError {}

impl From<cc_client::InferenceError> for EvalError {
    fn from(e: cc_client::InferenceError) -> Self {
        EvalError::Inference(e)
    }
}

// ---------------------------------------------------------------------------
// Replay-set loader (contract Decision 1).
// ---------------------------------------------------------------------------

/// In-repo replay set root: `<crate>/tests/fixtures/replay_set`. Resolved from
/// `CARGO_MANIFEST_DIR` so it is deterministic regardless of the process CWD.
pub fn default_replay_set_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("replay_set")
}

/// Loaded replay set: parsed manifest + every case in manifest order.
#[derive(Clone, Debug)]
pub struct ReplaySet {
    pub manifest: ReplayManifest,
    pub cases: Vec<ReplayCase>,
}

/// Load + parse the manifest and every listed case file from `dir`. Cases are
/// returned in manifest order with `id` populated from the file stem.
pub fn load_replay_set(dir: &Path) -> Result<ReplaySet, EvalError> {
    let manifest_path = dir.join("manifest.json");
    let manifest_raw = std::fs::read_to_string(&manifest_path)
        .map_err(|e| EvalError::ReplaySetIo(format!("{}: {e}", manifest_path.display())))?;
    let manifest: ReplayManifest = serde_json::from_str(&manifest_raw)
        .map_err(|e| EvalError::ReplaySetParse(format!("{}: {e}", manifest_path.display())))?;

    let mut cases = Vec::with_capacity(manifest.cases.len());
    for stem in &manifest.cases {
        let case_path = dir.join(format!("{stem}.json"));
        let raw = std::fs::read_to_string(&case_path)
            .map_err(|e| EvalError::ReplaySetIo(format!("{}: {e}", case_path.display())))?;
        let mut case: ReplayCase = serde_json::from_str(&raw)
            .map_err(|e| EvalError::ReplaySetParse(format!("{}: {e}", case_path.display())))?;
        case.id = stem.clone();
        cases.push(case);
    }

    Ok(ReplaySet { manifest, cases })
}

// ---------------------------------------------------------------------------
// Staleness verdict (contract Decision 1 / FR-023).
// ---------------------------------------------------------------------------

/// Compute the staleness verdict for a manifest evaluated by `judge_model`
/// at `now`. Stale iff the judge model differs from the pinned baseline OR
/// `age(frozen_at) > staleness_days`. An unparseable `frozen_at` is treated
/// as age-exceeded (conservative: a malformed freeze stamp cannot certify
/// freshness).
pub fn compute_staleness(
    manifest: &ReplayManifest,
    judge_model: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> EvalStaleness {
    let judge_model_mismatch = judge_model != manifest.baseline_assistant_model;

    let (age_days, age_exceeded) = match chrono::DateTime::parse_from_rfc3339(&manifest.frozen_at) {
        Ok(frozen) => {
            let age = now.signed_duration_since(frozen.with_timezone(&chrono::Utc));
            let days = age.num_days();
            (days, days > manifest.staleness_days)
        }
        Err(_) => (i64::MAX, true),
    };

    EvalStaleness {
        stale: judge_model_mismatch || age_exceeded,
        judge_model_mismatch,
        age_exceeded,
        age_days,
        baseline_assistant_model: manifest.baseline_assistant_model.clone(),
        judge_model: judge_model.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Pure scoring logic (contract Decision 3) — unit-tested by T055.
// ---------------------------------------------------------------------------

/// Deterministic regression rule: a quality drop deeper than the dead-band OR
/// any negative-transfer signal. `|delta| <= EPSILON` is within noise.
pub fn is_regression(delta: f64, negative_transfer: bool) -> bool {
    negative_transfer || delta < -EPSILON
}

/// Median of an `f64` sample (used for per-arm quality over `N` repeats).
/// Empty input ⇒ `0.0` (no signal). NaNs sort last but inputs are model
/// quality scores in `[0,1]`.
pub fn median(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut v = samples.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Greater));
    let mid = v.len() / 2;
    if v.len().is_multiple_of(2) {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    }
}

/// Max − min dispersion of a sample. Used for the per-arm
/// variance→`inconclusive` gate.
pub fn dispersion(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for &s in samples {
        lo = lo.min(s);
        hi = hi.max(s);
    }
    hi - lo
}

/// Strict majority element of a label sample (`> half`). `None` when no label
/// has a strict majority (e.g. a 1/1/1 three-way split) — the caller maps
/// `None` to [`EvalLabel::Inconclusive`] (contract Decision 3).
pub fn majority<T: Clone + PartialEq>(samples: &[T]) -> Option<T> {
    if samples.is_empty() {
        return None;
    }
    let mut best: Option<(T, usize)> = None;
    for s in samples {
        let count = samples.iter().filter(|x| *x == s).count();
        match &best {
            Some((_, bc)) if *bc >= count => {}
            _ => best = Some((s.clone(), count)),
        }
    }
    match best {
        Some((val, count)) if count * 2 > samples.len() => Some(val),
        _ => None,
    }
}

/// Map a single judge [`EvalVerdict`] to its coarse [`EvalLabel`]. A
/// deterministic regression dominates; otherwise a positive delta past the
/// dead-band is `Helps` and everything inside the band is `Neutral`.
pub fn label_from_verdict(v: &EvalVerdict) -> EvalLabel {
    if is_regression(v.delta, v.negative_transfer) {
        EvalLabel::Regresses
    } else if v.delta > EPSILON {
        EvalLabel::Helps
    } else {
        EvalLabel::Neutral
    }
}

/// Aggregate the per-case deltas into one representative delta for the row
/// (median over scored cases; `0.0` if none were scorable).
pub fn aggregate_delta(cases: &[CaseResult]) -> f64 {
    let deltas: Vec<f64> = cases
        .iter()
        .filter_map(|c| c.verdict.as_ref().map(|v| v.delta))
        .collect();
    median(&deltas)
}

/// Roll the per-case labels up to one outcome verdict. Any non-advisory
/// `Regresses` dominates (promotion-blocking signal); else `Helps` if any
/// case helped and none regressed; else `Neutral`; `Inconclusive` only when
/// every scorable case was inconclusive.
pub fn aggregate_label(cases: &[CaseResult]) -> EvalLabel {
    if cases.is_empty() {
        return EvalLabel::Inconclusive;
    }
    let mut any_helps = false;
    let mut any_scorable = false;
    for c in cases {
        match c.label {
            EvalLabel::Regresses => return EvalLabel::Regresses,
            EvalLabel::Helps => {
                any_helps = true;
                any_scorable = true;
            }
            EvalLabel::Neutral => any_scorable = true,
            EvalLabel::Inconclusive => {}
        }
    }
    if !any_scorable {
        EvalLabel::Inconclusive
    } else if any_helps {
        EvalLabel::Helps
    } else {
        EvalLabel::Neutral
    }
}

// ---------------------------------------------------------------------------
// Calibration: Cohen's κ vs frozen labels (contract Decision 4).
// ---------------------------------------------------------------------------

/// Cohen's κ + raw agreement between predicted and expected coarse labels
/// over the three-class set `{helps, neutral, regresses}`. Pairs whose
/// prediction is `inconclusive` are excluded from the scored denominator.
///
/// κ = (p_o − p_e) / (1 − p_e); when the marginals make `p_e == 1.0`
/// (degenerate single-class set) κ is defined as `1.0` iff `p_o == 1.0`
/// else `0.0`.
pub fn cohen_kappa(pairs: &[(EvalLabel, &str)]) -> Calibration {
    const CLASSES: [&str; 3] = ["helps", "neutral", "regresses"];

    let scored: Vec<(&str, &str)> = pairs
        .iter()
        .filter_map(|(pred, exp)| {
            let p = match pred {
                EvalLabel::Helps => "helps",
                EvalLabel::Neutral => "neutral",
                EvalLabel::Regresses => "regresses",
                EvalLabel::Inconclusive => return None,
            };
            Some((p, *exp))
        })
        .collect();

    let n = scored.len();
    if n == 0 {
        return Calibration {
            scored_cases: 0,
            agreement: 0.0,
            kappa: 0.0,
            judge_uncalibrated: true,
        };
    }

    let n_f = n as f64;
    let agree = scored.iter().filter(|(p, e)| p == e).count() as f64;
    let p_o = agree / n_f;

    // Expected agreement from the marginal label frequencies.
    let mut p_e = 0.0;
    for cls in CLASSES {
        let pred_marg = scored.iter().filter(|(p, _)| *p == cls).count() as f64 / n_f;
        let exp_marg = scored.iter().filter(|(_, e)| *e == cls).count() as f64 / n_f;
        p_e += pred_marg * exp_marg;
    }

    let kappa = if (1.0 - p_e).abs() < f64::EPSILON {
        if (p_o - 1.0).abs() < f64::EPSILON {
            1.0
        } else {
            0.0
        }
    } else {
        (p_o - p_e) / (1.0 - p_e)
    };

    Calibration {
        scored_cases: n,
        agreement: p_o,
        kappa,
        judge_uncalibrated: kappa < KAPPA_FLOOR,
    }
}

// ---------------------------------------------------------------------------
// Counterfactual evaluation driver (contract Decisions 2/3/5/6).
// ---------------------------------------------------------------------------

/// Judge model identity for this build. Synthesis/eval are pinned to the full
/// Sonnet 4.6 name (not the rolling `sonnet` alias) so calibration and
/// staleness compare against a stable model string (contract Decision 1; R-7
/// H-7 single-model pipeline).
pub const JUDGE_MODEL: &str = "claude-sonnet-4-6";

/// Build the WITHOUT-rule arm prompt for a case (the rule is absent; the
/// model produces a quality-scored solution sketch for the corpus task).
fn without_prompt(case: &ReplayCase) -> String {
    format!(
        "You are evaluating agent behavior quality for a behavioral-learning \
         replay case.\n\nCORPUS (pre-redacted, synthetic):\n{corpus}\n\n\
         EXISTING RULES:\n{rules}\n\nProvider scope: {scope:?}. Stream: \
         {stream}.\n\nNo additional candidate rule is in effect. Assess the \
         quality of correct agent behavior on this corpus on a 0.0-1.0 scale.",
        corpus = case.inputs.corpus,
        rules = case.inputs.existing_rules_summary,
        scope = case.inputs.provider_scope,
        stream = case.inputs.stream,
    )
}

/// Build the WITH-rule arm prompt: identical to [`without_prompt`] plus the
/// `rule_under_test` injected as an active candidate.
fn with_prompt(case: &ReplayCase) -> String {
    format!(
        "{base}\n\nCANDIDATE RULE NOW IN EFFECT:\nname: {name}\ndomain: \
         {domain}\nclaimed_confidence: {conf}\ncontent: {content}\n\nReassess \
         the quality of correct agent behavior on this corpus on a 0.0-1.0 \
         scale GIVEN this candidate rule is active.",
        base = without_prompt(case),
        name = case.rule_under_test.name,
        domain = case.rule_under_test.domain,
        conf = case.rule_under_test.claimed_confidence,
        content = case.rule_under_test.content,
    )
}

/// Single-field quality probe deserialized from one WITH/WITHOUT arm call.
/// Kept separate from [`EvalVerdict`] so each arm round-trips through
/// `invoke_typed` with a minimal schema.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct ArmQuality {
    /// Behavior quality for this arm in `[0,1]`.
    quality: f64,
}

/// Judge-call prompt: given the two arm qualities, emit a typed
/// [`EvalVerdict`]. The frozen `expected_judgment` is deliberately NOT shown
/// (calibration must score the judge, not leak the answer).
fn judge_prompt(case: &ReplayCase, with_q: f64, without_q: f64) -> String {
    format!(
        "Judge a counterfactual behavioral-rule evaluation.\n\nCORPUS:\n\
         {corpus}\n\nCANDIDATE RULE:\nname: {name}\ncontent: {content}\n\n\
         Measured WITH-rule quality: {with_q:.4}\nMeasured WITHOUT-rule \
         quality: {without_q:.4}\n\nReturn with_quality, without_quality, \
         delta (with - without), negative_transfer (true iff the rule would \
         harm a context different from the one it was derived from), \
         regression, and a one-paragraph rationale.",
        corpus = case.inputs.corpus,
        name = case.rule_under_test.name,
        content = case.rule_under_test.content,
    )
}

fn invoke_args(phase: Phase, prompt: String, preamble: &str) -> InvokeArgs {
    InvokeArgs {
        phase,
        prompt,
        preamble: preamble.to_string(),
        // Pinned per contract Decision 2 — NOT the rolling `sonnet` alias.
        model: Model::Sonnet46,
        max_tokens: 2048,
    }
}

/// Run the WITH/WITHOUT/judge counterfactual for one case: `N` paired arm
/// samples → median per arm → variance gate → `N` judge calls → majority
/// label. Returns the representative verdict + coarse label.
async fn evaluate_case(case: &ReplayCase) -> Result<CaseResult, EvalError> {
    let mut with_samples = Vec::with_capacity(N_REPEATS);
    let mut without_samples = Vec::with_capacity(N_REPEATS);

    for _ in 0..N_REPEATS {
        let with: cc_client::InvokeOutcome<ArmQuality> = cc_client::invoke_typed(invoke_args(
            Phase::Synthesis,
            with_prompt(case),
            "Behavioral-rule counterfactual evaluator (WITH arm).",
        ))
        .await?;
        let without: cc_client::InvokeOutcome<ArmQuality> = cc_client::invoke_typed(invoke_args(
            Phase::Synthesis,
            without_prompt(case),
            "Behavioral-rule counterfactual evaluator (WITHOUT arm).",
        ))
        .await?;
        with_samples.push(with.value.quality);
        without_samples.push(without.value.quality);
    }

    // High per-arm variance ⇒ no trustworthy delta (contract Decision 3).
    if dispersion(&with_samples) > VARIANCE_INCONCLUSIVE
        || dispersion(&without_samples) > VARIANCE_INCONCLUSIVE
    {
        return Ok(CaseResult {
            case_id: case.id.clone(),
            verdict: None,
            label: EvalLabel::Inconclusive,
            expected_verdict: case.expected_judgment.verdict.clone(),
            tags: case.tags.clone(),
        });
    }

    let with_q = median(&with_samples);
    let without_q = median(&without_samples);

    let mut verdicts: VecDeque<EvalVerdict> = VecDeque::with_capacity(N_REPEATS);
    for _ in 0..N_REPEATS {
        let judged: cc_client::InvokeOutcome<EvalVerdict> = cc_client::invoke_typed(invoke_args(
            Phase::Synthesis,
            judge_prompt(case, with_q, without_q),
            "Calibrated behavioral-rule judge.",
        ))
        .await?;
        verdicts.push_back(judged.value);
    }

    let labels: Vec<EvalLabel> = verdicts.iter().map(label_from_verdict).collect();
    let label = majority(&labels).unwrap_or(EvalLabel::Inconclusive);

    // Representative verdict: the first judge sample whose label matches the
    // majority (stable, deterministic given the FIFO double); fall back to
    // the first verdict when inconclusive so the delta is still reported.
    let rep = verdicts
        .iter()
        .find(|v| label_from_verdict(v) == label)
        .or_else(|| verdicts.front())
        .cloned()
        .map(|mut v| {
            // Recompute the deterministic flag from the harness rule so the
            // persisted verdict cannot disagree with `is_regression`.
            v.regression = is_regression(v.delta, v.negative_transfer);
            v
        });

    Ok(CaseResult {
        case_id: case.id.clone(),
        verdict: rep,
        label,
        expected_verdict: case.expected_judgment.verdict.clone(),
        tags: case.tags.clone(),
    })
}

/// Evaluate one candidate `rule` against the in-repo replay set.
///
/// This is the public entry point T052 (persistence) and T053 (promotion
/// coupling) build on: it returns a rich [`EvalOutcome`] whose
/// [`EvalOutcome::to_row`] yields the scalar [`EvalVerdictRow`] for
/// `evaluation_results`. The harness itself performs NO storage I/O.
///
/// In production every WITH/WITHOUT/judge call goes through the real
/// [`cc_client`] spawn; under `#[cfg(test)]` the T011 inference double
/// short-circuits those calls with scripted JSON (no live `claude`).
pub async fn run_evaluation(rule: RuleUnderTest) -> Result<EvalOutcome, EvalError> {
    let dir = default_replay_set_dir();
    let set = load_replay_set(&dir)?;
    run_evaluation_with_set(rule, &set, chrono::Utc::now()).await
}

/// Testable core of [`run_evaluation`] with an explicit replay set + clock so
/// the staleness verdict is deterministic. The candidate `rule` is
/// substituted into every case's `rule_under_test` so one rule is scored
/// across all archetypes.
pub async fn run_evaluation_with_set(
    rule: RuleUnderTest,
    set: &ReplaySet,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<EvalOutcome, EvalError> {
    let staleness = compute_staleness(&set.manifest, JUDGE_MODEL, now);

    let mut results = Vec::with_capacity(set.cases.len());
    for case in &set.cases {
        // Substitute the rule under evaluation into the frozen case context;
        // the corpus / existing-rules / labels stay frozen.
        let mut c = case.clone();
        c.rule_under_test = rule.clone();
        results.push(evaluate_case(&c).await?);
    }

    let cal_pairs: Vec<(EvalLabel, &str)> = results
        .iter()
        .map(|r| (r.label, r.expected_verdict.as_str()))
        .collect();
    let calibration = cohen_kappa(&cal_pairs);

    let verdict = aggregate_label(&results);
    let regression = results
        .iter()
        .any(|r| r.verdict.as_ref().is_some_and(|v| v.regression));
    let negative_transfer = results
        .iter()
        .any(|r| r.verdict.as_ref().is_some_and(|v| v.negative_transfer));

    Ok(EvalOutcome {
        rule_name: rule.name,
        learning_run_id: None,
        replay_set_version: set.manifest.replay_set_version,
        judge_model: JUDGE_MODEL.to_string(),
        verdict,
        regression,
        negative_transfer,
        calibration,
        staleness,
        cases: results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cc_client::{ScriptedResponse, clear_inference_double, set_inference_double};
    use serial_test::serial;

    // ---- contract Decision 1: replay-set loader -------------------------

    // @lat: [[features#Learning System#Counterfactual Evaluation Harness]]
    #[test]
    fn loads_frozen_replay_set_in_manifest_order() {
        let set = load_replay_set(&default_replay_set_dir()).expect("in-repo replay set must load");
        assert_eq!(set.manifest.replay_set_version, 1);
        assert_eq!(set.manifest.baseline_assistant_model, "claude-sonnet-4-6");
        assert!(
            set.cases.len() >= 12,
            "replay set must have >=12 cases, got {}",
            set.cases.len()
        );
        // Loader populates `id` from the file stem, in manifest order.
        assert_eq!(set.cases.len(), set.manifest.cases.len());
        for (case, stem) in set.cases.iter().zip(&set.manifest.cases) {
            assert_eq!(&case.id, stem);
            assert!(
                matches!(
                    case.expected_judgment.verdict.as_str(),
                    "helps" | "neutral" | "regresses"
                ),
                "{}: bad expected verdict {}",
                case.id,
                case.expected_judgment.verdict
            );
        }
        // All seven R-4 archetypes are represented via tags.
        let tags: std::collections::HashSet<&str> = set
            .cases
            .iter()
            .flat_map(|c| c.tags.iter().map(String::as_str))
            .collect();
        for arch in [
            "archetype:positive",
            "archetype:regressing",
            "archetype:negative-transfer",
            "archetype:hallucinated",
            "archetype:one-off",
            "archetype:conflicting",
            "archetype:suppressed-rederived",
            "archetype:empty",
        ] {
            assert!(tags.contains(arch), "missing archetype tag {arch}");
        }
    }

    #[test]
    fn missing_replay_set_dir_is_io_error() {
        let err = load_replay_set(Path::new("/nonexistent/replay_set_xyz"))
            .expect_err("missing dir must error");
        assert!(matches!(err, EvalError::ReplaySetIo(_)), "got {err:?}");
    }

    // ---- contract Decision 3: dead-band regression rule -----------------

    // @lat: [[features#Learning System#Counterfactual Evaluation Harness]]
    #[test]
    fn dead_band_regression_rule() {
        // Drop deeper than the dead-band ⇒ regression.
        assert!(is_regression(-0.06, false));
        assert!(is_regression(-1.0, false));
        // Exactly at / within the band ⇒ NOT a regression.
        assert!(!is_regression(-EPSILON, false));
        assert!(!is_regression(-0.04, false));
        assert!(!is_regression(0.0, false));
        assert!(!is_regression(0.5, false));
        // Negative transfer always regresses, even with a positive delta.
        assert!(is_regression(0.9, true));
        assert!(is_regression(0.0, true));
    }

    #[test]
    fn label_from_verdict_maps_dead_band_and_negative_transfer() {
        let mk = |delta: f64, nt: bool| EvalVerdict {
            with_quality: 0.5 + delta / 2.0,
            without_quality: 0.5 - delta / 2.0,
            delta,
            regression: false,
            negative_transfer: nt,
            rationale: "t".into(),
        };
        assert_eq!(label_from_verdict(&mk(0.2, false)), EvalLabel::Helps);
        assert_eq!(label_from_verdict(&mk(0.0, false)), EvalLabel::Neutral);
        assert_eq!(label_from_verdict(&mk(0.03, false)), EvalLabel::Neutral);
        assert_eq!(label_from_verdict(&mk(-0.2, false)), EvalLabel::Regresses);
        assert_eq!(label_from_verdict(&mk(0.9, true)), EvalLabel::Regresses);
    }

    // ---- contract Decision 3: majority-of-N + median --------------------

    // @lat: [[features#Learning System#Counterfactual Evaluation Harness]]
    #[test]
    fn majority_of_n_aggregation_including_no_majority() {
        // Strict majority (2 of 3).
        assert_eq!(
            majority(&[EvalLabel::Helps, EvalLabel::Helps, EvalLabel::Neutral]),
            Some(EvalLabel::Helps)
        );
        // Unanimous.
        assert_eq!(
            majority(&[
                EvalLabel::Regresses,
                EvalLabel::Regresses,
                EvalLabel::Regresses
            ]),
            Some(EvalLabel::Regresses)
        );
        // Three-way 1/1/1 split ⇒ no strict majority ⇒ None (inconclusive).
        assert_eq!(
            majority(&[EvalLabel::Helps, EvalLabel::Neutral, EvalLabel::Regresses]),
            None
        );
        // Even split 2/2 ⇒ not a strict majority.
        assert_eq!(
            majority(&[
                EvalLabel::Helps,
                EvalLabel::Helps,
                EvalLabel::Neutral,
                EvalLabel::Neutral
            ]),
            None
        );
        assert_eq!(majority::<EvalLabel>(&[]), None);
    }

    #[test]
    fn median_and_dispersion_pure_logic() {
        assert!((median(&[0.2, 0.8, 0.5]) - 0.5).abs() < 1e-9);
        assert!((median(&[0.2, 0.8]) - 0.5).abs() < 1e-9);
        assert!((median(&[]) - 0.0).abs() < 1e-9);
        assert!((dispersion(&[0.2, 0.9, 0.5]) - 0.7).abs() < 1e-9);
        assert!((dispersion(&[0.4]) - 0.0).abs() < 1e-9);
        assert!((dispersion(&[]) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn aggregate_label_and_delta_rollup() {
        let case = |label: EvalLabel, delta: f64| CaseResult {
            case_id: "c".into(),
            verdict: Some(EvalVerdict {
                with_quality: 0.0,
                without_quality: 0.0,
                delta,
                regression: is_regression(delta, false),
                negative_transfer: false,
                rationale: String::new(),
            }),
            label,
            expected_verdict: "neutral".into(),
            tags: vec![],
        };
        // Any Regresses dominates the rollup.
        assert_eq!(
            aggregate_label(&[
                case(EvalLabel::Helps, 0.3),
                case(EvalLabel::Regresses, -0.4)
            ]),
            EvalLabel::Regresses
        );
        // Helps when some help and none regress.
        assert_eq!(
            aggregate_label(&[case(EvalLabel::Helps, 0.3), case(EvalLabel::Neutral, 0.0)]),
            EvalLabel::Helps
        );
        // All neutral ⇒ Neutral.
        assert_eq!(
            aggregate_label(&[
                case(EvalLabel::Neutral, 0.0),
                case(EvalLabel::Neutral, 0.01)
            ]),
            EvalLabel::Neutral
        );
        // No scorable cases ⇒ Inconclusive.
        assert_eq!(aggregate_label(&[]), EvalLabel::Inconclusive);
        // Median delta over scored cases.
        assert!(
            (aggregate_delta(&[case(EvalLabel::Helps, 0.2), case(EvalLabel::Helps, 0.6)]) - 0.4)
                .abs()
                < 1e-9
        );
    }

    // ---- contract Decision 4: κ / agreement calibration -----------------

    // @lat: [[features#Learning System#Counterfactual Evaluation Harness]]
    #[test]
    fn kappa_agreement_computation() {
        // Perfect agreement on a multi-class set ⇒ κ == 1.0, calibrated.
        let perfect = vec![
            (EvalLabel::Helps, "helps"),
            (EvalLabel::Neutral, "neutral"),
            (EvalLabel::Regresses, "regresses"),
            (EvalLabel::Helps, "helps"),
        ];
        let c = cohen_kappa(&perfect);
        assert_eq!(c.scored_cases, 4);
        assert!((c.agreement - 1.0).abs() < 1e-9);
        assert!((c.kappa - 1.0).abs() < 1e-9, "kappa={}", c.kappa);
        assert!(!c.judge_uncalibrated);

        // Total disagreement on a balanced 2-class set ⇒ κ <= 0, uncalibrated.
        let bad = vec![
            (EvalLabel::Helps, "regresses"),
            (EvalLabel::Regresses, "helps"),
            (EvalLabel::Helps, "regresses"),
            (EvalLabel::Regresses, "helps"),
        ];
        let cb = cohen_kappa(&bad);
        assert!(cb.kappa <= 0.0 + 1e-9, "kappa={}", cb.kappa);
        assert!(cb.judge_uncalibrated);

        // `inconclusive` predictions are excluded from the scored denominator.
        let with_inconcl = vec![
            (EvalLabel::Helps, "helps"),
            (EvalLabel::Inconclusive, "regresses"),
            (EvalLabel::Neutral, "neutral"),
        ];
        let ci = cohen_kappa(&with_inconcl);
        assert_eq!(ci.scored_cases, 2);

        // Empty / all-inconclusive ⇒ uncalibrated, zero scored.
        let none = cohen_kappa(&[(EvalLabel::Inconclusive, "helps")]);
        assert_eq!(none.scored_cases, 0);
        assert!(none.judge_uncalibrated);

        // 4/5 agree on a 2-class split → κ ≈ 0.615 (>= 0.6 floor) ⇒ calibrated.
        let near = vec![
            (EvalLabel::Helps, "helps"),
            (EvalLabel::Helps, "helps"),
            (EvalLabel::Regresses, "regresses"),
            (EvalLabel::Regresses, "regresses"),
            (EvalLabel::Helps, "regresses"),
        ];
        let cn = cohen_kappa(&near);
        assert!(cn.kappa >= KAPPA_FLOOR, "kappa={}", cn.kappa);
        assert!(!cn.judge_uncalibrated);
    }

    // ---- contract Decision 1 / FR-023: staleness verdict ----------------

    fn manifest(frozen_at: &str) -> ReplayManifest {
        ReplayManifest {
            replay_set_version: 1,
            baseline_assistant_model: "claude-sonnet-4-6".into(),
            frozen_at: frozen_at.into(),
            schema_version: 1,
            staleness_days: 90,
            cases: vec![],
        }
    }

    // @lat: [[features#Learning System#Counterfactual Evaluation Harness]]
    #[test]
    fn staleness_verdict_model_mismatch_and_age() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-05-18T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        // Fresh + matching judge model ⇒ not stale.
        let fresh = compute_staleness(&manifest("2026-05-01T00:00:00Z"), "claude-sonnet-4-6", now);
        assert!(!fresh.stale);
        assert!(!fresh.judge_model_mismatch);
        assert!(!fresh.age_exceeded);
        assert_eq!(fresh.age_days, 17);

        // Judge model differs from the pinned baseline ⇒ stale (mismatch).
        let mism = compute_staleness(&manifest("2026-05-01T00:00:00Z"), "claude-3-5-haiku", now);
        assert!(mism.stale);
        assert!(mism.judge_model_mismatch);
        assert!(!mism.age_exceeded);

        // frozen_at older than staleness_days ⇒ stale (age).
        let old = compute_staleness(&manifest("2026-01-01T00:00:00Z"), "claude-sonnet-4-6", now);
        assert!(old.stale);
        assert!(!old.judge_model_mismatch);
        assert!(old.age_exceeded);
        assert!(old.age_days > 90);

        // Exactly at the boundary (90 days) is NOT yet stale (strictly `>`).
        let boundary =
            compute_staleness(&manifest("2026-02-17T00:00:00Z"), "claude-sonnet-4-6", now);
        assert_eq!(boundary.age_days, 90);
        assert!(!boundary.age_exceeded);
        assert!(!boundary.stale);

        // Unparseable frozen_at ⇒ conservatively age-exceeded ⇒ stale.
        let bad = compute_staleness(&manifest("not-a-timestamp"), "claude-sonnet-4-6", now);
        assert!(bad.stale);
        assert!(bad.age_exceeded);
    }

    // ---- contract Decisions 2/3/5: end-to-end via the T011 double -------
    //
    // No live `claude`: every WITH/WITHOUT/judge `invoke_typed` is scripted
    // FIFO. `#[serial]` because the double slot is process-global.

    /// FIFO script for one case: N×(WITH quality, WITHOUT quality) then
    /// N×(judge verdict).
    fn script_case(with_q: f64, without_q: f64, delta: f64, nt: bool) -> Vec<ScriptedResponse> {
        let mut v = Vec::new();
        for _ in 0..N_REPEATS {
            v.push(ScriptedResponse::TypedJson(
                serde_json::json!({ "quality": with_q }),
            ));
            v.push(ScriptedResponse::TypedJson(
                serde_json::json!({ "quality": without_q }),
            ));
        }
        for _ in 0..N_REPEATS {
            v.push(ScriptedResponse::TypedJson(serde_json::json!({
                "with_quality": with_q,
                "without_quality": without_q,
                "delta": delta,
                "regression": false,
                "negative_transfer": nt,
                "rationale": "scripted judge verdict"
            })));
        }
        v
    }

    fn one_case_set(expected: &str, tag: &str) -> ReplaySet {
        ReplaySet {
            manifest: ReplayManifest {
                replay_set_version: 7,
                baseline_assistant_model: "claude-sonnet-4-6".into(),
                frozen_at: "2026-05-18T00:00:00Z".into(),
                schema_version: 1,
                staleness_days: 90,
                cases: vec!["case_synth".into()],
            },
            cases: vec![ReplayCase {
                id: "case_synth".into(),
                inputs: CaseInputs {
                    corpus: "synthetic".into(),
                    existing_rules_summary: "none".into(),
                    provider_scope: vec!["claude".into()],
                    stream: "stream_a".into(),
                },
                rule_under_test: RuleUnderTest {
                    name: "placeholder".into(),
                    content: "placeholder".into(),
                    domain: "tooling".into(),
                    claimed_confidence: 0.5,
                },
                expected_judgment: ExpectedJudgment {
                    verdict: expected.into(),
                    rationale: "frozen label".into(),
                },
                tags: vec![tag.into()],
            }],
        }
    }

    fn rut() -> RuleUnderTest {
        RuleUnderTest {
            name: "candidate-under-test".into(),
            content: "Always do the grounded thing.".into(),
            domain: "tooling".into(),
            claimed_confidence: 0.8,
        }
    }

    #[tokio::test]
    #[serial]
    async fn end_to_end_clear_positive_via_double() {
        // WITH clearly better than WITHOUT, no negative transfer ⇒ helps,
        // not a regression, and the judge agrees with a "helps" label.
        set_inference_double(script_case(0.9, 0.4, 0.5, false));
        let set = one_case_set("helps", "archetype:positive");
        let now = chrono::DateTime::parse_from_rfc3339("2026-05-18T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let outcome = run_evaluation_with_set(rut(), &set, now)
            .await
            .expect("scripted evaluation must succeed");

        assert_eq!(outcome.verdict, EvalLabel::Helps);
        assert!(!outcome.regression);
        assert!(!outcome.negative_transfer);
        assert_eq!(outcome.replay_set_version, 7);
        assert_eq!(outcome.judge_model, JUDGE_MODEL);
        assert!(!outcome.staleness.stale);
        // Judge label matches the frozen "helps" label ⇒ perfect 1-case κ.
        assert!((outcome.calibration.agreement - 1.0).abs() < 1e-9);
        // Row projection mirrors the outcome.
        let row = outcome.to_row();
        assert_eq!(row.verdict, "helps");
        assert!(!row.regression);
        assert!((row.delta - 0.5).abs() < 1e-9);

        clear_inference_double();
    }

    #[tokio::test]
    #[serial]
    async fn end_to_end_negative_transfer_regresses_via_double() {
        // Positive raw delta but negative_transfer=true ⇒ deterministic
        // regression regardless of the model's own `regression` field.
        set_inference_double(script_case(0.85, 0.8, 0.05, true));
        let set = one_case_set("regresses", "archetype:negative-transfer");
        let now = chrono::DateTime::parse_from_rfc3339("2026-05-18T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let outcome = run_evaluation_with_set(rut(), &set, now)
            .await
            .expect("scripted evaluation must succeed");

        assert_eq!(outcome.verdict, EvalLabel::Regresses);
        assert!(outcome.regression);
        assert!(outcome.negative_transfer);
        let case0 = &outcome.cases[0];
        // Harness overwrites the model's `regression=false` with the
        // deterministic rule result.
        assert!(case0.verdict.as_ref().unwrap().regression);

        clear_inference_double();
    }

    #[tokio::test]
    #[serial]
    async fn end_to_end_high_variance_is_inconclusive_via_double() {
        // Per-arm samples disperse beyond the variance gate ⇒ the case is
        // inconclusive and NO judge call is consumed.
        let mut script = vec![
            ScriptedResponse::TypedJson(serde_json::json!({ "quality": 0.1 })),
            ScriptedResponse::TypedJson(serde_json::json!({ "quality": 0.5 })),
            ScriptedResponse::TypedJson(serde_json::json!({ "quality": 0.95 })),
            ScriptedResponse::TypedJson(serde_json::json!({ "quality": 0.5 })),
            ScriptedResponse::TypedJson(serde_json::json!({ "quality": 0.5 })),
            ScriptedResponse::TypedJson(serde_json::json!({ "quality": 0.5 })),
        ];
        // A trailing error proves the judge calls are never reached (if they
        // were, this would surface as an Inference error instead of a clean
        // inconclusive outcome).
        script.push(ScriptedResponse::Err(
            cc_client::InferenceError::RateLimited {
                message: "must not be consumed".into(),
            },
        ));
        set_inference_double(script);
        let set = one_case_set("helps", "archetype:positive");
        let now = chrono::DateTime::parse_from_rfc3339("2026-05-18T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let outcome = run_evaluation_with_set(rut(), &set, now)
            .await
            .expect("inconclusive case must not error");
        assert_eq!(outcome.verdict, EvalLabel::Inconclusive);
        assert_eq!(outcome.cases[0].label, EvalLabel::Inconclusive);
        assert!(outcome.cases[0].verdict.is_none());
        // The whole set was inconclusive ⇒ judge never scored ⇒ uncalibrated.
        assert!(outcome.calibration.judge_uncalibrated);

        clear_inference_double();
    }

    #[tokio::test]
    #[serial]
    async fn inference_error_propagates_via_double() {
        set_inference_double(vec![ScriptedResponse::Err(
            cc_client::InferenceError::NotSignedIn,
        )]);
        let set = one_case_set("helps", "archetype:positive");
        let now = chrono::Utc::now();
        let err = run_evaluation_with_set(rut(), &set, now)
            .await
            .expect_err("scripted inference error must propagate");
        assert!(matches!(err, EvalError::Inference(_)), "got {err:?}");
        clear_inference_double();
    }
}
