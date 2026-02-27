use serde::{Deserialize, Serialize};

// --- Enums ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Backlog,
    Todo,
    InProgress,
    Review,
    Blocked,
    Done,
    Cancelled,
    Handoff,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Backlog => "backlog",
            TaskStatus::Todo => "todo",
            TaskStatus::InProgress => "in_progress",
            TaskStatus::Review => "review",
            TaskStatus::Blocked => "blocked",
            TaskStatus::Done => "done",
            TaskStatus::Cancelled => "cancelled",
            TaskStatus::Handoff => "handoff",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "backlog" => Some(TaskStatus::Backlog),
            "todo" => Some(TaskStatus::Todo),
            "in_progress" => Some(TaskStatus::InProgress),
            "review" => Some(TaskStatus::Review),
            "blocked" => Some(TaskStatus::Blocked),
            "done" => Some(TaskStatus::Done),
            "cancelled" => Some(TaskStatus::Cancelled),
            "handoff" => Some(TaskStatus::Handoff),
            _ => None,
        }
    }

    pub fn valid_transitions(&self) -> Vec<TaskStatus> {
        match self {
            TaskStatus::Backlog => vec![
                TaskStatus::Todo,
                TaskStatus::InProgress,
                TaskStatus::Cancelled,
            ],
            TaskStatus::Todo => vec![
                TaskStatus::InProgress,
                TaskStatus::Blocked,
                TaskStatus::Cancelled,
            ],
            TaskStatus::InProgress => vec![
                TaskStatus::Review,
                TaskStatus::Done,
                TaskStatus::Blocked,
                TaskStatus::Cancelled,
                TaskStatus::Handoff,
            ],
            TaskStatus::Review => vec![TaskStatus::Done, TaskStatus::InProgress],
            TaskStatus::Blocked => vec![
                TaskStatus::Todo,
                TaskStatus::InProgress,
                TaskStatus::Cancelled,
            ],
            TaskStatus::Done => vec![],
            TaskStatus::Cancelled => vec![],
            TaskStatus::Handoff => vec![TaskStatus::InProgress],
        }
    }

    pub fn can_transition_to(&self, target: &TaskStatus) -> bool {
        self.valid_transitions().contains(target)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Critical,
    High,
    Medium,
    Low,
}

impl Priority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Priority::Critical => "critical",
            Priority::High => "high",
            Priority::Medium => "medium",
            Priority::Low => "low",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "critical" => Some(Priority::Critical),
            "high" => Some(Priority::High),
            "medium" => Some(Priority::Medium),
            "low" => Some(Priority::Low),
            _ => None,
        }
    }

    pub fn sort_order(&self) -> i32 {
        match self {
            Priority::Critical => 0,
            Priority::High => 1,
            Priority::Medium => 2,
            Priority::Low => 3,
        }
    }
}

// --- Domain models ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub assignee_type: Option<String>,
    pub assignee_id: Option<String>,
    pub context: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    pub tags: Vec<String>,
    pub due_date: Option<String>,
    pub reviewer_type: Option<String>,
    pub reviewer_id: Option<String>,
    pub status_history: Vec<StatusHistoryEntry>,
    pub artifacts: Vec<TaskArtifact>,
    /// ISO8601 datetime: task stays in backlog until this passes
    pub scheduled_at: Option<String>,
    /// JSON recurrence rule: {frequency, interval, cron, ...}
    pub recurrence_rule: Option<serde_json::Value>,
    /// Points to the original recurring task (parent)
    pub recurrence_parent_id: Option<String>,
    /// IDs of tasks this task depends on (loaded from task_dependencies)
    pub dependencies: Vec<String>,
    /// True if this task has blocking open questions
    pub has_open_questions: bool,
    /// ISO8601 timestamp: when the reviewer started reviewing this task
    pub started_review_at: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activities: Vec<TaskActivity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusHistoryEntry {
    pub status: String,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskActivity {
    pub id: String,
    pub task_id: String,
    pub author_type: String,
    pub author_id: String,
    pub content: String,
    pub activity_type: String,
    pub metadata: Option<serde_json::Value>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing)]
    pub api_key_hash: String,
    pub skills: Vec<String>,
    pub description: Option<String>,
    pub status: String,
    pub max_concurrent_tasks: i64,
    pub current_task_count: i64,
    /// Count of tasks where this agent is reviewer and status = review
    pub review_task_count: i64,
    pub webhook_url: Option<String>,
    /// Optional JSON array of event types to push via webhook. If null/empty, all events trigger push.
    pub webhook_events: Option<Vec<String>>,
    pub config: Option<serde_json::Value>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub cost_tier: Option<String>,
    pub capabilities: Vec<String>,
    /// senior | mid | junior
    pub seniority: String,
    /// orchestrator | executor
    pub role: String,
    /// Minutes before agent is considered stale/offline (default: 30)
    pub stale_timeout: i64,
    pub last_seen_at: Option<String>,
    pub created_at: String,
    /// Optional owner (e.g. Clerk user_id). None in standalone OSS mode.
    pub owner_id: Option<String>,
    /// Free-form category tags (e.g. ["rust", "frontend", "devops"])
    pub tags: Vec<String>,
}

