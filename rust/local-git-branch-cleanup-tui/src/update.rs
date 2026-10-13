//! The `update` subcommand: check for a newer release and install it (docs/specs/SELF_UPDATE.md).
//!
//! Copies installed with the one-line installer have an install receipt and update themselves with
//! axoupdater, which runs the new release's installer. Every other copy (Nix, Cargo, a manual
//! download) is never modified; `update` prints how to upgrade it instead.
//!
//! The newest version comes from the repository's releases feed, which does not count against the
//! GitHub API rate limit; the API is the fallback (SELF_UPDATE.md, section 12).

use axoupdater::{AxoUpdater, ReleaseSource, ReleaseSourceType, UpdateRequest, Version};
use std::path::{Path, PathBuf};
use std::time::Duration;

const APP: &str = env!("CARGO_PKG_NAME");
const OWNER: &str = "EmilIvanichkovv";
const REPO: &str = "omni-scripts";
const RELEASES_URL: &str = "https://github.com/EmilIvanichkovv/omni-scripts/releases/latest";
/// Optional token for the GitHub API, for users behind a shared, rate-limited IP address.
const TOKEN_ENV: &str = "LOCAL_GIT_BRANCH_CLEANUP_TUI_GITHUB_TOKEN";
/// axoupdater's override of the GitHub host. The feed follows it; tests use it to reach a local
/// mock.
const GHE_BASE_URL_ENV: &str = "LOCAL_GIT_BRANCH_CLEANUP_TUI_INSTALLER_GHE_BASE_URL";

/// Exit code of `update --check` when a newer release exists.
pub const EXIT_UPDATE_AVAILABLE: i32 = 10;

/// How the running copy was installed, which decides whether it can update itself.
#[derive(Debug, PartialEq, Eq)]
pub enum InstallChannel {
    /// The one-line installer; its receipt points at this binary's directory.
    Installer,
    /// An installer copy exists, but in another directory than this binary.
    OtherInstallerCopy {
        prefix: PathBuf,
    },
    Nix,
    Cargo,
    /// A manual download or anything else.
    Unknown,
}

/// Classify the running binary. `receipt` is the installer receipt's prefix and whether it is
/// this binary's directory; `cargo_bin` is Cargo's install directory.
fn classify(
    exe: &Path,
    receipt: Option<(&Path, bool)>,
    cargo_bin: Option<&Path>,
) -> InstallChannel {
    match receipt {
        Some((_, true)) => InstallChannel::Installer,
        _ if exe.starts_with("/nix/store") => InstallChannel::Nix,
        _ if cargo_bin.is_some_and(|bin| exe.starts_with(bin)) => InstallChannel::Cargo,
        Some((prefix, false)) => InstallChannel::OtherInstallerCopy {
            prefix: prefix.to_path_buf(),
        },
        None => InstallChannel::Unknown,
    }
}

fn cargo_bin_dir() -> Option<PathBuf> {
    std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")))
        .map(|cargo_home| cargo_home.join("bin"))
}

fn current_version() -> Version {
    env!("CARGO_PKG_VERSION")
        .parse()
        .expect("CARGO_PKG_VERSION is valid semver")
}

/// A client whose requests give up after `timeout`.
fn http_client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .connect_timeout(timeout.min(Duration::from_secs(10)))
        .timeout(timeout)
        .build()
        .expect("HTTP client with static settings builds")
}

/// The releases feed: GitHub's web feed of the repository's releases, newest first.
fn feed_url() -> String {
    match std::env::var(GHE_BASE_URL_ENV) {
        Ok(base) => format!(
            "{}/{OWNER}/{REPO}/releases.atom",
            base.trim_end_matches('/')
        ),
        Err(_) => format!("https://github.com/{OWNER}/{REPO}/releases.atom"),
    }
}

