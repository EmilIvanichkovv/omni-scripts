//! Git remote URL parsing and Bitbucket Data Center repository inference
//!
//! Parses the `origin` remote URL into a normalized [`GitRemote`] and infers
//! the Bitbucket base URL, project key, and repository slug according to
//! `docs/specs/BITBUCKET_SUPPORT.md` (section 6).

use color_eyre::Result;
use std::process::Command;

/// Transport scheme detected in a Git remote URL
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteTransport {
    /// `http://` URL
    Http,
    /// `https://` URL
    Https,
    /// `ssh://` URL
    Ssh,
    /// SCP-like syntax without a scheme (e.g. `git@host:path/repo.git`)
    ScpLike,
    /// Local path or any unrecognized scheme
    Local,
}

/// Normalized model of a Git remote
///
/// Credentials (userinfo such as `user:pass@`) present in the original URL
/// are stripped during parsing and never appear in any field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRemote {
    /// Remote name (e.g. "origin")
    pub name: String,
    /// The remote URL, sanitized: userinfo (credentials) removed
    pub raw_url: String,
    /// Host component, without userinfo.
    ///
    /// For HTTP(S) remotes this includes an explicit port (e.g. "host:7990")
    /// because the port participates in the derived base URL. For SSH remotes
    /// the port is dropped — an SSH port must never be reused as an HTTPS port.
    pub host: Option<String>,
    /// Path components, split on '/', empty components removed
    pub path_segments: Vec<String>,
    /// Detected transport
    pub transport: RemoteTransport,
}

/// Bitbucket repository identity inferred from a Git remote
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferredBitbucketRepo {
    /// Base URL without trailing slash, context path preserved
    /// (e.g. `https://host/bitbucket`)
    pub base_url: String,
    /// Project key, canonicalized to ASCII uppercase
    pub project_key: String,
    /// Repository slug with any final `.git` stripped
    pub repository_slug: String,
}

/// Read and parse the `origin` remote of the current repository
///
/// Executes `git remote get-url origin` and returns a configuration error
/// if no usable `origin` exists.
pub fn read_origin() -> Result<GitRemote> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()?;

    if !output.status.success() {
        return Err(color_eyre::eyre::eyre!(
            "No usable 'origin' remote is configured for this repository. \
             Add one with `git remote add origin <url>` or supply \
             --bitbucket-base-url, --bitbucket-project, and --bitbucket-repo."
        ));
    }

    let url = String::from_utf8(output.stdout)?.trim().to_string();
    if url.is_empty() {
        return Err(color_eyre::eyre::eyre!(
            "The 'origin' remote has an empty URL. \
             Fix it with `git remote set-url origin <url>` or supply \
             --bitbucket-base-url, --bitbucket-project, and --bitbucket-repo."
        ));
    }

    Ok(parse_remote_url("origin", &url))
}

/// Parse a Git remote URL into a normalized [`GitRemote`]
///
/// Pure function (no subprocesses) so parsing is unit-testable. Never fails:
/// unrecognized inputs are classified as [`RemoteTransport::Local`] and
/// rejected later by [`infer_bitbucket_repository`].
pub fn parse_remote_url(name: &str, url: &str) -> GitRemote {
    let url = url.trim();

    // URL-style remotes: <scheme>://[userinfo@]host[:port][/path]
    if let Some((scheme, rest)) = url.split_once("://") {
        let transport = match scheme.to_ascii_lowercase().as_str() {
            "http" => RemoteTransport::Http,
            "https" => RemoteTransport::Https,
            "ssh" => RemoteTransport::Ssh,
            _ => RemoteTransport::Local,
        };

        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));

        // Strip userinfo (credentials) — it must never leak into outputs.
        let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);

        // For SSH, drop an explicit port: it is an SSH port and must not be
        // reused when deriving an HTTPS base URL.
        let host = if transport == RemoteTransport::Ssh {
            strip_port(authority)
        } else {
            authority
        };

        let sanitized = if path.is_empty() {
            format!("{scheme}://{authority}")
        } else {
            format!("{scheme}://{authority}/{path}")
        };

        return GitRemote {
            name: name.to_string(),
            raw_url: sanitized,
            host: non_empty(host),
            path_segments: split_segments(path),
            transport,
        };
    }

    // SCP-like remotes: [user@]host:path — a colon before the first slash.
    if let Some((head, path)) = url.split_once(':') {
        if !head.contains('/') {
            // Strip userinfo (credentials) — it must never leak into outputs.
            let host = head.rsplit_once('@').map_or(head, |(_, h)| h);

            return GitRemote {
                name: name.to_string(),
                raw_url: format!("{host}:{path}"),
                host: non_empty(host),
                path_segments: split_segments(path),
                transport: RemoteTransport::ScpLike,
            };
        }
    }

    // Anything else is a local path (or otherwise unusable for inference).
    GitRemote {
        name: name.to_string(),
        raw_url: url.to_string(),
        host: None,
        path_segments: split_segments(url),
        transport: RemoteTransport::Local,
    }
}