// --- DTOs ---

#[derive(Debug, Deserialize)]
pub struct CreateProject {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProject {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTask {
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub tags: Option<Vec<String>>,
    pub context: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    pub due_date: Option<String>,
    pub assignee_type: Option<String>,
    pub assignee_id: Option<String>,
    /// ISO8601: defer task start until this datetime
    pub scheduled_at: Option<String>,
    /// Recurrence rule JSON
    pub recurrence_rule: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTask {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub tags: Option<Vec<String>>,
    pub context: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    pub due_date: Option<String>,
    pub assignee_type: Option<String>,
    pub assignee_id: Option<String>,
    pub reviewer_type: Option<String>,
    pub reviewer_id: Option<String>,
    /// ISO8601: defer task start until this datetime
    pub scheduled_at: Option<String>,
    /// Recurrence rule JSON (set to null object to clear)
    pub recurrence_rule: Option<serde_json::Value>,
}

/// Request to add dependencies to a task
#[derive(Debug, Deserialize)]
pub struct AddDependenciesRequest {
    pub depends_on: Vec<String>,
}

/// Response for schedule endpoint — task with its scheduled_at
#[derive(Debug, Serialize)]
pub struct ScheduledTaskEntry {
    pub id: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub scheduled_at: String,
    pub assignee_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgent {
    pub name: String,
    pub skills: Option<Vec<String>>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub cost_tier: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub seniority: Option<String>,
    pub role: Option<String>,
    pub owner_id: Option<String>,
}

impl CreateAgent {
    /// Builder for tests — single update point when schema changes.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            skills: None,
            model: None,
            provider: None,
            cost_tier: None,
            capabilities: None,
            seniority: None,
            role: None,
            owner_id: None,
        }
    }

    pub fn with_skills(mut self, skills: Vec<String>) -> Self {
        self.skills = Some(skills);
        self
    }

