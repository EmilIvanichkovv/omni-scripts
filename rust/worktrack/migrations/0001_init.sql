-- worktrack schema v1 (spec 10.1). Timestamps are Unix seconds UTC; JSON columns are TEXT.
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
  fetched_at INTEGER NOT NULL, closed_at INTEGER, PRIMARY KEY (repo_id, branch));

CREATE TABLE build_cache (sha TEXT PRIMARY KEY, state TEXT, fetched_at INTEGER NOT NULL);

CREATE TABLE session_files (path TEXT PRIMARY KEY, size INTEGER, mtime INTEGER, offset INTEGER);

CREATE TABLE session_queue (
  session_id TEXT NOT NULL, transcript_path TEXT, cwd TEXT, reason TEXT,
  received_at INTEGER NOT NULL);

CREATE TABLE sessions (
  id TEXT PRIMARY KEY, cwd TEXT, title TEXT, started_at INTEGER,
  last_activity_at INTEGER, ended_at INTEGER, end_reason TEXT,
  user_turns INTEGER, files_touched TEXT, explicit_todos TEXT,
  summary_status TEXT NOT NULL DEFAULT 'none', summary TEXT, summarized_at INTEGER,
  transcript_path TEXT, branches TEXT, malformed_lines INTEGER NOT NULL DEFAULT 0,
  summary_attempts INTEGER NOT NULL DEFAULT 0, summary_error TEXT);

CREATE TABLE session_links (
  session_id TEXT NOT NULL, repo_id TEXT NOT NULL, branch TEXT NOT NULL,
  PRIMARY KEY (session_id, repo_id, branch));

CREATE TABLE suggestions (
  id INTEGER PRIMARY KEY, rule TEXT NOT NULL, subject_key TEXT NOT NULL UNIQUE,
  payload TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending',
  created_at INTEGER NOT NULL, decided_at INTEGER);

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