/// Infer the Bitbucket base URL, project key, and repository slug from a remote
///
/// Never falls back to placeholder values: when inference is impossible the
/// `Err` names the missing component and the exact override flags to supply.
pub fn infer_bitbucket_repository(remote: &GitRemote) -> Result<InferredBitbucketRepo, String> {
    match remote.transport {
        RemoteTransport::Http | RemoteTransport::Https => infer_from_http(remote),
        RemoteTransport::Ssh | RemoteTransport::ScpLike => infer_from_ssh(remote),
        RemoteTransport::Local => Err(format!(
            "Could not infer a Bitbucket repository from remote '{}' ({}): \
             the URL is not an http(s), ssh, or SCP-like Git URL. \
             Supply --bitbucket-base-url, --bitbucket-project, and --bitbucket-repo.",
            remote.name, remote.raw_url
        )),
    }
}

/// Inference for HTTP(S) remotes shaped `<base>/scm/<project>/<repo>.git`
/// (an optional context path may precede `/scm/`)
fn infer_from_http(remote: &GitRemote) -> Result<InferredBitbucketRepo, String> {
    let host = remote.host.as_deref().ok_or_else(|| {
        format!(
            "Could not derive the Bitbucket base URL from remote '{}' ({}): \
             the URL has no host. \
             Supply --bitbucket-base-url, --bitbucket-project, and --bitbucket-repo.",
            remote.name, remote.raw_url
        )
    })?;

    // Expect path segments shaped [context..., "scm", <project>, <repo>].
    let segments = &remote.path_segments;
    if segments.len() < 3 || segments[segments.len() - 3] != "scm" {
        return Err(format!(
            "Could not derive the Bitbucket project key and repository slug \
             from remote '{}' ({}): expected a path shaped \
             '.../scm/<project>/<repo>.git'. \
             Supply --bitbucket-project and --bitbucket-repo and, if needed, \
             --bitbucket-base-url.",
            remote.name, remote.raw_url
        ));
    }

    // The base URL is everything before "/scm/": scheme, host (with any
    // explicit port), and the context path. No trailing slash.
    let scheme = match remote.transport {
        RemoteTransport::Http => "http",
        _ => "https",
    };
    let mut base_url = format!("{scheme}://{host}");
    for segment in &segments[..segments.len() - 3] {
        base_url.push('/');
        base_url.push_str(segment);
    }

    build_repo(
        remote,
        base_url,
        &segments[segments.len() - 2],
        &segments[segments.len() - 1],
    )
}

/// Inference for SSH (`ssh://git@host:7999/proj/repo.git`) and SCP-like
/// (`git@host:proj/repo.git`) remotes
///
/// Derives `https://<host>` — the SSH port is never reused as an HTTPS port,
/// and no context path can be inferred from an SSH remote.
fn infer_from_ssh(remote: &GitRemote) -> Result<InferredBitbucketRepo, String> {
    let host = remote.host.as_deref().ok_or_else(|| {
        format!(
            "Could not derive the Bitbucket base URL from remote '{}' ({}): \
             the URL has no host. \
             Supply --bitbucket-base-url, --bitbucket-project, and --bitbucket-repo.",
            remote.name, remote.raw_url
        )
    })?;

    // Expect path segments shaped exactly [<project>, <repo>].
    let segments = &remote.path_segments;
    if segments.len() != 2 {
        return Err(format!(
            "Could not derive the Bitbucket project key and repository slug \
             from remote '{}' ({}): expected a path shaped \
             '<project>/<repo>.git'. \
             Supply --bitbucket-project and --bitbucket-repo and, if needed, \
             --bitbucket-base-url.",
            remote.name, remote.raw_url
        ));
    }

    build_repo(
        remote,
        format!("https://{host}"),
        &segments[0],
        &segments[1],
    )
}

