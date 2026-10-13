//! The `update` subcommand: check for a newer release and install it (docs/specs/SELF_UPDATE.md).
//!
//! Copies installed with the one-line installer have an install receipt and update themselves with
//! axoupdater, which runs the new release's installer. Every other copy (Nix, Cargo, a manual
//! download) is never modified; `update` prints how to upgrade it instead.

use axoupdater::{AxoUpdater, ReleaseSource, ReleaseSourceType, Version};
use std::path::{Path, PathBuf};
use std::time::Duration;

const APP: &str = env!("CARGO_PKG_NAME");
const OWNER: &str = "EmilIvanichkovv";
const REPO: &str = "omni-scripts";
const RELEASES_URL: &str = "https://github.com/EmilIvanichkovv/omni-scripts/releases/latest";
/// Optional token for the GitHub API, for users behind a shared, rate-limited IP address.
const TOKEN_ENV: &str = "LOCAL_GIT_BRANCH_CLEANUP_TUI_GITHUB_TOKEN";

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

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()
        .expect("HTTP client with static settings builds")
}

/// An updater for this app, with its receipt loaded when there is one, and the channel it implies.
fn updater() -> Result<(AxoUpdater, InstallChannel), String> {
    let mut updater = AxoUpdater::new_for(APP);
    updater.set_client(http_client());
    if let Some(token) = std::env::var(TOKEN_ENV).ok().filter(|t| !t.is_empty()) {
        updater.set_github_token(&token);
    }

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

/// Run `update` (or `update --check`) and return the process exit code: 0 when up to date or
/// updated, `EXIT_UPDATE_AVAILABLE` when `--check` finds a newer release, 1 otherwise.
pub async fn run(check_only: bool) -> i32 {
    let (mut updater, channel) = match updater() {
        Ok(found) => found,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let current = current_version();
    let latest = match updater.query_new_version().await {
        Ok(Some(latest)) => latest.clone(),
        Ok(None) => {
            eprintln!("error: no release of {APP} found");
            return 1;
        }
        Err(e) => {
            eprintln!("error: could not check for updates: {e}");
            // GitHub answers 403 "rate limit exceeded" (sometimes 429) to anonymous requests
            // once an IP address has used its hourly quota.
            let message = e.to_string();
            if ["rate limit", "403", "429"]
                .iter()
                .any(|s| message.contains(s))
            {
                eprintln!(
                    "GitHub limits anonymous requests per IP address. Set {TOKEN_ENV} to a GitHub \
                     token to raise the limit, for example: export {TOKEN_ENV}=$(gh auth token)"
                );
            }
            return 1;
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

    // The release found above is reused; the installer prints its own progress.
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

    #[test]
    fn other_copy_instructions_name_the_installer_directory() {
        let text = upgrade_instructions(&InstallChannel::OtherInstallerCopy {
            prefix: PathBuf::from("/home/user/.local/bin"),
        });
        assert!(text.contains("/home/user/.local/bin"));
    }
}
