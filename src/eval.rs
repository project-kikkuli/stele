//! `stele eval` — CI for a semantic rule.
//!
//! A deterministic rule is exact and needs no eval. A semantic rule is judged by a
//! model, so its correctness lives in the prompt — which can be phrased wrong and
//! looks fine by inspection. This runs the prompt across the configured judge fleet
//! against held-out corrections and reports the strongest severity the rule earns:
//! a rule holds only as well as its weakest *measurable* vendor. A vendor stele
//! can't measure is a coverage gap (exit 3), not a passing grade.
//!
//! Cases are before→after. The judge rewrites the code; a rewrite passes when every
//! removed fragment is gone and every kept fragment survives. Scoring the resulting
//! edit — not the flag — means a judge that flags a whole comment but rewrites it to
//! the intended surgical cut still passes.

use crate::config::{self, Judge, LoadScope, Semantic, Severity};
use crate::substrate;
use serde::Deserialize;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const CONTRACT: &str = "\n\nReturn ONLY the corrected snippet, nothing else, between these exact markers:\n<<<REWRITE\n...corrected code...\nREWRITE>>>";

/// One held-out correction: the slop, and the surgical edit you actually made.
#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    before: String,
    #[serde(default)]
    removed: Vec<String>,
    #[serde(default)]
    kept: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Cell {
    Pass,
    Fail,
    Unmeasurable,
}

impl Cell {
    fn symbol(self) -> char {
        match self {
            Cell::Pass => '✓',
            Cell::Fail => '✗',
            Cell::Unmeasurable => '·',
        }
    }
}

pub fn run(rule_id: &str) -> u8 {
    let cwd = std::env::current_dir().unwrap_or_default();
    let root = match substrate::find_root(&cwd) {
        Ok(r) => r,
        Err(e) => return fail(&e),
    };
    let rules = match config::load(&root) {
        Ok(r) => r,
        Err(e) => return fail(&e),
    };
    let Some(rule) = rules.iter().find(|r| r.id == rule_id) else {
        return fail(&format!("no rule with id `{rule_id}`"));
    };
    let Some(sem) = &rule.semantic else {
        return fail(&format!("rule `{rule_id}` is not a `[rule.semantic]` rule"));
    };

    let judges = config::load_judges(&root, LoadScope::All);
    let mut fleet = Vec::new();
    for name in &sem.models {
        match judges.iter().find(|j| &j.name == name) {
            Some(j) => fleet.push(j.clone()),
            None => return fail(&format!("rule `{rule_id}`: no `[[judge]]` named `{name}`")),
        }
    }

    let cases = match load_cases(&root.join(&sem.cases)) {
        Ok(c) if !c.is_empty() => c,
        Ok(_) => return fail(&format!("no cases in {}", sem.cases)),
        Err(e) => return fail(&e),
    };

    let grid: Vec<Vec<Cell>> = fleet
        .iter()
        .map(|judge| cases.iter().map(|c| grade(judge, sem, c)).collect())
        .collect();

    report(rule_id, &fleet, &cases, &grid, rule.severity, sem)
}

/// Majority verdict over `samples` votes; a cell no vote could measure is unmeasurable.
fn grade(judge: &Judge, sem: &Semantic, case: &Case) -> Cell {
    let prompt = format!("{}{CONTRACT}\n\n--- code ---\n{}", sem.prompt, case.before);
    let (mut pass, mut fail) = (0u32, 0u32);
    for _ in 0..sem.samples.max(1) {
        match run_judge(&judge.command, &prompt) {
            Some(rewrite) if case_passes(&rewrite, case) => pass += 1,
            Some(_) => fail += 1,
            None => {}
        }
    }
    if pass + fail == 0 {
        Cell::Unmeasurable
    } else if pass > fail {
        Cell::Pass
    } else {
        Cell::Fail
    }
}

/// A rewrite passes when every removed fragment is gone and every kept one remains.
fn case_passes(rewrite: &str, case: &Case) -> bool {
    let r = normalize(rewrite);
    !case.removed.iter().any(|x| r.contains(&normalize(x)))
        && case.kept.iter().all(|x| r.contains(&normalize(x)))
}

/// Run one judge on one prompt (piped to stdin). Returns the isolable rewrite, or
/// None when no rewrite could be extracted — which counts as unmeasurable, not fail.
fn run_judge(command: &str, prompt: &str) -> Option<String> {
    let mut child = Command::new("bash")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Feed the prompt but don't lose the verdict if the judge didn't drain stdin
    // (a broken pipe here is the judge's choice, not our failure). Closing stdin
    // gives judges that do read it their EOF.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
    }
    let out = child.wait_with_output().ok()?;
    extract_rewrite(&strip_ansi(&String::from_utf8_lossy(&out.stdout)))
}

