# worktrack

A terminal UI that shows where your unfinished work is. It scans your git repositories with a
Bitbucket Data Center remote and lists, per branch and worktree, what state the work is in (local
only, pushed, PR open / approved / needs work / declined, merged, abandoned). It also shows the
Claude Code sessions that ran on each branch, keeps a todo list in a Markdown file that updates
itself from that state, and writes a daily summary.

The full design is in [docs/SPEC.md](docs/SPEC.md). This README covers setup and day-to-day use.

## Build and install

worktrack is a crate of the `omni-scripts` Rust workspace. From the repository root:

```sh
nix develop --command bash -c "cd rust && cargo build --release -p worktrack"
# or install it into ~/.cargo/bin
nix develop --command bash -c "cd rust && cargo install --path worktrack"
```

It needs `git` on `PATH`. Supported platforms are Linux and macOS.

## Configuration

The config lives at `$XDG_CONFIG_HOME/worktrack/config.toml` (`~/.config/worktrack/config.toml`,
also on macOS). On the first start without a config, worktrack explains what it does and offers to
write a commented default (`y`). Use `--config PATH` to point at another file.

The keys that matter first:

```toml
[discovery]
repos = ["~/code/my-service"] # main repos tracked explicitly (no scanning needed)
roots = ["~/work", "~/src"]   # also scanned recursively for git repositories
max_depth = 4

[identity]
emails = ["you@company.com"]  # your commit emails: branches with your commits are "yours"
bitbucket_user = ""           # PR author slug; empty = resolved from Bitbucket
```

Worktrees are tracked dynamically: every local reload (10 s) runs `git worktree list` for each known
repo, so a worktree you add with `git worktree add` shows up within seconds wherever it lives, even
outside `roots`, and one you remove drops out (an unfinished branch then gets a keep/abandon
suggestion). List a repo under `repos` to track it without scanning a whole directory; either way
only the main repo needs to be known.

Unknown keys are an error that names the key and line. Every key is described in section 3 of the
spec.

Secrets never go in the file. PR data needs a Bitbucket HTTP access token in the environment:

```sh
export BITBUCKET_TOKEN=...
```

Without it worktrack still works from local git, shows `PR: no token` in the header and leaves the
PR columns empty.

Data and logs:

| What        | Where                                                                       |
| ----------- | --------------------------------------------------------------------------- |
| Database    | `$XDG_DATA_HOME/worktrack/worktrack.db` (`~/.local/share/worktrack/`)       |
| Todo list   | `~/.config/worktrack/todos.md` (`todos.file`), plain Markdown you can edit  |
| Summaries   | `~/.local/share/worktrack/summaries/` (`summary.export_dir`)                |
| Log         | `$XDG_STATE_HOME/worktrack/worktrack.log`, level from `WORKTRACK_LOG=debug` |
| Hook errors | `$XDG_STATE_HOME/worktrack/hook.log`                                        |

## Claude Code hook

worktrack picks up Claude Code sessions by scanning `~/.claude/projects` on every reload. The
`SessionEnd` hook makes ended sessions show up (and get summarized) right away:

```sh
worktrack hook install          # print the settings snippet
worktrack hook install --apply  # merge it into ~/.claude/settings.json (writes settings.json.bak)
```

Session summaries run `claude -p` in the background (`sessions.summarize_command`); set
`sessions.summarize = false` to turn them off.

### How sessions link to branches

A session shows up on a Work item (its session count, the sessions list, `c`) when it is linked to
that branch. Links come from two places:

- **Where it ran.** Claude Code records the directory a session started in and the git branch there
  (`gitBranch`) on every transcript line. The session links to every branch it was on in the
  worktree containing that directory.
- **Where it worked.** A session started in one worktree can do its work in another: edit files by
  absolute path, or run `cd ~/work/other && git commit ...`. Claude Code records no branch for that.
  worktrack collects the directories a session worked in: every `cwd` in the transcript, the
  directory of each file given to `Edit`, `Write`, `MultiEdit` and `NotebookEdit`, and in Bash
  commands `cd <dir>` (at the start or after `&&` / `;`) and `git -C <dir>`. Each one links the
  session to the branch of the worktree containing it. Commands it cannot read without a shell
  (`$(...)`, globs, variables other than `$HOME`) are skipped. Reads do not count: `Read`, `Grep`
  and `Glob` in another worktree, or `ls` / `cat` there, do not link to it.

The Sessions details show the second kind as `Worked in  ~/work/other  feature/x`, after the Branch
row. A worktree's branch is taken the first time worktrack links the session to it and kept after
the worktree moves to another branch. A worktree discovered later picks up its sessions on the next
reload. Resuming (`c`) always `cd`s into the directory the session started in, because that is where
Claude Code stores it.

## Usage

