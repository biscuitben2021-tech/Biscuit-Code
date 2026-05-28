use crate::{
    agent::{self, TurnCapture},
    llm::{self, Config},
    memory::{redact_text, MemoryStore},
    tools::ToolRuntime,
};
use anyhow::{bail, Context, Result};
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Deserialize)]
pub struct EvalCase {
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub description: String,
    pub prompt: String,
    #[serde(default)]
    pub expected_tool_sequence: Vec<String>,
    #[serde(default)]
    pub tool_matching_mode: ToolMatchMode,
    #[serde(default)]
    pub output_matcher: OutputMatcher,
    #[serde(default)]
    pub rubric: String,
    #[serde(default = "default_threshold")]
    pub rubric_pass_threshold: f32,
    #[serde(default)]
    pub expects_refusal: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolMatchMode {
    ExactOrderedSequence,
    ContainsAll,
    ContainsAny,
    #[default]
    Subsequence,
}

#[derive(Clone, Default, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputMatcher {
    #[default]
    Any,
    ContainsSubstring {
        value: String,
    },
    MustNotContainSubstring {
        value: String,
    },
    Regex {
        value: String,
    },
    All {
        matchers: Vec<OutputMatcher>,
    },
    AnyOf {
        matchers: Vec<OutputMatcher>,
    },
}