/// Validate and normalize the project/repository components into the result
fn build_repo(
    remote: &GitRemote,
    base_url: String,
    project: &str,
    repo: &str,
) -> Result<InferredBitbucketRepo, String> {
    // Strip a final `.git` suffix from the repository slug.
    let repo = repo.strip_suffix(".git").unwrap_or(repo);

    if project.is_empty() {
        return Err(format!(
            "Could not derive the Bitbucket project key from remote '{}' ({}): \
             the project component is empty. \
             Supply --bitbucket-project and, if needed, --bitbucket-base-url \
             and --bitbucket-repo.",
            remote.name, remote.raw_url
        ));
    }
    if repo.is_empty() {
        return Err(format!(
            "Could not derive the Bitbucket repository slug from remote '{}' ({}): \
             the repository component is empty. \
             Supply --bitbucket-repo and, if needed, --bitbucket-base-url \
             and --bitbucket-project.",
            remote.name, remote.raw_url
        ));
    }

    Ok(InferredBitbucketRepo {
        base_url,
        // Inferred project keys are canonicalized to ASCII uppercase.
        project_key: project.to_ascii_uppercase(),
        repository_slug: repo.to_string(),
    })
}

/// Remove a trailing `:<digits>` port from an authority string
fn strip_port(authority: &str) -> &str {
    match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => host,
        _ => authority,
    }
}

