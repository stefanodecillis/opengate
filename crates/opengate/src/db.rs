use rusqlite::Connection;

pub fn init_db(path: &str) -> Connection {
    let conn = Connection::open(path).expect("Failed to open database");

    // Enable WAL mode for concurrent reads
    conn.execute_batch("PRAGMA journal_mode=WAL;")
        .expect("Failed to enable WAL mode");

    // Checkpoint any pending WAL data before running migrations.
    // This prevents data loss when upgrading the binary (old WAL + new schema = bad).
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .expect("Failed to checkpoint WAL");

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            status TEXT NOT NULL DEFAULT 'active',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL REFERENCES projects(id),
            title TEXT NOT NULL,
            description TEXT,
            status TEXT NOT NULL DEFAULT 'backlog',
            priority TEXT NOT NULL DEFAULT 'medium',
            assignee_type TEXT,
            assignee_id TEXT,
            context TEXT,
            output TEXT,
            due_date TEXT,
            created_by TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS task_tags (
            task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            tag TEXT NOT NULL,
            PRIMARY KEY (task_id, tag)
        );

        CREATE TABLE IF NOT EXISTS task_activity (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL REFERENCES tasks(id),
            author_type TEXT NOT NULL,
            author_id TEXT NOT NULL,
            content TEXT NOT NULL,
            activity_type TEXT NOT NULL,
            metadata TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS agents (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            api_key_hash TEXT NOT NULL UNIQUE,
            skills TEXT,
            last_seen_at TEXT,
            created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_tasks_project_id ON tasks(project_id);
        CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
        CREATE INDEX IF NOT EXISTS idx_tasks_assignee_id ON tasks(assignee_id);
        CREATE INDEX IF NOT EXISTS idx_task_activity_task_id ON task_activity(task_id);
        CREATE INDEX IF NOT EXISTS idx_agents_api_key_hash ON agents(api_key_hash);
        CREATE INDEX IF NOT EXISTS idx_task_tags_task_id ON task_tags(task_id);
        CREATE INDEX IF NOT EXISTS idx_task_tags_tag ON task_tags(tag);

        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_type TEXT NOT NULL,
            task_id TEXT,
            project_id TEXT NOT NULL,
            actor_type TEXT NOT NULL,
            actor_id TEXT NOT NULL,
            payload TEXT,
            created_at TEXT DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_events_project ON events(project_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type, created_at);

        CREATE TABLE IF NOT EXISTS notifications (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id TEXT NOT NULL,
            event_id INTEGER REFERENCES events(id),
            event_type TEXT NOT NULL,
            title TEXT NOT NULL,
            body TEXT,
            read INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_notifications_agent ON notifications(agent_id, read, created_at);
        ",
    )
    .expect("Failed to initialize database schema");

    // Migration: add output column if upgrading from v1
    let _ = conn.execute("ALTER TABLE tasks ADD COLUMN output TEXT", []);

    // v2 migrations: agent profiles
    let _ = conn.execute("ALTER TABLE agents ADD COLUMN description TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE agents ADD COLUMN status TEXT DEFAULT 'available'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE agents ADD COLUMN max_concurrent_tasks INTEGER DEFAULT 2",
        [],
    );
    let _ = conn.execute("ALTER TABLE agents ADD COLUMN webhook_url TEXT", []);
    let _ = conn.execute("ALTER TABLE agents ADD COLUMN config TEXT", []);

    // v2.1: agent model metadata
    let _ = conn.execute("ALTER TABLE agents ADD COLUMN model TEXT", []);
    let _ = conn.execute("ALTER TABLE agents ADD COLUMN provider TEXT", []);
    let _ = conn.execute("ALTER TABLE agents ADD COLUMN cost_tier TEXT", []);
    let _ = conn.execute("ALTER TABLE agents ADD COLUMN capabilities TEXT", []);

    // v2 migrations: task reviewers
    let _ = conn.execute("ALTER TABLE tasks ADD COLUMN reviewer_type TEXT", []);
    let _ = conn.execute("ALTER TABLE tasks ADD COLUMN reviewer_id TEXT", []);

    // v3: agent seniority + role
    let _ = conn.execute(
        "ALTER TABLE agents ADD COLUMN seniority TEXT DEFAULT 'mid'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE agents ADD COLUMN role TEXT DEFAULT 'executor'",
        [],
    );

    // v2: project knowledge base
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS project_knowledge (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL REFERENCES projects(id),
            key TEXT NOT NULL,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            metadata TEXT,
            created_by_type TEXT NOT NULL,
            created_by_id TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE(project_id, key)
        );
        CREATE INDEX IF NOT EXISTS idx_knowledge_project_id ON project_knowledge(project_id);
        CREATE INDEX IF NOT EXISTS idx_knowledge_project_key ON project_knowledge(project_id, key);
        ",
    )
    .expect("Failed to create project_knowledge table");

    // v3: knowledge tags + category
    let _ = conn.execute(
        "ALTER TABLE project_knowledge ADD COLUMN tags TEXT NOT NULL DEFAULT '[]'",
        [],
    );
    let _ = conn.execute("ALTER TABLE project_knowledge ADD COLUMN category TEXT", []);

    // v2: webhook delivery log
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS webhook_log (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL REFERENCES agents(id),
            event_type TEXT NOT NULL,
            payload TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            attempts INTEGER DEFAULT 0,
            last_attempt_at TEXT,
            response_status INTEGER,
            response_body TEXT,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_webhook_log_agent_id ON webhook_log(agent_id);
        ",
    )
    .expect("Failed to create webhook_log table");

    // v4: webhook_events filter on agents (JSON array of subscribed event types)
    let _ = conn.execute("ALTER TABLE agents ADD COLUMN webhook_events TEXT", []);

    // v4: webhook_status on notifications (delivered | failed | null)
    let _ = conn.execute(
        "ALTER TABLE notifications ADD COLUMN webhook_status TEXT",
        [],
    );

    // v5: status_history on tasks — JSON array of {status, agent_id, agent_type, timestamp}
    let _ = conn.execute(
        "ALTER TABLE tasks ADD COLUMN status_history TEXT NOT NULL DEFAULT '[]'",
        [],
    );

    // v6: per-agent stale timeout (minutes)
    let _ = conn.execute(
        "ALTER TABLE agents ADD COLUMN stale_timeout INTEGER NOT NULL DEFAULT 240",
        [],
    );

    // Migrate existing agents from old 30-min default to new 4-hour default
    let _ = conn.execute(
        "UPDATE agents SET stale_timeout = 240 WHERE stale_timeout = 30",
        [],
    );

    // v7: task artifacts
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_artifacts (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            artifact_type TEXT NOT NULL,
            value TEXT NOT NULL,
            created_by_type TEXT NOT NULL,
            created_by_id TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_artifacts_task ON task_artifacts(task_id);
        ",
    )
    .expect("Failed to create task_artifacts table");

    // v8: task dependencies (join table with cycle-detection support)
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_dependencies (
            task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            depends_on TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            PRIMARY KEY (task_id, depends_on)
        );
        CREATE INDEX IF NOT EXISTS idx_deps_task_id ON task_dependencies(task_id);
        CREATE INDEX IF NOT EXISTS idx_deps_depends_on ON task_dependencies(depends_on);
        ",
    )
    .expect("Failed to create task_dependencies table");

    // v8: task scheduling — scheduled_at column
    let _ = conn.execute("ALTER TABLE tasks ADD COLUMN scheduled_at TEXT", []);

    // v8: task recurrence — recurrence_rule (JSON) + recurrence_parent_id
    let _ = conn.execute("ALTER TABLE tasks ADD COLUMN recurrence_rule TEXT", []);
    let _ = conn.execute("ALTER TABLE tasks ADD COLUMN recurrence_parent_id TEXT", []);

    // v9/v10: drop legacy pipeline tables (removed feature)
    conn.execute_batch(
        "DROP TABLE IF EXISTS pipeline_stage_tasks;
         DROP TABLE IF EXISTS pipeline_stages;
         DROP TABLE IF EXISTS pipelines;
         DROP TABLE IF EXISTS pipeline_instances;
         DROP TABLE IF EXISTS pipeline_templates;
         DROP TABLE IF EXISTS users;
        ",
    )
    .expect("Failed to drop legacy tables");

    // v10: agent cost tracking — token usage per task
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_usage (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            agent_id TEXT NOT NULL,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cost_usd REAL,
            reported_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_usage_task ON task_usage(task_id);
        CREATE INDEX IF NOT EXISTS idx_usage_agent ON task_usage(agent_id);
        ",
    )
    .expect("Failed to create task_usage table");

    // v11: inbound webhook triggers
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS webhook_triggers (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            secret_hash TEXT NOT NULL,
            action_type TEXT NOT NULL,
            action_config TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_trigger_project ON webhook_triggers(project_id);

        CREATE TABLE IF NOT EXISTS webhook_trigger_logs (
            id TEXT PRIMARY KEY,
            trigger_id TEXT NOT NULL REFERENCES webhook_triggers(id) ON DELETE CASCADE,
            received_at TEXT NOT NULL,
            status TEXT NOT NULL,
            payload TEXT,
            result TEXT,
            error TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_trigger_log_trigger ON webhook_trigger_logs(trigger_id);
        ",
    )
    .expect("Failed to create webhook trigger tables");

    // v13: task questions system
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_questions (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            question TEXT NOT NULL,
            question_type TEXT NOT NULL DEFAULT 'clarification',
            context TEXT,
            asked_by_type TEXT NOT NULL,
            asked_by_id TEXT NOT NULL,
            target_type TEXT,
            target_id TEXT,
            required_capability TEXT,
            status TEXT NOT NULL DEFAULT 'open',
            blocking INTEGER NOT NULL DEFAULT 1,
            resolved_by_type TEXT,
            resolved_by_id TEXT,
            resolution TEXT,
            created_at TEXT NOT NULL,
            resolved_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_questions_task ON task_questions(task_id);
        CREATE INDEX IF NOT EXISTS idx_questions_target ON task_questions(target_type, target_id, status);
        ",
    )
    .expect("Failed to create task_questions table");
    let _ = conn.execute(
        "ALTER TABLE tasks ADD COLUMN has_open_questions INTEGER DEFAULT 0",
        [],
    );

    // v14: question replies + dismiss fields
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS question_replies (
            id TEXT PRIMARY KEY,
            question_id TEXT NOT NULL REFERENCES task_questions(id) ON DELETE CASCADE,
            author_type TEXT NOT NULL,
            author_id TEXT NOT NULL,
            body TEXT NOT NULL,
            is_resolution INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_replies_question ON question_replies(question_id);
        ",
    )
    .expect("Failed to create question_replies table");
    let _ = conn.execute(
        "ALTER TABLE task_questions ADD COLUMN dismissed_at TEXT",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE task_questions ADD COLUMN dismissed_reason TEXT",
        [],
    );

    // v15: review load tracking — timestamp when reviewer starts reviewing
    let _ = conn.execute("ALTER TABLE tasks ADD COLUMN started_review_at TEXT", []);

    // v16: agent owner scoping
    let _ = conn.execute("ALTER TABLE agents ADD COLUMN owner_id TEXT", []);

    // v17: agent tags (JSON array)
    let _ = conn.execute(
        "ALTER TABLE agents ADD COLUMN tags TEXT NOT NULL DEFAULT '[]'",
        [],
    );

    conn
}

/// Create a SQLite-backed StorageBackend from a path.
pub fn init_sqlite_storage(path: &str) -> std::sync::Arc<dyn crate::storage::StorageBackend> {
    let conn = init_db(path);
    let conn = std::sync::Arc::new(std::sync::Mutex::new(conn));
    std::sync::Arc::new(crate::storage::sqlite::SqliteBackend::new(conn))
}