#[derive(Deserialize, Serialize)]
pub struct EvalReport {
    pub start_time: u64,
    pub finish_time: u64,
    pub model: String,
    pub provider: String,
    pub total_cases: usize,
    pub passed_count: usize,
    pub failed_count: usize,
    pub average_rubric_score: Option<f32>,
    pub total_duration_ms: u128,
    pub results: Vec<CaseResult>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct CaseResult {
    pub name: String,
    pub category: String,
    pub passed: bool,
    pub rubric_score: Option<f32>,
    pub failure_reasons: Vec<String>,
    pub rubric_verdict: String,
    pub raw_capture: CaptureData,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct CaptureData {
    pub prompt: String,
    pub ordered_tool_calls: Vec<String>,
    pub final_message: String,
    pub runtime_ms: u128,
    pub loop_count: usize,
    pub token_usage: llm::UsageSnapshot,
    pub errors: Vec<String>,
    pub tool_results: Vec<String>,
}

#[derive(Deserialize, Serialize)]
struct Baseline {
    cases: BTreeMap<String, bool>,
}

#[derive(Default, Serialize)]
struct ReportDiff {
    newly_failing: Vec<String>,
    newly_passing: Vec<String>,
    still_failing: Vec<String>,
    rubric_score_delta: Option<f32>,
    pass_rate_delta: f32,
}

pub async fn handle_cli(args: &[String], workspace: &Path) -> Result<i32> {
    let opts = EvalOptions::parse(args)?;
    if opts.smoke {
        return smoke(workspace);
    }
    if opts.dashboard {
        return dashboard(workspace, opts.cases_path.as_deref());
    }

    let client = Client::new();
    let config = llm::from_env(&client).await?;
    let mut cases = load_cases(workspace, opts.cases_path.as_deref())?;
    if let Some(name) = &opts.name {
        cases.retain(|c| c.name.contains(name));
    }
    if let Some(category) = &opts.category {
        cases.retain(|c| c.category == *category);
    }
    if let Some(limit) = opts.limit {
        cases.truncate(limit);
    }
    if cases.is_empty() && opts.cases_path.is_none() {
        cases = serde_json::from_str(BUILTIN_CASES)?;
    }
    if cases.is_empty() {
        bail!("no eval cases found");
    }

    let report = run_suite(
        &client,
        &config,
        workspace,
        cases,
        opts.skip_rubric,
        opts.stop_between_cases,
    )
    .await?;
    let report_path = save_report(workspace, &report)?;
    println!("report: {}", report_path.display());

    if let Some(path) = opts.write_baseline {
        write_baseline(&path, &report)?;
        println!("baseline written: {}", path.display());
    }
    let mut baseline_regression = false;
    if let Some(path) = opts.compare_baseline {
        baseline_regression = compare_baseline(&path, &report)?;
    }
    if let Some(path) = opts.compare_report {
        compare_report(&path, &report)?;
    }

    Ok(if report.failed_count > 0 || baseline_regression {
        1
    } else {
        0
    })
}

async fn run_suite(
    client: &Client,
    config: &Config,
    workspace: &Path,
    cases: Vec<EvalCase>,
    skip_rubric: bool,
    stop_between_cases: bool,
) -> Result<EvalReport> {
    let start_time = now();
    let start = Instant::now();
    let mut results = Vec::new();

    for (i, case) in cases.iter().enumerate() {
        println!(
            "[{}/{}] {}{}",
            i + 1,
            cases.len(),
            if case.category.is_empty() {
                String::new()
            } else {
                format!("{}/", case.category)
            },
            case.name
        );
        if !case.description.trim().is_empty() {
            println!("  {}", case.description);
        }
        let result = run_case(client, config, workspace, case, skip_rubric).await;
        match &result {
            Ok(r) => println!(
                "  {} duration={}ms score={}",
                if r.passed { "PASS" } else { "FAIL" },
                r.raw_capture.runtime_ms,
                r.rubric_score
                    .map(|s| format!("{s:.1}"))
                    .unwrap_or_else(|| "-".into())
            ),
            Err(err) => println!("  FAIL error={err}"),
        }
        results.push(result.unwrap_or_else(|err| error_result(case, err.to_string())));
        if stop_between_cases && i + 1 < cases.len() {
            println!("press Enter for next case, or q then Enter to stop");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if line.trim().eq_ignore_ascii_case("q") {
                break;
            }
        }
    }

    let passed_count = results.iter().filter(|r| r.passed).count();
    let scores: Vec<f32> = results.iter().filter_map(|r| r.rubric_score).collect();
    let average_rubric_score = if scores.is_empty() {
        None
    } else {
        Some(scores.iter().sum::<f32>() / scores.len() as f32)
    };
    Ok(EvalReport {
        start_time,
        finish_time: now(),
        model: config.model.clone(),
        provider: llm::provider_name(config.provider).into(),
        total_cases: results.len(),
        passed_count,
        failed_count: results.len() - passed_count,
        average_rubric_score,
        total_duration_ms: start.elapsed().as_millis(),
        results,
    })
}

async fn run_case(
    client: &Client,
    config: &Config,
    workspace: &Path,
    case: &EvalCase,
    skip_rubric: bool,
) -> Result<CaseResult> {
    let eval_root = workspace.join(".biscuits/eval_sessions").join(format!(
        "{}-{}",
        now(),
        sanitize_name(&case.name)
    ));
    let mut memory = MemoryStore::open_isolated(workspace.to_path_buf(), eval_root)?;
    let mut tools = ToolRuntime::new(workspace.to_path_buf())?;
    let mut perms = crate::permissions::PermissionGuard::open(workspace);
    perms.set_mode(crate::permissions::Mode::Auto); // evals run unattended
    let mut history = Vec::new();
    let prompt = case.prompt.clone();
    let timeout = Duration::from_secs(case.timeout_secs.max(1));

    let capture = tokio::time::timeout(
        timeout,
        agent::run_turn(
            client,
            config,
            &mut memory,
            &mut tools,
            &mut perms,
            &mut history,
            &prompt,
            "<eval_mode>Run this as an isolated evaluation case.</eval_mode>",
            false,
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("case timed out after {}s", case.timeout_secs))??;

    let mut failure_reasons = Vec::new();
    check_tools(case, &capture.ordered_tool_calls, &mut failure_reasons);
    check_output(
        &case.output_matcher,
        &capture.final_message,
        &mut failure_reasons,
    );
    if case.expects_refusal && !looks_like_refusal(&capture.final_message) {
        failure_reasons.push("expected refusal but response did not look like one".into());
    }
    if !capture.errors.is_empty() {
        failure_reasons.extend(capture.errors.iter().map(|e| format!("runtime error: {e}")));
    }

    let mut rubric_score = None;
    let mut rubric_verdict = String::new();
    if !skip_rubric && !case.rubric.trim().is_empty() {
        match judge(client, config, case, &capture).await {
            Ok(j) => {
                rubric_score = Some(j.score);
                rubric_verdict = j.verdict;
                if j.score < case.rubric_pass_threshold {
                    failure_reasons.push(format!(
                        "rubric score {:.1} below threshold {:.1}",
                        j.score, case.rubric_pass_threshold
                    ));
                }
            }
            Err(err) => {
                failure_reasons.push(format!("rubric ungradeable: {err}"));
                rubric_verdict = "ungradeable".into();
            }
        }
    }

    let raw_capture = capture_data(case, capture);
    Ok(CaseResult {
        name: case.name.clone(),
        category: case.category.clone(),
        passed: failure_reasons.is_empty(),
        rubric_score,
        failure_reasons: failure_reasons
            .into_iter()
            .map(|s| redact_text(&s))
            .collect(),
        rubric_verdict: redact_text(&rubric_verdict),
        raw_capture,
    })
}

#[derive(Default)]
struct EvalOptions {
    cases_path: Option<PathBuf>,
    name: Option<String>,
    category: Option<String>,
    limit: Option<usize>,
    skip_rubric: bool,
    write_baseline: Option<PathBuf>,
    compare_baseline: Option<PathBuf>,
    compare_report: Option<PathBuf>,
    smoke: bool,
    dashboard: bool,
    stop_between_cases: bool,
}

impl EvalOptions {
    fn parse(args: &[String]) -> Result<Self> {
        let mut opts = Self::default();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--cases" => {
                    i += 1;
                    opts.cases_path =
                        Some(PathBuf::from(args.get(i).context("--cases needs path")?));
                }
                "--name" => {
                    i += 1;
                    opts.name = Some(args.get(i).context("--name needs value")?.clone());
                }
                "--category" => {
                    i += 1;
                    opts.category = Some(args.get(i).context("--category needs value")?.clone());
                }
                "--limit" => {
                    i += 1;
                    opts.limit = Some(args.get(i).context("--limit needs value")?.parse()?);
                }
                "--skip-rubric" => opts.skip_rubric = true,
                "--write-baseline" => {
                    i += 1;
                    opts.write_baseline = Some(PathBuf::from(
                        args.get(i).context("--write-baseline needs path")?,
                    ));
                }
                "--compare-baseline" => {
                    i += 1;
                    opts.compare_baseline = Some(PathBuf::from(
                        args.get(i).context("--compare-baseline needs path")?,
                    ));
                }
                "--compare-report" => {
                    i += 1;
                    opts.compare_report = Some(PathBuf::from(
                        args.get(i).context("--compare-report needs path")?,
                    ));
                }
                "--smoke" => opts.smoke = true,
                "--dashboard" => opts.dashboard = true,
                "--stop-between-cases" => opts.stop_between_cases = true,
                "--help" | "-h" => {
                    println!("eval [--cases PATH] [--name TEXT] [--category CAT] [--limit N] [--skip-rubric] [--write-baseline PATH] [--compare-baseline PATH] [--compare-report PATH] [--smoke] [--dashboard] [--stop-between-cases]");
                    std::process::exit(0);
                }
                other => bail!("unknown eval flag: {other}"),
            }
            i += 1;
        }
        Ok(opts)
    }
}