/// The newest stable release of this app in the feed, as its tag and version. Each entry's `<id>`
/// ends in the release's tag; the release notes in the feed are escaped HTML, so they cannot
/// contain an `<id>` element.
fn newest_release_in_feed(feed: &str) -> Option<(String, Version)> {
    let prefix = format!("{APP}-v");
    feed.split("<id>")
        .skip(1)
        .filter_map(|rest| {
            let id = &rest[..rest.find("</id>")?];
            let tag = &id[id.rfind('/')? + 1..];
            let version: Version = tag.strip_prefix(&prefix)?.parse().ok()?;
            version.pre.is_empty().then(|| (tag.to_string(), version))
        })
        .max_by(|a, b| a.1.cmp(&b.1))
}

/// `None` on any failure, and when the feed lists no release of this app (it holds only the
/// newest releases of the whole repository).
async fn newest_release_from_feed(client: &reqwest::Client) -> Option<(String, Version)> {
    let response = client
        .get(feed_url())
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    newest_release_in_feed(&response.text().await.ok()?)
}

/// Give `updater` the token from our variable, if set. Returns whether it got one.
fn authenticate(updater: &mut AxoUpdater) -> bool {
    match std::env::var(TOKEN_ENV).ok().filter(|t| !t.is_empty()) {
        Some(token) => {
            updater.set_github_token(&token);
            true
        }
        None => false,
    }
}

/// GitHub answers 403 "rate limit exceeded" (sometimes 429) to anonymous requests once an IP
/// address has used its hourly quota.
fn looks_rate_limited(message: &str) -> bool {
    ["rate limit", "403", "429"]
        .iter()
        .any(|s| message.contains(s))
}

fn print_rate_limit_hint() {
    eprintln!(
        "GitHub limits anonymous requests per IP address. Set {TOKEN_ENV} to a GitHub token to \
         raise the limit, for example: export {TOKEN_ENV}=$(gh auth token)"
    );
}

/// An updater for this app, with its receipt loaded when there is one, and the channel it implies.
fn updater(client: reqwest::Client) -> Result<(AxoUpdater, InstallChannel), String> {
    let mut updater = AxoUpdater::new_for(APP);
    updater.set_client(client);

    let exe = std::env::current_exe()
        .and_then(|exe| exe.canonicalize())
        .map_err(|e| format!("cannot locate the running binary: {e}"))?;

    let receipt = match updater.load_receipt() {
        Ok(loaded) => {
            let matches = loaded
                .check_receipt_is_for_this_executable()
                .map_err(|e| format!("cannot read the install receipt: {e}"))?;
            let prefix = loaded
                .install_prefix_root()
                .map_err(|e| format!("cannot read the install receipt: {e}"))?;
            Some((prefix.into_std_path_buf(), matches))
        }
        // No receipt: not installed with the installer. Query the releases directly.
        Err(_) => {
            updater.set_release_source(ReleaseSource {
                release_type: ReleaseSourceType::GitHub,
                owner: OWNER.to_string(),
                name: REPO.to_string(),
                app_name: APP.to_string(),
            });
            None
        }
    };
    // The receipt's version can be stale; the running binary knows its own.
    updater
        .set_current_version(current_version())
        .map_err(|e| e.to_string())?;

    let channel = classify(
        &exe,
        receipt
            .as_ref()
            .map(|(prefix, matches)| (prefix.as_path(), *matches)),
        cargo_bin_dir().as_deref(),
    );
    Ok((updater, channel))
}

fn installer_command() -> String {
    if cfg!(windows) {
        format!(
            "powershell -ExecutionPolicy Bypass -c \"irm https://github.com/{OWNER}/{REPO}/releases/latest/download/{APP}-installer.ps1 | iex\""
        )
    } else {
        format!(
            "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/{OWNER}/{REPO}/releases/latest/download/{APP}-installer.sh | sh"
        )
    }
}

