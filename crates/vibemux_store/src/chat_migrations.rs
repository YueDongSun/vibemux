//! Unified store schema 10-15: the chat family from PR #49 and the worker
//! reports table from PR #52.
//!
//! PR #49 used versions 10-14 for chat and PR #52 used version 10 for
//! `workflow_worker_reports`. The unified lineage keeps chat at 10-14 and
//! moves reports to 15. A database created by PR #52 already carries marker
//! 10 and the reports table, so its reconciliation applies the chat DDL
//! without a second marker 10 and later inserts only marker 15.

pub(crate) const MIGRATION_V10_CHAT_TABLES: &str = r#"
CREATE TABLE chat_sessions(
 chat_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN
 ('opening','ready','busy','suspended','recovery_required','closed','archived')),
 generation INTEGER NOT NULL CHECK(generation>0), version INTEGER NOT NULL CHECK(version>0),
 resource_key TEXT NOT NULL, owner_fence TEXT, record_json TEXT NOT NULL,
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence),
 CHECK(CASE WHEN state IN ('ready','busy') THEN owner_fence IS NOT NULL
 WHEN state IN ('suspended','closed','archived') THEN owner_fence IS NULL ELSE 1 END)
);
CREATE TABLE chat_turns(
 request_id TEXT PRIMARY KEY, chat_id TEXT NOT NULL REFERENCES chat_sessions(chat_id),
 turn_id TEXT NOT NULL UNIQUE, task_id TEXT NOT NULL UNIQUE, run_id TEXT NOT NULL UNIQUE,
 generation INTEGER NOT NULL CHECK(generation>0), version INTEGER NOT NULL CHECK(version>0),
 phase TEXT NOT NULL CHECK(phase IN ('admitted','running','cancel_requested','completed','failed','cancelled','interrupted','recovery_required')),
 resource_key TEXT NOT NULL, send_claimed INTEGER NOT NULL CHECK(send_claimed IN (0,1)),send_fence TEXT,
 binding_json TEXT NOT NULL, finish_json TEXT, record_json TEXT NOT NULL,
 admitted_sequence INTEGER NOT NULL REFERENCES events(sequence),
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE UNIQUE INDEX chat_active_turn ON chat_turns(chat_id)
 WHERE phase IN ('admitted','running','cancel_requested','recovery_required');
CREATE UNIQUE INDEX chat_active_resource ON chat_turns(resource_key)
 WHERE phase IN ('admitted','running','cancel_requested','recovery_required');
CREATE TABLE chat_content(
 blob_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_sessions(chat_id),sha256 TEXT NOT NULL,
 byte_count INTEGER NOT NULL CHECK(byte_count>0),state TEXT NOT NULL CHECK(state IN ('present','released'))
);
CREATE TABLE chat_records(
 record_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_sessions(chat_id),
 turn_id TEXT NOT NULL REFERENCES chat_turns(turn_id),sequence INTEGER NOT NULL CHECK(sequence>0),
 kind TEXT NOT NULL,binding_json TEXT NOT NULL,record_json TEXT NOT NULL,
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence),UNIQUE(chat_id,sequence)
);
CREATE TABLE chat_operations(
 operation_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_sessions(chat_id),
 kind TEXT NOT NULL,binding_json TEXT NOT NULL,released_json TEXT NOT NULL,
 sequence INTEGER NOT NULL REFERENCES events(sequence)
);
"#;