fn load_cases(workspace: &Path, explicit: Option<&Path>) -> Result<Vec<EvalCase>> {
    let mut roots = Vec::new();
    if let Some(path) = explicit {
        roots.push(path.to_path_buf());
    } else {
        roots.push(workspace.join("evals/cases"));
        roots.push(PathBuf::from("evals/bundled"));
    }
    let mut cases = Vec::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        if root.is_file() {
            load_case_file(&root, &mut cases)?;
        } else {
            for entry in fs::read_dir(root)? {
                let path = entry?.path();
                if path.extension().and_then(|s| s.to_str()) == Some("json") {
                    load_case_file(&path, &mut cases)?;
                }
            }
        }
    }
    cases.sort_by(|a, b| a.category.cmp(&b.category).then(a.name.cmp(&b.name)));
    Ok(cases)
}

fn load_case_file(path: &Path, out: &mut Vec<EvalCase>) -> Result<()> {
    let text = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("invalid eval case JSON: {}", path.display()))?;
    if value.is_array() {
        out.extend(serde_json::from_value::<Vec<EvalCase>>(value)?);
    } else {
        out.push(serde_json::from_value::<EvalCase>(value)?);
    }
    Ok(())
}

fn check_tools(case: &EvalCase, actual: &[String], failures: &mut Vec<String>) {
    if case.expected_tool_sequence.is_empty() {
        return;
    }
    let expected = &case.expected_tool_sequence;
    let pass = match case.tool_matching_mode {
        ToolMatchMode::ExactOrderedSequence => eq_tools(expected, actual),
        ToolMatchMode::ContainsAll => expected
            .iter()
            .all(|e| actual.iter().any(|a| tool_eq(e, a))),
        ToolMatchMode::ContainsAny => expected
            .iter()
            .any(|e| actual.iter().any(|a| tool_eq(e, a))),
        ToolMatchMode::Subsequence => is_subsequence(expected, actual),
    };
    if !pass {
        failures.push(format!(
            "tool sequence mismatch expected {:?} mode {} got {:?}",
            expected,
            tool_mode_name(case.tool_matching_mode.clone()),
            actual
        ));
    }
}

