use axum::{
    http::StatusCode,
    routing::{delete, get, patch, post},
    Router,
};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use tower_http::cors::{Any, CorsLayer};

use crate::db;
use crate::db_ops;
use crate::handlers;
use opengate_models::CreateActivity;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub setup_token: String,
}

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let api = Router::new()
        // Auth
        .route("/api/auth/me", get(handlers::auth::me))
        // Schema
        .route("/api/schema", get(handlers::schema::get_schema))
        // Projects
        .route(
            "/api/projects",
            get(handlers::projects::list_projects).post(handlers::projects::create_project),
        )
        .route(
            "/api/projects/:id",
            get(handlers::projects::get_project)
                .patch(handlers::projects::update_project)
                .delete(handlers::projects::archive_project),
        )
        // Pulse
        .route(
            "/api/projects/:id/pulse",
            get(handlers::projects::get_pulse),
        )
        // v4: Schedule
        .route(
            "/api/projects/:id/schedule",
            get(handlers::projects::get_schedule),
        )
        // Project Questions
        .route(
            "/api/projects/:id/questions",
            get(handlers::questions::project_questions),
        )
        // Tasks - project scoped
        .route(
            "/api/projects/:id/tasks",
            get(handlers::tasks::list_tasks_by_project).post(handlers::tasks::create_task),
        )
        // Tasks - global
        .route("/api/tasks", get(handlers::tasks::list_tasks_global))
        .route("/api/tasks/mine", get(handlers::tasks::my_tasks))
        .route("/api/tasks/next", get(handlers::tasks::next_task))
        .route(
            "/api/tasks/batch/status",
            post(handlers::tasks::batch_status),
        )
        .route(
            "/api/tasks/:id",
            get(handlers::tasks::get_task)
                .patch(handlers::tasks::update_task)
                .delete(handlers::tasks::delete_task),
        )
        .route(
            "/api/tasks/:id/context",
            patch(handlers::tasks::update_context),
        )
        .route("/api/tasks/:id/claim", post(handlers::tasks::claim_task))
        .route(
            "/api/tasks/:id/release",
            post(handlers::tasks::release_task),
        )
        .route(
            "/api/tasks/:id/complete",
            post(handlers::tasks::complete_task),
        )
        .route("/api/tasks/:id/block", post(handlers::tasks::block_task))
        // v2: Assignment, handoff, review
        .route("/api/tasks/:id/assign", post(handlers::tasks::assign_task))
        .route(
            "/api/tasks/:id/handoff",
            post(handlers::tasks::handoff_task),
        )
        .route(
            "/api/tasks/:id/approve",
            post(handlers::tasks::approve_task),
        )
        .route(
            "/api/tasks/:id/request-changes",
            post(handlers::tasks::request_changes),
        )
        .route(
            "/api/tasks/:id/submit-review",
            post(handlers::tasks::submit_review),
        )
        .route(
            "/api/tasks/:id/start-review",
            post(handlers::tasks::start_review),
        )
        // v4: Dependencies
        .route(
            "/api/tasks/:id/dependencies",
            get(handlers::tasks::list_dependencies).post(handlers::tasks::add_dependencies),
        )
        .route(
            "/api/tasks/:id/dependencies/:dep_id",
            delete(handlers::tasks::remove_dependency),
        )
        .route(
            "/api/tasks/:id/dependents",
            get(handlers::tasks::list_dependents),
        )
        // v4: Scheduled task auto-transition (manual trigger)
        .route(
            "/api/tasks/scheduled/transition",
            post(handlers::tasks::trigger_scheduled_transition),
        )
        // Questions
        .route(
            "/api/tasks/:id/questions",
            get(handlers::questions::list_questions).post(handlers::questions::create_question),
        )
        .route(
            "/api/tasks/:id/questions/:qid",
            get(handlers::questions::get_question),
        )
        .route(
            "/api/tasks/:id/questions/:qid/resolve",
            post(handlers::questions::resolve_question),
        )
        .route(
            "/api/tasks/:id/questions/:qid/replies",
            get(handlers::questions::list_replies).post(handlers::questions::create_reply),
        )
        .route(
            "/api/tasks/:id/questions/:qid/dismiss",
            post(handlers::questions::dismiss_question),
        )
        .route(
            "/api/tasks/:id/questions/:qid/assign",
            post(handlers::questions::assign_question),
        )
        // Activity
        .route(
            "/api/tasks/:id/activity",
            get(handlers::activity::list_activity).post(handlers::activity::create_activity),
        )
        // Usage tracking
        .route("/api/tasks/:id/usage", get(handlers::usage::get_task_usage).post(handlers::usage::report_usage))
        .route("/api/projects/:id/usage", get(handlers::usage::get_project_usage))
        .route("/api/agents/:id/usage", get(handlers::usage::get_agent_usage_range))
        // Artifacts
        .route(
            "/api/tasks/:id/artifacts",
            get(handlers::artifacts::list_artifacts).post(handlers::artifacts::create_artifact),
        )
        .route(
            "/api/tasks/:id/artifacts/:artifact_id",
            delete(handlers::artifacts::delete_artifact),
        )
        // Agents
        .route(
            "/api/agents",
            get(handlers::agents::list_agents).post(handlers::agents::create_agent),
        )
        .route(
            "/api/agents/register",
            post(handlers::agents::register_agent),
        )
        .route(
            "/api/agents/match",
            get(handlers::agents::match_best_agent),
        )
        .route(
            "/api/agents/:id",
            get(handlers::agents::get_agent)
                .patch(handlers::agents::update_agent)
                .delete(handlers::agents::delete_agent),
        )
        .route("/api/agents/heartbeat", post(handlers::agents::heartbeat))
        .route(
            "/api/agents/me/inbox",
            get(handlers::agents::inbox),
        )
        .route(
            "/api/agents/me/questions",
            get(handlers::questions::my_questions),
        )
        .route(
            "/api/agents/me/notifications",
            get(handlers::agents::my_notifications),
        )
        .route(
            "/api/agents/me/notifications/:id/ack",
            post(handlers::agents::ack_notification),
        )
        .route(
            "/api/agents/me/notifications/ack-all",
            post(handlers::agents::ack_all_notifications),
        )
        // Knowledge base
        .route(
            "/api/projects/:id/knowledge",
            get(handlers::knowledge::list_knowledge),
        )
        .route(
            "/api/projects/:id/knowledge/search",
            get(handlers::knowledge::search_knowledge),
        )
        .route(
            "/api/projects/:id/knowledge/*key",
            get(handlers::knowledge::get_knowledge)
                .put(handlers::knowledge::upsert_knowledge)
                .delete(handlers::knowledge::delete_knowledge),
        )
        // Stats
        .route("/api/stats", get(handlers::stats::get_stats))
        // v4: Inbound webhook triggers (management — require auth)
        .route(
            "/api/projects/:id/triggers",
            get(handlers::triggers::list_triggers).post(handlers::triggers::create_trigger),
        )
        .route(
            "/api/projects/:id/triggers/:tid",
            delete(handlers::triggers::delete_trigger),
        )
        .route(
            "/api/projects/:id/triggers/:tid/logs",
            get(handlers::triggers::list_trigger_logs),
        )
        // v4: Inbound webhook receiver (no auth — secret-validated)
        .route(
            "/api/webhooks/trigger/:trigger_id",
            post(handlers::triggers::receive_webhook),
        );

    api.fallback(|| async { (StatusCode::NOT_FOUND, "Not found") })
        .layer(cors)
        .with_state(state)
}

