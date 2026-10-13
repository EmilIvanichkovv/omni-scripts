---
description:
  Run a QA session for local-git-branch-cleanup-tui — record reported bugs/features in
  ISSUES_AND_FEATURES.md and as GitHub issues, then fix them one by one
argument-hint: [optional context about what is being tested]
---

# QA Session — local-git-branch-cleanup-tui

You are running a QA session with the user. The user reports bugs and feature requests one at a
time; you record them, and only after the list is complete do you start fixing.

Tracking document: `rust/local-git-branch-cleanup-tui/docs/ISSUES_AND_FEATURES.md` Project root:
`rust/local-git-branch-cleanup-tui/`

Every tracker entry is mirrored as a GitHub issue in the repo `gh` resolves from the working
directory (`gh repo view --json nameWithOwner,url`). The tracker stays the source of truth; the
GitHub issue carries a copy of the entry and links back to it.

Session context from the user: $ARGUMENTS

## Phase 1 — Report

For each item the user describes:

1. **Understand it first.** If the report is ambiguous (which screen, which key, which branch
   state), ask a short clarifying question before writing anything. Do not guess at repro steps.
2. **Locate the cause.** Read the relevant code (`src/`) to confirm the behavior and find the likely
   root cause. Keep this brief — the goal is a credible "Proposed Fix", not the fix itself.
3. **Add an entry** to the tracking document:
   - Number it `#N+1` where `#N` is the highest existing issue number in the file.
   - Put it under the matching open section (`Critical Issues`, `UI/UX Issues`,
     `Performance Issues`, `Minor/Cosmetic Issues`). Replace the `_No open ... issues._` placeholder
     if present. Newest entry goes first in its section.
   - Use the template below. Bugs get `Steps to Reproduce`; feature requests may omit them and
     describe the desired behavior under `Expected Behavior` instead.
   - `Category` is `<Section> / Bug` or `<Section> / Enhancement`.
   - Write the description in present tense (it is still broken). Wrap prose at ~100 columns.
4. **Open the GitHub issue** (see GitHub issue mechanics) and fill the entry's `GitHub` bullet with
   the link `gh issue create` prints.
5. **Update `Last Updated`** at the top (`DD-MM-YYYY HH:MM`, local time) and append a
   `Reported: ...` row to the `Change Log` table at the bottom.
6. **Do not touch source code** in this phase. Do not commit yet.
7. Confirm to the user in one or two lines what you recorded, with the GitHub issue link, then wait
   for the next item.

Entry template (open):

```markdown
### Issue #N: <Short imperative title>

- **Status:** 🔴 Open
- **Reported:** DD-MM-YYYY
- **Category:** <Critical|UI/UX|Performance|Minor/Cosmetic> / <Bug|Enhancement>
- **GitHub:** [#G](https://github.com/<owner>/<repo>/issues/G)
- **Description:**
  - <What is wrong, and the root cause if found, with the relevant function/field in backticks>
- **Steps to Reproduce:**
  1. <step>
  2. <step>
- **Expected Behavior:** <one line, or a bullet list>
- **Actual Behavior:** <one line, or a bullet list>
- **Proposed Fix:**
  - <How we can handle it: which module/function changes, any tradeoff worth flagging>

---
```

### Closing the report phase

When the user says the list is complete (or asks to start fixing), commit **all reported entries in
a single commit**, doc only:

- One issue: `docs(tui-issue): <issue title>`
- Several issues: `docs(tui-issue): Report QA findings TUI-A..TUI-B` with one line per issue in the
  body (`- TUI-A: <issue title> (#G)`).

Do not use closing keywords (`Closes`, `Fixes`, `Resolves #G`) in this commit.

## Phase 2 — Fix

Work through the open issues in the order the user chooses (default: the order reported, critical
first). **One commit per issue.** For each:

1. Implement the fix in `src/`, add or adjust tests where the change is testable.
2. Run the verification suite (see Build & commit mechanics). All four must pass.
3. **Update the tracking document in the same commit:**
   - Move the entry from its open section to the top of `Resolved Issues`. Restore the
     `_No open ... issues._` placeholder if the section becomes empty.
   - `Status` → `🟢 Resolved`, add `- **Resolved:** DD-MM-YYYY` after `Reported`.
   - Rewrite `Description`/`Actual Behavior` in past tense (it _crashed_, it _showed_).
   - Rename `Proposed Fix` to `Fix` and rewrite it to describe what was actually done.
   - Add `- **Commit:** \`<the commit subject>\`` as the last bullet.
   - Update `Last Updated` and append a `Resolved: ...` row to the `Change Log`.
   - Keep the `GitHub` bullet as is.