fn extract_rewrite(text: &str) -> Option<String> {
    if let Some(start) = text.find("<<<REWRITE") {
        let rest = &text[start + "<<<REWRITE".len()..];
        if let Some(end) = rest.find("REWRITE>>>") {
            return Some(rest[..end].trim().to_string());
        }
    }
    // Judges that ignore the markers but fence their code.
    let mut fences = text.match_indices("```");
    if let (Some((a, _)), Some((b, _))) = (fences.next(), fences.next()) {
        let inner = &text[a + 3..b];
        let inner = inner.split_once('\n').map_or(inner, |(_, body)| body);
        return Some(inner.trim().to_string());
    }
    None
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI/other escape: drop until the terminating letter (or a lone '=' / '>').
            if chars.peek() == Some(&'[') {
                for e in chars.by_ref() {
                    if e.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if c == '\n' || c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    out
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn load_cases(path: &Path) -> Result<Vec<Case>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut cases = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let case = serde_json::from_str(line)
            .map_err(|e| format!("{}: line {}: {e}", path.display(), i + 1))?;
        cases.push(case);
    }
    Ok(cases)
}

fn report(
    rule_id: &str,
    fleet: &[Judge],
    cases: &[Case],
    grid: &[Vec<Cell>],
    declared: Severity,
    sem: &Semantic,
) -> u8 {
    let w = cases.iter().map(|c| c.id.len()).max().unwrap_or(4) + 2;
    println!("\nrule {rule_id} · {} cases · {} judges\n", cases.len(), fleet.len());
    print!("{:w$}", "case");
    for j in fleet {
        print!("{:15}", j.name);
    }
    println!();
    for (ci, case) in cases.iter().enumerate() {
        print!("{:w$}", case.id);
        for row in grid {
            print!("{:15}", row[ci].symbol());
        }
        println!();
    }

    // Score each model over the cells it could measure; a model with none is a
    // coverage gap, excluded from the floor rather than counted as zero.
    let mut scores = Vec::new();
    let mut uncertified = Vec::new();
    print!("\n{:w$}", "score");
    for (mi, j) in fleet.iter().enumerate() {
        let measured: Vec<Cell> = grid[mi].iter().copied().filter(|c| *c != Cell::Unmeasurable).collect();
        if measured.is_empty() {
            uncertified.push(j.name.clone());
            print!("{:15}", "n/a");
        } else {
            let s = measured.iter().filter(|c| **c == Cell::Pass).count() as f64 / measured.len() as f64;
            scores.push(s);
            print!("{:15}", format!("{s:.2}"));
        }
    }
    println!();

    let (weakest, earned) = match scores.iter().copied().reduce(f64::min) {
        Some(w) => (w, earned_severity(w, sem)),
        None => {
            println!("\nnothing measurable — no judge produced a gradeable rewrite");
            return 3;
        }
    };
    println!(
        "\nweakest measurable vendor: {weakest:.2}  →  rule earns: {}  ·  declared: {}",
        earned.map_or("unproven", Severity::name),
        declared.name(),
    );
    if !uncertified.is_empty() {
        println!("coverage gap (no `[[judge]]` output was gradeable): {}", uncertified.join(", "));
    }

    if !earned.is_some_and(|e| meets(e, declared)) {
        println!("\n✗ rule does not hold at declared severity `{}`", declared.name());
        return 1;
    }
    if !uncertified.is_empty() {
        println!("\n✗ holds where measured, but the fleet is not fully covered");
        return 3;
    }
    println!("\n✓ proven across the fleet at `{}`", declared.name());
    0
}

fn earned_severity(score: f64, sem: &Semantic) -> Option<Severity> {
    if score >= sem.block_bar {
        Some(Severity::Block)
    } else if score >= sem.nudge_bar {
        Some(Severity::Nudge)
    } else {
        None
    }
}

/// Block is stronger than nudge; a rule earning block also satisfies a nudge declaration.
fn meets(earned: Severity, declared: Severity) -> bool {
    matches!(
        (earned, declared),
        (Severity::Block, _) | (Severity::Nudge, Severity::Nudge)
    )
}

fn fail(msg: &str) -> u8 {
    eprintln!("stele eval: {msg}");
    2
}