```sh
worktrack                       # the TUI
worktrack --tab todos           # open a tab: work, todos, sessions, summary, inbox
worktrack --offline             # no git fetch, no Bitbucket, no summarizer; local git + cache only
worktrack --no-auto-sync        # no periodic full sync (the startup sync and R still run)

worktrack summary [--since YYYY-MM-DD] [--out FILE]   # daily summary as Markdown
worktrack status --json         # WorkItems with state, flags, PR and links
worktrack todo add "TEXT" [--link PROJECT/repo:branch] [--priority A|B|C]
worktrack doctor                # diagnostics as text; exit code 1 when something is wrong
```

`summary`, `status` and `doctor` do not use the network: they reload local git state and the Claude
Code transcripts, and use the PR data cached by the last TUI sync. `doctor` reports a problem
(exit 1) for a missing or invalid config, a missing `BITBUCKET_TOKEN`, no Bitbucket repositories
under the roots, a todo file that does not parse, and git, session or database errors.

The TUI reloads local git state every 10 s and runs a full sync (discovery, `git fetch --prune`,
Bitbucket) at startup and every 5 minutes. On the first launch of each workday it opens the Summary
tab.

### Keys

The full list is in section 9.6 of the spec and behind `?` in the app.

| Keys                          | Action                                                                                           |
| ----------------------------- | ------------------------------------------------------------------------------------------------ |
| `1`–`5`, `Tab`                | Work, Todos, Sessions, Summary, Inbox                                                            |
| `j` `k` / `↓` `↑`, `g` `G`    | move, top / bottom                                                                               |
| `Enter` / `Space` on a header | fold / unfold the group (Work and Todos repos, Sessions days, Summary directories)               |
| `z`, `Z`                      | fold / unfold the selected row's group, fold all (unfold all when all are folded)                |
| `/`, `f`, `v`                 | search, cycle filter, cycle layout                                                               |
| `Enter` / `Esc`               | details full screen in the list-only layout / back                                               |
| `r` / `R`                     | local reload / full sync                                                                         |
| `Ctrl-l`                      | repaint the screen                                                                               |
| `i`, `?`, `q`                 | diagnostics, help, quit                                                                          |
| `Esc` `Esc`                   | quit (twice within a second, when there is nothing left to close)                                |
| Work: `o`, `t`, `c`           | open PR (or Jira), new linked todo (dialog), copy resume command                                 |
| Work: `A` / `U`, `X`          | mark / unmark abandoned, remove a cleanup worktree                                               |
| Todos: `h` `l` / `←` `→`      | previous / next status filter (Active, Todo, Doing, Done, Dropped, All)                          |
| Todos: `a` `e` `E`            | add, edit title, edit notes in `$EDITOR`                                                         |
| Todos: `Space` `s` `p` `L`    | mark done (`Enter` confirms) / reopen, cycle status, cycle priority, link to a branch            |
| Todos: `Enter`                | go to the todo's session: the one it was created from, else the latest on its branch             |
| Todos: `O`                    | open the whole todo file in the editor                                                           |
| Sessions: `c`, `s`            | copy `cd … && claude --resume …`, summarize again                                                |
| Work / Sessions: `t`          | New todo dialog: link shown, `Tab` priority, `↑↓` + `Enter` takes a session suggestion           |
| New todo: `Enter`, `Ctrl-s`   | add and close, add and keep open for the next one (also `Alt-Enter` where the terminal sends it) |
| Summary: `h` `l` / `←` `→`    | previous / next section (Done, In progress, Todo, Blockers, Cleanup)                             |
| Summary: `Enter`              | open the entry in its tab; in the list-only layout `Enter` shows details, `Enter` again jumps    |
| Summary: `o`, `Space`         | open the item's PR (or Jira), mark a todo entry done (`Enter` confirms)                          |
| Summary: `[` `]`, `y`, `w`    | shift the window, copy Markdown, write the file                                                  |
| Inbox: `Enter` / `x`, `k`/`b` | accept / reject, keep / abandon                                                                  |

Lists are grouped (Work by repo, Todos by repo with `G`, Sessions by day, Summary by working
directory), with a blank row between groups when the pane has room. The cursor stops on group
headers too; there `Enter` / `Space` fold and the details pane sums up the group. Folded groups stay
folded across refreshes; a search shows every group open. Keys that act on one item only show a hint
on a header.

The tabs and the filter and section bars of Todos and Summary also take a mouse click.

The editor is `ui.editor`, else `$VISUAL`, else `$EDITOR`, else `vi`.

## Status

Milestones 1–6 of the spec and most of 7 are in: discovery, local git state, the Bitbucket state
machine with caching, todos with the file watcher, rules and the Inbox, sessions with the hook and
summarizer, the daily summary, diagnostics and `doctor`, `status --json` and `todo add`.

Not done yet (milestone 7):

- Self-update: there is no `worktrack update` subcommand and no update notice; `--no-update-check`
  is accepted and does nothing.
- Release packaging: worktrack has no cargo-dist release configuration yet (no
  `[package.metadata.dist]` and no release flow), so there are no prebuilt binaries. Build it from
  source as above.
