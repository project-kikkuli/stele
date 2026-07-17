//! The measurement substrate: one canonical answer to "what is this session's
//! change-set", computed once per check. Rule checkers receive this and never
//! run git themselves — every environment-variance bug (cwd, worktrees, merge
//! parents) gets fixed here or nowhere.
//!
//! Mechanism (ported from august's battle-tested intent-hook):
//! 1. Snapshot the worktree — uncommitted AND untracked — into a throwaway
//!    tree via a temp index, with snapshot objects written to a temp object
//!    directory (the repo's own object store never sees a snapshot blob).
//! 2. Parent a throwaway commit on HEAD *and every MERGE_HEAD*, so during an
//!    in-progress merge the three-dot diff resolves its base against the
//!    mainline tip and incoming mainline content is NOT attributed to this
//!    session's change-set (august bugs #12886/#12905).
//! 3. change-set = `git diff --name-only base...snapshot`;
//!    signature = the snapshot tree hash + base (content-exact, cheap).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const BASE_CANDIDATES: [&str; 4] = ["origin/main", "origin/master", "main", "master"];

#[derive(Debug)]
pub struct Substrate {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub base: Option<String>,
    /// Throwaway commit capturing the exact measured state (worktree incl.
    /// untracked). Objects live in a temp dir that is deleted after compute;
    /// treat this id as valid only for logging, not later dereference.
    pub snapshot: Option<String>,
    /// Repo-relative paths of the session's change-set.
    pub changed: Vec<String>,
    /// Content-exact key for the speak-once / verdict caches.
    pub signature: String,
}

fn git_env(root: &Path, envs: &[(&str, &str)], args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    git_env(root, &[], args)
}

/// Like `git()` but a failure is an empty answer, not an error (for probes).
fn git_ok(root: &Path, args: &[&str]) -> String {
    git(root, args).unwrap_or_default()
}

pub fn find_root(start: &Path) -> Result<PathBuf, String> {
    Ok(PathBuf::from(
        git(start, &["rev-parse", "--show-toplevel"])?.trim(),
    ))
}

pub fn find_git_dir(root: &Path) -> Result<PathBuf, String> {
    Ok(PathBuf::from(
        git(root, &["rev-parse", "--absolute-git-dir"])?.trim(),
    ))
}

/// (diff_base, merge_base): `diff_base` is the mainline REF when one exists —
/// the three-dot diff must re-resolve its merge-base against the mainline TIP
/// so that a snapshot parented on MERGE_HEAD excludes incoming mainline work.
/// Using the precomputed merge-base commit there would resolve to the old
/// fork point and re-attribute the whole merge (the august #12886 bug).
fn resolve_base(root: &Path) -> Option<(String, String)> {
    let has_head = !git_ok(root, &["rev-parse", "--verify", "-q", "HEAD"])
        .trim()
        .is_empty();
    if !has_head {
        return None;
    }
    for cand in BASE_CANDIDATES {
        let mb = git_ok(root, &["merge-base", cand, "HEAD"]);
        if !mb.trim().is_empty() {
            return Some((cand.to_string(), mb.trim().to_string()));
        }
    }
    // No mainline ref: measure everything since the root commit.
    git_ok(root, &["rev-list", "--max-parents=0", "HEAD"])
        .split_whitespace()
        .last()
        .map(|c| (c.to_string(), c.to_string()))
}