/// How to upgrade a copy that cannot update itself.
fn upgrade_instructions(channel: &InstallChannel) -> String {
    match channel {
        InstallChannel::Installer => format!("Run `{APP} update` to install it."),
        InstallChannel::OtherInstallerCopy { prefix } => format!(
            "This copy was not installed by the installer, so it cannot update itself. The \
             installer copy is in {}: run `{APP} update` from there, or remove one of the two.",
            prefix.display()
        ),
        InstallChannel::Nix => "This copy was installed with Nix: upgrade it with \
             `nix profile upgrade`, or update the flake input that provides it."
            .to_string(),
        InstallChannel::Cargo => format!(
            "This copy was installed with Cargo: upgrade it with\n  \
             cargo install --git https://github.com/{OWNER}/{REPO} {APP} --force"
        ),
        InstallChannel::Unknown => format!(
            "This copy was not installed with the one-line installer, so it cannot update \
             itself. Download the new version from {RELEASES_URL}, or install it with:\n  {}",
            installer_command()
        ),
    }
}

/// The newest release's version, or `None` on any error. For the background check
/// (`update_notice`), which stays silent on failure.
pub async fn latest_version(timeout: Duration) -> Option<Version> {
    let client = http_client(timeout);
    if let Some((_, version)) = newest_release_from_feed(&client).await {
        return Some(version);
    }
    let (mut updater, _) = updater(client).ok()?;
    authenticate(&mut updater);
    updater.query_new_version().await.ok().flatten().cloned()
}