/// Split a path into non-empty '/'-separated segments
fn split_segments(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Convert a possibly-empty &str into Option<String>
fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse and infer in one step for test brevity
    fn infer(url: &str) -> Result<InferredBitbucketRepo, String> {
        infer_bitbucket_repository(&parse_remote_url("origin", url))
    }

    fn assert_inferred(url: &str, base_url: &str, project: &str, repo: &str) {
        let inferred = infer(url).expect("inference should succeed");
        assert_eq!(inferred.base_url, base_url);
        assert_eq!(inferred.project_key, project);
        assert_eq!(inferred.repository_slug, repo);
    }

    // --- parse_remote_url ---

    #[test]
    fn test_parse_https_remote() {
        let remote = parse_remote_url(
            "origin",
            "https://bitbucket.example.com/scm/proj/demo-repo.git",
        );
        assert_eq!(remote.name, "origin");
        assert_eq!(remote.transport, RemoteTransport::Https);
        assert_eq!(remote.host.as_deref(), Some("bitbucket.example.com"));
        assert_eq!(remote.path_segments, ["scm", "proj", "demo-repo.git"]);
    }

    #[test]
    fn test_parse_http_remote_keeps_port_in_host() {
        let remote = parse_remote_url("origin", "http://host:7990/scm/proj/demo-repo.git");
        assert_eq!(remote.transport, RemoteTransport::Http);
        assert_eq!(remote.host.as_deref(), Some("host:7990"));
    }

    #[test]
    fn test_parse_ssh_remote_drops_port_from_host() {
        let remote = parse_remote_url(
            "origin",
            "ssh://git@bitbucket.example.com:7999/proj/demo-repo.git",
        );
        assert_eq!(remote.transport, RemoteTransport::Ssh);
        assert_eq!(remote.host.as_deref(), Some("bitbucket.example.com"));
        assert_eq!(remote.path_segments, ["proj", "demo-repo.git"]);
    }

    #[test]
    fn test_parse_scp_like_remote() {
        let remote = parse_remote_url("origin", "git@bitbucket.example.com:proj/demo-repo.git");
        assert_eq!(remote.transport, RemoteTransport::ScpLike);
        assert_eq!(remote.host.as_deref(), Some("bitbucket.example.com"));
        assert_eq!(remote.path_segments, ["proj", "demo-repo.git"]);
    }

    #[test]
    fn test_parse_local_path() {
        let remote = parse_remote_url("origin", "/home/user/repos/demo-repo");
        assert_eq!(remote.transport, RemoteTransport::Local);
        assert_eq!(remote.host, None);
    }

    #[test]
    fn test_parse_unknown_scheme_is_local() {
        let remote = parse_remote_url("origin", "ftp://host/scm/proj/demo-repo.git");
        assert_eq!(remote.transport, RemoteTransport::Local);
    }

    // --- Required inference results (spec section 6.3, one test per row) ---

    #[test]
    fn test_infer_https_standard() {
        assert_inferred(
            "https://bitbucket.example.com/scm/proj/demo-repo.git",
            "https://bitbucket.example.com",
            "PROJ",
            "demo-repo",
        );
    }

    #[test]
    fn test_infer_https_with_context_path() {
        assert_inferred(
            "https://host/bitbucket/scm/proj/demo-repo.git",
            "https://host/bitbucket",
            "PROJ",
            "demo-repo",
        );
    }

    #[test]
    fn test_infer_http_with_port() {
        assert_inferred(
            "http://host:7990/scm/proj/demo-repo.git",
            "http://host:7990",
            "PROJ",
            "demo-repo",
        );
    }

    #[test]
    fn test_infer_ssh_url() {
        assert_inferred(
            "ssh://git@bitbucket.example.com:7999/proj/demo-repo.git",
            "https://bitbucket.example.com",
            "PROJ",
            "demo-repo",
        );
    }

    #[test]
    fn test_infer_scp_like() {
        assert_inferred(
            "git@bitbucket.example.com:proj/demo-repo.git",
            "https://bitbucket.example.com",
            "PROJ",
            "demo-repo",
        );
    }

    // --- Normalization rules (spec section 13.1) ---

    #[test]
    fn test_project_key_uppercase_input_preserved() {
        assert_inferred(
            "https://host/scm/PROJ/demo-repo.git",
            "https://host",
            "PROJ",
            "demo-repo",
        );
    }

    #[test]
    fn test_project_key_lowercase_input_canonicalized() {
        assert_inferred(
            "git@host:mixedCase/repo.git",
            "https://host",
            "MIXEDCASE",
            "repo",
        );
    }

    #[test]
    fn test_repo_with_dashes_and_dots() {
        assert_inferred(
            "https://host/scm/proj/crash.games-v2.git",
            "https://host",
            "PROJ",
            "crash.games-v2",
        );
    }

    #[test]
    fn test_context_path_preserved_multi_segment() {
        assert_inferred(
            "https://host/tools/bitbucket/scm/proj/repo.git",
            "https://host/tools/bitbucket",
            "PROJ",
            "repo",
        );
    }

    #[test]
    fn test_trailing_git_stripped_only_once() {
        // Only the final `.git` is stripped.
        assert_inferred(
            "https://host/scm/proj/repo.git.git",
            "https://host",
            "PROJ",
            "repo.git",
        );
    }

    #[test]
    fn test_repo_without_git_suffix_unchanged() {
        assert_inferred("https://host/scm/proj/repo", "https://host", "PROJ", "repo");
    }

    #[test]
    fn test_malformed_url_returns_err() {
        let err = infer("this is not a remote url").unwrap_err();
        assert!(err.contains("--bitbucket-base-url"));
        assert!(err.contains("--bitbucket-project"));
        assert!(err.contains("--bitbucket-repo"));
    }

    #[test]
    fn test_https_without_scm_segment_returns_err() {
        // GitHub-shaped remote — no `/scm/` marker, so no project can be derived.
        let err = infer("https://github.com/owner/repo.git").unwrap_err();
        assert!(err.contains("scm/<project>/<repo>.git"));
        assert!(err.contains("--bitbucket-project"));
        assert!(err.contains("--bitbucket-repo"));
    }

    #[test]
    fn test_repo_named_only_git_suffix_returns_err() {
        // Stripping `.git` leaves an empty repository component.
        let err = infer("https://host/scm/proj/.git").unwrap_err();
        assert!(err.contains("--bitbucket-repo"));
    }

    #[test]
    fn test_userinfo_stripped_from_parse_outputs() {
        let remote = parse_remote_url("origin", "https://user:secret@host/scm/proj/repo.git");
        assert!(!remote.raw_url.contains("secret"));
        assert!(!remote.raw_url.contains("user"));
        assert_eq!(remote.host.as_deref(), Some("host"));

        let inferred = infer_bitbucket_repository(&remote).unwrap();
        assert_eq!(inferred.base_url, "https://host");
    }

    #[test]
    fn test_userinfo_never_in_error_text() {
        // Malformed shape AND credentials: the error must not echo them.
        let remote = parse_remote_url("origin", "https://user:secret@host/owner/repo.git");
        let err = infer_bitbucket_repository(&remote).unwrap_err();
        assert!(!err.contains("secret"));
        assert!(!err.contains("user:"));
    }

    #[test]
    fn test_scp_like_userinfo_stripped() {
        let remote = parse_remote_url("origin", "deploy@host:proj/repo.git");
        assert_eq!(remote.host.as_deref(), Some("host"));
        assert!(!remote.raw_url.contains("deploy"));
    }

    #[test]
    fn test_ssh_port_not_reused_as_https_port() {
        let inferred = infer("ssh://git@host:7999/proj/repo.git").unwrap();
        assert_eq!(inferred.base_url, "https://host");
        assert!(!inferred.base_url.contains("7999"));
    }

    #[test]
    fn test_ssh_without_port() {
        assert_inferred(
            "ssh://git@host/proj/repo.git",
            "https://host",
            "PROJ",
            "repo",
        );
    }

    #[test]
    fn test_base_url_has_no_trailing_slash() {
        let inferred = infer("https://host/bitbucket/scm/proj/repo.git").unwrap();
        assert!(!inferred.base_url.ends_with('/'));
    }
}