fn check_output(matcher: &OutputMatcher, text: &str, failures: &mut Vec<String>) -> bool {
    let ok = match matcher {
        OutputMatcher::Any => true,
        OutputMatcher::ContainsSubstring { value } => text.contains(value),
        OutputMatcher::MustNotContainSubstring { value } => !text.contains(value),
        OutputMatcher::Regex { value } => {
            Regex::new(value).map(|r| r.is_match(text)).unwrap_or(false)
        }
        OutputMatcher::All { matchers } => matchers.iter().all(|m| check_output_silent(m, text)),
        OutputMatcher::AnyOf { matchers } => matchers.iter().any(|m| check_output_silent(m, text)),
    };
    if !ok {
        failures.push("assistant output matcher failed".into());
    }
    ok
}

fn check_output_silent(matcher: &OutputMatcher, text: &str) -> bool {
    let mut failures = Vec::new();
    check_output(matcher, text, &mut failures)
}

struct Judge {
    score: f32,
    verdict: String,
}

async fn judge(
    client: &Client,
    config: &Config,
    case: &EvalCase,
    capture: &TurnCapture,
) -> Result<Judge> {
    let prompt = format!(
        "Return JSON only: {{\"score\":0-10,\"verdict\":\"short sentence\"}}\n\nUser prompt:\n{}\n\nAssistant response:\n{}\n\nTool sequence:\n{:?}\n\nRubric:\n{}",
        redact_text(&case.prompt),
        redact_text(&capture.final_message),
        capture.ordered_tool_calls,
        redact_text(&case.rubric)
    );
    let raw = llm::complete(client, config, JUDGE_SYSTEM, &prompt).await?;
    parse_judge(&raw)
}

fn parse_judge(raw: &str) -> Result<Judge> {
    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if let Ok(v) = serde_json::from_str::<Value>(cleaned) {
        let score = v.get("score").and_then(Value::as_f64).unwrap_or(-1.0) as f32;
        let verdict = v
            .get("verdict")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if score >= 0.0 {
            return Ok(Judge {
                score: score.clamp(0.0, 10.0),
                verdict,
            });
        }
    }
    let re = Regex::new(r"(?i)(?:score[^0-9]*)?([0-9]+(?:\.[0-9]+)?)")?;
    if let Some(cap) = re.captures(cleaned) {
        let score = cap[1].parse::<f32>()?.clamp(0.0, 10.0);
        return Ok(Judge {
            score,
            verdict: "score extracted from non-json judge response".into(),
        });
    }
    bail!("could not parse judge score")
}

fn save_report(workspace: &Path, report: &EvalReport) -> Result<PathBuf> {
    let dir = workspace.join(".biscuits/eval_reports");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("eval-report-{}.json", report.start_time));
    write_private(
        &path,
        &format!("{}\n", serde_json::to_string_pretty(report)?),
    )?;
    Ok(path)
}

