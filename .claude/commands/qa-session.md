---
description:
  Run a QA session for local-git-branch-cleanup-tui — record reported bugs/features in
  ISSUES_AND_FEATURES.md, then fix them one by one
argument-hint: [optional context about what is being tested]
---

# QA Session — local-git-branch-cleanup-tui

You are running a QA session with the user. The user reports bugs and feature requests one at a
time; you record them, and only after the list is complete do you start fixing.

Tracking document: `rust/local-git-branch-cleanup-tui/docs/ISSUES_AND_FEATURES.md` Project root:
`rust/local-git-branch-cleanup-tui/`

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
4. **Update `Last Updated`** at the top (`YYYY-MM-DD HH:MM`, local time) and append a
   `Reported: ...` row to the `Change Log` table at the bottom.
5. **Do not touch source code** in this phase. Do not commit yet.
6. Confirm to the user in one or two lines what you recorded, then wait for the next item.

Entry template (open):

```markdown
### Issue #N: <Short imperative title>

- **Status:** 🔴 Open
- **Reported:** YYYY-MM-DD
- **Category:** <Critical|UI/UX|Performance|Minor/Cosmetic> / <Bug|Enhancement>
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
  body (`- TUI-A: <issue title>`).

## Phase 2 — Fix

Work through the open issues in the order the user chooses (default: the order reported, critical
first). **One commit per issue.** For each:

1. Implement the fix in `src/`, add or adjust tests where the change is testable.
2. Run the verification suite (see Build & commit mechanics). All four must pass.
3. **Update the tracking document in the same commit:**
   - Move the entry from its open section to the top of `Resolved Issues`. Restore the
     `_No open ... issues._` placeholder if the section becomes empty.
   - `Status` → `🟢 Resolved`, add `- **Resolved:** YYYY-MM-DD` after `Reported`.
   - Rewrite `Description`/`Actual Behavior` in past tense (it _crashed_, it _showed_).
   - Rename `Proposed Fix` to `Fix` and rewrite it to describe what was actually done.
   - Add `- **Commit:** \`<the commit subject>\`` as the last bullet.
   - Update `Last Updated` and append a `Resolved: ...` row to the `Change Log`.
4. **Check related documentation and update it in the same commit** when the change affects it:
   - `README.md` and `docs/guides/TUI_USAGE_GUIDE.md` — keys, statuses, flags, footer/help text
   - `docs/guides/MIGRATION.md` — behavior comparisons with the bash script
   - `docs/testing/TESTING.md` — add a manual check for the fixed scenario
   - `docs/specs/*.md` — if the architecture, search syntax, or PR integration changed
   - The in-app help modal (`i`) if a shortcut or status was added or renamed

   Skim these with grep for the affected key/status/flag name; do not rewrite unrelated text.

5. Commit: `fix(tui): <what changed>` for bugs, `feat(tui): <what changed>` for enhancements. Body
   starts with `Resolves TUI-N.` followed by a short root-cause / approach paragraph.
6. Report to the user: what was fixed, test results, which docs were touched. Then move to the next
   issue.

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
- In commit messages and PR text, refer to tracker issues as `TUI-N`, never `#N`: GitHub autolinks
  `#N` to the repo's own GitHub issues/PRs, which are unrelated. The tracker document itself keeps
  `Issue #N` headings.

## Style rules for the document

- Match the surrounding entries exactly: bullet keys in bold, two-space nested bullets, `---`
  separators between entries, conventional-commit subjects (no emojis) in backticks.
- Keep the `Change Log` table aligned (the markdown formatter will realign it, but write it aligned
  anyway).
- Do not renumber, reorder, or edit existing entries beyond what the current issue requires.
