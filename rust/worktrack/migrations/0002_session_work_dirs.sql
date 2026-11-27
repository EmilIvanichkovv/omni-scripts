-- Directories a session worked in (cwd values, edited files' directories, `cd` / `git -C`
-- targets of Bash commands), as a JSON array. NULL = not collected yet: ingest backfills it
-- from the bytes already parsed.
ALTER TABLE sessions ADD COLUMN work_dirs TEXT;

-- Worktree roots already resolved to worked-in links for the session (JSON array), so a
-- worktree that later moves to another branch does not add a second link.
ALTER TABLE sessions ADD COLUMN work_worktrees TEXT;

-- Worktree root a link came from when it was made from a work dir; NULL for links from the
-- session's own cwd and gitBranch values.
ALTER TABLE session_links ADD COLUMN via TEXT;