/// Temp directory that cleans up on drop (std-only, no tempfile dep in lib).
struct TmpDir(PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Option<Self> {
        let dir = std::env::temp_dir().join(format!(
            "stele-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).ok()?;
        Some(TmpDir(dir))
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn compute(start: &Path) -> Result<Substrate, String> {
    let root = find_root(start)?;
    let git_dir = PathBuf::from(git(&root, &["rev-parse", "--absolute-git-dir"])?.trim());
    let resolved = resolve_base(&root);
    let base = resolved.as_ref().map(|(_, mb)| mb.clone());

    // Snapshot path: hermetic view, merge-aware attribution, exact signature.
    // The temp object dirs must outlive the diffs, so all snapshot-dependent
    // reads happen inside this block.
    if let Some((diff_base, _)) = &resolved {
        let obj_tmp = TmpDir::new("objects");
        let idx_tmp = TmpDir::new("index");
        if let (Some(obj_tmp), Some(idx_tmp)) = (obj_tmp, idx_tmp) {
            let index_file = idx_tmp.0.join("index");
            let obj_real = git_dir.join("objects");
            let envs: Vec<(String, String)> = vec![
                ("GIT_INDEX_FILE".into(), index_file.display().to_string()),
                ("GIT_OBJECT_DIRECTORY".into(), obj_tmp.0.display().to_string()),
                (
                    "GIT_ALTERNATE_OBJECT_DIRECTORIES".into(),
                    obj_real.display().to_string(),
                ),
            ];
            let env_refs: Vec<(&str, &str)> =
                envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

            let made = (|| -> Option<(String, String)> {
                git_env(&root, &env_refs, &["add", "-A"]).ok()?;
                let tree = git_env(&root, &env_refs, &["write-tree"]).ok()?.trim().to_string();
                let mut parent_args: Vec<String> = Vec::new();
                let head = git_ok(&root, &["rev-parse", "--verify", "-q", "HEAD"]);
                if !head.trim().is_empty() {
                    parent_args.extend(["-p".into(), head.trim().to_string()]);
                }
                if let Ok(mh) = std::fs::read_to_string(git_dir.join("MERGE_HEAD")) {
                    for line in mh.lines().filter(|l| !l.trim().is_empty()) {
                        parent_args.extend(["-p".into(), line.trim().to_string()]);
                    }
                }
                let mut args: Vec<&str> = vec!["commit-tree", &tree];
                for a in &parent_args {
                    args.push(a);
                }
                args.extend(["-m", "stele-snapshot"]);
                let commit = git_env(&root, &env_refs, &args).ok()?.trim().to_string();
                (!commit.is_empty()).then_some((tree, commit))
            })();

            if let Some((tree, commit)) = made {
                // Three-dot against the mainline REF: merge-base(ref, snapshot)
                // — with MERGE_HEAD parents this lands on the mainline tip
                // mid-merge, so incoming mainline work is excluded.
                let range = format!("{diff_base}...{commit}");
                let changed: Vec<String> = git_env(
                    &root,
                    &env_refs,
                    &["diff", "--name-only", &range],
                )
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
                return Ok(Substrate {
                    root,
                    git_dir,
                    base,
                    snapshot: Some(commit),
                    signature: format!("{tree}:{diff_base}"),
                    changed,
                });
            }
        }
    }

    // Fallback (no HEAD / snapshot impossible): live-worktree measurement.
    compute_fallback(root, git_dir, base)
}

fn compute_fallback(
    root: PathBuf,
    git_dir: PathBuf,
    base: Option<String>,
) -> Result<Substrate, String> {
    use sha2::{Digest, Sha256};
    let mut changed: BTreeSet<String> = BTreeSet::new();
    let mut hasher = Sha256::new();

    if let Some(b) = &base {
        let range = format!("{b}...HEAD");
        for f in git_ok(&root, &["diff", "--name-only", &range]).split_whitespace() {
            changed.insert(f.to_string());
        }
        hasher.update(git_ok(&root, &["diff", &range]));
    }
    for f in git_ok(&root, &["diff", "--name-only"]).split_whitespace() {
        changed.insert(f.to_string());
    }
    for f in git_ok(&root, &["diff", "--name-only", "--cached"]).split_whitespace() {
        changed.insert(f.to_string());
    }
    let untracked = git_ok(&root, &["ls-files", "--others", "--exclude-standard"]);
    hasher.update(git_ok(&root, &["diff"]));
    hasher.update(git_ok(&root, &["diff", "--cached"]));
    for f in untracked.split_whitespace() {
        changed.insert(f.to_string());
        hasher.update(f);
        if let Ok(bytes) = std::fs::read(root.join(f)) {
            hasher.update(&bytes);
        }
    }

    Ok(Substrate {
        root,
        git_dir,
        base,
        snapshot: None,
        changed: changed.into_iter().collect(),
        signature: format!("{:x}", hasher.finalize()),
    })
}

/// Commit messages between base and HEAD — the ack-trailer search space.
pub fn commit_messages_since_base(sub: &Substrate) -> String {
    let Some(base) = &sub.base else {
        return String::new();
    };
    git_ok(&sub.root, &["log", "--format=%B", &format!("{base}..HEAD")])
}
