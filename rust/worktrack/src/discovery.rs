//! Discovery of repos and worktrees (spec 4.1). Local git state lives in `localgit`, WorkItem
//! assembly in `workitems`.
//!
//! Public API (blocking: run inside `spawn_blocking`):
//!
//! - [`discover`]`(&Config) -> DiscoveryResult` walks `discovery.roots`, groups candidates by
//!   canonical git common dir, lists worktrees (`git worktree list --porcelain`, authoritative,
//!   worktrees outside the roots included), resolves the Bitbucket identity of `origin` (with
//!   `bitbucket.base_url` and `[[bitbucket.repo_override]]`) and the trunk. Repos without a
//!   Bitbucket DC origin are returned with `skipped_reason` set and no worktrees; every skip and
//!   walk problem is also a line in `diagnostics`.
//! - [`list_worktrees`]`(&Repo)` re-reads the worktree list of a known repo (local reload).
//! - [`store_discovery`]`(&Connection, &DiscoveryResult, now)` replaces the `repos` and
//!   `worktrees` rows; [`load_discovery`]`(&Connection)` reads them back (first frame from cache
//!   and the local reload, which only re-reads known repos).
//! - [`is_protected`]`(branch, trunk)`: trunk, `master`, `main`, `develop`, `release/*`.
//!
//! Full sync order: `discover` -> `store_discovery` -> `lgbc::git::fetch_remote_in` per repo ->
//! `localgit::local_reload` -> `workitems::assemble` -> `workitems::persist`.

