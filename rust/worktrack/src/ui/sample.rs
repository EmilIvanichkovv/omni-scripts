//! Fixed sample data and a fixed clock for UI snapshots and key-handling tests.

use std::path::PathBuf;

use chrono::{FixedOffset, NaiveDate, TimeZone};

use crate::app::{App, Now, Tab};
use crate::config::Config;
use crate::model::{
    BitbucketIdentity, CiState, Flags, LocalBranchState, PrData, PrStateKind, Repo, Reviewer,
    ReviewerStatus, WorkItem, WorkState, Worktree,
};
use crate::refresh::{AppEvent, PrStatus};
use crate::sessions::{Session, SessionLink, SessionSummary, SummaryStatus};
use crate::summary::{self, Summary};
use crate::todos::rules::{Suggestion, SuggestionPayload, SuggestionStatus};
use crate::todos::{Priority, Section, Todo, TodoStatus};

/// Wed 2026-11-25 14:00 UTC.
pub fn now() -> Now {
    FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2026, 11, 25, 14, 0, 0)
        .unwrap()
}

fn ts(mins_ago: i64) -> i64 {
    now().timestamp() - mins_ago * 60
}

const HOUR: i64 = 60;
const DAY: i64 = 24 * 60;

fn repo(id: &str, project: &str, name: &str, dir: &str) -> Repo {
    Repo {
        id: id.into(),
        common_dir: PathBuf::from(format!("/home/dev/work/{dir}/.git")),
        git_dir_for_commands: PathBuf::from(format!("/home/dev/work/{dir}")),
        origin_url: Some(format!(
            "ssh://git@bitbucket.example.com/{project}/{name}.git"
        )),
        bb: Some(BitbucketIdentity {
            base_url: "https://bitbucket.example.com".into(),
            project: project.into(),
            repo: name.into(),
        }),
        trunk: Some("main".into()),
        skipped_reason: None,
    }
}

fn worktree(repo_id: &str, path: &str, branch: &str, is_main: bool) -> Worktree {
    Worktree {
        path: PathBuf::from(path),
        repo_id: repo_id.into(),
        branch: Some(branch.into()),
        head: Some("a1b2c3d".into()),
        is_main,
        is_bare: false,
        locked: false,
        prunable: false,
        status: None,
    }
}

fn pr(id: u64, state: PrStateKind, title: &str, reviewers: Vec<Reviewer>, tasks: u32) -> PrData {
    PrData {
        id,
        state,
        title: title.into(),
        url: format!("https://bitbucket.example.com/projects/API/repos/x/pull-requests/{id}"),
        author_slug: "dev".into(),
        reviewers,
        open_tasks: tasks,
        comment_count: tasks * 2,
        from_latest_commit: None,
        updated_at: None,
        closed_at: None,
    }
}

fn reviewer(name: &str, status: ReviewerStatus) -> Reviewer {
    Reviewer {
        name: name.into(),
        slug: name.to_lowercase(),
        status,
    }
}

#[allow(clippy::too_many_arguments)]
fn item(
    repo_id: &str,
    repo_name: &str,
    branch: &str,
    state: WorkState,
    flags: Flags,
    worktree: Option<&str>,
    pr: Option<PrData>,
    activity_mins_ago: i64,
    todos: u32,
    sessions: u32,
) -> WorkItem {
    WorkItem {
        repo_id: repo_id.into(),
        repo_name: repo_name.into(),
        branch: branch.into(),
        state,
        local: LocalBranchState {
            branch: branch.into(),
            local_exists: state != WorkState::Abandoned,
            remote_exists: !matches!(state, WorkState::LocalOnly | WorkState::Abandoned),
            worktree: worktree.map(PathBuf::from),
            dirty: flags.dirty,
            ahead: flags.ahead,
            behind: flags.behind,
            behind_trunk: flags.behind_trunk,
            ahead_of_trunk: 3,
            last_commit_at: Some(ts(activity_mins_ago)),
            ..Default::default()
        },
        flags,
        pr,
        abandoned_manual: state == WorkState::Abandoned,
        last_activity: Some(ts(activity_mins_ago)),
        state_since: Some(ts(activity_mins_ago + 38)),
        linked_todos: todos,
        session_count: sessions,
    }
}