pub(crate) const MIGRATION_V11: &str = r#"
CREATE TABLE chat_execution_owners(
 chat_id TEXT PRIMARY KEY REFERENCES chat_sessions(chat_id),
 project_id TEXT NOT NULL, task_id TEXT NOT NULL UNIQUE, run_id TEXT NOT NULL UNIQUE,
 phase TEXT NOT NULL CHECK(phase IN ('provisioning','open','recovery_required','finalized')),
 version INTEGER NOT NULL CHECK(version>0), resource_key TEXT,
 binding_json TEXT NOT NULL, record_json TEXT NOT NULL,
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE UNIQUE INDEX chat_owner_workspace_reservation ON chat_execution_owners(resource_key)
 WHERE phase!='finalized';
INSERT INTO schema_migrations(version,applied_at) VALUES(11,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
"#;

pub(crate) const MIGRATION_V12: &str = r#"
CREATE TABLE chat_execution_attempts(
 request_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_execution_owners(chat_id),
 attempt_id TEXT NOT NULL UNIQUE,generation INTEGER NOT NULL CHECK(generation>0),
 version INTEGER NOT NULL CHECK(version>0),phase TEXT NOT NULL CHECK(phase IN
 ('admitted','running','cancel_requested','completed','failed','cancelled','interrupted','recovery_required')),
 send_claimed INTEGER NOT NULL CHECK(send_claimed IN(0,1)),send_fence TEXT,
 binding_json TEXT NOT NULL,finish_json TEXT,record_json TEXT NOT NULL,
 admitted_sequence INTEGER NOT NULL REFERENCES events(sequence),updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE UNIQUE INDEX chat_execution_active_attempt ON chat_execution_attempts(chat_id)
 WHERE phase IN('admitted','running','cancel_requested','recovery_required');
CREATE TABLE chat_execution_records(
 record_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_execution_owners(chat_id),
 attempt_id TEXT NOT NULL REFERENCES chat_execution_attempts(attempt_id),sequence INTEGER NOT NULL CHECK(sequence>0),
 kind TEXT NOT NULL,binding_json TEXT NOT NULL,record_json TEXT NOT NULL,
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence),UNIQUE(chat_id,sequence)
);
INSERT INTO schema_migrations(version,applied_at) VALUES(12,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
"#;

pub(crate) const MIGRATION_V13: &str = r#"
CREATE TABLE chat_approvals(
 approval_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_execution_owners(chat_id),
 request_id TEXT NOT NULL REFERENCES chat_execution_attempts(request_id),
 generation INTEGER NOT NULL CHECK(generation>0),version INTEGER NOT NULL CHECK(version>0),
 vendor_request_key TEXT NOT NULL,tool_use_key TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN('pending','decided','expired','invalidated')),
 delivery TEXT CHECK(delivery IN('claimed','written','unknown')),claim_id TEXT UNIQUE,
 decision_operation_id TEXT UNIQUE REFERENCES chat_operations(operation_id),
 binding_json TEXT NOT NULL,decision_json TEXT,record_json TEXT NOT NULL,
 admitted_sequence INTEGER NOT NULL REFERENCES events(sequence),updated_sequence INTEGER NOT NULL REFERENCES events(sequence),
 UNIQUE(chat_id,generation,vendor_request_key),UNIQUE(chat_id,generation,tool_use_key),
 CHECK((state='decided' AND delivery IS NOT NULL AND claim_id IS NOT NULL AND decision_operation_id IS NOT NULL AND decision_json IS NOT NULL)
 OR (state!='decided' AND delivery IS NULL AND claim_id IS NULL AND decision_operation_id IS NULL AND decision_json IS NULL))
);
CREATE UNIQUE INDEX chat_approval_pending ON chat_approvals(request_id)
 WHERE state='pending' OR delivery IN('claimed','unknown');
INSERT INTO schema_migrations(version,applied_at) VALUES(13,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
"#;

pub(crate) const MIGRATION_V14: &str = r#"
CREATE TABLE workspace_cleanups(
 cleanup_id TEXT PRIMARY KEY,chat_id TEXT NOT NULL REFERENCES chat_execution_owners(chat_id),
 run_id TEXT NOT NULL,resource_key TEXT NOT NULL,worktree_path_key TEXT NOT NULL,branch_key TEXT NOT NULL,
 phase TEXT NOT NULL CHECK(phase IN('planned','claimed','armed','quiescent','removed','refused','unknown')),
 version INTEGER NOT NULL CHECK(version>0),record_json TEXT NOT NULL,
 updated_sequence INTEGER NOT NULL REFERENCES events(sequence)
);
CREATE UNIQUE INDEX workspace_cleanup_resource ON workspace_cleanups(resource_key)
 WHERE phase IN('claimed','armed','quiescent','unknown');
CREATE UNIQUE INDEX workspace_cleanup_path ON workspace_cleanups(worktree_path_key)
 WHERE phase IN('claimed','armed','quiescent','unknown');
CREATE UNIQUE INDEX workspace_cleanup_branch ON workspace_cleanups(branch_key)
 WHERE phase IN('claimed','armed','quiescent','unknown');
CREATE INDEX workspace_cleanup_run ON workspace_cleanups(run_id);
CREATE TABLE workspace_cleanup_commands(
 operation_id TEXT PRIMARY KEY REFERENCES chat_operations(operation_id),
 cleanup_id TEXT NOT NULL REFERENCES workspace_cleanups(cleanup_id)
);
INSERT INTO schema_migrations(version,applied_at) VALUES(14,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
"#;

/// PR #52's worker report table, renumbered to the unified version 15.
pub(crate) const MIGRATION_V15_REPORTS: &str = r#"
CREATE TABLE workflow_worker_reports (
    request_id TEXT PRIMARY KEY REFERENCES workflow_attempts(request_id),
    workflow_id TEXT NOT NULL REFERENCES workflows(workflow_id),
    record_json TEXT NOT NULL,
    created_sequence INTEGER NOT NULL UNIQUE REFERENCES events(sequence)
);
CREATE INDEX workflow_worker_reports_page ON workflow_worker_reports(workflow_id, created_sequence);
INSERT INTO schema_migrations(version, applied_at)
VALUES (15, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
"#;
