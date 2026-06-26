use crate::memory::redact_text;
use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TestRun {
    pub command: String,
    pub flavor: String,
    pub passed: u64,
    pub failed: u64,
    pub skipped: u64,
    pub exit_code: i32,
    pub duration_ms: u128,
    pub raw_output: String,
    pub timestamp: u64,
}

#[derive(Deserialize, Serialize)]
pub struct TestDiff {
    pub baseline: TestRun,
    pub current: TestRun,
    pub net_new_failures: i64,
    pub net_fixed_failures: i64,
    pub has_regression: bool,
}

struct TestPlan {
    flavor: &'static str,
    command: String,
}

pub fn handle_cli(args: &[String], workspace: &Path) -> Result<i32> {
    let Some(op) = args.first().map(String::as_str) else {
        println!("usage: harness baseline|run|diff [--filter PATTERN]");
        return Ok(2);
    };
    let filter = flag_value(args, "--filter")?;

    match op {
        "baseline" => {
            let run = run_suite(workspace, filter.as_deref())?;
            save_baseline(workspace, &run)?;
            println!(
                "baseline: {} | passed={} failed={} skipped={} duration={}ms",
                run.command, run.passed, run.failed, run.skipped, run.duration_ms
            );
            Ok(if run.failed == 0 && run.exit_code == 0 {
                0
            } else {
                1
            })
        }
        "run" => {
            let run = run_suite(workspace, filter.as_deref())?;
            save_run(workspace, &run)?;
            println!(
                "run: {} | passed={} failed={} skipped={} exit={} duration={}ms",
                run.command, run.passed, run.failed, run.skipped, run.exit_code, run.duration_ms
            );
            Ok(if run.failed == 0 && run.exit_code == 0 {
                0
            } else {
                1
            })
        }
        "diff" => {
            let diff = diff_suite(workspace, filter.as_deref())?;
            println!(
                "diff: new_failures={} fixed_failures={} current_failed={}",
                diff.net_new_failures, diff.net_fixed_failures, diff.current.failed
            );
            save_diff(workspace, &diff)?;
            Ok(if diff.has_regression { 1 } else { 0 })
        }
        _ => {
            println!("unknown harness op: {op}");
            Ok(2)
        }
    }
}

pub fn baseline_context(workspace: &Path) -> String {
    let path = baseline_path(workspace);
    let Ok(text) = fs::read_to_string(path) else {
        return "<test_harness>No test baseline exists yet.</test_harness>".into();
    };
    let Ok(run) = serde_json::from_str::<TestRun>(&text) else {
        return "<test_harness>Test baseline file exists but could not be parsed.</test_harness>"
            .into();
    };
    let green = run.failed == 0 && run.exit_code == 0;
    let latest = latest_diff_note(workspace);
    format!(
        "<test_harness>Baseline command: {}. Flavor: {}. Passed: {}. Failed: {}. Skipped: {}. Suite green: {}. {} Do not introduce additional failures; fix regressions before claiming completion.</test_harness>",
        run.command, run.flavor, run.passed, run.failed, run.skipped, green, latest
    )
}

