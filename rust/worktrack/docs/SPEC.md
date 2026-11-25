# worktrack — Implementation Specification

Nov 25, 2026

## 1. Overview

`worktrack` (working name) is a Rust + ratatui TUI that shows, across all configured repositories
with a Bitbucket Data Center remote, which branches and worktrees hold unfinished work, what Claude
Code did in each, and an editable todo list that updates itself from that state. It produces a daily
summary on demand.

Key words MUST, SHOULD, MAY are used as in RFC 2119.

### 1.1 Goals

- One view of all unfinished work across repos and worktrees, classified by a single state machine
  (section 4).
- PR state per branch from Bitbucket Data Center.
- Claude Code sessions captured, summarized, and linked to the branch they ran on.
- A todo list stored as a hand-editable Markdown file, editable in the TUI, and updated by rules
  (section 7).
- A daily summary covering Done / In progress / Todo / Blockers, exportable as Markdown.

### 1.2 Non-goals (v1)

- claude.ai chats. There is no API for them.
- GitHub, Bitbucket Cloud, Jira API, or any other provider.
- Creating, approving, or merging PRs.
- Opening terminals or Zellij panes. Resume is copy-to-clipboard only.
- Windows support. Target is macOS and Linux.

### 1.3 Definition of done

A branch's work is **done** when its PR is merged. Nothing else marks work done; a deleted branch
without a merged PR is **abandoned**, not done.

## 2. Inherited decisions and crate layout

The app is a new crate `rust/worktrack` in the `omni-scripts` Rust workspace (added to `members` in
`rust/Cargo.toml`, dependencies from `[workspace.dependencies]`) and reuses the approach of
`local-git-branch-cleanup-tui` (LGBC). Code from LGBC is copied, not shared (section 2.2).

| Area                  | Decision taken from LGBC                                                                                                                                           |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Bitbucket             | Data Center REST, Bearer token from `BITBUCKET_TOKEN` only, PR lookup by source branch with `state=ALL`, state mapping OPEN/MERGED/DECLINED, unknown state = error |
| Remote identity       | Base URL, project key, repo slug derived from `origin`; per-repo overrides in config                                                                               |
| HTTP                  | reqwest + rustls with native root certificates; `User-Agent: worktrack/<version>`                                                                                  |
| Cache                 | SQLite via rusqlite; PR TTL 1 h; errors never cached                                                                                                               |
| Concurrency           | tokio, bounded concurrent PR fetches                                                                                                                               |
| Refresh               | local reload every 10 s, full sync every 5 min, `r` / `R` manual                                                                                                   |
| Layout                | responsive breakpoints, scrollbars, `v` cycles layout, no panic on resize                                                                                          |
| Input                 | vim + arrows, mouse wheel only (SGR 1000/1006)                                                                                                                     |
| Distribution          | cargo-dist, self-update subcommand, update notice                                                                                                                  |
| Branch classification | concepts reused (merged, gone, local, protected); LGBC's classifier code is not reused, worktrack derives state per repo (section 4.3)                             |

### 2.1 Module layout

```
worktrack/src/
  main.rs          clap, mode selection, terminal lifecycle, event loop
  config.rs        load + validate TOML config
  discovery.rs     scan roots, find repos and worktrees
  model.rs         WorkItem, WorkState, derivation (section 4)
  bitbucket.rs     worktrack-specific Bitbucket calls on top of lgbc/bitbucket.rs
  lgbc/            files copied from LGBC (section 2.2) + ORIGIN.md
  sessions/
    mod.rs         Session model, linking
    ingest.rs      transcript JSONL parser
    summarize.rs   claude -p invocation, JSON schema validation
    hook.rs        `worktrack hook session-end`
  todos/
    mod.rs         Todo model
    markdown.rs    parse + serialize todo file (round-trip safe)
    rules.rs       auto-update rules engine (section 7.5)
    watch.rs       file watcher for external edits
  summary.rs       daily summary builder + Markdown export
  store.rs         SQLite schema, migrations, queries
  refresh.rs       scheduler, background tasks, channels to UI
  app.rs           App state, actions
  ui/              one file per view + widgets/
```

Rule: `ui/` MUST NOT perform IO. All IO runs in `refresh.rs` tasks and reaches `App` through an mpsc
channel of `AppEvent`.

### 2.2 Code reused by copy

Shared code is copied from LGBC into `worktrack/src/lgbc/`; nothing moves to `omni-lib` and LGBC is
not modified. Every copy records its source commit so a later extraction or sync is a mechanical
diff.

Rules:

1. worktrack PRs MUST NOT change any file under `rust/local-git-branch-cleanup-tui/` or
   `rust/omni-lib/`.
2. Each copied file starts with
   `// Copied from rust/local-git-branch-cleanup-tui/src/<path> at <commit sha>. Local changes: see src/lgbc/ORIGIN.md.`
3. `src/lgbc/ORIGIN.md` has one row per copied file: source path, source commit, list of local
   changes.
4. Inline unit tests in a copied file come with it and MUST pass after the local changes.
5. Copy only what is listed below; unused code is not copied.

