//! TOML configuration (spec section 3). Unknown keys are an error naming the key and line.

use color_eyre::eyre::{eyre, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub discovery: Discovery,
    pub identity: Identity,
    pub bitbucket: Bitbucket,
    pub refresh: Refresh,
    pub sessions: Sessions,
    pub todos: Todos,
    pub jira: Jira,
    pub summary: Summary,
    pub ui: Ui,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Discovery {
    pub roots: Vec<String>,
    pub max_depth: usize,
    pub exclude: Vec<String>,
}

impl Default for Discovery {
    fn default() -> Self {
        Self {
            roots: vec!["~/work".into(), "~/src".into()],
            max_depth: 4,
            exclude: vec!["**/node_modules".into(), "**/target".into()],
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Identity {
    pub emails: Vec<String>,
    /// Empty = resolve via `X-AUSERNAME`.
    pub bitbucket_user: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Bitbucket {
    /// Empty = derive from origin.
    pub base_url: String,
    pub repo_override: Vec<RepoOverride>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepoOverride {
    pub path: String,
    pub project: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Refresh {
    pub local_interval_secs: u64,
    pub sync_interval_mins: u64,
    pub pr_cache_ttl_mins: u64,
}

impl Default for Refresh {
    fn default() -> Self {
        Self {
            local_interval_secs: 10,
            sync_interval_mins: 5,
            pr_cache_ttl_mins: 60,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Sessions {
    pub enabled: bool,
    pub claude_projects_dir: String,
    pub summarize: bool,
    pub summarize_command: Vec<String>,
    pub summarize_timeout_secs: u64,
    pub min_turns_to_summarize: u32,
}

impl Default for Sessions {
    fn default() -> Self {
        Self {
            enabled: true,
            claude_projects_dir: "~/.claude/projects".into(),
            summarize: true,
            summarize_command: [
                "claude",
                "-p",
                "--model",
                "haiku",
                "--output-format",
                "json",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            summarize_timeout_secs: 120,
            min_turns_to_summarize: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Todos {
    pub file: String,
    pub archive_after_days: u32,
    pub rules: Rules,
}

impl Default for Todos {
    fn default() -> Self {
        Self {
            file: "~/.config/worktrack/todos.md".into(),
            archive_after_days: 14,
            rules: Rules::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleMode {
    Auto,
    Suggest,
    Off,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Rules {
    pub new_unfinished_branch: RuleMode,
    pub pr_merged: RuleMode,
    pub pr_declined: RuleMode,
    pub pr_needs_work: RuleMode,
    pub session_unfinished: RuleMode,
    pub worktree_removed_unfinished: RuleMode,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            new_unfinished_branch: RuleMode::Suggest,
            pr_merged: RuleMode::Auto,
            pr_declined: RuleMode::Suggest,
            pr_needs_work: RuleMode::Suggest,
            session_unfinished: RuleMode::Suggest,
            worktree_removed_unfinished: RuleMode::Suggest,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Jira {
    pub key_pattern: String,
    /// Empty = no links. `{key}` is replaced with the issue key.
    pub browse_url: String,
}

impl Default for Jira {
    fn default() -> Self {
        Self {
            key_pattern: "[A-Z][A-Z0-9]+-[0-9]+".into(),
            browse_url: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Summary {
    pub workdays: Vec<String>,
    pub export_dir: String,
}

impl Default for Summary {
    fn default() -> Self {
        Self {
            workdays: ["Mon", "Tue", "Wed", "Thu", "Fri"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            export_dir: "~/.local/share/worktrack/summaries".into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Ui {
    /// Empty = $VISUAL, then $EDITOR, then vi.
    pub editor: String,
}

/// Commented default written by the first-run screen.
pub const DEFAULT_CONFIG_TOML: &str = r#"# worktrack configuration. See the spec for every key.

[discovery]
roots = ["~/work", "~/src"]          # scanned recursively
max_depth = 4                        # directory depth below each root
exclude = ["**/node_modules", "**/target"]

[identity]
emails = []                          # commit author emails that are "me"
bitbucket_user = ""                  # PR author slug; empty = resolve via API

[bitbucket]
base_url = ""                        # empty = derive from origin

# [[bitbucket.repo_override]]        # optional, per repo path
# path = "~/work/legacy-api"
# project = "LEG"
# repo = "api"

[refresh]
local_interval_secs = 10
sync_interval_mins = 5
pr_cache_ttl_mins = 60

[sessions]
enabled = true
claude_projects_dir = "~/.claude/projects"
summarize = true
summarize_command = ["claude", "-p", "--model", "haiku", "--output-format", "json"]
summarize_timeout_secs = 120
min_turns_to_summarize = 2           # skip trivial sessions

[todos]
file = "~/.config/worktrack/todos.md"
archive_after_days = 14

[todos.rules]                        # auto | suggest | off
new_unfinished_branch = "suggest"
pr_merged = "auto"
pr_declined = "suggest"
pr_needs_work = "suggest"
session_unfinished = "suggest"
worktree_removed_unfinished = "suggest"

[jira]
key_pattern = "[A-Z][A-Z0-9]+-[0-9]+"
browse_url = ""                      # e.g. "https://jira.company.com/browse/{key}"; empty = no links

[summary]
workdays = ["Mon", "Tue", "Wed", "Thu", "Fri"]
export_dir = "~/.local/share/worktrack/summaries"

[ui]
editor = ""                          # empty = $VISUAL, then $EDITOR, then vi
"#;

/// Secrets must not appear in the file (spec 3).
const SECRET_KEYS: &[&str] = &["token", "password", "secret"];

impl Config {
    /// Parse and validate. Error messages name the offending key and line.
    pub fn parse(text: &str) -> Result<Self> {
        let cfg: Config = toml::from_str(text).map_err(|e| {
            let line = e
                .span()
                .map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1)
                .unwrap_or(0);
            eyre!("config error at line {line}: {}", e.message())
        })?;
        for (idx, l) in text.lines().enumerate() {
            let key = l
                .split('=')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if l.contains('=')
                && !l.trim_start().starts_with('#')
                && SECRET_KEYS.iter().any(|s| key.contains(s))
            {
                return Err(eyre!("config error at line {}: secrets must not be stored in the config file (key `{key}`); use BITBUCKET_TOKEN", idx + 1));
            }
        }
        for w in &cfg.summary.workdays {
            if parse_weekday(w).is_none() {
                return Err(eyre!(
                    "config error: summary.workdays has unknown day `{w}`"
                ));
            }
        }
        regex::Regex::new(&cfg.jira.key_pattern)
            .map_err(|e| eyre!("config error: jira.key_pattern is not a valid regex: {e}"))?;
        if cfg.sessions.summarize_command.is_empty() {
            return Err(eyre!(
                "config error: sessions.summarize_command must not be empty"
            ));
        }
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| eyre!("cannot read config {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| eyre!("{}: {e}", path.display()))
    }

    pub fn todo_file(&self) -> PathBuf {
        expand_tilde(&self.todos.file)
    }
    pub fn claude_projects_dir(&self) -> PathBuf {
        expand_tilde(&self.sessions.claude_projects_dir)
    }
    pub fn export_dir(&self) -> PathBuf {
        expand_tilde(&self.summary.export_dir)
    }
    pub fn roots(&self) -> Vec<PathBuf> {
        self.discovery
            .roots
            .iter()
            .map(|r| expand_tilde(r))
            .collect()
    }
    pub fn workdays(&self) -> Vec<chrono::Weekday> {
        self.summary
            .workdays
            .iter()
            .filter_map(|w| parse_weekday(w))
            .collect()
    }
}

pub fn parse_weekday(s: &str) -> Option<chrono::Weekday> {
    use chrono::Weekday::*;
    Some(match s.to_ascii_lowercase().get(..3)? {
        "mon" => Mon,
        "tue" => Tue,
        "wed" => Wed,
        "thu" => Thu,
        "fri" => Fri,
        "sat" => Sat,
        "sun" => Sun,
        _ => return None,
    })
}

pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// Expand a leading `~` / `~/`.
pub fn expand_tilde(p: &str) -> PathBuf {
    if p == "~" {
        home_dir()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// Replace the home prefix with `~` for display.
pub fn tildify(p: &Path) -> String {
    let home = home_dir();
    match p.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(fallback))
}

/// `$XDG_CONFIG_HOME/worktrack` (macOS too: `~/.config`, not `~/Library`).
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("worktrack")
}
/// `$XDG_DATA_HOME/worktrack`.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("worktrack")
}
/// `$XDG_STATE_HOME/worktrack`.
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("worktrack")
}
pub fn default_config_path() -> PathBuf {
    config_dir().join("config.toml")
}
pub fn db_path() -> PathBuf {
    data_dir().join("worktrack.db")
}
/// cwd of the summarizer's `claude -p` run; sessions here are ignored (recursion guard).
pub fn summarizer_dir() -> PathBuf {
    data_dir().join("summarizer")
}
pub fn hook_log_path() -> PathBuf {
    state_dir().join("hook.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_toml_parses_to_default() {
        let cfg = Config::parse(DEFAULT_CONFIG_TOML).unwrap();
        let mut expected = Config::default();
        expected.identity.emails.clear();
        assert_eq!(cfg, expected);
    }

    #[test]
    fn unknown_key_names_key_and_line() {
        let err = Config::parse("[discovery]\nroots = []\nbogus = 1\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("bogus"), "{err}");
        assert!(err.contains("line 3"), "{err}");
    }

    #[test]
    fn secrets_rejected() {
        assert!(Config::parse("[bitbucket]\ntoken = \"x\"\n").is_err());
    }

    #[test]
    fn empty_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }
}