use crate::config::{expand_tilde, Config};
use crate::lgbc::remote::{infer_bitbucket_repository, parse_remote_url, RemoteTransport};
use crate::model::{BitbucketIdentity, Repo, RepoId, Worktree};
use color_eyre::eyre::{eyre, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Output of [`discover`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveryResult {
    /// Every repo found, including skipped ones (`skipped_reason` set).
    pub repos: Vec<Repo>,
    /// Worktrees of tracked (non-skipped) repos.
    pub worktrees: Vec<Worktree>,
    /// Human-readable problems for the diagnostics overlay (spec 9.7).
    pub diagnostics: Vec<String>,
}

impl DiscoveryResult {
    /// Repos that are tracked (not skipped).
    pub fn tracked_repos(&self) -> impl Iterator<Item = &Repo> {
        self.repos.iter().filter(|r| r.skipped_reason.is_none())
    }
}

/// `repo_id` = first 16 hex chars of SHA-256 of the canonical common dir path (spec 4.1).
pub fn repo_id_for(common_dir: &Path) -> RepoId {
    let digest = Sha256::digest(common_dir.to_string_lossy().as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Protected = trunk, `master`, `main`, `develop`, `release/*` (spec 4.1).
pub fn is_protected(branch: &str, trunk: Option<&str>) -> bool {
    trunk == Some(branch)
        || matches!(branch, "master" | "main" | "develop")
        || branch.starts_with("release/")
}

/// Scan the configured roots. Never fails: problems become diagnostics.
pub fn discover(cfg: &Config) -> DiscoveryResult {
    let mut diagnostics = Vec::new();
    let excludes = build_excludes(&cfg.discovery.exclude, &mut diagnostics);
    let mut candidates = Vec::new();
    for root in cfg.roots() {
        if !root.is_dir() {
            diagnostics.push(format!(
                "discovery root {} does not exist or is not a directory",
                root.display()
            ));
            continue;
        }
        walk(
            &root,
            0,
            cfg.discovery.max_depth,
            &excludes,
            &mut candidates,
        );
    }
    let mut result = discover_candidates(&candidates, cfg);
    diagnostics.append(&mut result.diagnostics);
    result.diagnostics = diagnostics;
    result
}

fn build_excludes(patterns: &[String], diagnostics: &mut Vec<String>) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        let expanded = expand_tilde(p);
        match Glob::new(&expanded.to_string_lossy()) {
            Ok(g) => {
                builder.add(g);
            }
            Err(e) => diagnostics.push(format!("discovery.exclude: invalid glob `{p}`: {e}")),
        }
    }
    builder.build().unwrap_or_else(|e| {
        diagnostics.push(format!("discovery.exclude: {e}"));
        GlobSet::empty()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateKind {
    /// Has a `.git` dir or file.
    WorkTree,
    /// `HEAD`, `objects/`, `refs/`.
    Bare,
}

fn candidate_kind(dir: &Path) -> Option<CandidateKind> {
    if dir.join(".git").exists() {
        return Some(CandidateKind::WorkTree);
    }
    if dir.join("HEAD").is_file() && dir.join("objects").is_dir() && dir.join("refs").is_dir() {
        return Some(CandidateKind::Bare);
    }
    None
}

/// Depth-first walk. Descends into working trees (bounded by `max_depth`) so nested worktrees
/// and nested repos are found; never descends into `.git` or a bare repo; does not follow
/// symlinked directories.
fn walk(dir: &Path, depth: usize, max_depth: usize, ex: &GlobSet, out: &mut Vec<PathBuf>) {
    match candidate_kind(dir) {
        Some(CandidateKind::Bare) => {
            out.push(dir.to_path_buf());
            return;
        }
        Some(CandidateKind::WorkTree) => out.push(dir.to_path_buf()),
        None => {}
    }
    if depth >= max_depth {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut subdirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| e.file_name() != ".git")
        .map(|e| e.path())
        .filter(|p| !ex.is_match(p))
        .collect();
    subdirs.sort();
    for sub in subdirs {
        walk(&sub, depth + 1, max_depth, ex, out);
    }
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn git_output(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(eyre!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Canonical `git rev-parse --git-common-dir` of `dir`.
pub fn common_dir_of(dir: &Path) -> Result<PathBuf> {
    let out = git_output(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let p = PathBuf::from(out.trim());
    Ok(canonical(&p))
}

/// Group candidate directories into repos and resolve each repo. Exposed for tests.
pub fn discover_candidates(candidates: &[PathBuf], cfg: &Config) -> DiscoveryResult {
    let mut result = DiscoveryResult::default();
    // Paths already covered by a repo's worktree list: no rev-parse needed for them.
    let mut covered: HashSet<PathBuf> = HashSet::new();
    let mut seen_common: HashSet<PathBuf> = HashSet::new();
    let overrides = resolve_overrides(cfg);
    for cand in candidates {
        let cand_c = canonical(cand);
        if covered.contains(&cand_c) {
            continue;
        }
        let common = match common_dir_of(cand) {
            Ok(c) => c,
            Err(e) => {
                result.diagnostics.push(format!(
                    "{}: not a usable git repository: {}",
                    cand.display(),
                    crate::sanitize::clean_line(&e.to_string())
                ));
                continue;
            }
        };
        covered.insert(cand_c);
        if !seen_common.insert(common.clone()) {
            continue;
        }
        let id = repo_id_for(&common);
        let worktrees = match list_worktrees_in(cand, &id) {
            Ok(w) => w,
            Err(e) => {
                result.diagnostics.push(format!(
                    "{}: git worktree list failed: {}",
                    cand.display(),
                    crate::sanitize::clean_line(&e.to_string())
                ));
                continue;
            }
        };
        for wt in &worktrees {
            covered.insert(canonical(&wt.path));
        }
        let repo = resolve_repo(id, common, &worktrees, cfg, &overrides);
        if let Some(reason) = &repo.skipped_reason {
            result.diagnostics.push(format!(
                "skipped {}: {}",
                repo.git_dir_for_commands.display(),
                crate::sanitize::clean_line(reason)
            ));
        } else {
            result.worktrees.extend(worktrees);
        }
        result.repos.push(repo);
    }
    result
}

/// Canonicalized override paths -> (project, repo).
fn resolve_overrides(cfg: &Config) -> HashMap<PathBuf, (String, String)> {
    cfg.bitbucket
        .repo_override
        .iter()
        .map(|o| {
            (
                canonical(&expand_tilde(&o.path)),
                (o.project.clone(), o.repo.clone()),
            )
        })
        .collect()
}

fn resolve_repo(
    id: RepoId,
    common_dir: PathBuf,
    worktrees: &[Worktree],
    cfg: &Config,
    overrides: &HashMap<PathBuf, (String, String)>,
) -> Repo {
    let git_dir_for_commands = worktrees
        .iter()
        .find(|w| w.is_main && w.path.exists())
        .map(|w| w.path.clone())
        .unwrap_or_else(|| common_dir.clone());
    let mut repo = Repo {
        id,
        common_dir: common_dir.clone(),
        git_dir_for_commands: git_dir_for_commands.clone(),
        origin_url: None,
        bb: None,
        trunk: None,
        skipped_reason: None,
    };
    // An override matches the common dir, its parent (`repo/.git`), or any worktree path.
    let override_for = {
        let mut keys = vec![common_dir.clone()];
        if let Some(p) = common_dir.parent() {
            keys.push(p.to_path_buf());
        }
        keys.extend(worktrees.iter().map(|w| canonical(&w.path)));
        keys.into_iter().find_map(|k| overrides.get(&k).cloned())
    };
    let origin_url = git_output(&git_dir_for_commands, &["remote", "get-url", "origin"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(url) = origin_url else {
        repo.skipped_reason = Some("no `origin` remote".into());
        return repo;
    };
    let remote = parse_remote_url("origin", &url);
    repo.origin_url = Some(remote.raw_url.clone());
    match resolve_identity(&remote, &cfg.bitbucket.base_url, override_for.as_ref()) {
        Ok(bb) => repo.bb = Some(bb),
        Err(reason) => {
            repo.skipped_reason = Some(reason);
            return repo;
        }
    }
    repo.trunk = crate::lgbc::git::get_default_branch_in(&git_dir_for_commands, None).ok();
    repo
}

/// The public hosting service behind `host` (`github.com`, `ssh.github.com:443`, ...), if any.
fn public_host(host: &str) -> Option<&'static str> {
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    let is = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    if is("github.com") {
        Some("GitHub")
    } else if is("gitlab.com") {
        Some("GitLab")
    } else if is("bitbucket.org") {
        Some("Bitbucket Cloud")
    } else {
        None
    }
}

/// Bitbucket DC identity of `origin` with config overrides applied. Pure.
pub fn resolve_identity(
    remote: &crate::lgbc::remote::GitRemote,
    base_url_override: &str,
    repo_override: Option<&(String, String)>,
) -> std::result::Result<BitbucketIdentity, String> {
    // An explicit per-repo override wins; otherwise a public hosting service is never Bitbucket
    // DC, even though its URL shape (`host/owner/repo`) parses like one.
    if repo_override.is_none() {
        if let Some(service) = remote.host.as_deref().and_then(public_host) {
            return Err(format!(
                "origin `{}` is a {service} remote, not Bitbucket Data Center",
                remote.raw_url
            ));
        }
    }
    let inferred = infer_bitbucket_repository(remote);
    let base_override = base_url_override.trim().trim_end_matches('/');
    let base_url = if !base_override.is_empty() {
        base_override.to_string()
    } else if let Ok(i) = &inferred {
        i.base_url.clone()
    } else if let (Some(host), Some(_)) = (&remote.host, repo_override) {
        match remote.transport {
            RemoteTransport::Http => format!("http://{host}"),
            RemoteTransport::Https => format!("https://{host}"),
            RemoteTransport::Ssh | RemoteTransport::ScpLike => {
                // SSH port is never an HTTPS port.
                format!("https://{}", host.split(':').next().unwrap_or(host))
            }
            RemoteTransport::Local => {
                return Err(format!(
                    "origin `{}` is not a Bitbucket Data Center URL; set bitbucket.base_url",
                    remote.raw_url
                ))
            }
        }
    } else {
        return Err(format!(
            "origin `{}` is not a Bitbucket Data Center remote",
            remote.raw_url
        ));
    };
    let (project, repo) = match (repo_override, &inferred) {
        (Some((p, r)), _) => (p.clone(), r.clone()),
        (None, Ok(i)) => (i.project_key.clone(), i.repository_slug.clone()),
        (None, Err(_)) => {
            return Err(format!(
                "origin `{}` is not a Bitbucket Data Center remote (no project/repo; add a \
                 [[bitbucket.repo_override]])",
                remote.raw_url
            ))
        }
    };
    Ok(BitbucketIdentity {
        base_url,
        project,
        repo,
    })
}

/// `git worktree list --porcelain` for a known repo.
pub fn list_worktrees(repo: &Repo) -> Result<Vec<Worktree>> {
    list_worktrees_in(&repo.git_dir_for_commands, &repo.id)
}

fn list_worktrees_in(dir: &Path, repo_id: &str) -> Result<Vec<Worktree>> {
    let out = git_output(dir, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_porcelain(&out, repo_id))
}

/// Parse `git worktree list --porcelain`. The first entry is the main worktree (or bare dir).
pub fn parse_worktree_porcelain(text: &str, repo_id: &str) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    for block in text.split("\n\n") {
        let mut wt: Option<Worktree> = None;
        for line in block.lines() {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            if key == "worktree" {
                wt = Some(Worktree {
                    path: PathBuf::from(value),
                    repo_id: repo_id.to_string(),
                    branch: None,
                    head: None,
                    is_main: out.is_empty(),
                    is_bare: false,
                    locked: false,
                    prunable: false,
                    status: None,
                });
                continue;
            }
            let Some(w) = wt.as_mut() else { continue };
            match key {
                "HEAD" => w.head = Some(value.to_string()),
                "branch" => {
                    w.branch = Some(
                        value
                            .strip_prefix("refs/heads/")
                            .unwrap_or(value)
                            .to_string(),
                    )
                }
                "detached" => w.branch = None,
                "bare" => w.is_bare = true,
                "locked" => w.locked = true,
                "prunable" => w.prunable = true,
                _ => {}
            }
        }
        if let Some(w) = wt {
            out.push(w);
        }
    }
    out
}

/// Replace the `repos` and `worktrees` rows with this discovery result.
pub fn store_discovery(conn: &Connection, result: &DiscoveryResult, now: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM worktrees", [])?;
    tx.execute("DELETE FROM repos", [])?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO repos (id, common_dir, origin_url, bb_base_url, bb_project, bb_repo, \
             trunk, skipped_reason, last_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        for r in &result.repos {
            ins.execute(rusqlite::params![
                r.id,
                r.common_dir.to_string_lossy(),
                r.origin_url,
                r.bb.as_ref().map(|b| &b.base_url),
                r.bb.as_ref().map(|b| &b.project),
                r.bb.as_ref().map(|b| &b.repo),
                r.trunk,
                r.skipped_reason,
                now
            ])?;
        }
        let mut ins = tx.prepare(
            "INSERT OR REPLACE INTO worktrees (path, repo_id, branch, head, is_main, locked, \
             prunable, last_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for w in &result.worktrees {
            ins.execute(rusqlite::params![
                w.path.to_string_lossy(),
                w.repo_id,
                w.branch,
                w.head,
                w.is_main,
                w.locked,
                w.prunable,
                now
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Read back what [`store_discovery`] wrote. Worktree `status` is `None`; `is_bare` is
/// reconstructed (main entry without HEAD).
pub fn load_discovery(conn: &Connection) -> Result<(Vec<Repo>, Vec<Worktree>)> {
    let mut stmt = conn.prepare(
        "SELECT path, repo_id, branch, head, is_main, locked, prunable FROM worktrees \
         ORDER BY repo_id, is_main DESC, path",
    )?;
    let worktrees: Vec<Worktree> = stmt
        .query_map([], |r| {
            let is_main: bool = r.get::<_, Option<bool>>(4)?.unwrap_or(false);
            let head: Option<String> = r.get(3)?;
            Ok(Worktree {
                path: PathBuf::from(r.get::<_, String>(0)?),
                repo_id: r.get(1)?,
                branch: r.get(2)?,
                is_bare: is_main && head.is_none(),
                head,
                is_main,
                locked: r.get::<_, Option<bool>>(5)?.unwrap_or(false),
                prunable: r.get::<_, Option<bool>>(6)?.unwrap_or(false),
                status: None,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare(
        "SELECT id, common_dir, origin_url, bb_base_url, bb_project, bb_repo, trunk, \
         skipped_reason FROM repos ORDER BY common_dir",
    )?;
    let repos = stmt
        .query_map([], |r| {
            let id: String = r.get(0)?;
            let common_dir = PathBuf::from(r.get::<_, String>(1)?);
            let bb = match (
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ) {
                (Some(base_url), Some(project), Some(repo)) => Some(BitbucketIdentity {
                    base_url,
                    project,
                    repo,
                }),
                _ => None,
            };
            Ok((id, common_dir, r.get(2)?, bb, r.get(6)?, r.get(7)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(
            |(id, common_dir, origin_url, bb, trunk, skipped_reason): (
                String,
                PathBuf,
                Option<String>,
                Option<BitbucketIdentity>,
                Option<String>,
                Option<String>,
            )| {
                let git_dir_for_commands = worktrees
                    .iter()
                    .find(|w| w.repo_id == id && w.is_main)
                    .map(|w| w.path.clone())
                    .unwrap_or_else(|| common_dir.clone());
                Repo {
                    id,
                    common_dir,
                    git_dir_for_commands,
                    origin_url,
                    bb,
                    trunk,
                    skipped_reason,
                }
            },
        )
        .collect();
    Ok((repos, worktrees))
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Tempfile git helpers shared by the discovery, localgit and workitems tests.
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub const ME: &str = "me@example.com";
    pub const FAKE_ORIGIN: &str = "https://bitbucket.example.com/scm/proj/repo.git";

    /// Run git with a fixed identity; panics on failure. Returns stdout.
    pub fn git(dir: &Path, args: &[&str]) -> String {
        git_as(dir, ME, args)
    }

    pub fn git_as(dir: &Path, email: &str, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                &format!("user.email={email}"),
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .env("GIT_AUTHOR_EMAIL", email)
            .env("GIT_COMMITTER_EMAIL", email)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    pub fn commit(dir: &Path, file: &str, msg: &str) -> String {
        commit_as(dir, ME, file, msg)
    }

    pub fn commit_as(dir: &Path, email: &str, file: &str, msg: &str) -> String {
        let p = dir.join(file);
        let prev = std::fs::read_to_string(&p).unwrap_or_default();
        std::fs::write(&p, format!("{prev}{msg}\n")).unwrap();
        git_as(dir, email, &["add", file]);
        git_as(dir, email, &["commit", "-q", "-m", msg]);
        git(dir, &["rev-parse", "HEAD"])
    }

    /// A bare "server" repo plus a clone at `<root>/work` whose `origin` URL is a fake Bitbucket
    /// URL. Remote-tracking refs are maintained with [`sync`].
    pub struct Fixture {
        pub server: PathBuf,
        pub work: PathBuf,
    }

    impl Fixture {
        pub fn new(root: &Path) -> Self {
            let server = root.join("server.git");
            std::fs::create_dir_all(&server).unwrap();
            git(&server, &["init", "-q", "--bare", "-b", "main"]);
            let work = root.join("work");
            git(
                root,
                &[
                    "clone",
                    "-q",
                    server.to_str().unwrap(),
                    work.to_str().unwrap(),
                ],
            );
            git(&work, &["symbolic-ref", "HEAD", "refs/heads/main"]);
            commit(&work, "README", "init");
            git(&work, &["push", "-q", "origin", "main"]);
            git(&work, &["remote", "set-url", "origin", FAKE_ORIGIN]);
            let f = Self { server, work };
            f.sync();
            git(
                &f.work,
                &[
                    "symbolic-ref",
                    "refs/remotes/origin/HEAD",
                    "refs/remotes/origin/main",
                ],
            );
            git(&f.work, &["branch", "-q", "--set-upstream-to=origin/main"]);
            f
        }

        /// Push `branch` of `dir` to the server and refresh remote-tracking refs.
        pub fn push(&self, dir: &Path, branch: &str) {
            git(
                dir,
                &[
                    "push",
                    "-q",
                    "-f",
                    self.server.to_str().unwrap(),
                    &format!("refs/heads/{branch}:refs/heads/{branch}"),
                ],
            );
            self.sync();
        }

        /// `git fetch --prune` from the server into `refs/remotes/origin/*`.
        pub fn sync(&self) {
            git(
                &self.work,
                &[
                    "fetch",
                    "-q",
                    "--prune",
                    self.server.to_str().unwrap(),
                    "+refs/heads/*:refs/remotes/origin/*",
                ],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;
    use crate::config::RepoOverride;

    fn cfg_for(root: &Path) -> Config {
        let mut cfg = Config::default();
        cfg.discovery.roots = vec![root.to_string_lossy().into_owned()];
        cfg.discovery.max_depth = 4;
        cfg
    }

    #[test]
    fn repo_id_is_16_hex() {
        let id = repo_id_for(Path::new("/x/.git"));
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(id, repo_id_for(Path::new("/y/.git")));
    }

    #[test]
    fn protected_names() {
        assert!(is_protected("main", Some("trunk")));
        assert!(is_protected("trunk", Some("trunk")));
        assert!(is_protected("master", None));
        assert!(is_protected("develop", None));
        assert!(is_protected("release/1.2", None));
        assert!(!is_protected("feature/x", Some("main")));
        assert!(!is_protected("releases", None));
    }

    #[test]
    fn parses_porcelain() {
        let text = "worktree /r/.bare\nbare\n\nworktree /r/main\nHEAD aaa\nbranch refs/heads/main\n\n\
                    worktree /r/det\nHEAD bbb\ndetached\nlocked reason here\n\n\
                    worktree /gone\nHEAD ccc\nbranch refs/heads/feat/x\nprunable gitdir file points to non-existent location\n\n";
        let w = parse_worktree_porcelain(text, "id");
        assert_eq!(w.len(), 4);
        assert!(w[0].is_main && w[0].is_bare && w[0].head.is_none());
        assert_eq!(w[1].branch.as_deref(), Some("main"));
        assert!(!w[1].is_main);
        assert!(w[2].branch.is_none() && w[2].locked && w[2].head.as_deref() == Some("bbb"));
        assert!(w[3].prunable && w[3].branch.as_deref() == Some("feat/x"));
    }

    #[test]
    fn identity_overrides() {
        let r = parse_remote_url("origin", "ssh://git@bb.corp:7999/proj/api.git");
        let id = resolve_identity(&r, "", None).unwrap();
        assert_eq!(id.base_url, "https://bb.corp");
        assert_eq!((id.project.as_str(), id.repo.as_str()), ("PROJ", "api"));
        let id = resolve_identity(&r, "https://bb.corp/ctx/", None).unwrap();
        assert_eq!(id.base_url, "https://bb.corp/ctx");
        let weird = parse_remote_url("origin", "https://bb.corp/some/where/api");
        assert!(resolve_identity(&weird, "", None).is_err());
        let ov = ("LEG".to_string(), "api".to_string());
        let id = resolve_identity(&weird, "", Some(&ov)).unwrap();
        assert_eq!(id.base_url, "https://bb.corp");
        assert_eq!(id.project, "LEG");
        let local = parse_remote_url("origin", "/srv/git/repo.git");
        assert!(resolve_identity(&local, "", None).is_err());
    }

    #[test]
    fn discovers_normal_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = canonical(tmp.path());
        let f = Fixture::new(&root);
        let res = discover(&cfg_for(&root));
        // server.git is a bare repo with no origin: skipped with a reason.
        let tracked: Vec<_> = res.tracked_repos().collect();
        assert_eq!(tracked.len(), 1, "{res:#?}");
        let repo = tracked[0];
        assert_eq!(repo.common_dir, f.work.join(".git"));
        assert_eq!(repo.git_dir_for_commands, f.work);
        assert_eq!(repo.id, repo_id_for(&f.work.join(".git")));
        let bb = repo.bb.as_ref().unwrap();
        assert_eq!(bb.base_url, "https://bitbucket.example.com");
        assert_eq!((bb.project.as_str(), bb.repo.as_str()), ("PROJ", "repo"));
        assert_eq!(repo.trunk.as_deref(), Some("main"));
        assert_eq!(repo.display_name(), "PROJ/repo");
        let skipped: Vec<_> = res
            .repos
            .iter()
            .filter(|r| r.skipped_reason.is_some())
            .collect();
        assert_eq!(skipped.len(), 1);
        assert!(res.diagnostics.iter().any(|d| d.contains("no `origin`")));
        assert_eq!(res.worktrees.len(), 1);
        assert!(res.worktrees[0].is_main);
        assert_eq!(res.worktrees[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn nested_outside_detached_and_prunable_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let base = canonical(tmp.path());
        let root = base.join("roots");
        std::fs::create_dir_all(&root).unwrap();
        let f = Fixture::new(&root);
        // Nested inside the main working tree.
        let nested = f.work.join("wt/feat");
        git(
            &f.work,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat",
                nested.to_str().unwrap(),
            ],
        );
        // Outside the roots.
        let outside = base.join("elsewhere/wt2");
        git(
            &f.work,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "other",
                outside.to_str().unwrap(),
            ],
        );
        // Detached + locked.
        let det = root.join("det");
        git(
            &f.work,
            &["worktree", "add", "-q", "--detach", det.to_str().unwrap()],
        );
        git(&f.work, &["worktree", "lock", det.to_str().unwrap()]);
        // Prunable: directory removed behind git's back.
        let gone = root.join("gone");
        git(
            &f.work,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "gone",
                gone.to_str().unwrap(),
            ],
        );
        std::fs::remove_dir_all(&gone).unwrap();

        let res = discover(&cfg_for(&root));
        let tracked: Vec<_> = res.tracked_repos().collect();
        assert_eq!(
            tracked.len(),
            1,
            "nested worktree must not be a separate repo"
        );
        let by_path: HashMap<_, _> = res.worktrees.iter().map(|w| (w.path.clone(), w)).collect();
        assert_eq!(res.worktrees.len(), 5, "{:#?}", res.worktrees);
        assert_eq!(by_path[&nested].branch.as_deref(), Some("feat"));
        assert_eq!(by_path[&outside].branch.as_deref(), Some("other"));
        assert!(by_path[&det].branch.is_none() && by_path[&det].locked);
        assert!(by_path[&gone].prunable);
        assert!(res.worktrees.iter().all(|w| w.repo_id == tracked[0].id));
    }

    #[test]
    fn bare_with_worktrees_and_override() {
        let tmp = tempfile::tempdir().unwrap();
        let root = canonical(tmp.path());
        let f = Fixture::new(&root.join("src"));
        let proj = root.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let bare = proj.join(".bare");
        git(
            &root,
            &[
                "clone",
                "-q",
                "--bare",
                f.server.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        std::fs::write(proj.join(".git"), "gitdir: ./.bare\n").unwrap();
        git(
            &bare,
            &[
                "remote",
                "set-url",
                "origin",
                "git@bb.corp:legacy/thing.git",
            ],
        );
        git(
            &bare,
            &[
                "worktree",
                "add",
                "-q",
                proj.join("main").to_str().unwrap(),
                "main",
            ],
        );
        git(
            &bare,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature/a",
                proj.join("feature-a").to_str().unwrap(),
            ],
        );
        let mut cfg = cfg_for(&root);
        cfg.discovery.exclude = vec!["**/src".into()];
        cfg.bitbucket.repo_override = vec![RepoOverride {
            path: proj.to_string_lossy().into_owned(),
            project: "LEG".into(),
            repo: "api".into(),
        }];
        let res = discover(&cfg);
        let tracked: Vec<_> = res.tracked_repos().collect();
        assert_eq!(tracked.len(), 1, "{res:#?}");
        let repo = tracked[0];
        assert_eq!(repo.common_dir, bare);
        assert_eq!(repo.git_dir_for_commands, bare);
        let bb = repo.bb.as_ref().unwrap();
        assert_eq!(
            (bb.base_url.as_str(), bb.project.as_str(), bb.repo.as_str()),
            ("https://bb.corp", "LEG", "api")
        );
        assert_eq!(repo.trunk.as_deref(), Some("main"));
        assert_eq!(res.worktrees.len(), 3);
        assert!(res.worktrees[0].is_bare && res.worktrees[0].is_main);
        assert!(res
            .worktrees
            .iter()
            .any(|w| w.branch.as_deref() == Some("feature/a")));
    }

    #[test]
    fn max_depth_and_excludes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = canonical(tmp.path());
        Fixture::new(&root.join("a/b/c"));
        let mut cfg = cfg_for(&root);
        cfg.discovery.max_depth = 2;
        assert_eq!(discover(&cfg).tracked_repos().count(), 0);
        cfg.discovery.max_depth = 4;
        assert_eq!(discover(&cfg).tracked_repos().count(), 1);
        cfg.discovery.exclude = vec!["**/b".into()];
        assert_eq!(discover(&cfg).tracked_repos().count(), 0);
    }

    #[test]
    fn store_and_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = canonical(tmp.path());
        let f = Fixture::new(&root);
        git(
            &f.work,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "x",
                root.join("x").to_str().unwrap(),
            ],
        );
        let res = discover(&cfg_for(&root));
        let conn = crate::store::open_in_memory().unwrap();
        store_discovery(&conn, &res, 100).unwrap();
        store_discovery(&conn, &res, 200).unwrap();
        let (repos, worktrees) = load_discovery(&conn).unwrap();
        let mut expected = res.repos.clone();
        expected.sort_by(|a, b| a.common_dir.cmp(&b.common_dir));
        assert_eq!(repos, expected);
        assert_eq!(worktrees.len(), res.worktrees.len());
        let tracked = repos.iter().find(|r| r.skipped_reason.is_none()).unwrap();
        assert_eq!(tracked.git_dir_for_commands, f.work);
        let live = list_worktrees(tracked).unwrap();
        assert_eq!(live.len(), 2);
    }

    #[test]
    fn public_hosts_are_not_bitbucket_dc() {
        for (url, service) in [
            ("git@github.com:EmilIvanichkovv/omni-scripts.git", "GitHub"),
            ("https://github.com/storytold/pdfcraft", "GitHub"),
            ("ssh://git@ssh.github.com:443/owner/repo.git", "GitHub"),
            ("https://gitlab.com/group/repo.git", "GitLab"),
            ("git@bitbucket.org:team/repo.git", "Bitbucket Cloud"),
        ] {
            let remote = parse_remote_url("origin", url);
            let err = resolve_identity(&remote, "", None).unwrap_err();
            assert!(err.contains(service), "{url}: {err}");
            // Even a global base_url does not turn them into Bitbucket DC repos.
            assert!(resolve_identity(&remote, "https://bitbucket.example.com", None).is_err());
        }
        // A Bitbucket DC host is still accepted, and an explicit per-repo override wins.
        let dc = parse_remote_url(
            "origin",
            "ssh://git@bitbucket.example.com:7999/og/crash-games.git",
        );
        assert_eq!(resolve_identity(&dc, "", None).unwrap().repo, "crash-games");
        let gh = parse_remote_url("origin", "git@github.com:owner/repo.git");
        let id = resolve_identity(
            &gh,
            "https://bitbucket.example.com",
            Some(&("P".to_string(), "r".to_string())),
        )
        .unwrap();
        assert_eq!((id.project.as_str(), id.repo.as_str()), ("P", "r"));
    }
}