    pub fn with_seniority(mut self, seniority: impl Into<String>) -> Self {
        self.seniority = Some(seniority.into());
        self
    }

    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    pub fn with_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.capabilities = Some(capabilities);
        self
    }
}

#[derive(Debug, Deserialize)]
pub struct RegisterAgentRequest {
    pub name: String,
    pub skills: Option<Vec<String>>,
    pub setup_token: String,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub cost_tier: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub owner_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateActivity {
    pub content: String,
    pub activity_type: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct TaskFilters {
    pub project_id: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub assignee_id: Option<String>,
    pub tag: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BatchStatusUpdate {
    pub updates: Vec<BatchStatusItem>,
}

#[derive(Debug, Deserialize)]
pub struct BatchStatusItem {
    pub task_id: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct BatchResult {
    pub succeeded: Vec<String>,
    pub failed: Vec<BatchError>,
}

#[derive(Debug, Serialize)]
pub struct BatchError {
    pub task_id: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct DashboardStats {
    pub tasks_by_status: std::collections::HashMap<String, i64>,
    pub total_tasks: i64,
    pub active_agents: i64,
    pub total_projects: i64,
    pub recent_activity: Vec<TaskActivity>,
}

#[derive(Debug, Serialize)]
pub struct ProjectWithStats {
    pub project: Project,
    pub task_count: i64,
    pub tasks_by_status: std::collections::HashMap<String, i64>,
}

#[derive(Debug, Serialize)]
pub struct AgentCreated {
    pub agent: Agent,
    pub api_key: String,
}

#[derive(Debug, Deserialize)]
pub struct CompleteRequest {
    pub summary: Option<String>,
    pub output: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct SubmitReviewRequest {
    /// Optional summary of what was done (recorded as activity).
    pub summary: Option<String>,
    /// Explicit reviewer agent ID override. If omitted, one is auto-selected.
    pub reviewer_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BlockRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NextTaskQuery {
    pub skills: Option<String>,
}

// --- Identity (from auth) ---

#[derive(Debug, Clone)]
pub enum Identity {
    AgentIdentity { id: String, name: String },
    Human { id: String },
    Anonymous,
}

impl Identity {
    pub fn author_type(&self) -> &'static str {
        match self {
            Identity::AgentIdentity { .. } => "agent",
            Identity::Human { .. } => "human",
            Identity::Anonymous => "system",
        }
    }

    pub fn author_id(&self) -> &str {
        match self {
            Identity::AgentIdentity { id, .. } => id,
            Identity::Human { id } => id,
            Identity::Anonymous => "system",
        }
    }

    pub fn display_name(&self) -> &str {
        match self {
            Identity::AgentIdentity { name, .. } => name,
            Identity::Human { id } => id,
            Identity::Anonymous => "system",
        }
    }
}

// --- v2 DTOs ---

#[derive(Debug, Deserialize)]
pub struct UpdateAgent {
    pub description: Option<String>,
    pub skills: Option<Vec<String>>,
    pub max_concurrent_tasks: Option<i64>,
    pub webhook_url: Option<String>,
    /// JSON array of event types to subscribe to for webhook push. null = all events.
    pub webhook_events: Option<Vec<String>>,
    pub config: Option<serde_json::Value>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub cost_tier: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub seniority: Option<String>,
    pub role: Option<String>,
    /// Minutes before agent is considered stale (default: 30)
    pub stale_timeout: Option<i64>,
    /// Free-form category tags (e.g. ["rust", "frontend", "devops"])
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct AssignRequest {
    pub agent_id: String,
}

#[derive(Debug, Deserialize)]
pub struct HandoffRequest {
    pub to_agent_id: String,
    pub summary: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ApproveRequest {
    pub comment: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RequestChangesRequest {
    pub comment: String,
}

// --- Knowledge Base ---

/// Valid category values for knowledge entries.
pub const VALID_CATEGORIES: &[&str] =
    &["architecture", "pattern", "gotcha", "decision", "reference"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: String,
    pub project_id: String,
    pub key: String,
    pub title: String,
    pub content: String,
    pub metadata: Option<serde_json::Value>,
    /// Tag list stored as JSON array in SQLite.
    pub tags: Vec<String>,
    /// Optional category: architecture | pattern | gotcha | decision | reference
    pub category: Option<String>,
    pub created_by_type: String,
    pub created_by_id: String,
    pub updated_at: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct UpsertKnowledge {
    pub title: String,
    pub content: String,
    pub metadata: Option<serde_json::Value>,
    /// Tags to attach (optional, defaults to empty list on create).
    pub tags: Option<Vec<String>>,
    /// Category: architecture | pattern | gotcha | decision | reference
    pub category: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct KnowledgeSearchQuery {
    pub q: Option<String>,
    pub prefix: Option<String>,
    /// Comma-separated tags to filter by (OR match): ?tags=rust,performance
    pub tags: Option<String>,
    /// Filter by category: ?category=pattern
    pub category: Option<String>,
}

// --- Task Artifacts ---

pub const VALID_ARTIFACT_TYPES: &[&str] = &["url", "text", "json", "file"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskArtifact {
    pub id: String,
    pub task_id: String,
    pub name: String,
    pub artifact_type: String,
    pub value: String,
    pub created_by_type: String,
    pub created_by_id: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateArtifact {
    pub name: String,
    pub artifact_type: String,
    pub value: String,
}

// --- Webhook Log ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookLogEntry {
    pub id: String,
    pub agent_id: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub status: String,
    pub attempts: i64,
    pub last_attempt_at: Option<String>,
    pub created_at: String,
}

// --- Notifications ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: i64,
    pub agent_id: String,
    pub event_id: Option<i64>,
    pub event_type: String,
    pub title: String,
    pub body: Option<String>,
    pub read: bool,
    /// Webhook delivery status: "delivered" | "failed" | null (not attempted)
    pub webhook_status: Option<String>,
    pub created_at: String,
}

/// Carries information about a newly-inserted notification that may need webhook delivery.
#[derive(Debug, Clone)]
pub struct PendingNotifWebhook {
    pub agent_id: String,
    pub notification_id: i64,
    pub event_type: String,
    pub title: String,
    pub body: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NotificationQuery {
    pub unread: Option<bool>,
}

// --- Pulse ---

#[derive(Debug, Serialize)]
pub struct PulseResponse {
    pub active_tasks: Vec<PulseTask>,
    pub blocked_tasks: Vec<PulseTask>,
    pub pending_review: Vec<PulseTask>,
    pub recently_completed: Vec<PulseTask>,
    pub unread_events: i64,
    pub agents: Vec<PulseAgent>,
    pub recent_knowledge_updates: Vec<PulseKnowledge>,
    /// Number of tasks currently blocked by unmet dependencies
    pub blocked_by_deps: i64,
}

#[derive(Debug, Serialize)]
pub struct PulseTask {
    pub id: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub assignee_name: Option<String>,
    pub reviewer_name: Option<String>,
    pub tags: Vec<String>,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct PulseAgent {
    pub id: String,
    pub name: String,
    pub status: String,
    pub seniority: String,
    pub role: String,
    pub current_task: Option<String>,
    pub last_seen_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PulseKnowledge {
    pub key: String,
    pub title: String,
    pub category: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct AgentQuery {
    pub capability: Option<String>,
    pub seniority: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AgentMatchQuery {
    pub capability: Option<String>,
    pub seniority: Option<String>,
    pub role: Option<String>,
}

/// Strategy for auto-assigning agents based on capability, seniority, or explicit ID
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignStrategy {
    pub strategy: String,
    pub capabilities: Option<Vec<String>>,
    pub seniority: Option<String>,
    pub role: Option<String>,
    pub agent_id: Option<String>,
}

// --- Capability Targeting ---

#[derive(Debug, Clone)]
pub struct CapabilityTarget {
    pub target_type: String,
    pub target_id: String,
}

// ===== Task Questions =====

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskQuestion {
    pub id: String,
    pub task_id: String,
    pub question: String,
    pub question_type: String,
    pub context: Option<String>,
    pub asked_by_type: String,
    pub asked_by_id: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub required_capability: Option<String>,
    pub status: String,
    pub blocking: bool,
    pub resolved_by_type: Option<String>,
    pub resolved_by_id: Option<String>,
    pub resolution: Option<String>,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateQuestion {
    pub question: String,
    pub question_type: Option<String>,
    pub context: Option<String>,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub required_capability: Option<String>,
    pub blocking: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ResolveQuestion {
    pub resolution: String,
}

#[derive(Debug, Deserialize)]
pub struct QuestionQuery {
    pub status: Option<String>,
    pub unrouted: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionReply {
    pub id: String,
    pub question_id: String,
    pub author_type: String,
    pub author_id: String,
    pub body: String,
    pub is_resolution: bool,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateReply {
    pub body: String,
    pub is_resolution: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct DismissQuestion {
    pub reason: String,
}

#[derive(Debug, Deserialize)]
pub struct AssignQuestion {
    pub target_type: String,
    pub target_id: String,
}

// ===== Agent Inbox =====

#[derive(Debug, Serialize)]
pub struct InboxItem {
    pub id: String,
    pub item_type: String,
    pub title: String,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub action: String,
    pub action_hint: String,
    pub project_id: Option<String>,
    pub tags: Vec<String>,
    pub updated_at: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct InboxCapacity {
    pub max_concurrent_tasks: i64,
    pub current_active_tasks: i64,
    pub has_capacity: bool,
}

#[derive(Debug, Serialize)]
pub struct AgentInbox {
    pub summary: String,
    pub todo_tasks: Vec<InboxItem>,
    pub in_progress_tasks: Vec<InboxItem>,
    pub review_tasks: Vec<InboxItem>,
    pub blocked_tasks: Vec<InboxItem>,
    pub handoff_tasks: Vec<InboxItem>,
    pub open_questions: Vec<InboxItem>,
    pub unread_notifications: Vec<InboxItem>,
    pub capacity: InboxCapacity,
}

// ===== Inbound Webhook Triggers =====

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WebhookTrigger {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub action_type: String,
    pub action_config: serde_json::Value,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Returned only on creation (raw secret is not stored)
#[derive(Debug, Serialize)]
pub struct TriggerCreatedResponse {
    pub trigger: WebhookTrigger,
    pub secret: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateTriggerRequest {
    pub name: String,
    pub action_type: String,
    pub action_config: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WebhookTriggerLog {
    pub id: String,
    pub trigger_id: String,
    pub received_at: String,
    pub status: String,
    pub payload: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}