/// Run `update` (or `update --check`) and return the process exit code: 0 when up to date or
/// updated, `EXIT_UPDATE_AVAILABLE` when `--check` finds a newer release, 1 otherwise.
pub async fn run(check_only: bool) -> i32 {
    let client = http_client(Duration::from_secs(120));
    let (mut updater, channel) = match updater(client.clone()) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let current = current_version();
    let from_feed = newest_release_from_feed(&client).await;
    // Without the feed, ask the API, which needs a token behind a busy IP address.
    let mut authenticated = false;
    let latest = match &from_feed {
        Some((_, latest)) => latest.clone(),
        None => {
            authenticated = authenticate(&mut updater);
            match updater.query_new_version().await {
                Ok(Some(latest)) => latest.clone(),
                Ok(None) => {
                    eprintln!("error: no release of {APP} found");
                    return 1;
                }
                Err(e) => {
                    eprintln!("error: could not check for updates: {e}");
                    if looks_rate_limited(&e.to_string()) {
                        print_rate_limit_hint();
                    }
                    return 1;
                }
            }
        }
    };

    if latest <= current {
        println!("{APP} {current} is the latest version.");
        return 0;
    }

    println!("Update available: {current} -> {latest}");
    if check_only {
        println!("{}", upgrade_instructions(&channel));
        return EXIT_UPDATE_AVAILABLE;
    }
    if channel != InstallChannel::Installer {
        eprintln!("{}", upgrade_instructions(&channel));
        return 1;
    }

    // From the feed's tag, axoupdater makes one API request to find the installer. Otherwise it
    // reuses the release found above. The installer prints its own progress.
    if let Some((tag, _)) = from_feed {
        authenticated = authenticate(&mut updater);
        updater.configure_version_specifier(UpdateRequest::SpecificTag(tag));
    }
    match updater.run().await {
        Ok(Some(result)) => {
            println!("Updated {APP} to {}.", result.new_version);
            0
        }
        Ok(None) => {
            println!("{APP} {current} is the latest version.");
            0
        }
        Err(e) => {
            eprintln!("error: the update failed: {e}");
            // axoupdater reports any failed tag lookup, a rate-limited one too, as "not found".
            let message = e.to_string();
            if !authenticated && (looks_rate_limited(&message) || message.contains("was not found"))
            {
                print_rate_limit_hint();
            }
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/home/user/.local/bin/local-git-branch-cleanup-tui";

    #[test]
    fn receipt_for_this_binary_is_the_installer_channel() {
        let prefix = Path::new("/home/user/.local/bin");
        assert_eq!(
            classify(Path::new(EXE), Some((prefix, true)), None),
            InstallChannel::Installer
        );
    }

    #[test]
    fn receipt_for_another_directory_is_another_installer_copy() {
        let prefix = Path::new("/home/user/.local/bin");
        let exe = Path::new("/opt/tools/local-git-branch-cleanup-tui");
        assert_eq!(
            classify(exe, Some((prefix, false)), None),
            InstallChannel::OtherInstallerCopy {
                prefix: prefix.to_path_buf()
            }
        );
    }

    #[test]
    fn nix_store_binary_is_nix_even_with_an_installer_copy() {
        let exe = Path::new("/nix/store/abc-local-git-branch-cleanup-tui/bin/x");
        let prefix = Path::new("/home/user/.local/bin");
        assert_eq!(classify(exe, None, None), InstallChannel::Nix);
        assert_eq!(
            classify(exe, Some((prefix, false)), None),
            InstallChannel::Nix
        );
    }

    #[test]
    fn cargo_bin_binary_is_cargo() {
        let cargo_bin = Path::new("/home/user/.cargo/bin");
        let exe = cargo_bin.join("local-git-branch-cleanup-tui");
        assert_eq!(classify(&exe, None, Some(cargo_bin)), InstallChannel::Cargo);
    }

    #[test]
    fn anything_else_is_unknown() {
        let cargo_bin = Path::new("/home/user/.cargo/bin");
        assert_eq!(
            classify(Path::new(EXE), None, Some(cargo_bin)),
            InstallChannel::Unknown
        );
    }

    #[test]
    fn unknown_channel_instructions_include_the_installer_command() {
        let text = upgrade_instructions(&InstallChannel::Unknown);
        assert!(text.contains(RELEASES_URL));
        assert!(text.contains(&installer_command()));
    }

    const FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>tag:github.com,2008:https://github.com/EmilIvanichkovv/omni-scripts/releases</id>
  <entry>
    <id>tag:github.com,2008:Repository/1/other-tool-v9.0.0</id>
    <link rel="alternate" type="text/html" href="https://github.com/o/r/releases/tag/other-tool-v9.0.0"/>
  </entry>
  <entry>
    <id>tag:github.com,2008:Repository/1/local-git-branch-cleanup-tui-v0.5.0-rc.1</id>
  </entry>
  <entry>
    <id>tag:github.com,2008:Repository/1/local-git-branch-cleanup-tui-v0.4.0</id>
    <content type="html">&lt;a href=&quot;https://github.com/o/r/releases/tag/local-git-branch-cleanup-tui-v8.0.0&quot;&gt;</content>
  </entry>
  <entry>
    <id>tag:github.com,2008:Repository/1/local-git-branch-cleanup-tui-v0.10.0</id>
  </entry>
  <entry>
    <id>tag:github.com,2008:Repository/1/local-git-branch-cleanup-tui-v0.3.0</id>
  </entry>
</feed>"#;

    #[test]
    fn feed_gives_the_highest_stable_release_of_this_app() {
        let (tag, version) = newest_release_in_feed(FEED).unwrap();
        // Not other-tool's 9.0.0, the 0.5.0 release candidate, or a link in the release notes.
        assert_eq!(tag, "local-git-branch-cleanup-tui-v0.10.0");
        assert_eq!(version, Version::new(0, 10, 0));
    }

    #[test]
    fn feed_without_this_app_or_garbage_gives_nothing() {
        let other = "<feed><id>x/releases</id><entry><id>t/other-tool-v1.0.0</id></entry></feed>";
        assert_eq!(newest_release_in_feed(other), None);
        assert_eq!(newest_release_in_feed("<html>rate limited</html>"), None);
        assert_eq!(newest_release_in_feed("<id>unterminated"), None);
    }

    #[test]
    fn other_copy_instructions_name_the_installer_directory() {
        let text = upgrade_instructions(&InstallChannel::OtherInstallerCopy {
            prefix: PathBuf::from("/home/user/.local/bin"),
        });
        assert!(text.contains("/home/user/.local/bin"));
    }
}