fn work_items() -> Vec<WorkItem> {
    let up = ("r_upload", "API/upload-svc");
    let auth = ("r_auth", "API/auth");
    let portal = ("r_portal", "WEB/portal");
    vec![
        item(
            up.0,
            up.1,
            "feature/PROJ-123-retry",
            WorkState::PrNeedsWork,
            Flags {
                dirty: true,
                ahead: 1,
                ci: Some(CiState::Failed),
                open_tasks: 2,
                session_live: true,
                ..Default::default()
            },
            Some("/home/dev/work/upload-svc-wt/retry"),
            Some(pr(
                44,
                PrStateKind::Open,
                "Add retry with backoff to uploads",
                vec![
                    reviewer("ivan", ReviewerStatus::NeedsWork),
                    reviewer("maria", ReviewerStatus::Unapproved),
                ],
                2,
            )),
            2,
            1,
            2,
        ),
        item(
            up.0,
            up.1,
            "feature/PROJ-130-chunking",
            WorkState::PrOpen,
            Flags {
                behind: 3,
                ci: Some(CiState::InProgress),
                no_worktree: true,
                ..Default::default()
            },
            None,
            Some(pr(47, PrStateKind::Open, "Chunked uploads", vec![], 0)),
            HOUR,
            0,
            1,
        ),
        item(
            up.0,
            up.1,
            "spike/s3-multipart",
            WorkState::Pushed,
            Flags {
                no_worktree: true,
                stale: true,
                ..Default::default()
            },
            None,
            None,
            16 * DAY,
            0,
            1,
        ),
        item(
            up.0,
            up.1,
            "chore/bump-deps",
            WorkState::LocalOnly,
            Flags {
                dirty: true,
                ahead: 2,
                ..Default::default()
            },
            Some("/home/dev/work/upload-svc"),
            None,
            3 * HOUR,
            1,
            0,
        ),
        item(
            up.0,
            up.1,
            "main",
            WorkState::Protected,
            Flags::default(),
            Some("/home/dev/work/upload-svc"),
            None,
            HOUR,
            0,
            0,
        ),
        item(
            auth.0,
            auth.1,
            "feature/PROJ-118-sso",
            WorkState::PrApproved,
            Flags {
                ci: Some(CiState::Successful),
                ..Default::default()
            },
            Some("/home/dev/work/auth-wt/sso"),
            Some(pr(
                39,
                PrStateKind::Open,
                "SAML SSO login",
                vec![reviewer("maria", ReviewerStatus::Approved)],
                0,
            )),
            DAY,
            1,
            2,
        ),
        item(
            auth.0,
            auth.1,
            "feature/PROJ-101-legacy",
            WorkState::PrDeclined,
            Flags::default(),
            None,
            Some(pr(
                31,
                PrStateKind::Declined,
                "Legacy login cleanup",
                vec![],
                0,
            )),
            4 * DAY,
            0,
            0,
        ),
        item(
            auth.0,
            auth.1,
            "bugfix/flaky",
            WorkState::Merged,
            Flags {
                cleanup: true,
                ..Default::default()
            },
            Some("/home/dev/work/auth-wt/flaky"),
            Some(pr(
                41,
                PrStateKind::Merged,
                "Fix flaky auth test",
                vec![],
                0,
            )),
            20 * HOUR,
            1,
            0,
        ),
        item(
            portal.0,
            portal.1,
            "feature/PROJ-140-webpack",
            WorkState::Pushed,
            Flags {
                no_worktree: true,
                ..Default::default()
            },
            None,
            None,
            2 * DAY,
            0,
            1,
        ),
        item(
            portal.0,
            portal.1,
            "fix/old-banner",
            WorkState::Abandoned,
            Flags::default(),
            None,
            None,
            9 * DAY,
            0,
            0,
        ),
    ]
}

fn session(id: &str, title: &str, mins_ago: i64, turns: u32, status: SummaryStatus) -> Session {
    Session {
        id: id.into(),
        cwd: None,
        transcript_path: None,
        title: Some(title.into()),
        started_at: Some(ts(mins_ago + 40)),
        last_activity_at: Some(ts(mins_ago)),
        ended_at: Some(ts(mins_ago)),
        end_reason: None,
        user_turns: turns,
        branches: Vec::new(),
        files_touched: Vec::new(),
        explicit_todos: Vec::new(),
        summary_status: status,
        summary: None,
        summarized_at: None,
        links: Vec::new(),
    }
}

fn link(repo_id: &str, branch: &str) -> SessionLink {
    SessionLink {
        repo_id: repo_id.into(),
        branch: branch.into(),
    }
}

