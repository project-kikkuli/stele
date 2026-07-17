//! Ergonomic agent launcher: create an isolated linked worktree, then select
//! the strongest available Stele channel for the chosen harness.
//!
//! `stele run cursor "task"` hides Cursor headless's synthesized stop-loop.
//! Native-hook harnesses are launched normally from the same managed
//! worktree. This keeps worktree policy and harness quirks out of muscle
//! memory while leaving `stele wrap` available as a low-level adapter.

use crate::{config, engine, substrate, wrap};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{SystemTime, UNIX_EPOCH};

pub const STATE_HOME_ENV: &str = "STELE_STATE_HOME";

pub fn run(
    agent: &str,
    prompt: Option<&str>,
    args: &[String],
    max_loops: u32,
    name: Option<&str>,
) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            eprintln!("stele run: {error}");
            return 2;
        }
    };
    let root = match substrate::find_root(&cwd) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("stele run: {error}");
            return 2;
        }
    };
    if let Err(error) = config::load(&root) {
        eprintln!("stele run: {error} (run `stele install global` first)");
        return 2;
    }

    let target = match launch_target(&root, &cwd, name) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("stele run: {error}");
            return 2;
        }
    };
    if target.created {
        eprintln!(
            "stele run: created linked worktree {} on `{}`",
            target.root.display(),
            target.branch.as_deref().unwrap_or("detached")
        );
    } else {
        eprintln!("stele run: using linked worktree {}", target.root.display());
    }

    match preflight(&target.cwd) {
        Ok(Some(reason)) => {
            eprintln!("stele run: session preflight failed:\n{reason}");
            return 1;
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("stele run: couldn't measure managed worktree: {error}");
            return 2;
        }
    }

    let cursor = matches!(agent, "cursor" | "cursor-agent");
    if cursor && prompt.is_some() {
        let mut command = vec![
            "cursor-agent".to_string(),
            "-p".to_string(),
            "--force".to_string(),
        ];
        command.extend(args.iter().cloned());
        if let Err(error) = std::env::set_current_dir(&target.cwd) {
            eprintln!("stele run: {}: {error}", target.cwd.display());
            return 2;
        }
        return wrap::run(max_loops, prompt.unwrap_or_default(), &command);
    }

    let executable = if agent == "cursor" {
        "cursor-agent"
    } else {
        agent
    };
    let mut command = Command::new(executable);
    command.args(args).current_dir(&target.cwd);
    if let Some(prompt) = prompt {
        if agent == "hermes" {
            command.args(["-z", prompt]);
        } else {
            command.arg(prompt);
        }
    }
    let status = match command.status() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("stele run: failed to launch `{executable}`: {error}");
            return 2;
        }
    };
    if !status.success() {
        return exit_code(status);
    }
    postcheck(&target.cwd)
}

struct LaunchTarget {
    root: PathBuf,
    cwd: PathBuf,
    branch: Option<String>,
    created: bool,
}

fn launch_target(root: &Path, cwd: &Path, name: Option<&str>) -> Result<LaunchTarget, String> {
    if is_linked_worktree(root)? {
        return Ok(LaunchTarget {
            root: root.to_path_buf(),
            cwd: cwd.to_path_buf(),
            branch: None,
            created: false,
        });
    }

    if !git(root, &["status", "--porcelain"])?.trim().is_empty() {
        eprintln!(
            "stele run: note: primary-checkout changes are not copied; the managed worktree starts at HEAD"
        );
    }

    let id = name
        .map(sanitize)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default();
            format!("{nanos:x}-{}", std::process::id())
        });
    let repo_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .map(sanitize)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "repo".to_string());
    let mut hasher = Sha256::new();
    hasher.update(root.as_os_str().as_encoded_bytes());
    let digest = format!("{:x}", hasher.finalize());
    let repo_key = format!("{repo_name}-{}", &digest[..8]);
    let worktree = state_home()?.join("worktrees").join(repo_key).join(&id);
    let branch = format!("stele/{id}");
    if let Some(parent) = worktree.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["worktree", "add", "--quiet", "-b"])
        .arg(&branch)
        .arg(&worktree)
        .arg("HEAD")
        .output()
        .map_err(|e| format!("git worktree add: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git worktree add: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let relative = cwd.strip_prefix(root).unwrap_or(Path::new(""));
    let candidate = worktree.join(relative);
    let launch_cwd = if candidate.is_dir() {
        candidate
    } else {
        worktree.clone()
    };
    Ok(LaunchTarget {
        root: worktree,
        cwd: launch_cwd,
        branch: Some(branch),
        created: true,
    })
}

fn is_linked_worktree(root: &Path) -> Result<bool, String> {
    let git_dir = git(root, &["rev-parse", "--path-format=absolute", "--git-dir"])?;
    let common_dir = git(
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    Ok(git_dir.trim() != common_dir.trim())
}

fn state_home() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os(STATE_HOME_ENV) {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(path).join("stele"));
    }
    let home = std::env::var_os("HOME").ok_or("no $HOME; set STELE_STATE_HOME")?;
    Ok(PathBuf::from(home).join(".local/state/stele"))
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn preflight(cwd: &Path) -> Result<Option<String>, String> {
    let sub = substrate::compute(cwd)?;
    let rules = config::load(&sub.root)?;
    let verdict = engine::check(&rules, &sub);
    if !verdict.errors().is_empty() {
        return Err(verdict
            .errors()
            .iter()
            .map(|result| {
                format!(
                    "rule `{}`: {}",
                    result.rule.id,
                    result.error.as_deref().unwrap_or("couldn't measure")
                )
            })
            .collect::<Vec<_>>()
            .join("\n"));
    }
    Ok((!verdict.preflight().is_empty()).then(|| engine::render_preflight(&verdict)))
}

fn postcheck(cwd: &Path) -> i32 {
    let sub = match substrate::compute(cwd) {
        Ok(sub) => sub,
        Err(error) => {
            eprintln!("stele run: post-run measurement failed: {error}");
            return 3;
        }
    };
    let rules = match config::load(&sub.root) {
        Ok(rules) => rules,
        Err(error) => {
            eprintln!("stele run: post-run measurement failed: {error}");
            return 3;
        }
    };
    let verdict = engine::check(&rules, &sub);
    if !verdict.errors().is_empty() {
        eprintln!("stele run: one or more rules couldn't be measured");
        return 3;
    }
    if !verdict.blocking().is_empty() {
        eprintln!("{}", engine::render_reason(&verdict));
        return 1;
    }
    0
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1).clamp(1, 255)
}