fn latest_diff_note(workspace: &Path) -> String {
    let dir = workspace.join("biscuits/test_runs");
    let Ok(entries) = fs::read_dir(dir) else {
        return "No latest diff is available.".into();
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.starts_with("test-diff-") && s.ends_with(".json"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    paths.sort();
    let Some(path) = paths.last() else {
        return "No latest diff is available.".into();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return "Latest diff could not be read.".into();
    };
    let Ok(diff) = serde_json::from_str::<TestDiff>(&text) else {
        return "Latest diff could not be parsed.".into();
    };
    format!(
        "Latest diff: new_failures={}, fixed_failures={}, regression={}.",
        diff.net_new_failures, diff.net_fixed_failures, diff.has_regression
    )
}

fn diff_suite(workspace: &Path, filter: Option<&str>) -> Result<TestDiff> {
    let baseline: TestRun = serde_json::from_str(&fs::read_to_string(baseline_path(workspace))?)
        .context("no test baseline found; run `harness baseline` first")?;
    let current = run_suite(workspace, filter)?;
    let net_new_failures = current.failed as i64 - baseline.failed as i64;
    let net_fixed_failures = baseline.failed as i64 - current.failed as i64;
    Ok(TestDiff {
        baseline,
        current,
        net_new_failures,
        net_fixed_failures,
        has_regression: net_new_failures > 0,
    })
}

fn run_suite(workspace: &Path, filter: Option<&str>) -> Result<TestRun> {
    let mut plan = detect(workspace).context("no test suite detected")?;
    if let Some(filter) = filter {
        validate_filter(filter)?;
        plan.command = filtered_command(plan.flavor, &plan.command, filter);
    }
    let start = Instant::now();
    let output = run_shell(workspace, &plan.command, 600)?;
    let duration_ms = start.elapsed().as_millis();
    let mut run = parse_output(plan.flavor, &plan.command, output.status, &output.text);
    run.duration_ms = duration_ms;
    save_run(workspace, &run)?;
    Ok(run)
}

struct ShellOutput {
    status: i32,
    text: String,
}

fn run_shell(workspace: &Path, command: &str, timeout_secs: u64) -> Result<ShellOutput> {
    let mut child = crate::shell::command(command)
        .current_dir(workspace)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run test command: {command}"))?;
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(timeout_secs) {
            let _ = child.kill();
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let output = child.wait_with_output()?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(ShellOutput {
        status: output.status.code().unwrap_or(1),
        text: redact_text(&text),
    })
}

fn detect(workspace: &Path) -> Option<TestPlan> {
    let exists = |p: &str| workspace.join(p).exists();
    if exists("Package.swift") {
        Some(TestPlan {
            flavor: "swift",
            command: "swift test".into(),
        })
    } else if exists("pyproject.toml") || exists("pytest.ini") || exists("setup.py") {
        Some(TestPlan {
            flavor: "pytest",
            command: "pytest".into(),
        })
    } else if exists("package.json") {
        Some(TestPlan {
            flavor: "node",
            command: "npm test".into(),
        })
    } else if exists("Cargo.toml") {
        Some(TestPlan {
            flavor: "cargo",
            command: format!("{} test", cargo_cmd()),
        })
    } else if exists("go.mod") {
        Some(TestPlan {
            flavor: "go",
            command: "go test ./...".into(),
        })
    } else if exists("Gemfile") || exists(".rspec") {
        Some(TestPlan {
            flavor: "rspec",
            command: "bundle exec rspec".into(),
        })
    } else if exists("Makefile") {
        Some(TestPlan {
            flavor: "generic",
            command: "make test".into(),
        })
    } else {
        None
    }
}

fn cargo_cmd() -> String {
    if let Ok(cargo) = std::env::var("CARGO") {
        return cargo;
    }
    if let Ok(home) = std::env::var("HOME") {
        let path = Path::new(&home).join(".cargo/bin/cargo");
        if path.exists() {
            return path.to_string_lossy().to_string();
        }
    }
    "cargo".into()
}

fn filtered_command(flavor: &str, base: &str, filter: &str) -> String {
    match flavor {
        "cargo" => format!("{base} {filter}"),
        "pytest" => format!("{base} -k {filter}"),
        "go" => format!("{base} -run {filter}"),
        "rspec" => format!("{base} {filter}"),
        "node" => format!("{base} -- {filter}"),
        _ => format!("{base} {filter}"),
    }
}

fn validate_filter(filter: &str) -> Result<()> {
    let ok = Regex::new(r"^[A-Za-z0-9_./:=-]+$")?.is_match(filter);
    if !ok {
        bail!("unsafe test filter; allowed chars are letters, numbers, _ . / : = -");
    }
    Ok(())
}

fn parse_output(flavor: &str, command: &str, exit_code: i32, output: &str) -> TestRun {
    let mut passed = 0;
    let mut failed = if exit_code == 0 { 0 } else { 1 };
    let mut skipped = 0;

    match flavor {
        "cargo" => {
            if let Some(c) = cap(
                output,
                r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored",
            ) {
                passed = c[0];
                failed = c[1];
                skipped = c[2];
            }
        }
        "pytest" => {
            passed = sum_caps(output, r"(\d+) passed");
            failed = sum_caps(output, r"(\d+) failed") + sum_caps(output, r"(\d+) errors?");
            skipped = sum_caps(output, r"(\d+) skipped");
        }
        "node" => {
            passed = sum_caps(output, r"(\d+) passed") + sum_caps(output, r"(\d+) passing");
            failed = sum_caps(output, r"(\d+) failed") + sum_caps(output, r"(\d+) failing");
            skipped = sum_caps(output, r"(\d+) skipped") + sum_caps(output, r"(\d+) pending");
        }
        "go" => {
            passed = output.lines().filter(|l| l.starts_with("ok ")).count() as u64;
            failed = output.lines().filter(|l| l.starts_with("FAIL")).count() as u64;
            if failed == 0 && exit_code != 0 {
                failed = 1;
            }
        }
        "rspec" => {
            if let Some(c) = cap(
                output,
                r"(\d+) examples?, (\d+) failures?(?:, (\d+) pending)?",
            ) {
                passed = c[0].saturating_sub(c[1]);
                failed = c[1];
                skipped = *c.get(2).unwrap_or(&0);
            }
        }
        "swift" => {
            failed = sum_caps(output, r"(\d+) failures?");
            passed = sum_caps(output, r"(\d+) tests? passed");
        }
        _ => {
            failed = if exit_code == 0 { 0 } else { 1 };
            passed = if exit_code == 0 { 1 } else { 0 };
        }
    }

    TestRun {
        command: command.into(),
        flavor: flavor.into(),
        passed,
        failed,
        skipped,
        exit_code,
        duration_ms: 0,
        raw_output: redact_text(output),
        timestamp: now(),
    }
}

fn cap(text: &str, pat: &str) -> Option<Vec<u64>> {
    let re = Regex::new(pat).ok()?;
    let caps = re.captures(text)?;
    Some(
        (1..caps.len())
            .map(|i| {
                caps.get(i)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0)
            })
            .collect(),
    )
}

fn sum_caps(text: &str, pat: &str) -> u64 {
    let Ok(re) = Regex::new(pat) else {
        return 0;
    };
    re.captures_iter(text)
        .filter_map(|c| c.get(1)?.as_str().parse::<u64>().ok())
        .sum()
}

fn save_baseline(workspace: &Path, run: &TestRun) -> Result<()> {
    write_json(&baseline_path(workspace), run)
}

fn save_run(workspace: &Path, run: &TestRun) -> Result<()> {
    let dir = workspace.join("biscuits/test_runs");
    fs::create_dir_all(&dir)?;
    write_json(&dir.join(format!("test-run-{}.json", run.timestamp)), run)
}

fn save_diff(workspace: &Path, diff: &TestDiff) -> Result<()> {
    let dir = workspace.join("biscuits/test_runs");
    fs::create_dir_all(&dir)?;
    write_json(&dir.join(format!("test-diff-{}.json", now())), diff)
}

fn baseline_path(workspace: &Path) -> PathBuf {
    workspace.join("biscuits/test_baseline.json")
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
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
    file.write_all(format!("{}\n", serde_json::to_string_pretty(value)?).as_bytes())?;
    Ok(())
}

fn flag_value(args: &[String], flag: &str) -> Result<Option<String>> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return Ok(Some(
                iter.next().context("flag requires a value")?.to_string(),
            ));
        }
    }
    Ok(None)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