pub async fn run_server(port: u16, db_path: &str, setup_token: &str) {
    let conn = db::init_db(db_path);
    let state = AppState {
        db: Arc::new(Mutex::new(conn)),
        setup_token: setup_token.to_string(),
    };

    // Spawn background stale agent cleanup (with startup grace period)
    let bg_state = state.clone();
    tokio::spawn(async move {
        // Grace period: wait 5 minutes after startup before first stale check
        // This prevents false positives when agents haven't had time to heartbeat after a restart
        tokio::time::sleep(tokio::time::Duration::from_secs(300)).await;
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            let conn = bg_state.db.lock().unwrap();
            let released = db_ops::release_stale_tasks(&conn, 30);
            for task in &released {
                eprintln!(
                    "[cleanup] Auto-released stale task: {} ({})",
                    task.id, task.title
                );
                db_ops::create_activity(
                    &conn,
                    &task.id,
                    "system",
                    "system",
                    &CreateActivity {
                        content: "Task auto-released due to stale agent heartbeat".to_string(),
                        activity_type: Some("assignment".to_string()),
                        metadata: None,
                    },
                );
            }
        }
    });

    // Spawn background scheduled-task promoter
    {
        let db2 = state.db.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                let conn = db2.lock().unwrap();
                let count = db_ops::transition_ready_scheduled_tasks(&conn);
                if count > 0 {
                    eprintln!("[scheduler] Promoted {} scheduled task(s) backlog→todo", count);
                }
            }
        });
    }

    // Graceful shutdown: checkpoint WAL on SIGTERM/SIGINT
    let shutdown_db = state.db.clone();
    let shutdown_signal = async move {
        let ctrl_c = tokio::signal::ctrl_c();
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => eprintln!("[shutdown] Received SIGINT"),
            _ = sigterm.recv() => eprintln!("[shutdown] Received SIGTERM"),
        }
        // Checkpoint WAL before exit to prevent data loss on restart
        let conn = shutdown_db.lock().unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap_or_else(|e| eprintln!("[shutdown] WAL checkpoint failed: {}", e));
        eprintln!("[shutdown] WAL checkpointed, shutting down gracefully");
    };

    let router = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .expect("Failed to bind port");

    eprintln!("TaskForge listening on http://0.0.0.0:{}", port);

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal)
        .await
        .expect("Server error");
}

