pub mod sqlite;

use opengate_models::*;

/// Error type for storage operations.
#[derive(Debug, Clone)]
pub struct StorageError(pub String);

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for StorageError {}

impl From<String> for StorageError {
    fn from(s: String) -> Self {
        StorageError(s)
    }
}

// --- Storage Traits ---
// Each trait covers a domain. All methods take `tenant: Option<&str>`:
// - `None` for single-tenant (OSS standalone)
// - `Some(id)` for multi-tenant (product)

pub trait ProjectStore: Send + Sync {
    fn create_project(
        &self,
        tenant: Option<&str>,
        input: &CreateProject,
        created_by: &str,
    ) -> Project;
    fn get_project(&self, tenant: Option<&str>, id: &str) -> Option<Project>;
    fn list_projects(&self, tenant: Option<&str>, status_filter: Option<&str>) -> Vec<Project>;
    fn update_project(
        &self,
        tenant: Option<&str>,
        id: &str,
        input: &UpdateProject,
    ) -> Option<Project>;
    fn archive_project(&self, tenant: Option<&str>, id: &str) -> bool;
    fn get_project_with_stats(&self, tenant: Option<&str>, id: &str) -> Option<ProjectWithStats>;
    fn get_schedule(
        &self,
        tenant: Option<&str>,
        project_id: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Vec<ScheduledTaskEntry>;
    fn get_pulse(
        &self,
        tenant: Option<&str>,
        project_id: &str,
        caller_agent_id: Option<&str>,
    ) -> PulseResponse;
}

pub trait TaskStore: Send + Sync {
    fn create_task(
        &self,
        tenant: Option<&str>,
        project_id: &str,
        input: &CreateTask,
        created_by: &str,
    ) -> Task;
    fn get_task(&self, tenant: Option<&str>, id: &str) -> Option<Task>;
    fn list_tasks(&self, tenant: Option<&str>, filters: &TaskFilters) -> Vec<Task>;
    fn update_task(
        &self,
        tenant: Option<&str>,
        id: &str,
        input: &UpdateTask,
    ) -> Result<Option<Task>, StorageError>;
    fn delete_task(&self, tenant: Option<&str>, id: &str) -> bool;
    fn claim_task(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        agent_id: &str,
        agent_name: &str,
    ) -> Result<Task, StorageError>;
    fn release_task(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        agent_id: &str,
    ) -> Result<Task, StorageError>;
    fn get_next_task(&self, tenant: Option<&str>, skills: &[String]) -> Option<Task>;
    fn get_tasks_for_assignee(&self, tenant: Option<&str>, assignee_id: &str) -> Vec<Task>;
    fn merge_context(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        patch: &serde_json::Value,
    ) -> Result<Option<Task>, StorageError>;
    fn batch_update_status(
        &self,
        tenant: Option<&str>,
        updates: &[(String, String)],
    ) -> BatchResult;
    fn release_stale_tasks(&self, tenant: Option<&str>, default_timeout_minutes: i64) -> Vec<Task>;
    fn transition_ready_scheduled_tasks(&self, tenant: Option<&str>) -> usize;
    fn create_next_recurrence(&self, tenant: Option<&str>, completed_task: &Task)
        -> Option<String>;
    fn append_status_history(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        new_status: &str,
        agent_type: Option<&str>,
        agent_id: Option<&str>,
    );

    // Dependencies
    fn check_dependencies(&self, tenant: Option<&str>, task: &Task) -> Result<(), Vec<String>>;
    fn add_dependency(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        depends_on_id: &str,
    ) -> Result<(), StorageError>;
    fn remove_dependency(&self, tenant: Option<&str>, task_id: &str, depends_on_id: &str) -> bool;
    fn get_task_dependencies(&self, tenant: Option<&str>, task_id: &str) -> Vec<Task>;
    fn get_task_dependents(&self, tenant: Option<&str>, task_id: &str) -> Vec<Task>;
    fn unblock_dependents_on_complete(
        &self,
        tenant: Option<&str>,
        completed_task_id: &str,
    ) -> Vec<PendingNotifWebhook>;
    fn all_dependencies_done(&self, tenant: Option<&str>, task: &Task) -> bool;
    fn inject_upstream_outputs(&self, tenant: Option<&str>, completed_task: &Task);