fn write_baseline(path: &Path, report: &EvalReport) -> Result<()> {
    let baseline = Baseline {
        cases: report
            .results
            .iter()
            .map(|r| (r.name.clone(), r.passed))
            .collect(),
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_private(
        path,
        &format!("{}\n", serde_json::to_string_pretty(&baseline)?),
    )?;
    Ok(())
}

fn compare_baseline(path: &Path, report: &EvalReport) -> Result<bool> {
    let baseline: Baseline = serde_json::from_str(&fs::read_to_string(path)?)?;
    let current: BTreeMap<_, _> = report
        .results
        .iter()
        .map(|r| (r.name.clone(), r.passed))
        .collect();
    let regressions: Vec<_> = baseline
        .cases
        .iter()
        .filter(|(name, passed)| **passed && current.get(*name) == Some(&false))
        .map(|(name, _)| name.clone())
        .collect();
    if !regressions.is_empty() {
        println!("baseline regressions: {}", regressions.join(", "));
    }
    Ok(!regressions.is_empty())
}

fn compare_report(path: &Path, current: &EvalReport) -> Result<()> {
    let previous: EvalReport = serde_json::from_str(&fs::read_to_string(path)?)?;
    let diff = report_diff(&previous, current);
    println!("{}", serde_json::to_string_pretty(&diff)?);
    Ok(())
}

fn report_diff(previous: &EvalReport, current: &EvalReport) -> ReportDiff {
    let prev = previous
        .results
        .iter()
        .map(|r| (r.name.clone(), r))
        .collect::<BTreeMap<_, _>>();
    let curr = current
        .results
        .iter()
        .map(|r| (r.name.clone(), r))
        .collect::<BTreeMap<_, _>>();
    let names = prev
        .keys()
        .chain(curr.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut diff = ReportDiff::default();
    for name in names {
        match (prev.get(&name), curr.get(&name)) {
            (Some(p), Some(c)) if p.passed && !c.passed => diff.newly_failing.push(name),
            (Some(p), Some(c)) if !p.passed && c.passed => diff.newly_passing.push(name),
            (Some(p), Some(c)) if !p.passed && !c.passed => diff.still_failing.push(name),
            _ => {}
        }
    }
    diff.pass_rate_delta = current.passed_count as f32 / current.total_cases.max(1) as f32
        - previous.passed_count as f32 / previous.total_cases.max(1) as f32;
    diff.rubric_score_delta = match (current.average_rubric_score, previous.average_rubric_score) {
        (Some(c), Some(p)) => Some(c - p),
        _ => None,
    };
    diff
}

fn dashboard(workspace: &Path, explicit: Option<&Path>) -> Result<i32> {
    let mut cases = load_cases(workspace, explicit)?;
    if cases.is_empty() && explicit.is_none() {
        cases = serde_json::from_str(BUILTIN_CASES)?;
    }

    println!("eval dashboard");
    if cases.is_empty() {
        println!("state: no cases found");
    } else {
        println!("cases: {}", cases.len());
        for case in cases.iter().take(20) {
            println!(
                "- {}{}",
                if case.category.is_empty() {
                    String::new()
                } else {
                    format!("{}/", case.category)
                },
                case.name
            );
        }
    }

    let mut reports = report_paths(workspace)?;
    if reports.is_empty() {
        println!("latest report: none");
        return Ok(if cases.is_empty() { 1 } else { 0 });
    }
    reports.sort();
    let latest: EvalReport = serde_json::from_str(&fs::read_to_string(reports.last().unwrap())?)?;
    println!(
        "latest report: passed={} failed={} total={} state={}",
        latest.passed_count,
        latest.failed_count,
        latest.total_cases,
        if latest.failed_count == 0 {
            "finished_all_passing"
        } else {
            "finished_with_failures"
        }
    );
    for result in latest.results.iter().filter(|r| !r.passed) {
        println!(
            "- FAIL {}: {}",
            result.name,
            result.failure_reasons.join("; ")
        );
        if !result.rubric_verdict.is_empty() {
            println!("  verdict: {}", result.rubric_verdict);
        }
        println!("  tools: {:?}", result.raw_capture.ordered_tool_calls);
        println!(
            "  response: {}",
            truncate(&result.raw_capture.final_message, 300)
        );
    }
    if reports.len() >= 2 {
        let previous: EvalReport =
            serde_json::from_str(&fs::read_to_string(&reports[reports.len() - 2])?)?;
        let diff = report_diff(&previous, &latest);
        println!(
            "diff vs prior report: {}",
            serde_json::to_string_pretty(&diff)?
        );
    }
    Ok(if latest.failed_count == 0 { 0 } else { 1 })
}

fn report_paths(workspace: &Path) -> Result<Vec<PathBuf>> {
    let dir = workspace.join(".biscuits/eval_reports");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.starts_with("eval-report-") && s.ends_with(".json"))
                .unwrap_or(false)
        })
        .collect())
}