fn sessions() -> Vec<Session> {
    let mut live = session(
        "s-live",
        "Add backoff to uploads",
        1,
        14,
        SummaryStatus::None,
    );
    live.ended_at = None;
    live.cwd = Some("/home/dev/work/upload-svc-wt/retry".into());
    live.links = vec![link("r_upload", "feature/PROJ-123-retry")];
    live.branches = vec!["feature/PROJ-123-retry".into()];
    live.files_touched = vec!["src/upload.rs".into(), "tests/retry.rs".into()];
    live.explicit_todos = vec!["integration test for retry".into()];

    let mut sso = session(
        "s-sso",
        "Fix SAML redirect",
        5 * HOUR,
        22,
        SummaryStatus::Done,
    );
    sso.cwd = Some("/home/dev/work/auth-wt/sso".into());
    sso.links = vec![link("r_auth", "feature/PROJ-118-sso")];
    sso.summary = Some(SessionSummary {
        title: "Fix SAML redirect".into(),
        summary: "Fixed the SAML redirect loop; login works against staging IdP.".into(),
        done: vec!["Redirect loop fixed".into()],
        unfinished: vec!["Logout flow untested".into()],
        todos: vec!["Test logout on staging".into()],
        blockers: vec!["Waiting for IdP metadata from IT".into()],
    });
    sso.files_touched = vec!["src/saml.rs".into()];

    let mut nobranch = session(
        "s-nobranch",
        "Explain webpack config",
        DAY,
        3,
        SummaryStatus::Done,
    );
    nobranch.cwd = Some("/tmp/scratch".into());

    let mut failed = session(
        "s-failed",
        "Try multipart API",
        DAY + 2 * HOUR,
        8,
        SummaryStatus::Failed,
    );
    failed.cwd = Some("/home/dev/work/upload-svc".into());
    failed.links = vec![link("r_upload", "spike/s3-multipart")];

    let mut queued = session(
        "s-queued",
        "Chunk size experiments",
        2 * DAY,
        5,
        SummaryStatus::Queued,
    );
    queued.links = vec![link("r_upload", "feature/PROJ-130-chunking")];

    vec![live, sso, nobranch, failed, queued]
}

fn todo(id: &str, title: &str, status: TodoStatus) -> Todo {
    let mut t = Todo::new(title);
    t.id = Some(id.into());
    t.status = status;
    t.created = NaiveDate::from_ymd_opt(2026, 11, 20);
    t.source = Some("manual".into());
    if !status.is_active() {
        t.section = Section::Done;
        t.done = NaiveDate::from_ymd_opt(2026, 11, 24);
    }
    t
}

fn todos() -> Vec<Todo> {
    let mut t1 = todo("t_1", "Finish upload retry logic", TodoStatus::Doing);
    t1.priority = Some(Priority::A);
    t1.link = Some("API/upload-svc:feature/PROJ-123-retry".into());
    t1.notes = "Backoff with jitter.\nCheck the integration test.".into();
    let mut t2 = todo("t_2", "Write migration doc", TodoStatus::Todo);
    t2.priority = Some(Priority::B);
    let t3 = todo("t_3", "Call Ivan about staging access", TodoStatus::Todo);
    let mut t4 = todo("t_4", "Fix flaky auth test", TodoStatus::Done);
    t4.link = Some("API/auth:bugfix/flaky".into());
    let t5 = todo("t_5", "Old banner experiment", TodoStatus::Dropped);
    let mut t6 = todo("t_6", "Review SSO rollout plan", TodoStatus::Todo);
    t6.link = Some("API/auth:feature/PROJ-118-sso".into());
    vec![t1, t2, t3, t4, t5, t6]
}

fn suggestion(id: i64, rule: &str, payload: SuggestionPayload, mins_ago: i64) -> Suggestion {
    Suggestion {
        id,
        rule: rule.into(),
        subject_key: format!("{rule}:{id}"),
        payload,
        status: SuggestionStatus::Pending,
        created_at: ts(mins_ago),
        decided_at: None,
    }
}

fn suggestions() -> Vec<Suggestion> {
    vec![
        suggestion(
            1,
            "new_unfinished_branch",
            SuggestionPayload::AddTodo {
                title: "chore/bump-deps".into(),
                link: Some("API/upload-svc:chore/bump-deps".into()),
                source: "rule:new_unfinished_branch".into(),
            },
            3 * HOUR,
        ),
        suggestion(
            2,
            "pr_declined",
            SuggestionPayload::KeepOrAbandon {
                repo_id: "r_auth".into(),
                branch: "feature/PROJ-101-legacy".into(),
                link: "API/auth:feature/PROJ-101-legacy".into(),
                message: "PR #31 declined: keep or abandon".into(),
            },
            2 * HOUR,
        ),
        suggestion(
            3,
            "session_unfinished",
            SuggestionPayload::AddTodo {
                title: "Integration test for retry fails".into(),
                link: Some("API/upload-svc:feature/PROJ-123-retry".into()),
                source: "session:s-live".into(),
            },
            HOUR,
        ),
        suggestion(
            4,
            "pr_needs_work",
            SuggestionPayload::AddTodo {
                title: "Address review on PR #44".into(),
                link: Some("API/upload-svc:feature/PROJ-123-retry".into()),
                source: "rule:pr_needs_work".into(),
            },
            30,
        ),
    ]
}