4. **Check related documentation and update it in the same commit** when the change affects it:
   - `README.md` and `docs/guides/TUI_USAGE_GUIDE.md` — keys, statuses, flags, footer/help text
   - `docs/guides/MIGRATION.md` — behavior comparisons with the bash script
   - `docs/testing/TESTING.md` — add a manual check for the fixed scenario
   - `docs/specs/*.md` — if the architecture, search syntax, or PR integration changed
   - The in-app help modal (`i`) if a shortcut or status was added or renamed

   Skim these with grep for the affected key/status/flag name; do not rewrite unrelated text.

5. Commit: `fix(tui): <what changed>` for bugs, `feat(tui): <what changed>` for enhancements. Body
   starts with `Resolves TUI-N.` followed by a short root-cause / approach paragraph, and ends with
   a `Closes #G` line so GitHub closes the issue when the commit reaches `main`.
6. **Update the GitHub issue description** to match the resolved tracker entry (see GitHub issue
   mechanics). Do not close the issue: the `Closes #G` line does that on merge.
7. Report to the user: what was fixed, test results, which docs were touched, and that the GitHub
   issue description was updated. Then move to the next issue.

If the user asks for a pull request, list `Closes #G` for every issue resolved on the branch in the
PR description too, so the issues still close if the PR is squash-merged with an edited message.

## GitHub issue mechanics

- **Title:** `TUI-N: <Short imperative title>`, the same title as the tracker heading.
- **Label:** `bug` for `/ Bug` entries, `enhancement` for `/ Enhancement` entries.
- **Body:** the tracker entry's bullets from `Reported` onward, copied verbatim, minus the `Status`
  and `GitHub` bullets (GitHub tracks state itself). End the body with a footer:

  ```markdown
  ---

  Tracked as TUI-N in
  [`ISSUES_AND_FEATURES.md`](https://github.com/<owner>/<repo>/blob/main/rust/local-git-branch-cleanup-tui/docs/ISSUES_AND_FEATURES.md).
  ```

- Write the body to a file in the scratchpad directory and pass it with `--body-file`, never inline
  `--body` (backticks and `$` in the entry break shell quoting).
- Create: `gh issue create --title "..." --label <label> --body-file <file>`. It prints the issue
  URL; `G` is the number at its end.
- Update after a fix: rebuild the body from the resolved entry, then
  `gh issue edit G --body-file <file>`. Change the title too (`--title`) if the tracker title
  changed.
- If `gh` fails (not authenticated, no network), still record the tracker entry, set the bullet to
  `- **GitHub:** _not created_`, tell the user the error, and retry before committing.

## Build & commit mechanics

- `cargo` is not on PATH. Run from the repo root:
  `nix develop --command bash -c "cd rust && cargo <cmd>"`
- Verification suite (all from `rust/`): `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `cargo test --workspace --all-features`, `cargo build --workspace --release`
- Commit via `nix develop --command git commit ...` so hook tools resolve.
- The pre-commit hook reformats markdown (reflows to ~100 columns). The first commit attempt that
  touches `.md` files usually fails because the hook rewrote them: `git add` the files again and
  re-run the same commit.
- Never amend or rewrite a commit that is already pushed.
- In commit messages and PR text, refer to tracker issues as `TUI-N` and to GitHub issues as `#G`.
  Never write a tracker number as `#N`: GitHub autolinks it to whatever GitHub issue or PR has that
  number. The tracker document itself keeps `Issue #N` headings.
- Entries reported before GitHub mirroring was added have no `GitHub` bullet. Do not backfill them
  unless the user asks.

## Style rules for the document

- Match the surrounding entries exactly: bullet keys in bold, two-space nested bullets, `---`
  separators between entries, conventional-commit subjects (no emojis) in backticks.
- Keep the `Change Log` table aligned (the markdown formatter will realign it, but write it aligned
  anyway).
- Do not renumber, reorder, or edit existing entries beyond what the current issue requires.