    // Assignment & review
    fn assign_task(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        agent_id: &str,
    ) -> Result<Task, StorageError>;
    fn handoff_task(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        from_agent_id: &str,
        to_agent_id: &str,
        summary: Option<&str>,
    ) -> Result<Task, StorageError>;
    fn approve_task(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        reviewer_id: &str,
        comment: Option<&str>,
    ) -> Result<Task, StorageError>;
    fn request_changes(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        reviewer_id: &str,
        comment: &str,
    ) -> Result<Task, StorageError>;
    fn submit_review_task(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        submitter_id: &str,
        summary: Option<&str>,
        explicit_reviewer_id: Option<&str>,
    ) -> Result<Task, StorageError>;
    fn start_review_task(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        caller_id: &str,
        caller_type: &str,
    ) -> Result<Task, StorageError>;
}

pub trait AgentStore: Send + Sync {
    fn create_agent(&self, tenant: Option<&str>, input: &CreateAgent) -> (Agent, String);
    fn get_agent(&self, tenant: Option<&str>, id: &str) -> Option<Agent>;
    fn get_agent_by_key_hash(&self, tenant: Option<&str>, hash: &str) -> Option<Agent>;
    fn list_agents(&self, tenant: Option<&str>) -> Vec<Agent>;
    fn list_agents_by_owner(&self, tenant: Option<&str>, owner_id: &str) -> Vec<Agent>;
    fn update_agent(&self, tenant: Option<&str>, id: &str, input: &UpdateAgent) -> Option<Agent>;
    fn delete_agent(&self, tenant: Option<&str>, id: &str) -> bool;
    fn update_heartbeat(&self, tenant: Option<&str>, agent_id: &str) -> bool;
    fn find_best_agent(&self, tenant: Option<&str>, strategy: &AssignStrategy) -> Option<String>;
    fn get_agent_name(&self, tenant: Option<&str>, agent_id: &str) -> Option<String>;
    fn get_agent_inbox(&self, tenant: Option<&str>, agent_id: &str) -> AgentInbox;
}

pub trait ActivityStore: Send + Sync {
    fn create_activity(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        author_type: &str,
        author_id: &str,
        input: &CreateActivity,
    ) -> TaskActivity;
    fn list_activity(&self, tenant: Option<&str>, task_id: &str) -> Vec<TaskActivity>;
}

pub trait KnowledgeStore: Send + Sync {
    fn upsert_knowledge(
        &self,
        tenant: Option<&str>,
        project_id: &str,
        key: &str,
        input: &UpsertKnowledge,
        author_type: &str,
        author_id: &str,
    ) -> KnowledgeEntry;
    fn get_knowledge(
        &self,
        tenant: Option<&str>,
        project_id: &str,
        key: &str,
    ) -> Option<KnowledgeEntry>;
    fn list_knowledge(
        &self,
        tenant: Option<&str>,
        project_id: &str,
        prefix: Option<&str>,
    ) -> Vec<KnowledgeEntry>;
    fn search_knowledge(
        &self,
        tenant: Option<&str>,
        project_id: &str,
        query: &str,
        tag_list: &[String],
        category: Option<&str>,
    ) -> Vec<KnowledgeEntry>;
    fn delete_knowledge(&self, tenant: Option<&str>, project_id: &str, key: &str) -> bool;
}

pub trait ArtifactStore: Send + Sync {
    fn create_artifact(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        input: &CreateArtifact,
        author_type: &str,
        author_id: &str,
    ) -> TaskArtifact;
    fn list_artifacts(&self, tenant: Option<&str>, task_id: &str) -> Vec<TaskArtifact>;
    fn get_artifact(&self, tenant: Option<&str>, artifact_id: &str) -> Option<TaskArtifact>;
    fn delete_artifact(&self, tenant: Option<&str>, artifact_id: &str) -> bool;
}

pub trait QuestionStore: Send + Sync {
    fn create_question(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        input: &CreateQuestion,
        asked_by_type: &str,
        asked_by_id: &str,
    ) -> TaskQuestion;
    fn get_question(&self, tenant: Option<&str>, id: &str) -> Option<TaskQuestion>;
    fn list_questions(
        &self,
        tenant: Option<&str>,
        task_id: &str,
        status: Option<&str>,
    ) -> Vec<TaskQuestion>;
    fn list_questions_for_agent(
        &self,
        tenant: Option<&str>,
        agent_id: &str,
        status: Option<&str>,
    ) -> Vec<TaskQuestion>;
    fn list_questions_for_project(
        &self,
        tenant: Option<&str>,
        project_id: &str,
        status: Option<&str>,
        unrouted: bool,
    ) -> Vec<TaskQuestion>;
    fn resolve_question(
        &self,
        tenant: Option<&str>,
        question_id: &str,
        resolution: &str,
        resolved_by_type: &str,
        resolved_by_id: &str,
    ) -> Option<TaskQuestion>;
    fn recalculate_has_open_questions(&self, tenant: Option<&str>, task_id: &str);
    fn create_reply(
        &self,
        tenant: Option<&str>,
        question_id: &str,
        input: &CreateReply,
        author_type: &str,
        author_id: &str,
    ) -> QuestionReply;
    fn list_replies(&self, tenant: Option<&str>, question_id: &str) -> Vec<QuestionReply>;
    fn dismiss_question(
        &self,
        tenant: Option<&str>,
        question_id: &str,
        reason: &str,
    ) -> Option<TaskQuestion>;
    fn assign_question(
        &self,
        tenant: Option<&str>,
        question_id: &str,
        target_type: &str,
        target_id: &str,
    ) -> Option<TaskQuestion>;
    fn find_capability_targets(
        &self,
        tenant: Option<&str>,
        required_capability: &str,
    ) -> Vec<CapabilityTarget>;
    fn auto_target_question(
        &self,
        tenant: Option<&str>,
        question_id: &str,
        required_capability: &str,
    ) -> Vec<CapabilityTarget>;
}

pub trait EventStore: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    fn emit_event(
        &self,
        tenant: Option<&str>,
        event_type: &str,
        task_id: Option<&str>,
        project_id: &str,
        actor_type: &str,
        actor_id: &str,
        payload: &serde_json::Value,
    ) -> Vec<PendingNotifWebhook>;
    fn get_last_event_id(&self, tenant: Option<&str>) -> i64;
    fn insert_question_notification(
        &self,
        tenant: Option<&str>,
        agent_id: &str,
        event_id: i64,
        event_type: &str,
        title: &str,
        body: Option<&str>,
    ) -> PendingNotifWebhook;
    fn list_notifications(
        &self,
        tenant: Option<&str>,
        agent_id: &str,
        unread: Option<bool>,
    ) -> Vec<Notification>;
    fn ack_notification(&self, tenant: Option<&str>, agent_id: &str, notification_id: i64) -> bool;
    fn ack_all_notifications(&self, tenant: Option<&str>, agent_id: &str) -> i64;
    fn ack_notification_system(&self, tenant: Option<&str>, notification_id: i64);
    fn update_notification_webhook_status(
        &self,
        tenant: Option<&str>,
        notification_id: i64,
        status: &str,
    );
}