/// The summary of the sample data for the window since Tue 2026-11-24 00:00 UTC, with
/// `bugfix/flaky` merged in the window and the declined PR's keep/abandon choice pending.
pub fn summary() -> Summary {
    let since = NaiveDate::from_ymd_opt(2026, 11, 24).unwrap();
    let window = summary::Window {
        since,
        today: now().date_naive(),
        start_ts: now().timestamp() - 38 * 3600,
    };
    let events = vec![summary::StateEvent {
        repo_id: "r_auth".into(),
        branch: "bugfix/flaky".into(),
        from_state: Some(WorkState::PrApproved),
        to_state: WorkState::Merged,
        at: ts(20 * HOUR),
    }];
    let unresolved = vec![summary::UnresolvedChoice {
        rule: "pr_declined".into(),
        message: "PR #31 declined: keep or abandon".into(),
    }];
    summary::build(
        &summary::SummaryInput {
            items: &work_items(),
            todos: &todos(),
            sessions: &sessions(),
            events: &events,
            unresolved: &unresolved,
        },
        &window,
    )
}

/// An App on the Work tab with every kind of data filled in, at the fixed `now()`.
pub fn app() -> App {
    let mut cfg = Config::default();
    cfg.jira.browse_url = "https://jira.example.com/browse/{key}".into();
    let mut app = App::new(&cfg, Tab::Work);
    app.home = Some(PathBuf::from("/home/dev"));
    app.tick(now());
    let mut skipped = repo("r_skip", "X", "y", "scratch");
    skipped.bb = None;
    skipped.skipped_reason = Some("origin is not Bitbucket Data Center".into());
    app.handle_event(AppEvent::ReposUpdated {
        repos: vec![
            repo("r_upload", "API", "upload-svc", "upload-svc"),
            repo("r_auth", "API", "auth", "auth"),
            repo("r_portal", "WEB", "portal", "portal"),
            skipped,
        ],
        worktrees: vec![
            worktree("r_upload", "/home/dev/work/upload-svc", "main", true),
            worktree(
                "r_upload",
                "/home/dev/work/upload-svc-wt/retry",
                "feature/PROJ-123-retry",
                false,
            ),
            worktree("r_auth", "/home/dev/work/auth", "main", true),
            worktree(
                "r_auth",
                "/home/dev/work/auth-wt/sso",
                "feature/PROJ-118-sso",
                false,
            ),
            worktree(
                "r_auth",
                "/home/dev/work/auth-wt/flaky",
                "bugfix/flaky",
                false,
            ),
        ],
    });
    app.handle_event(AppEvent::WorkItemsUpdated(work_items()));
    app.handle_event(AppEvent::PrUpdated {
        status: PrStatus::Ok,
        synced_at: Some(ts(2)),
    });
    app.handle_event(AppEvent::SessionsUpdated(sessions()));
    app.handle_event(AppEvent::TodosReloaded(Ok(todos())));
    app.handle_event(AppEvent::SuggestionsUpdated(suggestions()));
    let summary = summary();
    app.handle_event(AppEvent::SummaryUpdated {
        since: NaiveDate::from_ymd_opt(2026, 11, 24).unwrap(),
        markdown: summary::to_markdown(&summary),
        summary,
    });
    app.handle_event(AppEvent::Diagnostics(vec![
        "Bitbucket:".into(),
        "  token  set (BITBUCKET_TOKEN)".into(),
        "  user   dev".into(),
        "Hook:".into(),
        "  SessionEnd hook installed".into(),
        "Paths:".into(),
        "  config ~/.config/worktrack/config.toml".into(),
        "  db     ~/.local/share/worktrack/worktrack.db".into(),
    ]));
    app.handle_event(AppEvent::Progress {
        task: "syncing".into(),
        done: 3,
        total: 7,
    });
    app.status = None;
    app.cursor = [0; 5];
    app
}