| Source in LGBC                          | Copy in worktrack                          | Local changes required                                                                                                                                                                                        |
| --------------------------------------- | ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/remote.rs`                         | `src/lgbc/remote.rs`                       | `read_origin()` → `read_origin_in(repo: &Path)`                                                                                                                                                               |
| `src/pr/mod.rs`                         | `src/lgbc/pr.rs`                           | drop the GitHub variant of `PrProviderKind`                                                                                                                                                                   |
| `src/pr/bitbucket.rs`                   | `src/lgbc/bitbucket.rs`                    | `User-Agent` → `worktrack/<version>`; `resolve_repository` takes the repo path; new methods in milestone 2                                                                                                    |
| `src/tls.rs`, `src/clipboard.rs`        | `src/lgbc/tls.rs`, `src/lgbc/clipboard.rs` | none                                                                                                                                                                                                          |
| `src/responsive.rs`                     | `src/lgbc/responsive.rs`                   | keep `LayoutMode`, `Arrangement`, `arrange`, `split_content`, `is_too_small`, `is_compact`, `flow_lines`, `str_width`, `truncate_end`, `truncate_middle`, `Scroll`; drop `branch_columns`, `worktree_columns` |
| `src/git.rs` (helpers only)             | `src/lgbc/git.rs`                          | copy `fetch_remote_in`, `refs_fingerprint_in`, `open_url_in_browser`, `is_protected_branch`, `get_default_branch` as `get_default_branch_in(repo, override)`                                                  |
| `src/update.rs`, `src/update_notice.rs` | `src/lgbc/update*.rs` in milestone 7       | app name, env var prefix (`WORKTRACK_`), and cache path changed                                                                                                                                               |

Not copied: LGBC's branch classification and `cache.rs`; worktrack derives state itself (section
4.3) and has its own schema (section 10.1).

Known cost: a fix to the Bitbucket client or remote parser must be applied in both apps until the
code is extracted. ORIGIN.md makes that a diff against the recorded commit.

## 3. Configuration

One TOML file at `$XDG_CONFIG_HOME/worktrack/config.toml` (macOS: `~/.config/worktrack/config.toml`,
not `~/Library`). Missing file → the app starts a first-run screen that writes a commented default.
Unknown keys MUST be an error naming the key and line. Secrets MUST NOT appear in the file.

```toml
[discovery]
roots = ["~/work", "~/src"]        # scanned recursively
max_depth = 4                        # directory depth below each root
exclude = ["**/node_modules", "**/target"]

[identity]
emails = ["emil@company.com"]        # commit author emails that are "me"
bitbucket_user = "eivanichkov"       # PR author slug; empty = resolve via API

[bitbucket]
base_url = ""                        # empty = derive from origin

[[bitbucket.repo_override]]          # optional, per repo path
path = "~/work/legacy-api"
project = "LEG"
repo = "api"

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

[todos.rules]                        # auto | suggest | off (section 7.5)
new_unfinished_branch = "suggest"
pr_merged = "auto"
pr_declined = "suggest"
pr_needs_work = "suggest"
session_unfinished = "suggest"
worktree_removed_unfinished = "suggest"

[jira]
key_pattern = "[A-Z][A-Z0-9]+-[0-9]+"
browse_url = "https://jira.company.com/browse/{key}"   # empty = no links

[summary]
workdays = ["Mon", "Tue", "Wed", "Thu", "Fri"]
export_dir = "~/.local/share/worktrack/summaries"

