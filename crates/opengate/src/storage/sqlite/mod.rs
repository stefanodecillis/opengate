use opengate_models::*;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

use crate::db_ops;
use crate::storage::*;

/// SQLite-backed storage implementation.
/// Wraps a `Mutex<Connection>` and delegates to existing `db_ops` functions.
/// The `tenant` parameter is ignored in single-tenant (OSS) mode.
pub struct SqliteBackend {
    pub conn: Arc<Mutex<Connection>>,
}

impl SqliteBackend {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }
}

impl ProjectStore for SqliteBackend {
    fn create_project(
        &self,
        _tenant: Option<&str>,
        input: &CreateProject,
        created_by: &str,
    ) -> Project {
        db_ops::create_project(&self.lock(), _tenant, input, created_by)
    }
    fn get_project(&self, _tenant: Option<&str>, id: &str) -> Option<Project> {
        db_ops::get_project(&self.lock(), _tenant, id)
    }
    fn list_projects(&self, _tenant: Option<&str>, status_filter: Option<&str>) -> Vec<Project> {
        db_ops::list_projects(&self.lock(), _tenant, status_filter)
    }
    fn update_project(
        &self,
        _tenant: Option<&str>,
        id: &str,
        input: &UpdateProject,
    ) -> Option<Project> {
        db_ops::update_project(&self.lock(), _tenant, id, input)
    }
    fn archive_project(&self, _tenant: Option<&str>, id: &str) -> bool {
        db_ops::archive_project(&self.lock(), _tenant, id)
    }
    fn get_project_with_stats(&self, _tenant: Option<&str>, id: &str) -> Option<ProjectWithStats> {
        db_ops::get_project_with_stats(&self.lock(), _tenant, id)
    }
    fn get_schedule(
        &self,
        _tenant: Option<&str>,
        project_id: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Vec<ScheduledTaskEntry> {
        db_ops::get_schedule(&self.lock(), _tenant, project_id, from, to)
    }
    fn get_pulse(
        &self,
        _tenant: Option<&str>,
        project_id: &str,
        caller_agent_id: Option<&str>,
    ) -> PulseResponse {
        db_ops::get_pulse(&self.lock(), _tenant, project_id, caller_agent_id)
    }
}

impl TaskStore for SqliteBackend {
    fn create_task(
        &self,
        _tenant: Option<&str>,
        project_id: &str,
        input: &CreateTask,
        created_by: &str,
    ) -> Task {
        db_ops::create_task(&self.lock(), _tenant, project_id, input, created_by)
    }
    fn get_task(&self, _tenant: Option<&str>, id: &str) -> Option<Task> {
        db_ops::get_task(&self.lock(), _tenant, id)
    }
    fn list_tasks(&self, _tenant: Option<&str>, filters: &TaskFilters) -> Vec<Task> {
        db_ops::list_tasks(&self.lock(), _tenant, filters)
    }
    fn update_task(
        &self,
        _tenant: Option<&str>,
        id: &str,
        input: &UpdateTask,
    ) -> Result<Option<Task>, StorageError> {
        db_ops::update_task(&self.lock(), _tenant, id, input).map_err(StorageError)
    }
    fn delete_task(&self, _tenant: Option<&str>, id: &str) -> bool {
        db_ops::delete_task(&self.lock(), _tenant, id)
    }
    fn claim_task(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        agent_id: &str,
        agent_name: &str,
    ) -> Result<Task, StorageError> {
        db_ops::claim_task(&self.lock(), _tenant, task_id, agent_id, agent_name)
            .map_err(StorageError)
    }
    fn release_task(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        agent_id: &str,
    ) -> Result<Task, StorageError> {
        db_ops::release_task(&self.lock(), _tenant, task_id, agent_id).map_err(StorageError)
    }
    fn get_next_task(&self, _tenant: Option<&str>, skills: &[String]) -> Option<Task> {
        db_ops::get_next_task(&self.lock(), _tenant, skills)
    }
    fn get_tasks_for_assignee(&self, _tenant: Option<&str>, assignee_id: &str) -> Vec<Task> {
        db_ops::get_tasks_for_assignee(&self.lock(), _tenant, assignee_id)
    }
    fn merge_context(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        patch: &serde_json::Value,
    ) -> Result<Option<Task>, StorageError> {
        db_ops::merge_context(&self.lock(), _tenant, task_id, patch).map_err(StorageError)
    }
    fn batch_update_status(
        &self,
        _tenant: Option<&str>,
        updates: &[(String, String)],
    ) -> BatchResult {
        db_ops::batch_update_status(&self.lock(), _tenant, updates)
    }
    fn release_stale_tasks(
        &self,
        _tenant: Option<&str>,
        default_timeout_minutes: i64,
    ) -> Vec<Task> {
        db_ops::release_stale_tasks(&self.lock(), default_timeout_minutes)
    }
    fn transition_ready_scheduled_tasks(&self, _tenant: Option<&str>) -> usize {
        db_ops::transition_ready_scheduled_tasks(&self.lock())
    }
    fn create_next_recurrence(
        &self,
        _tenant: Option<&str>,
        completed_task: &Task,
    ) -> Option<String> {
        db_ops::create_next_recurrence(&self.lock(), completed_task)
    }
    fn append_status_history(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        new_status: &str,
        agent_type: Option<&str>,
        agent_id: Option<&str>,
    ) {
        db_ops::append_status_history(&self.lock(), task_id, new_status, agent_type, agent_id)
    }
    fn check_dependencies(&self, _tenant: Option<&str>, task: &Task) -> Result<(), Vec<String>> {
        db_ops::check_dependencies(&self.lock(), _tenant, task)
    }
    fn add_dependency(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        depends_on_id: &str,
    ) -> Result<(), StorageError> {
        db_ops::add_dependency(&self.lock(), _tenant, task_id, depends_on_id).map_err(StorageError)
    }
    fn remove_dependency(&self, _tenant: Option<&str>, task_id: &str, depends_on_id: &str) -> bool {
        db_ops::remove_dependency(&self.lock(), _tenant, task_id, depends_on_id)
    }
    fn get_task_dependencies(&self, _tenant: Option<&str>, task_id: &str) -> Vec<Task> {
        db_ops::get_task_dependencies(&self.lock(), _tenant, task_id)
    }
    fn get_task_dependents(&self, _tenant: Option<&str>, task_id: &str) -> Vec<Task> {
        db_ops::get_task_dependents(&self.lock(), _tenant, task_id)
    }
    fn unblock_dependents_on_complete(
        &self,
        _tenant: Option<&str>,
        completed_task_id: &str,
    ) -> Vec<PendingNotifWebhook> {
        db_ops::unblock_dependents_on_complete(&self.lock(), _tenant, completed_task_id)
    }
    fn all_dependencies_done(&self, _tenant: Option<&str>, task: &Task) -> bool {
        db_ops::all_dependencies_done(&self.lock(), _tenant, task)
    }
    fn inject_upstream_outputs(&self, _tenant: Option<&str>, completed_task: &Task) {
        db_ops::inject_upstream_outputs(&self.lock(), _tenant, completed_task)
    }
    fn assign_task(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        agent_id: &str,
    ) -> Result<Task, StorageError> {
        db_ops::assign_task(&self.lock(), _tenant, task_id, agent_id).map_err(StorageError)
    }
    fn handoff_task(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        from_agent_id: &str,
        to_agent_id: &str,
        summary: Option<&str>,
    ) -> Result<Task, StorageError> {
        db_ops::handoff_task(
            &self.lock(),
            _tenant,
            task_id,
            from_agent_id,
            to_agent_id,
            summary,
        )
        .map_err(StorageError)
    }
    fn approve_task(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        reviewer_id: &str,
        comment: Option<&str>,
    ) -> Result<Task, StorageError> {
        db_ops::approve_task(&self.lock(), _tenant, task_id, reviewer_id, comment)
            .map_err(StorageError)
    }
    fn request_changes(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        reviewer_id: &str,
        comment: &str,
    ) -> Result<Task, StorageError> {
        db_ops::request_changes(&self.lock(), _tenant, task_id, reviewer_id, comment)
            .map_err(StorageError)
    }
    fn submit_review_task(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        submitter_id: &str,
        summary: Option<&str>,
        explicit_reviewer_id: Option<&str>,
    ) -> Result<Task, StorageError> {
        db_ops::submit_review_task(
            &self.lock(),
            _tenant,
            task_id,
            submitter_id,
            summary,
            explicit_reviewer_id,
        )
        .map_err(StorageError)
    }
    fn start_review_task(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        caller_id: &str,
        caller_type: &str,
    ) -> Result<Task, StorageError> {
        db_ops::start_review_task(&self.lock(), _tenant, task_id, caller_id, caller_type)
            .map_err(StorageError)
    }
}

impl AgentStore for SqliteBackend {
    fn create_agent(&self, _tenant: Option<&str>, input: &CreateAgent) -> (Agent, String) {
        db_ops::create_agent(&self.lock(), input)
    }
    fn get_agent(&self, _tenant: Option<&str>, id: &str) -> Option<Agent> {
        db_ops::get_agent(&self.lock(), id)
    }
    fn get_agent_by_key_hash(&self, _tenant: Option<&str>, hash: &str) -> Option<Agent> {
        db_ops::get_agent_by_key_hash(&self.lock(), hash)
    }
    fn list_agents(&self, _tenant: Option<&str>) -> Vec<Agent> {
        db_ops::list_agents(&self.lock(), _tenant)
    }
    fn list_agents_by_owner(&self, _tenant: Option<&str>, owner_id: &str) -> Vec<Agent> {
        db_ops::list_agents_by_owner(&self.lock(), owner_id)
    }
    fn update_agent(&self, _tenant: Option<&str>, id: &str, input: &UpdateAgent) -> Option<Agent> {
        db_ops::update_agent(&self.lock(), id, input)
    }
    fn delete_agent(&self, _tenant: Option<&str>, id: &str) -> bool {
        db_ops::delete_agent(&self.lock(), id)
    }
    fn update_heartbeat(&self, _tenant: Option<&str>, agent_id: &str) -> bool {
        db_ops::update_heartbeat(&self.lock(), agent_id)
    }
    fn find_best_agent(&self, _tenant: Option<&str>, strategy: &AssignStrategy) -> Option<String> {
        db_ops::find_best_agent(&self.lock(), _tenant, strategy)
    }
    fn get_agent_name(&self, _tenant: Option<&str>, agent_id: &str) -> Option<String> {
        db_ops::get_agent_name(&self.lock(), agent_id)
    }
    fn get_agent_inbox(&self, _tenant: Option<&str>, agent_id: &str) -> AgentInbox {
        db_ops::get_agent_inbox(&self.lock(), _tenant, agent_id)
    }
}

impl ActivityStore for SqliteBackend {
    fn create_activity(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        author_type: &str,
        author_id: &str,
        input: &CreateActivity,
    ) -> TaskActivity {
        db_ops::create_activity(&self.lock(), task_id, author_type, author_id, input)
    }
    fn list_activity(&self, _tenant: Option<&str>, task_id: &str) -> Vec<TaskActivity> {
        db_ops::list_activity(&self.lock(), task_id)
    }
}

impl KnowledgeStore for SqliteBackend {
    fn upsert_knowledge(
        &self,
        _tenant: Option<&str>,
        project_id: &str,
        key: &str,
        input: &UpsertKnowledge,
        author_type: &str,
        author_id: &str,
    ) -> KnowledgeEntry {
        db_ops::upsert_knowledge(&self.lock(), project_id, key, input, author_type, author_id)
    }
    fn get_knowledge(
        &self,
        _tenant: Option<&str>,
        project_id: &str,
        key: &str,
    ) -> Option<KnowledgeEntry> {
        db_ops::get_knowledge(&self.lock(), project_id, key)
    }
    fn list_knowledge(
        &self,
        _tenant: Option<&str>,
        project_id: &str,
        prefix: Option<&str>,
    ) -> Vec<KnowledgeEntry> {
        db_ops::list_knowledge(&self.lock(), project_id, prefix)
    }
    fn search_knowledge(
        &self,
        _tenant: Option<&str>,
        project_id: &str,
        query: &str,
        tag_list: &[String],
        category: Option<&str>,
    ) -> Vec<KnowledgeEntry> {
        db_ops::search_knowledge(&self.lock(), project_id, query, tag_list, category)
    }
    fn delete_knowledge(&self, _tenant: Option<&str>, project_id: &str, key: &str) -> bool {
        db_ops::delete_knowledge(&self.lock(), project_id, key)
    }
}

impl ArtifactStore for SqliteBackend {
    fn create_artifact(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        input: &CreateArtifact,
        author_type: &str,
        author_id: &str,
    ) -> TaskArtifact {
        db_ops::create_artifact(&self.lock(), task_id, input, author_type, author_id)
    }
    fn list_artifacts(&self, _tenant: Option<&str>, task_id: &str) -> Vec<TaskArtifact> {
        db_ops::list_artifacts(&self.lock(), task_id)
    }
    fn get_artifact(&self, _tenant: Option<&str>, artifact_id: &str) -> Option<TaskArtifact> {
        db_ops::get_artifact(&self.lock(), artifact_id)
    }
    fn delete_artifact(&self, _tenant: Option<&str>, artifact_id: &str) -> bool {
        db_ops::delete_artifact(&self.lock(), artifact_id)
    }
    fn update_artifact(&self, _tenant: Option<&str>, artifact_id: &str, input: &UpdateArtifact) -> Option<TaskArtifact> {
        db_ops::update_artifact(&self.lock(), artifact_id, input)
    }
}

impl QuestionStore for SqliteBackend {
    fn create_question(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        input: &CreateQuestion,
        asked_by_type: &str,
        asked_by_id: &str,
    ) -> TaskQuestion {
        db_ops::create_question(&self.lock(), task_id, input, asked_by_type, asked_by_id)
    }
    fn get_question(&self, _tenant: Option<&str>, id: &str) -> Option<TaskQuestion> {
        db_ops::get_question(&self.lock(), id)
    }
    fn list_questions(
        &self,
        _tenant: Option<&str>,
        task_id: &str,
        status: Option<&str>,
    ) -> Vec<TaskQuestion> {
        db_ops::list_questions(&self.lock(), task_id, status)
    }
    fn list_questions_for_agent(
        &self,
        _tenant: Option<&str>,
        agent_id: &str,
        status: Option<&str>,
    ) -> Vec<TaskQuestion> {
        db_ops::list_questions_for_agent(&self.lock(), agent_id, status)
    }
    fn list_questions_for_project(
        &self,
        _tenant: Option<&str>,
        project_id: &str,
        status: Option<&str>,
        unrouted: bool,
    ) -> Vec<TaskQuestion> {
        db_ops::list_questions_for_project(&self.lock(), project_id, status, unrouted)
    }
    fn resolve_question(
        &self,
        _tenant: Option<&str>,
        question_id: &str,
        resolution: &str,
        resolved_by_type: &str,
        resolved_by_id: &str,
    ) -> Option<TaskQuestion> {
        db_ops::resolve_question(
            &self.lock(),
            question_id,
            resolution,
            resolved_by_type,
            resolved_by_id,
        )
    }
    fn recalculate_has_open_questions(&self, _tenant: Option<&str>, task_id: &str) {
        db_ops::recalculate_has_open_questions(&self.lock(), task_id)
    }
    fn create_reply(
        &self,
        _tenant: Option<&str>,
        question_id: &str,
        input: &CreateReply,
        author_type: &str,
        author_id: &str,
    ) -> QuestionReply {
        db_ops::create_reply(&self.lock(), question_id, input, author_type, author_id)
    }
    fn list_replies(&self, _tenant: Option<&str>, question_id: &str) -> Vec<QuestionReply> {
        db_ops::list_replies(&self.lock(), question_id)
    }
    fn dismiss_question(
        &self,
        _tenant: Option<&str>,
        question_id: &str,
        reason: &str,
    ) -> Option<TaskQuestion> {
        db_ops::dismiss_question(&self.lock(), question_id, reason)
    }
    fn assign_question(
        &self,
        _tenant: Option<&str>,
        question_id: &str,
        target_type: &str,
        target_id: &str,
    ) -> Option<TaskQuestion> {
        db_ops::assign_question(&self.lock(), question_id, target_type, target_id)
    }
    fn find_capability_targets(
        &self,
        _tenant: Option<&str>,
        required_capability: &str,
    ) -> Vec<CapabilityTarget> {
        db_ops::find_capability_targets(&self.lock(), required_capability)
    }
    fn auto_target_question(
        &self,
        _tenant: Option<&str>,
        question_id: &str,
        required_capability: &str,
    ) -> Vec<CapabilityTarget> {
        db_ops::auto_target_question(&self.lock(), question_id, required_capability)
    }
}

impl EventStore for SqliteBackend {
    fn emit_event(
        &self,
        _tenant: Option<&str>,
        event_type: &str,
        task_id: Option<&str>,
        project_id: &str,
        actor_type: &str,
        actor_id: &str,
        payload: &serde_json::Value,
    ) -> Vec<PendingNotifWebhook> {
        db_ops::emit_event(
            &self.lock(),
            event_type,
            task_id,
            project_id,
            actor_type,
            actor_id,
            payload,
        )
    }
    fn get_last_event_id(&self, _tenant: Option<&str>) -> i64 {
        self.lock()
            .query_row("SELECT MAX(id) FROM events", [], |row| row.get::<_, i64>(0))
            .unwrap_or(0)
    }
    fn insert_question_notification(
        &self,
        _tenant: Option<&str>,
        agent_id: &str,
        event_id: i64,
        event_type: &str,
        title: &str,
        body: Option<&str>,
    ) -> PendingNotifWebhook {
        db_ops::insert_question_notification(
            &self.lock(),
            agent_id,
            event_id,
            event_type,
            title,
            body,
        )
    }
    fn list_notifications(
        &self,
        _tenant: Option<&str>,
        agent_id: &str,
        unread: Option<bool>,
    ) -> Vec<Notification> {
        db_ops::list_notifications(&self.lock(), agent_id, unread)
    }
    fn ack_notification(
        &self,
        _tenant: Option<&str>,
        agent_id: &str,
        notification_id: i64,
    ) -> bool {
        db_ops::ack_notification(&self.lock(), agent_id, notification_id)
    }
    fn ack_all_notifications(&self, _tenant: Option<&str>, agent_id: &str) -> i64 {
        db_ops::ack_all_notifications(&self.lock(), agent_id)
    }
    fn ack_notification_system(&self, _tenant: Option<&str>, notification_id: i64) {
        db_ops::ack_notification_system(&self.lock(), notification_id)
    }
    fn update_notification_webhook_status(
        &self,
        _tenant: Option<&str>,
        notification_id: i64,
        status: &str,
    ) {
        db_ops::update_notification_webhook_status(&self.lock(), notification_id, status)
    }
}

impl WebhookStore for SqliteBackend {
    fn create_webhook_trigger(
        &self,
        _tenant: Option<&str>,
        project_id: &str,
        name: &str,
        action_type: &str,
        action_config: &serde_json::Value,
    ) -> (WebhookTrigger, String) {
        db_ops::create_webhook_trigger(&self.lock(), project_id, name, action_type, action_config)
    }
    fn list_webhook_triggers(
        &self,
        _tenant: Option<&str>,
        project_id: &str,
    ) -> Vec<WebhookTrigger> {
        db_ops::list_webhook_triggers(&self.lock(), project_id)
    }
    fn get_webhook_trigger_for_validation(
        &self,
        _tenant: Option<&str>,
        trigger_id: &str,
    ) -> Option<(WebhookTrigger, String)> {
        db_ops::get_webhook_trigger_for_validation(&self.lock(), trigger_id)
    }
    fn update_webhook_trigger(
        &self,
        _tenant: Option<&str>,
        trigger_id: &str,
        input: &UpdateTriggerRequest,
    ) -> Option<WebhookTrigger> {
        db_ops::update_webhook_trigger(&self.lock(), trigger_id, input)
    }
    fn delete_webhook_trigger(&self, _tenant: Option<&str>, trigger_id: &str) -> bool {
        db_ops::delete_webhook_trigger(&self.lock(), trigger_id)
    }
    fn log_trigger_execution(
        &self,
        _tenant: Option<&str>,
        trigger_id: &str,
        status: &str,
        payload: Option<&serde_json::Value>,
        result: Option<&serde_json::Value>,
        error: Option<&str>,
    ) -> String {
        db_ops::log_trigger_execution(&self.lock(), trigger_id, status, payload, result, error)
    }
    fn list_trigger_logs(
        &self,
        _tenant: Option<&str>,
        trigger_id: &str,
        limit: i64,
    ) -> Vec<WebhookTriggerLog> {
        db_ops::list_trigger_logs(&self.lock(), trigger_id, limit)
    }
    fn create_webhook_log(
        &self,
        _tenant: Option<&str>,
        agent_id: &str,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> String {
        db_ops::create_webhook_log(&self.lock(), agent_id, event_type, payload)
    }
    fn update_webhook_log(
        &self,
        _tenant: Option<&str>,
        id: &str,
        status: &str,
        attempts: i64,
        response_status: Option<i64>,
        response_body: Option<&str>,
    ) {
        db_ops::update_webhook_log(
            &self.lock(),
            id,
            status,
            attempts,
            response_status,
            response_body,
        )
    }
}

impl StatsStore for SqliteBackend {
    fn get_stats(&self, tenant: Option<&str>) -> DashboardStats {
        db_ops::get_stats(&self.lock(), tenant)
    }
}

impl StorageBackend for SqliteBackend {
    fn hash_api_key(&self, key: &str) -> String {
        db_ops::hash_api_key(key)
    }
}