fn smoke(workspace: &Path) -> Result<i32> {
    fs::create_dir_all(workspace.join(".biscuits/eval_reports"))?;
    let case = EvalCase {
        name: "smoke".into(),
        category: "infrastructure".into(),
        description: "local matcher smoke test".into(),
        prompt: "hello".into(),
        expected_tool_sequence: vec!["Read".into(), "Grep".into()],
        tool_matching_mode: ToolMatchMode::Subsequence,
        output_matcher: OutputMatcher::ContainsSubstring { value: "ok".into() },
        rubric: String::new(),
        rubric_pass_threshold: 7.0,
        expects_refusal: false,
        timeout_secs: 1,
    };
    let mut failures = Vec::new();
    check_tools(
        &case,
        &["Read".into(), "Glob".into(), "Grep".into()],
        &mut failures,
    );
    check_output(&case.output_matcher, "ok", &mut failures);
    if failures.is_empty() {
        println!("eval smoke: ok");
        Ok(0)
    } else {
        println!("eval smoke failed: {:?}", failures);
        Ok(1)
    }
}

fn capture_data(case: &EvalCase, capture: TurnCapture) -> CaptureData {
    CaptureData {
        prompt: redact_text(&case.prompt),
        ordered_tool_calls: capture.ordered_tool_calls,
        final_message: redact_text(&capture.final_message),
        runtime_ms: capture.runtime_ms,
        loop_count: capture.loop_count,
        token_usage: capture.token_usage,
        errors: capture
            .errors
            .into_iter()
            .map(|e| redact_text(&e))
            .collect(),
        tool_results: capture
            .tool_results
            .into_iter()
            .map(|t| redact_text(&t))
            .collect(),
    }
}

fn error_result(case: &EvalCase, error: String) -> CaseResult {
    CaseResult {
        name: case.name.clone(),
        category: case.category.clone(),
        passed: false,
        rubric_score: None,
        failure_reasons: vec![redact_text(&error)],
        rubric_verdict: String::new(),
        raw_capture: CaptureData {
            prompt: redact_text(&case.prompt),
            ..Default::default()
        },
    }
}

fn eq_tools(expected: &[String], actual: &[String]) -> bool {
    expected.len() == actual.len() && is_subsequence(expected, actual)
}

fn is_subsequence(expected: &[String], actual: &[String]) -> bool {
    let mut i = 0;
    for tool in actual {
        if i < expected.len() && tool_eq(&expected[i], tool) {
            i += 1;
        }
    }
    i == expected.len()
}

fn tool_eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn tool_mode_name(mode: ToolMatchMode) -> &'static str {
    match mode {
        ToolMatchMode::ExactOrderedSequence => "exact_ordered_sequence",
        ToolMatchMode::ContainsAll => "contains_all",
        ToolMatchMode::ContainsAny => "contains_any",
        ToolMatchMode::Subsequence => "subsequence",
    }
}

fn looks_like_refusal(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("i can't")
        || t.contains("i cannot")
        || t.contains("can't help")
        || t.contains("cannot help")
        || t.contains("not able to")
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(max).collect::<String>())
    }
}

fn default_threshold() -> f32 {
    7.0
}

fn default_timeout() -> u64 {
    120
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn write_private(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut opts = fs::OpenOptions::new();
    opts.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

const JUDGE_SYSTEM: &str = r#"You are an eval judge.
Score the assistant answer against the rubric from 0 to 10.
Return JSON only with {"score": number, "verdict": "one short sentence"}.
"#;

const BUILTIN_CASES: &str = r#"[
  {
    "name": "builtin_smoke_basic_answer",
    "category": "smoke",
    "description": "Default packaged eval that checks the assistant can answer a simple prompt.",
    "prompt": "Reply with exactly: eval smoke ok",
    "tool_matching_mode": "contains_any",
    "output_matcher": {
      "type": "contains_substring",
      "value": "eval smoke ok"
    },
    "timeout_secs": 60
  }
]"#;