[ui]
editor = ""                          # empty = $VISUAL, then $EDITOR, then vi
```

Environment: `BITBUCKET_TOKEN` (required for PR data; without it the app runs with PR columns shown
as `—` and a header warning).

## 4. Discovery and work model

The unit of tracking is a **WorkItem**, keyed by `(repo_id, branch_name)`. Worktrees, PRs, sessions,
and todos attach to a WorkItem.

### 4.1 Discovery

1. Walk each `discovery.roots` entry to `max_depth`, skipping `exclude` globs and not descending
   into a found repo's working tree except to find nested worktrees.
2. A directory is a candidate if it has a `.git` directory, a `.git` file (linked worktree), or is a
   bare repo (`HEAD`, `objects/`, `refs/`).
3. Group candidates by `git rev-parse --git-common-dir` (canonicalized). One group = one **Repo**.
   `repo_id` = SHA-256 of the canonical common dir path, first 16 hex chars.
4. For each Repo, run `git worktree list --porcelain` as the authoritative worktree list. Worktrees
   outside the roots are still included.
5. Parse `origin` with the copied remote parser (section 2.2). A repo whose `origin` does not
   resolve to a Bitbucket DC identity (or has no `origin`) is skipped and listed under Diagnostics
   (section 9.7).
6. Trunk = `origin/HEAD` target; fallback `main`, then `master`. Protected = trunk, `master`,
   `main`, `develop`, `release/*`.

Discovery runs at startup and on full sync. The local reload (every 10 s) only re-reads known repos.

### 4.2 Ownership

Only my branches are tracked. A branch is mine if any holds, checked in order:

1. It is checked out in a local worktree.
2. Its PR author slug equals `identity.bitbucket_user`.
3. A commit in `trunk..branch` has an author email in `identity.emails`.

Remote-only branches (`refs/remotes/origin/*` with no local branch) are included only by rules 2
or 3.

### 4.3 Work states

Exactly one state per WorkItem, derived on every reload. First matching row wins.

| #   | State         | Condition                                                                                                                                             | Unfinished                       |
| --- | ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------- |
| 1   | `Protected`   | branch is protected                                                                                                                                   | excluded                         |
| 2   | `Merged`      | latest PR state MERGED                                                                                                                                | no (done)                        |
| 3   | `MergedNoPr`  | no PR, branch tip is an ancestor of trunk, and the branch reflog shows ≥1 commit made on it (a fresh branch with no own commits falls to row 9 or 10) | no (done, labelled `merged·git`) |
| 4   | `Abandoned`   | user marked abandoned, or branch gone locally and remotely without a merged PR                                                                        | no                               |
| 5   | `PrDeclined`  | latest PR state DECLINED                                                                                                                              | yes                              |
| 6   | `PrNeedsWork` | PR OPEN and any reviewer status NEEDS_WORK                                                                                                            | yes                              |
| 7   | `PrApproved`  | PR OPEN and ≥1 APPROVED and none NEEDS_WORK                                                                                                           | yes                              |
| 8   | `PrOpen`      | PR OPEN                                                                                                                                               | yes                              |
| 9   | `Pushed`      | branch exists on origin, no PR                                                                                                                        | yes                              |
| 10  | `LocalOnly`   | branch exists only locally                                                                                                                            | yes                              |

"Latest PR" = the newest PR whose source branch is this branch. A branch reused after a merge (new
commits after the merged PR's head) MUST be treated as `Pushed` or `LocalOnly`, not `Merged`:
compare the PR's `fromRef.latestCommit` with the branch tip and check `merged_head..tip` is
non-empty.

Without a token, rows 2 and 5–8 cannot be evaluated; items fall to rows 3, 9, 10 and the UI marks PR
data as unavailable.

### 4.4 Flags

Flags are orthogonal to state and shown as icons.

| Flag             | Condition                                                               |
| ---------------- | ----------------------------------------------------------------------- |
| `dirty`          | worktree has staged, unstaged, or untracked changes                     |
| `ahead N`        | local commits not on upstream                                           |
| `behind N`       | upstream commits not local                                              |
| `behind-trunk N` | trunk commits not in branch                                             |
| `no-worktree`    | unfinished branch with no worktree                                      |
| `cleanup`        | state is done/abandoned and a worktree or local branch still exists     |
| `stale`          | unfinished and no commit or session for 14 days                         |
| `ci:<state>`     | latest build status on the tip commit: SUCCESSFUL / FAILED / INPROGRESS |
| `comments N`     | unresolved PR comment count (open PRs only)                             |
| `session-live`   | a Claude session on this branch has activity in the last 5 min          |

### 4.5 State history

Every state transition is written to the `state_events` table with a timestamp. The summary
(section 8) and the rules engine (section 7.5) read transitions from there, never by diffing live
state.

## 5. Bitbucket Data Center integration

One dashboard request per sync covers all my open PRs across every repo; per-branch lookups are
needed only for branches with no open PR. Transport, auth, error mapping, and state mapping are
reused from LGBC (`BITBUCKET_SUPPORT.md`).

### 5.1 Requests

| Purpose               | Request                                                                                                                           | When                                                           |
| --------------------- | --------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------- |
| My username           | `X-AUSERNAME` response header of any authenticated request                                                                        | first request per run, if `identity.bitbucket_user` is empty   |
| All my open PRs       | `GET /rest/api/1.0/dashboard/pull-requests?role=AUTHOR&state=OPEN&limit=100` (paged)                                              | every full sync                                                |
| PR for one branch     | `GET /rest/api/1.0/projects/{p}/repos/{r}/pull-requests?at=refs/heads/{branch}&direction=OUTGOING&state=ALL&order=NEWEST&limit=1` | branches not matched by the dashboard result, subject to cache |
| Build status          | `GET /rest/build-status/1.0/commits/{sha}`                                                                                        | tip commit of unfinished branches, every full sync             |
| Open tasks / comments | PR `properties.openTaskCount` and `properties.commentCount` from the PR payload                                                   | with the PR                                                    |

Reviewer status comes from `reviewers[].status` in the PR payload (`APPROVED`, `NEEDS_WORK`,
`UNAPPROVED`). The `comments N` flag shows `openTaskCount`. Unresolved comment threads (not tasks)
MAY be added later via the activities endpoint once verified against your instance's version.

Dashboard results are matched to WorkItems by `toRef.repository.project.key` +
`toRef.repository.slug` + `fromRef.displayId`. PRs from forks are ignored.

Build status aggregation: any `FAILED` → FAILED; else any `INPROGRESS` → INPROGRESS; else all
`SUCCESSFUL` → SUCCESSFUL; none → no flag.

### 5.2 Caching

| PR state   | Cache rule                                                                                  |
| ---------- | ------------------------------------------------------------------------------------------- |
| MERGED     | cached until the branch tip moves past the PR's merged head (section 4.3 reuse rule)        |
| DECLINED   | TTL `pr_cache_ttl_mins` (can be reopened)                                                   |
| OPEN       | not served from cache during a full sync; cache is used only at startup to render instantly |
| none found | TTL `pr_cache_ttl_mins`                                                                     |

Errors are never cached. Concurrency: max 8 in-flight requests per Bitbucket host. On 429, honor
`Retry-After` and pause that host's queue.

### 5.3 Failure behavior

- No token: PR features off, header shows `PR: no token`.
- 401/403: header shows the error and the affected repos; local data keeps working.
- Host unreachable: last cached PR data shown with an age marker (`cached 2h`).

## 6. Claude Code sessions

Sessions are captured from Claude Code transcripts by a `SessionEnd` hook and by a directory scan,
linked to WorkItems by cwd and branch, and summarized once with `claude -p`. Only metadata and
summaries are stored; transcripts are never copied.

### 6.1 Capture

**Hook (immediate).** `worktrack hook session-end` reads the hook JSON from stdin (`session_id`,
`transcript_path`, `cwd`, `reason`), inserts a row into `session_queue`, and exits. It MUST exit 0
in under 200 ms in all cases, including DB lock or parse errors (errors go to
`~/.local/state/worktrack/hook.log`). It MUST NOT parse the transcript or call `claude`.

`worktrack hook install` prints the settings snippet below. `worktrack hook install --apply` merges
it into `~/.claude/settings.json` after writing `settings.json.bak`, and refuses if an identical
hook already exists.

```json
{
  "hooks": {
    "SessionEnd": [{ "hooks": [{ "type": "command", "command": "worktrack hook session-end" }] }]
  }
}
```

**Scan (backfill).** On every local reload, list `*.jsonl` under `sessions.claude_projects_dir`,
compare `(path, size, mtime)` with `session_files`, and parse new bytes only (stored byte offset).
This catches sessions when the hook is not installed and live sessions in progress.

A session is **ended** when its `SessionEnd` was received, or its file has not changed for 30 min.

### 6.2 Transcript parsing

The parser MUST be tolerant: one JSON object per line, unknown `type` values and fields ignored,
malformed lines skipped and counted. Fields used per line when present: `sessionId`, `timestamp`,
`cwd`, `gitBranch`, `type`, `message.content` (text blocks and `tool_use` blocks). The format is
undocumented and changes between Claude Code versions; tests MUST use real transcript fixtures
(section 12).

Extracted metadata:

| Field                            | Source                                                                                                           |
| -------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| `started_at`, `last_activity_at` | first / last `timestamp`                                                                                         |
| `cwd`                            | first `cwd`                                                                                                      |
| `branches`                       | distinct `gitBranch` values, in order seen                                                                       |
| `title`                          | first user text prompt, first 120 chars                                                                          |
| `user_turns`                     | count of user text messages (tool results excluded)                                                              |
| `files_touched`                  | `file_path` / `notebook_path` inputs of `Edit`, `Write`, `MultiEdit`, `NotebookEdit` tool calls, relative to cwd |
| `explicit_todos`                 | lines in user prompts matching `^\s*TODO:\s*(.+)$`                                                               |

### 6.3 Linking

- Worktree: longest known worktree path that is a prefix of `cwd`.
- Branch: each value in `branches`; if empty, the worktree's branch at ingest time.
- A session links to every `(repo_id, branch)` it touched. No matching worktree → **unlinked**,
  still shown in the Sessions view and summary.

### 6.4 Summarization

Runs in the background for ended sessions with `user_turns >= min_turns_to_summarize`, max 2
concurrent, never blocking the UI.

1. Build a condensed input: user prompts, assistant text, and one line per tool call
   (`Edit src/foo.rs`). If over 60 000 chars, keep the first 15 000 and last 45 000.
2. Run `summarize_command` with the prompt on stdin, cwd = `~/.local/share/worktrack/summarizer/`,
   timeout `summarize_timeout_secs`.
3. Expect a JSON object, validated with serde:

```json
{
  "title": "≤ 80 chars",
  "summary": "≤ 400 chars",
  "done": ["..."],
  "unfinished": ["..."],
  "todos": ["..."],
  "blockers": ["..."]
}
```

1. Invalid output → retry once; then `summary_status = failed`, metadata kept, `s` in the TUI
   retries manually.

**Recursion guard.** The summarizer's own `claude -p` run creates a transcript and fires
`SessionEnd`. The hook and the scanner MUST ignore any session whose `cwd` is the summarizer
directory.

### 6.5 Resume

Key `c` on a session copies `cd '<worktree path>' && claude --resume <session_id>` to the clipboard.
The `cd` is required because Claude Code resolves sessions per project directory. If the worktree no
longer exists, the status line says so and nothing is copied.

## 7. Todo list

Todos live in one global Markdown file that stays readable and hand-editable; the app parses it,
edits it in place, and watches it for external changes. Suggestions from rules live in SQLite and
reach the file only when accepted.

### 7.1 Fields

| Field      | Required              | Values                                                                        |
| ---------- | --------------------- | ----------------------------------------------------------------------------- |
| `id`       | yes (assigned by app) | `t_` + ULID                                                                   |
| `title`    | yes                   | one line                                                                      |
| `notes`    | no                    | multi-line Markdown                                                           |
| `status`   | yes                   | `todo`, `doing`, `done`, `dropped`                                            |
| `priority` | no                    | `A`, `B`, `C`                                                                 |
| `link`     | no                    | branch reference `<PROJECT>/<repo>:<branch>`; PR and Jira key derived from it |
| `jira`     | no                    | explicit Jira key, if not derivable from link or title                        |
| `created`  | yes                   | date                                                                          |
| `done`     | when done/dropped     | date                                                                          |
| `source`   | no                    | `manual`, `rule:<rule_name>`, `session:<session_id>`                          |

`dropped` means abandoned: shown struck through, excluded from Done in the summary.

### 7.2 File format

```markdown
# Todos

## Active

- [ ] (A) Finish upload retry logic
      <!-- wt id=t_01JD8Q status=doing link=API/upload-svc:feature/PROJ-123-retry created=2026-11-20 -->
      Notes are indented by 4 spaces. They can span lines.
- [ ] Write migration doc <!-- wt id=t_01JD9A created=2026-11-24 -->
- [ ] Call Ivan about staging access

## Done

- [x] Fix flaky auth test
      <!-- wt id=t_01JD7Z link=API/auth:bugfix/flaky created=2026-11-18 done=2026-11-24 -->

## Archive

- [x] ...
```

Parsing rules:

1. Only list items under `## Active`, `## Done`, `## Archive` are todos. Everything else (other
   headings, paragraphs) is preserved byte-for-byte.
2. `(A)`/`(B)`/`(C)` prefix = priority. `- [x]` = done unless metadata says `status=dropped`.
   `- [ ]` = todo unless `status=doing`.
3. Metadata is the trailing `<!-- wt key=value ... -->` comment. Values with spaces are
   double-quoted. Unknown keys are preserved.
4. A hand-written item with no metadata is valid; the app assigns `id` and `created` on its next
   write.
5. Section is derived from status on write: todo/doing → Active, done/dropped → Done, done/dropped
   older than `archive_after_days` → Archive. Order within Active is the user's order.

### 7.3 Writes and external edits

- Every change is an id-based operation (`Add`, `Update{id, fields}`, `Move{id, after}`,
  `Delete{id}`).
- Write = re-read file, check its hash against the last read, apply operation, write to a temp file
  in the same directory, `rename` over the original.
- If the hash differs, re-parse first and apply the operation to the new content. An operation on an
  id that no longer exists is dropped with a status message.
- A file watcher (`notify` crate, 300 ms debounce) reloads after external edits. Parse errors keep
  the last good state and show the error line in the status bar; the app MUST NOT write until the
  file parses.

### 7.4 Editing in the TUI

Inline single-key editing; notes and bulk edits go to the editor. Keys are in section 9.6. `E` opens
the selected todo's notes in `$VISUAL`/`$EDITOR` (temp file, TUI suspended). `O` opens the whole
todo file in the editor.

### 7.5 Auto-update rules

Rules run after each reload on new rows in `state_events` and new session summaries. Mode per rule:
`auto` applies directly, `suggest` creates an Inbox suggestion, `off` does nothing.

| Rule                          | Trigger                                                                         | Action                                                                                         | Default |
| ----------------------------- | ------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- | ------- |
| `new_unfinished_branch`       | a WorkItem enters an unfinished state for the first time and has no linked todo | add todo `<branch>` linked to it                                                               | suggest |
| `pr_merged`                   | linked WorkItem → `Merged`                                                      | set linked todos (status todo/doing) to done, `done` = merge date                              | auto    |
| `pr_declined`                 | linked WorkItem → `PrDeclined`                                                  | suggestion with two actions: keep, or abandon (WorkItem → `Abandoned`, linked todos → dropped) | suggest |
| `pr_needs_work`               | WorkItem → `PrNeedsWork`, or `openTaskCount` increases                          | add todo `Address review on PR #<id>` linked                                                   | suggest |
| `session_unfinished`          | session summary or `TODO:` lines yield `unfinished`/`todos` items               | one todo per item, linked to the session's branch, `source=session:<id>`                       | suggest |
| `worktree_removed_unfinished` | worktree disappears while its WorkItem is unfinished                            | same two actions as `pr_declined`                                                              | suggest |

Deduplication:

- Each suggestion has a subject key, e.g. `pr_needs_work:API/auth:42:2026-11-25T10:02Z` (transition
  time included so a second review round triggers again).
- A subject key is suggested at most once. Accept and reject are recorded in the `suggestions` table
  (status accepted/rejected) and never re-suggested.
- `session_unfinished` skips an item whose normalized text (lowercase, collapsed whitespace) equals
  an existing todo title.

First run: existing branches produce `new_unfinished_branch` suggestions in one batch. The Inbox
supports accept-all and reject-all.

## 8. Daily summary

The summary covers the window from 00:00 local time on the previous workday until now. It opens
automatically on the first launch of each workday and is available any time via tab `4` or
`worktrack summary`.

### 8.1 Window

- Previous workday = the latest day in `summary.workdays` before today (Monday → Friday).
- Override: `--since YYYY-MM-DD` on the CLI, or `[` / `]` in the Summary view to shift the start by
  one workday.
- Event times come from `state_events`, todo `done` dates, and session timestamps, never from live
  state.

### 8.2 Sections

| Section     | Contents                                                                                                                                         | Order                       |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------- |
| Done        | WorkItems that became `Merged` or `MergedNoPr` in the window, each with its linked todos and session titles nested; then done todos with no link | merge time                  |
| In progress | unfinished WorkItems with a commit, session, or PR event in the window, with state, flags, and session summaries; plus all `doing` todos         | last activity, newest first |
| Todo        | Active `todo` items, priority A → none, then file order; first 10 shown, `+N more`                                                               | priority                    |
| Blockers    | `PrNeedsWork`, `ci:FAILED`, unresolved `pr_declined` / `worktree_removed_unfinished` suggestions, session `blockers`, `stale` items              | severity, as listed         |
| Cleanup     | items with the `cleanup` flag                                                                                                                    | repo                        |

Empty sections are omitted. Unlinked sessions appear under In progress as `(no branch)`.

### 8.3 Export

- `y` in the Summary view copies the Markdown to the clipboard.
- `w` writes `<export_dir>/<YYYY-MM-DD>.md`, overwriting the same day's file.
- `worktrack summary [--since DATE]` prints the same Markdown to stdout and exits, for cron or
  scripts.

Markdown shape:

```markdown
# Summary — Wed 2026-11-25 (since Tue 2026-11-24)

## Done

- **API/auth** `bugfix/flaky` — PR #41 merged
  - Fix flaky auth test

## In progress

- **API/upload-svc** `feature/PROJ-123-retry` — PR #44 needs work · ci failed · 2 tasks
  - Session: Added exponential backoff; integration test still failing

## Todo

- (A) Finish upload retry logic
- Write migration doc

## Blockers

- PR #44: reviewer requested changes
```

## 9. TUI design

Five tabs share one frame: header, tab bar, a list pane with a details pane, and a footer with key
hints. Every view follows LGBC's responsive rules.

### 9.1 Frame

```
┌ worktrack ── 7 repos · 23 items · PR ✓ · synced 2m ago ─────────────── ⟳ syncing 3/7 ┐
│ 1 Work   2 Todos   3 Sessions   4 Summary   5 Inbox·4                                 │
├───────────────────────────────────────────────────────┬───────────────────────────────┤
│ list pane                                             │ details pane                  │
│                                                       │                               │
├───────────────────────────────────────────────────────┴───────────────────────────────┤
│ status message / error                                                                │
│ j/k move  enter open  o PR  t todo  c resume  f filter  / search  ? help  q quit      │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

- Header: repo and item counts, PR integration status (`PR ✓`, `PR: no token`, `PR: 401`), last full
  sync age, background progress on the right.
- Tab bar: Inbox shows its pending count; Sessions shows `·live` when any session is live.
- Footer: one status line (last message or error, cleared after 5 s) and context hints for the
  active view, truncated with `…`, full list in `?`.

Breakpoints: ≥ 120 columns → side by side, list 60 %; 80–119 → list above details (60/40); < 80 →
list only, `Enter` opens details full screen, `Esc` returns. Below 40×10 the app shows
`terminal too small` and nothing else.

### 9.2 Work view (tab 1, default)

WorkItems grouped by repo; groups collapse with `Enter` on the group row. Default filter:
Unfinished.

```
 Filter: Unfinished ▾   Sort: activity                                   12 items
 ▾ API/upload-svc                                                    4
   ✎ feature/PROJ-123-retry       PR #44  ✗ci  ☐2  *  ↑1   ● live   2m   T1 S3
   ◐ feature/PROJ-130-chunking    PR #47  …ci              ↓3       1h   T0 S1
   ↑ spike/s3-multipart           —                ∅       z        16d  T0 S0
   ○ chore/bump-deps              —            *  ↑2                3h   T1 S0
 ▾ API/auth                                                          2
   ✓ feature/PROJ-118-sso         PR #39  ✓ci                       1d   T1 S2
   ✕ feature/PROJ-101-legacy      PR #31                            4d   T0 S0
 ▸ WEB/portal                                                        6
```

Columns: state icon, branch (truncated with `…`), PR id, CI, open tasks, flags, last activity
(newest of commit, session, PR update), linked todo count `T`, session count `S`. Narrow layouts
drop columns right to left: S/T, then flags, then CI.

Details pane for a WorkItem:

```
 feature/PROJ-123-retry                       PROJ-123 ↗
 State   PR needs work (since 10:02)
 Path    ~/work/upload-svc-wt/retry
 Flags   dirty · ahead 1 · ci failed · 2 open tasks
 PR #44  Add retry with backoff to uploads
         ivan ✗ needs work · maria ○ unapproved
 Commits ahead of trunk (5)
   a1b2c3d  Add backoff jitter              2h
   ...
 Todos
   [~] (A) Finish upload retry logic
 Sessions
   10:40  Added exponential backoff; integration test still failing
   Tue    Explored S3 SDK retry config
```

Filters (`f` cycles): Unfinished, Needs attention (needs work, ci failed, declined, stale), Cleanup,
Done (window of the summary), All.

### 9.3 Todos view (tab 2)

```
 Filter: Active ▾   Group: none                                          9 todos
 [~] A  Finish upload retry logic            upload-svc:feature/PROJ-123-retry
 [ ] A  Answer Ivan's review on PR #44       upload-svc:feature/PROJ-123-retry
 [ ] B  Write migration doc
 [ ]    Call Ivan about staging access
 [x]    Fix flaky auth test                  auth:bugfix/flaky        Tue
```

`[ ]` todo, `[~]` doing, `[x]` done, `[-]` dropped (struck through). Details pane shows notes
rendered as plain text, link with WorkItem state, source, created and done dates. `G` toggles
grouping by repo. Filters: Active, Done, All.

Add and edit use a one-line input at the bottom of the list pane; `Enter` saves, `Esc` cancels. The
link picker (`L`) is a fuzzy list of WorkItems.

### 9.4 Sessions view (tab 3)

```
 10:40  ● upload-svc  feature/PROJ-123-retry   Add backoff to uploads      14 turns  live
 09:12  ✓ auth        feature/PROJ-118-sso     Fix SAML redirect           22 turns
 Tue    ✓ portal      (no branch)              Explain webpack config       3 turns
 Tue    ! upload-svc  spike/s3-multipart       Try multipart API            8 turns  summary failed
```

Status marks: `●` live, `✓` summarized, `…` queued, `!` failed, blank = below turn threshold.
Details: summary, done, unfinished, todos, blockers, files touched, resume command preview.

### 9.5 Summary (tab 4) and Inbox (tab 5)

Summary renders the section 8 Markdown as styled text in a single scrollable pane; the header shows
the window, e.g. `since Tue 2026-11-24 ([ ] to shift)`.

Inbox lists pending suggestions, newest first:

```
 pr_needs_work         + Address review on PR #44            upload-svc:feature/PROJ-123-retry
 session_unfinished    + Integration test for retry fails    upload-svc:feature/PROJ-123-retry
 pr_declined           ? PR #31 declined: keep or abandon    auth:feature/PROJ-101-legacy
 new_unfinished_branch + chore/bump-deps                     upload-svc:chore/bump-deps
```

`+` adds a todo on accept; `?` needs a choice (keep / abandon).

### 9.6 Keybindings

Tab switching uses `1`–`5`. This differs from LGBC, where digits select filters; filters here cycle
with `f`.

| Scope    | Key                                         | Action                                                            |
| -------- | ------------------------------------------- | ----------------------------------------------------------------- |
| Global   | `1`–`5`, `Tab` / `Shift-Tab`                | switch tab                                                        |
| Global   | `j` `k` `↓` `↑`, `g` `G`, `Ctrl-d` `Ctrl-u` | move, top/bottom, half page                                       |
| Global   | `J` `K`                                     | scroll details                                                    |
| Global   | `/`                                         | search current list (`@repo:`, `@state:` prefixes in Work)        |
| Global   | `f`                                         | cycle filter                                                      |
| Global   | `v`                                         | cycle layout (auto / side / stacked / list)                       |
| Global   | `r` / `R`                                   | local reload / full sync                                          |
| Global   | `i`                                         | diagnostics overlay                                               |
| Global   | `?` / `q`                                   | help / quit                                                       |
| Work     | `Enter`                                     | collapse group, or focus details on narrow layout                 |
| Work     | `o`                                         | open PR in browser (Jira if no PR)                                |
| Work     | `y` / `Y`                                   | copy branch name / worktree path                                  |
| Work     | `t`                                         | add todo linked to this item                                      |
| Work     | `c`                                         | copy resume command of latest session                             |
| Work     | `A` / `U`                                   | mark abandoned (confirm) / unmark                                 |
| Work     | `X`                                         | remove worktree, only on `cleanup` items (confirm; dirty refused) |
| Todos    | `a` / `e` / `E`                             | add / edit title / edit notes in editor                           |
| Todos    | `Space`                                     | toggle done                                                       |
| Todos    | `s` / `p`                                   | cycle status todo→doing→done / cycle priority A→B→C→none          |
| Todos    | `L`                                         | link picker                                                       |
| Todos    | `+` / `-`                                   | move up / down                                                    |
| Todos    | `D` / `d`                                   | drop / delete (confirm)                                           |
| Todos    | `G` / `O`                                   | toggle group by repo / open file in editor                        |
| Sessions | `c`                                         | copy resume command                                               |
| Sessions | `s`                                         | re-summarize                                                      |
| Sessions | `Enter`                                     | jump to linked WorkItem                                           |
| Summary  | `[` / `]`                                   | shift window start by one workday                                 |
| Summary  | `y` / `w`                                   | copy Markdown / write file                                        |
| Inbox    | `Enter` / `x`                               | accept / reject                                                   |
| Inbox    | `e`                                         | edit text, then accept                                            |
| Inbox    | `k` / `b`                                   | keep / abandon (choice suggestions)                               |
| Inbox    | `A` / `X`                                   | accept all / reject all (confirm)                                 |

### 9.7 Diagnostics overlay (`i`)

Shows: repos skipped and why, Bitbucket token and user status, last sync errors per repo, hook
installed or not (checks `~/.claude/settings.json`), session parse warnings, summarizer failures,
todo file parse errors, config path, DB path, version.

### 9.8 Visual language

| State                  | Icon | Color     |
| ---------------------- | ---- | --------- |
| `LocalOnly`            | `○`  | gray      |
| `Pushed`               | `↑`  | blue      |
| `PrOpen`               | `◐`  | yellow    |
| `PrApproved`           | `✓`  | green     |
| `PrNeedsWork`          | `✎`  | red       |
| `PrDeclined`           | `✕`  | magenta   |
| `Merged`, `MergedNoPr` | `✔` | dim green |
| `Abandoned`            | `–`  | dim gray  |

Flags: `*` dirty, `↑N` ahead, `↓N` behind, `✓ci` `✗ci` `…ci`, `☐N` open tasks, `∅` no worktree, `z`
stale, `⌫` cleanup, `●` live session (magenta). Color is never the only signal: every state has an
icon, and `NO_COLOR` disables colors.

## 10. Storage, refresh, concurrency

One SQLite database holds all derived state and history; the todo file is the only other persistent
store. The UI thread never blocks on IO.

### 10.1 Database

Path `$XDG_DATA_HOME/worktrack/worktrack.db` (default `~/.local/share/worktrack/`). WAL mode. Schema
version in `PRAGMA user_version`, migrations as embedded SQL files applied in order at startup. The
hook opens its own connection with `busy_timeout = 100` ms.

```sql
CREATE TABLE repos (
  id TEXT PRIMARY KEY, common_dir TEXT NOT NULL, origin_url TEXT,
  bb_base_url TEXT, bb_project TEXT, bb_repo TEXT, trunk TEXT,
  skipped_reason TEXT, last_seen INTEGER NOT NULL);

CREATE TABLE worktrees (
  path TEXT PRIMARY KEY, repo_id TEXT NOT NULL REFERENCES repos(id),
  branch TEXT, head TEXT, is_main INTEGER, locked INTEGER, prunable INTEGER,
  last_seen INTEGER NOT NULL);

CREATE TABLE work_items (
  repo_id TEXT NOT NULL, branch TEXT NOT NULL, state TEXT NOT NULL,
  abandoned_manual INTEGER NOT NULL DEFAULT 0,
  first_seen INTEGER NOT NULL, last_activity INTEGER,
  PRIMARY KEY (repo_id, branch));

CREATE TABLE state_events (
  id INTEGER PRIMARY KEY, repo_id TEXT NOT NULL, branch TEXT NOT NULL,
  from_state TEXT, to_state TEXT NOT NULL, at INTEGER NOT NULL, detail TEXT);
CREATE INDEX state_events_at ON state_events(at);

CREATE TABLE pr_cache (
  repo_id TEXT NOT NULL, branch TEXT NOT NULL, pr_id INTEGER, state TEXT,
  title TEXT, url TEXT, author TEXT, reviewers TEXT, open_tasks INTEGER,
  comment_count INTEGER, from_latest_commit TEXT, pr_updated_at INTEGER,
  fetched_at INTEGER NOT NULL, PRIMARY KEY (repo_id, branch));

CREATE TABLE build_cache (sha TEXT PRIMARY KEY, state TEXT, fetched_at INTEGER NOT NULL);

CREATE TABLE session_files (path TEXT PRIMARY KEY, size INTEGER, mtime INTEGER, offset INTEGER);

CREATE TABLE session_queue (
  session_id TEXT NOT NULL, transcript_path TEXT, cwd TEXT, reason TEXT,
  received_at INTEGER NOT NULL);

CREATE TABLE sessions (
  id TEXT PRIMARY KEY, cwd TEXT, title TEXT, started_at INTEGER,
  last_activity_at INTEGER, ended_at INTEGER, end_reason TEXT,
  user_turns INTEGER, files_touched TEXT, explicit_todos TEXT,
  summary_status TEXT NOT NULL DEFAULT 'none', summary TEXT, summarized_at INTEGER);

CREATE TABLE session_links (
  session_id TEXT NOT NULL, repo_id TEXT NOT NULL, branch TEXT NOT NULL,
  PRIMARY KEY (session_id, repo_id, branch));

CREATE TABLE suggestions (
  id INTEGER PRIMARY KEY, rule TEXT NOT NULL, subject_key TEXT NOT NULL UNIQUE,
  payload TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending',
  created_at INTEGER NOT NULL, decided_at INTEGER);

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
```

Timestamps are Unix seconds UTC; JSON columns are TEXT. `meta` holds `last_launch_date` (drives the
auto-open Summary) and `first_run_done`.

Todo dates come from the file, not the DB. An item ticked by hand without a `done` date gets today's
date on the next write.

### 10.2 Tasks

| Task          | Trigger                                                        | Work                                                                                                                                                                                        |
| ------------- | -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Local reload  | startup, every `local_interval_secs`, terminal focus gain, `r` | per known repo: `git worktree list --porcelain`, one `git for-each-ref` with upstream track info, `git status --porcelain=v2 --branch` per worktree; session scan; derive states; run rules |
| Full sync     | after startup reload, every `sync_interval_mins`, `R`          | discovery; `git fetch --prune origin` per repo (max 4 concurrent, 60 s timeout); Bitbucket requests (section 5); then a local reload                                                        |
| Session queue | after each local reload                                        | drain `session_queue`, mark sessions ended                                                                                                                                                  |
| Summarizer    | session ended                                                  | section 6.4, max 2 workers                                                                                                                                                                  |
| Todo watcher  | file change                                                    | parse, emit `TodosReloaded`                                                                                                                                                                 |

Git runs as the `git` CLI inside `spawn_blocking`, as in LGBC. State derivation is a pure function
`derive_state(local, pr, manual) -> WorkState` with no IO.

### 10.3 Threading

- Main thread: crossterm event loop with a 100 ms tick; drains an `mpsc::Receiver<AppEvent>` each
  tick; renders.
- Tokio runtime on worker threads runs all tasks above.
- One DB writer task owns the write connection; other tasks send write requests through a channel.
  Readers use their own connections.
- `AppEvent` variants: `ReposUpdated`, `WorkItemsUpdated`, `PrUpdated`, `SessionsUpdated`,
  `TodosReloaded`, `SuggestionsUpdated`, `Progress{task, done, total}`, `Error{scope, message}`.

### 10.4 Performance targets

| Measure                               | Target   |
| ------------------------------------- | -------- |
| First frame from cache                | < 200 ms |
| Local reload, 20 repos / 60 worktrees | < 1 s    |
| Full sync, 20 repos, warm cache       | < 10 s   |
| Key press to redraw                   | < 16 ms  |
| Hook runtime                          | < 200 ms |

## 11. CLI, errors, security

The binary runs the TUI by default; subcommands cover the hook, scripting, and the summary for cron.

### 11.1 Commands

```
worktrack [--tab work|todos|sessions|summary|inbox] [--config PATH]
          [--offline] [--no-auto-sync] [--no-update-check]
worktrack summary [--since YYYY-MM-DD] [--out FILE]
worktrack status --json                  # WorkItems with state, flags, PR, links
worktrack todo add "TEXT" [--link PROJECT/repo:branch] [--priority A|B|C]
worktrack hook session-end               # called by Claude Code, reads stdin
worktrack hook install [--apply]
worktrack doctor                         # diagnostics overlay as text, exit 1 on problems
worktrack update [--check]               # copied from LGBC in milestone 7
```

`--offline` disables fetch, Bitbucket, and the summarizer; everything else works from local git and
cache.

### 11.2 Errors and logging

- `color-eyre` for error reports; a panic hook restores the terminal before printing.
- `tracing` to `$XDG_STATE_HOME/worktrack/worktrack.log`, rotated at 5 MB, 3 files kept. Level from
  `WORKTRACK_LOG`, default `info`.
- Per-repo and per-task failures never stop other work. They show in the status line once and stay
  in diagnostics until the next success.

### 11.3 Security

- `BITBUCKET_TOKEN` is read from the environment only, never written to config, DB, logs, or error
  messages. `Authorization` headers are redacted in all debug output.
- All external text (PR titles, reviewer names, commit messages, transcript text, summaries, todo
  file content) MUST have ANSI escapes and control characters other than newline stripped before
  rendering, to prevent terminal escape injection.
- The summarizer prompt states that the transcript is data to summarize, not instructions. Its
  output is schema-validated and can only create Inbox suggestions; it never triggers git, file, or
  network actions.
- The resume command quotes the path with POSIX single-quote escaping.
- Destructive actions are limited to `X` (cleanup items only): `git worktree remove` without
  `--force`, then `git branch -d` (never `-D`) when the state is `Merged` or `MergedNoPr`. The
  confirmation lists both commands.
- `~/.claude/settings.json` is modified only by `hook install --apply`, after a backup.
- Writes to the todo file and settings file are atomic (temp file + rename).

## 12. Testing, milestones, later

Each milestone ships a usable binary with its tests; the state machine, todo round-trip, and
transcript parser carry the most risk and get the most tests.

### 12.1 Tests

| Area              | Method                                                                                                                                                               |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `derive_state`    | table-driven unit tests: every row of 4.3, branch reused after merge, no token, protected                                                                            |
| Todo file         | round-trip: parse → serialize is byte-identical for files containing unknown headings, paragraphs, and metadata keys (proptest); hand-written items gain id and date |
| Todo writes       | concurrent external edit during a write; deleted id; parse error blocks writes                                                                                       |
| Rules             | dedupe by subject key; reject is permanent; second review round re-triggers; `pr_merged` auto-completes only linked items                                            |
| Transcript parser | fixtures from real (anonymized) Claude Code transcripts of the current version, plus malformed lines and unknown types                                               |
| Summarizer        | fake `summarize_command` scripts returning valid JSON, invalid JSON, and hanging past the timeout; recursion guard                                                   |
| Hook              | stdin JSON → queue row; exits 0 in < 200 ms with a locked DB                                                                                                         |
| Discovery and git | tempfile repos: normal clone, bare + worktrees, nested worktrees, detached HEAD, prunable worktree                                                                   |
| Bitbucket         | wiremock: dashboard paging, per-branch lookup, build status, 401, 403, 429 with `Retry-After`                                                                        |
| Summary           | window calc (Monday → Friday, `--since`), section contents from seeded events                                                                                        |
| UI                | ratatui `TestBackend` + insta snapshots of every view at 60×20, 100×30, 160×45                                                                                       |

### 12.2 Milestones

1. **Core**: crate setup, copies per section 2.2 (except update), config, discovery, local git
   state, `LocalOnly`/`Pushed`/`MergedNoPr`, Work view, SQLite, refresh loop.
2. **Bitbucket**: new methods in `lgbc/bitbucket.rs` (dashboard, build status, current user),
   per-branch PRs, CI, tasks, full state machine, cache.
3. **Todos**: file format, Todos view, inline editing, editor integration, watcher, link picker.
4. **Rules**: rules engine, Inbox view, first-run batch.
5. **Sessions**: hook + install, scan, parser, linking, Sessions view, resume copy.
6. **Summaries**: summarizer, Summary view, auto-open per workday, `worktrack summary`, export.
7. **Polish**: copy update modules, diagnostics + `doctor`, `status --json`, `todo add`, cargo-dist
   release, self-update.

### 12.3 Verify before implementing

- Your Bitbucket DC version supports `/rest/build-status/1.0/commits/{sha}` and the dashboard
  endpoint with `role=AUTHOR`.
- Current Claude Code transcript fields (`gitBranch`, `cwd`, tool_use input names) and `SessionEnd`
  hook input fields match section 6.
- `claude -p --output-format json` envelope: the summary JSON is inside the `result` field and may
  be wrapped in code fences; strip them before parsing.

### 12.4 Later (not v1)

- Zellij resume: open a new tab in the worktree running `claude --resume <id>` via `zellij action`.
- Unresolved PR comment threads (not only tasks).
- Jira API for ticket titles and status.
- Bitbucket Cloud and GitHub providers.
- Desktop notifications for state changes (PR approved, CI failed).