pub trait WebhookStore: Send + Sync {
    fn create_webhook_trigger(
        &self,
        tenant: Option<&str>,
        project_id: &str,
        name: &str,
        action_type: &str,
        action_config: &serde_json::Value,
    ) -> (WebhookTrigger, String);
    fn list_webhook_triggers(&self, tenant: Option<&str>, project_id: &str) -> Vec<WebhookTrigger>;
    fn get_webhook_trigger_for_validation(
        &self,
        tenant: Option<&str>,
        trigger_id: &str,
    ) -> Option<(WebhookTrigger, String)>;
    fn update_webhook_trigger(
        &self,
        tenant: Option<&str>,
        trigger_id: &str,
        input: &UpdateTriggerRequest,
    ) -> Option<WebhookTrigger>;
    fn delete_webhook_trigger(&self, tenant: Option<&str>, trigger_id: &str) -> bool;
    fn log_trigger_execution(
        &self,
        tenant: Option<&str>,
        trigger_id: &str,
        status: &str,
        payload: Option<&serde_json::Value>,
        result: Option<&serde_json::Value>,
        error: Option<&str>,
    ) -> String;
    fn list_trigger_logs(
        &self,
        tenant: Option<&str>,
        trigger_id: &str,
        limit: i64,
    ) -> Vec<WebhookTriggerLog>;
    fn create_webhook_log(
        &self,
        tenant: Option<&str>,
        agent_id: &str,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> String;
    fn update_webhook_log(
        &self,
        tenant: Option<&str>,
        id: &str,
        status: &str,
        attempts: i64,
        response_status: Option<i64>,
        response_body: Option<&str>,
    );
}

pub trait StatsStore: Send + Sync {
    fn get_stats(&self, tenant: Option<&str>) -> DashboardStats;
}

/// Super-trait combining all domain stores.
pub trait StorageBackend:
    ProjectStore
    + TaskStore
    + AgentStore
    + ActivityStore
    + KnowledgeStore
    + ArtifactStore
    + QuestionStore
    + EventStore
    + WebhookStore
    + StatsStore
{
    /// Hash an API key (utility, doesn't need &self but lives here for convenience).
    fn hash_api_key(&self, key: &str) -> String;

    /// Like get_task, but also loads the activity timeline and enriches
    /// context with project repo metadata (read-time only, not persisted).
    fn get_task_full(&self, tenant: Option<&str>, id: &str) -> Option<Task> {
        let mut task = self.get_task(tenant, id)?;
        task.activities = self.list_activity(tenant, &task.id);

        // Enrich context with project repo info if not already set
        if let Some(project) = self.get_project(tenant, &task.project_id) {
            if let Some(ref repo_url) = project.repo_url {
                let ctx = task.context.get_or_insert_with(|| serde_json::json!({}));
                if let serde_json::Value::Object(ref mut map) = ctx {
                    map.entry("repo_url")
                        .or_insert_with(|| serde_json::json!(repo_url));
                    if let Some(ref branch) = project.default_branch {
                        map.entry("branch")
                            .or_insert_with(|| serde_json::json!(branch));
                    }
                }
            }
        }

        Some(task)
    }
}
