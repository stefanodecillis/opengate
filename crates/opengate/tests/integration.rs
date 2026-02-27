use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use opengate::app::{build_router, AppState};
use opengate::db;
use opengate::db_ops;
use opengate::storage::sqlite::SqliteBackend;
use opengate_models::CreateAgent;

/// A self-contained test server with its own temp DB, agent, and random port.
struct TestServer {
    base_url: String,
    api_key: String,
    agent_id: String,
    _tmp: TempDir, // dropped (and cleaned up) when TestServer is dropped
}

impl TestServer {
    async fn start() -> Self {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let db_path = tmp.path().join("test.db");
        let db_path_str = db_path.to_str().unwrap();

        // Init fresh DB + create a test agent
        let conn = db::init_db(db_path_str);
        let (agent, api_key) = db_ops::create_agent(
            &conn,
            &CreateAgent::new("test-agent")
                .with_skills(vec!["rust".to_string(), "testing".to_string()])
                .with_seniority("senior"),
        );
        let agent_id = agent.id.clone();

        let storage = SqliteBackend::new(Arc::new(Mutex::new(conn)));
        let state = AppState {
            storage: Arc::new(storage),
            setup_token: "test-setup-token".to_string(),
            event_bus: opengate::events::EventBus::default(),
        };

        let router = build_router(state);

        // Bind to port 0 → OS picks a free port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        TestServer {
            base_url: format!("http://{addr}"),
            api_key,
            agent_id,
            _tmp: tmp,
        }
    }

    fn client(&self) -> Client {
        Client::new()
    }

    fn agent_id(&self) -> &str {
        &self.agent_id
    }

    fn auth_header(&self) -> String {
        format!("Bearer {}", self.api_key)
    }

    async fn create_project(&self, name: &str) -> Value {
        let resp = self
            .client()
            .post(format!("{}/api/projects", self.base_url))
            .header("Authorization", self.auth_header())
            .json(&json!({ "name": name, "description": "Integration test project" }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);
        resp.json::<Value>().await.unwrap()
    }

    fn ws_url(&self) -> String {
        self.base_url.replacen("http://", "ws://", 1) + "/api/ws"
    }

    async fn create_task(&self, project_id: &str, title: &str) -> Value {
        let resp = self
            .client()
            .post(format!(
                "{}/api/projects/{}/tasks",
                self.base_url, project_id
            ))
            .header("Authorization", self.auth_header())
            .json(&json!({
                "title": title,
                "description": "Test task",
                "priority": "high",
                "tags": ["rust", "testing"]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);
        resp.json::<Value>().await.unwrap()
    }
}

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WsMessage,
>;
type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

/// Connect to the WS endpoint, authenticate, and return the split (sink, stream) pair.
/// Panics if auth fails.
async fn ws_auth(url: &str, api_key: &str) -> (WsSink, WsStream) {
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("WS connect failed");
    let (mut sink, mut stream) = ws.split();

    // Send auth
    let auth_msg = json!({"type": "auth", "token": api_key}).to_string();
    sink.send(WsMessage::Text(auth_msg.into())).await.unwrap();

    // Read auth_ok
    let resp = recv_json(&mut stream, 2000)
        .await
        .expect("expected auth_ok");
    assert_eq!(resp["type"], "auth_ok", "auth response: {resp}");

    (sink, stream)
}

/// Read the next JSON text message from the WS stream, skipping server pings.
/// Returns None on timeout.
async fn recv_json(stream: &mut WsStream, timeout_ms: u64) -> Option<Value> {
    let deadline = std::time::Duration::from_millis(timeout_ms);
    loop {
        match tokio::time::timeout(deadline, stream.next()).await {
            Ok(Some(Ok(WsMessage::Text(ref text)))) => {
                let v: Value = serde_json::from_str(text.as_ref()).expect("invalid JSON from WS");
                // Skip server pings
                if v["type"] == "ping" {
                    continue;
                }
                return Some(v);
            }
            Ok(Some(Ok(WsMessage::Close(_)))) | Ok(None) => return None,
            Ok(Some(Ok(_))) => continue, // skip binary/ping/pong frames
            Ok(Some(Err(e))) => panic!("WS read error: {e}"),
            Err(_) => return None, // timeout
        }
    }
}

// 1. Agent identity: GET /api/auth/me returns agent info
#[tokio::test]
async fn test_agent_identity() {
    let s = TestServer::start().await;
    let resp = s
        .client()
        .get(format!("{}/api/auth/me", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["type"], "agent");
    assert!(body["id"].is_string());
    assert!(body["name"].is_string());
}

// 2. Create project
#[tokio::test]
async fn test_create_project() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Create").await;
    assert!(project["id"].is_string());
    assert_eq!(project["name"], "Test Project - Create");
    assert_eq!(project["status"], "active");
}

// 3. List projects includes newly created project
#[tokio::test]
async fn test_list_projects() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - List").await;
    let project_id = project["id"].as_str().unwrap();

    let resp = s
        .client()
        .get(format!("{}/api/projects", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let projects: Vec<Value> = resp.json().await.unwrap();
    assert!(projects
        .iter()
        .any(|p| p["id"].as_str() == Some(project_id)));
}

// 4. Create task in a project (starts in backlog)
#[tokio::test]
async fn test_create_task() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Task Create").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Task Create").await;

    assert!(task["id"].is_string());
    assert_eq!(task["title"], "Test Task Create");
    assert_eq!(task["status"], "backlog");
    assert_eq!(task["priority"], "high");
    assert_eq!(task["project_id"].as_str().unwrap(), pid);
    let tags = task["tags"].as_array().unwrap();
    assert!(tags.contains(&json!("rust")));
    assert!(tags.contains(&json!("testing")));
}

// 5. Get task by ID
#[tokio::test]
async fn test_get_task() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Get Task").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Get Task").await;
    let task_id = task["id"].as_str().unwrap();

    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let fetched: Value = resp.json().await.unwrap();
    assert_eq!(fetched["id"].as_str().unwrap(), task_id);
    assert_eq!(fetched["title"], "Test Get Task");
}

// 6. Claim task transitions backlog -> in_progress and assigns agent
#[tokio::test]
async fn test_claim_task() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Claim").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Claim Task").await;
    let task_id = task["id"].as_str().unwrap();
    assert_eq!(task["status"], "backlog");

    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let claimed: Value = resp.json().await.unwrap();
    assert_eq!(claimed["status"], "in_progress");
    assert_eq!(claimed["assignee_type"], "agent");
    assert!(claimed["assignee_id"].is_string());
}

// 7. Idempotent claim: claiming again by same agent returns 200
#[tokio::test]
async fn test_idempotent_claim() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Idempotent").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Idempotent Claim").await;
    let task_id = task["id"].as_str().unwrap();

    // First claim
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Second claim — idempotent, should still be 200
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "in_progress");
}

// 8. Update context with merge-patch
#[tokio::test]
async fn test_update_context() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Context").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Context Update").await;
    let task_id = task["id"].as_str().unwrap();

    // First patch
    let resp = s
        .client()
        .patch(format!("{}/api/tasks/{}/context", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "repo_url": "https://github.com/test/repo", "branch": "main" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["context"]["repo_url"], "https://github.com/test/repo");
    assert_eq!(body["context"]["branch"], "main");

    // Second patch — merge, not replace
    let resp = s
        .client()
        .patch(format!("{}/api/tasks/{}/context", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "notes": "Additional context" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["context"]["repo_url"], "https://github.com/test/repo");
    assert_eq!(body["context"]["notes"], "Additional context");
}

// 9. Complete task from in_progress
#[tokio::test]
async fn test_complete_task() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Complete").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Complete Task").await;
    let task_id = task["id"].as_str().unwrap();

    // Claim first (backlog -> in_progress)
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    // Complete
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/complete", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "summary": "All tests passing",
            "output": { "tests_passed": 10 }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "done");
    assert_eq!(body["output"]["tests_passed"], 10);
}

// 10. Post and list activity on a task
#[tokio::test]
async fn test_task_activity() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Activity").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Activity Task").await;
    let task_id = task["id"].as_str().unwrap();

    // Post a comment
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/activity", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "content": "Starting work on this task",
            "activity_type": "comment"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let activity: Value = resp.json().await.unwrap();
    assert_eq!(activity["content"], "Starting work on this task");
    assert_eq!(activity["activity_type"], "comment");
    assert_eq!(activity["task_id"].as_str().unwrap(), task_id);

    // List activity — should contain creation + our comment
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}/activity", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let activities: Vec<Value> = resp.json().await.unwrap();
    assert!(activities.len() >= 2); // at least creation + our comment
    assert!(activities
        .iter()
        .any(|a| a["content"] == "Starting work on this task"));
}

// 11. Agent heartbeat
#[tokio::test]
async fn test_agent_heartbeat() {
    let s = TestServer::start().await;
    let resp = s
        .client()
        .post(format!("{}/api/agents/heartbeat", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

// 12. My tasks: lists tasks assigned to the authenticated agent
#[tokio::test]
async fn test_my_tasks() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - My Tasks").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test My Tasks").await;
    let task_id = task["id"].as_str().unwrap();

    // Claim the task
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    // Get my tasks
    let resp = s
        .client()
        .get(format!("{}/api/tasks/mine", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let tasks: Vec<Value> = resp.json().await.unwrap();
    assert!(tasks.iter().any(|t| t["id"].as_str() == Some(task_id)));
}

// 13. Invalid status transition returns 400
#[tokio::test]
async fn test_invalid_status_transition() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Invalid Transition").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Invalid Transition").await;
    let task_id = task["id"].as_str().unwrap();
    // Task is in backlog; trying to go directly to done should fail
    let resp = s
        .client()
        .patch(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "done" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("Invalid status transition"));
}

// 14. Next task discovery returns highest-priority unassigned task
#[tokio::test]
async fn test_next_task_discovery() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Next Task").await;
    let pid = project["id"].as_str().unwrap();

    // Create low-priority task
    let resp = s
        .client()
        .post(format!("{}/api/projects/{}/tasks", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .json(&json!({ "title": "Low prio task", "priority": "low", "tags": ["next-test"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Create critical-priority task
    let resp = s
        .client()
        .post(format!("{}/api/projects/{}/tasks", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .json(&json!({ "title": "Critical prio task", "priority": "critical", "tags": ["next-test"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Get next task with matching skill
    let resp = s
        .client()
        .get(format!("{}/api/tasks/next?skills=next-test", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let task: Value = resp.json().await.unwrap();
    // Should return the critical one first
    assert_eq!(task["priority"], "critical");
    assert_eq!(task["title"], "Critical prio task");
}

// 15. Release task returns it to the pool
#[tokio::test]
async fn test_release_task() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Release").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Release Task").await;
    let task_id = task["id"].as_str().unwrap();

    // Claim
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    // Release
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/release", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "todo");
    assert!(body["assignee_id"].is_null());
    assert!(body["assignee_type"].is_null());
}

// 17. Cannot complete task from backlog (must be in_progress or review)
#[tokio::test]
async fn test_cannot_complete_from_backlog() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - No Complete").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test No Complete From Backlog").await;
    let task_id = task["id"].as_str().unwrap();

    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/complete", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "summary": "Trying to skip" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("Cannot complete task"));
}

// 18. Dashboard stats endpoint
#[tokio::test]
async fn test_dashboard_stats() {
    let s = TestServer::start().await;
    let resp = s
        .client()
        .get(format!("{}/api/stats", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["total_tasks"].is_number());
    assert!(body["total_projects"].is_number());
    assert!(body["active_agents"].is_number());
    assert!(body["tasks_by_status"].is_object());
}

// ===== v2 Tests =====

// 19. Get agent profile returns computed status fields
#[tokio::test]
async fn test_get_agent_profile() {
    let s = TestServer::start().await;
    // First get the agent list to find our agent's ID
    let resp = s
        .client()
        .get(format!("{}/api/agents", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let agents: Vec<Value> = resp.json().await.unwrap();
    assert!(!agents.is_empty());
    let agent_id = agents[0]["id"].as_str().unwrap();

    // Get individual agent profile
    let resp = s
        .client()
        .get(format!("{}/api/agents/{}", s.base_url, agent_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let agent: Value = resp.json().await.unwrap();
    assert_eq!(agent["name"], "test-agent");
    assert!(agent["status"].is_string());
    assert!(agent["max_concurrent_tasks"].is_number());
    assert!(agent["current_task_count"].is_number());
}

// 20. Update agent profile
#[tokio::test]
async fn test_update_agent_profile() {
    let s = TestServer::start().await;
    let resp = s
        .client()
        .get(format!("{}/api/agents", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let agents: Vec<Value> = resp.json().await.unwrap();
    let agent_id = agents[0]["id"].as_str().unwrap();

    let resp = s
        .client()
        .patch(format!("{}/api/agents/{}", s.base_url, agent_id))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "description": "A test agent for integration tests",
            "max_concurrent_tasks": 3,
            "webhook_url": "https://example.com/webhook"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let agent: Value = resp.json().await.unwrap();
    assert_eq!(agent["description"], "A test agent for integration tests");
    assert_eq!(agent["max_concurrent_tasks"], 3);
    assert_eq!(agent["webhook_url"], "https://example.com/webhook");
}

// 21. Manual assignment via POST /api/tasks/:id/assign
#[tokio::test]
async fn test_assign_task() {
    let s = TestServer::start().await;
    // Get agent ID
    let resp = s
        .client()
        .get(format!("{}/api/agents", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let agents: Vec<Value> = resp.json().await.unwrap();
    let agent_id = agents[0]["id"].as_str().unwrap();

    // Heartbeat so agent is not offline
    s.client()
        .post(format!("{}/api/agents/heartbeat", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    let project = s.create_project("Test Project - Assign").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Assign Task").await;
    let task_id = task["id"].as_str().unwrap();

    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/assign", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "agent_id": agent_id }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "todo");
    assert_eq!(body["assignee_id"].as_str().unwrap(), agent_id);
    assert_eq!(body["assignee_type"], "agent");
}

// 22. Knowledge base CRUD
#[tokio::test]
async fn test_knowledge_base_crud() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - KB").await;
    let pid = project["id"].as_str().unwrap();

    // Upsert a knowledge entry
    let resp = s
        .client()
        .put(format!(
            "{}/api/projects/{}/knowledge/architecture%2Foverview",
            s.base_url, pid
        ))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "title": "Architecture Overview",
            "content": "TaskForge uses Axum + SQLite",
            "metadata": { "type": "doc" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let entry: Value = resp.json().await.unwrap();
    assert_eq!(entry["key"], "architecture/overview");
    assert_eq!(entry["content"], "TaskForge uses Axum + SQLite");
    assert_eq!(entry["metadata"]["type"], "doc");

    // Get by key
    let resp = s
        .client()
        .get(format!(
            "{}/api/projects/{}/knowledge/architecture%2Foverview",
            s.base_url, pid
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let entry: Value = resp.json().await.unwrap();
    assert_eq!(entry["content"], "TaskForge uses Axum + SQLite");

    // List all
    let resp = s
        .client()
        .get(format!("{}/api/projects/{}/knowledge", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let entries: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(entries.len(), 1);

    // Search
    let resp = s
        .client()
        .get(format!(
            "{}/api/projects/{}/knowledge/search?q=Axum",
            s.base_url, pid
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let results: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["key"], "architecture/overview");

    // Delete
    let resp = s
        .client()
        .delete(format!(
            "{}/api/projects/{}/knowledge/architecture%2Foverview",
            s.base_url, pid
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    // Verify deleted
    let resp = s
        .client()
        .get(format!(
            "{}/api/projects/{}/knowledge/architecture%2Foverview",
            s.base_url, pid
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// 23. Review flow: in_progress -> review -> approve -> done
#[tokio::test]
async fn test_review_approve_flow() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Review").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Review Flow").await;
    let task_id = task["id"].as_str().unwrap();

    // Claim (backlog -> in_progress)
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    // Move to review
    let resp = s
        .client()
        .patch(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "review" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "review");

    // Approve
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/approve", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "comment": "Looks good!" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "done");
}

// 24. Review flow: request changes sends task back to in_progress
#[tokio::test]
async fn test_review_request_changes() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Changes").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Request Changes").await;
    let task_id = task["id"].as_str().unwrap();

    // Claim + move to review
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    s.client()
        .patch(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "review" }))
        .send()
        .await
        .unwrap();

    // Request changes
    let resp = s
        .client()
        .post(format!(
            "{}/api/tasks/{}/request-changes",
            s.base_url, task_id
        ))
        .header("Authorization", s.auth_header())
        .json(&json!({ "comment": "Please fix the error handling" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "in_progress"); // handoff back to executor — they fix and re-submit for review
}

// 25. Handoff status transition: in_progress -> handoff -> in_progress
#[tokio::test]
async fn test_handoff_status() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Handoff").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Handoff").await;
    let task_id = task["id"].as_str().unwrap();

    // Claim
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    // Move to handoff
    let resp = s
        .client()
        .patch(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "handoff" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "handoff");

    // Move back to in_progress
    let resp = s
        .client()
        .patch(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "in_progress" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "in_progress");
}

// 26. Downstream output injection: completing parent injects output into child context
#[tokio::test]
async fn test_downstream_output_injection() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Downstream").await;
    let pid = project["id"].as_str().unwrap();

    // Create parent task
    let parent = s.create_task(pid, "Parent Task").await;
    let parent_id = parent["id"].as_str().unwrap();

    // Create child task with dependency on parent
    let resp = s
        .client()
        .post(format!("{}/api/projects/{}/tasks", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "title": "Child Task",
            "priority": "medium",
            "context": { "dependencies": [parent_id] }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let child: Value = resp.json().await.unwrap();
    let child_id = child["id"].as_str().unwrap();

    // Claim and complete parent with output
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, parent_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    s.client()
        .post(format!("{}/api/tasks/{}/complete", s.base_url, parent_id))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "summary": "Parent done",
            "output": { "pr_url": "https://github.com/test/pr/1" }
        }))
        .send()
        .await
        .unwrap();

    // Check child's context for upstream_outputs
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}", s.base_url, child_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let child_task: Value = resp.json().await.unwrap();
    assert!(child_task["context"]["upstream_outputs"][parent_id].is_object());
    assert_eq!(
        child_task["context"]["upstream_outputs"][parent_id]["output"]["pr_url"],
        "https://github.com/test/pr/1"
    );
}

// 27. Agent self-registration with setup token
#[tokio::test]
async fn test_agent_self_registration() {
    let s = TestServer::start().await;
    let resp = s
        .client()
        .post(format!("{}/api/agents/register", s.base_url))
        .json(&json!({
            "name": "self-registered-agent",
            "skills": ["python", "ml"],
            "setup_token": "test-setup-token"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: Value = resp.json().await.unwrap();
    assert!(body["api_key"].is_string());
    assert!(body["agent"]["id"].is_string());
    assert_eq!(body["agent"]["name"], "self-registered-agent");
}

// 28. Schema endpoint includes v2 endpoints
#[tokio::test]
async fn test_schema_includes_v2() {
    let s = TestServer::start().await;
    let resp = s
        .client()
        .get(format!("{}/api/schema", s.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let schema: Value = resp.json().await.unwrap();
    let endpoints = schema["endpoints"].as_array().unwrap();

    // Check v2 endpoints exist
    let paths: Vec<&str> = endpoints
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"/api/tasks/{id}/assign"));
    assert!(paths.contains(&"/api/tasks/{id}/handoff"));
    assert!(paths.contains(&"/api/tasks/{id}/approve"));
    assert!(paths.contains(&"/api/tasks/{id}/request-changes"));
    assert!(paths.contains(&"/api/agents/{id}"));
    assert!(paths.contains(&"/api/projects/{id}/knowledge"));
    assert!(paths.contains(&"/api/projects/{id}/knowledge/search"));
    assert!(paths.contains(&"/api/projects/{id}/knowledge/{key}"));

    // Check handoff in status_flow
    assert!(schema["status_flow"]["handoff"].is_array());
}

// Offline agent assignment — should succeed with warning, not block
#[tokio::test]
async fn test_assign_to_offline_agent() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Offline Assign").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Offline Assignment").await;
    let task_id = task["id"].as_str().unwrap();

    // Agent has never sent heartbeat → offline status
    // Assign should still work
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/assign", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "agent_id": s.agent_id() }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "todo");
    assert!(body["assignee_id"].as_str().is_some());
}

// Stale release should NOT auto-release tasks in review status
#[tokio::test]
async fn test_stale_release_preserves_review() {
    let s = TestServer::start().await;

    // Register a second agent to serve as reviewer (via setup-token registration)
    let resp = s.client()
        .post(format!("{}/api/agents/register", s.base_url))
        .json(&json!({"name": "reviewer-agent", "setup_token": "test-setup-token", "skills": ["rust"]}))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "Failed to register reviewer agent: {}",
        resp.status()
    );
    let reviewer: Value = resp.json().await.unwrap();
    let reviewer_id = reviewer["agent"]["id"].as_str().unwrap().to_string();

    let project = s.create_project("Test Project - Stale Release").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Review Preservation").await;
    let task_id = task["id"].as_str().unwrap();

    // Claim
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    // Submit for review (explicitly nominate the reviewer agent)
    let resp = s
        .client()
        .post(format!(
            "{}/api/tasks/{}/submit-review",
            s.base_url, task_id
        ))
        .header("Authorization", s.auth_header())
        .json(&json!({"reviewer_id": reviewer_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "review");

    // Verify task stays in review even without heartbeat
    // (release_stale_tasks excludes review status)
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "review");
    assert!(body["assignee_id"].as_str().is_some());
}

// ============================================================
// Webhook notification tests
// ============================================================

/// Spawn a tiny axum server that records incoming POST payloads and
/// responds with `response_status`. Returns (url, Arc to recorded bodies).
async fn start_mock_webhook(response_status: u16) -> (String, Arc<tokio::sync::Mutex<Vec<Value>>>) {
    use axum::extract::State;
    use axum::{http::StatusCode, routing::post, Router};

    let received: Arc<tokio::sync::Mutex<Vec<Value>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let received_clone = received.clone();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();

    let app = Router::new()
        .route(
            "/hook",
            post(
                move |State(state): State<(Arc<tokio::sync::Mutex<Vec<Value>>>, StatusCode)>,
                      body: axum::body::Bytes| async move {
                    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    state.0.lock().await.push(parsed);
                    state.1
                },
            ),
        )
        .with_state((
            received_clone,
            StatusCode::from_u16(response_status).unwrap(),
        ));

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://{}/hook", addr), received)
}

/// Helper: set webhook_url (and optional webhook_events) on the test agent.
async fn set_agent_webhook(s: &TestServer, url: &str, events: Option<Vec<&str>>) {
    let mut body = json!({ "webhook_url": url });
    if let Some(ev) = events {
        body["webhook_events"] = json!(ev);
    }
    let resp = s
        .client()
        .patch(format!("{}/api/agents/{}", s.base_url, s.agent_id()))
        .header("Authorization", s.auth_header())
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "Failed to update agent webhook config");
}

/// Helper: create a project + task and assign it to the test agent.
/// Returns task_id.
async fn assign_task_to_self(s: &TestServer, project_suffix: &str) -> String {
    let project = s
        .create_project(&format!("Webhook Test Project {}", project_suffix))
        .await;
    let pid = project["id"].as_str().unwrap();
    let task = s
        .create_task(pid, &format!("Webhook Test Task {}", project_suffix))
        .await;
    let task_id = task["id"].as_str().unwrap().to_string();

    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/assign", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "agent_id": s.agent_id() }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "Failed to assign task: {}",
        resp.text().await.unwrap()
    );
    task_id
}

/// Helper: fetch unread notifications for the test agent.
async fn unread_notifications(s: &TestServer) -> Vec<Value> {
    let resp = s
        .client()
        .get(format!(
            "{}/api/agents/me/notifications?unread=true",
            s.base_url
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    resp.json::<Value>()
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

/// Helper: fetch all notifications for the test agent.
async fn all_notifications(s: &TestServer) -> Vec<Value> {
    let resp = s
        .client()
        .get(format!("{}/api/agents/me/notifications", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    resp.json::<Value>()
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

// ------ Test 1: successful delivery → webhook fired + notification auto-acked ------

#[tokio::test]
async fn test_webhook_delivery_auto_ack() {
    let s = TestServer::start().await;
    let (hook_url, received) = start_mock_webhook(200).await;

    set_agent_webhook(&s, &hook_url, None).await;
    assign_task_to_self(&s, "auto-ack").await;

    // Give the async webhook task time to fire and process
    tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;

    // Notification should be auto-acked after successful delivery
    let unread = unread_notifications(&s).await;
    assert!(
        unread.is_empty(),
        "Expected no unread notifications after auto-ack, found: {}",
        unread.len()
    );

    // Webhook should have been called
    let reqs = received.lock().await;
    assert!(!reqs.is_empty(), "Mock webhook server received no requests");
    // Verify event_type in payload
    let notification_reqs: Vec<_> = reqs
        .iter()
        .filter(|r| r.get("event").and_then(|e| e.as_str()) == Some("notification"))
        .collect();
    assert!(
        !notification_reqs.is_empty(),
        "Expected at least one notification webhook payload"
    );
    let ev_type = notification_reqs[0]["event_type"].as_str().unwrap_or("");
    assert_eq!(ev_type, "task.assigned");

    // webhook_status in DB should be "delivered"
    let all = all_notifications(&s).await;
    let notif = all
        .iter()
        .find(|n| n["event_type"].as_str() == Some("task.assigned"))
        .expect("task.assigned notification missing");
    assert_eq!(
        notif["webhook_status"].as_str(),
        Some("delivered"),
        "webhook_status should be 'delivered'"
    );
}

// ------ Test 2: event filter — subscribed only to review_requested, not assigned ------

#[tokio::test]
async fn test_webhook_event_filter_blocks_unsubscribed() {
    let s = TestServer::start().await;
    let (hook_url, received) = start_mock_webhook(200).await;

    // Subscribe only to task.review_requested — task.assigned should be filtered
    set_agent_webhook(&s, &hook_url, Some(vec!["task.review_requested"])).await;
    assign_task_to_self(&s, "event-filter").await;

    tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;

    // Webhook should NOT have fired for task.assigned
    let reqs = received.lock().await;
    let notification_reqs: Vec<_> = reqs
        .iter()
        .filter(|r| {
            r.get("event").and_then(|e| e.as_str()) == Some("notification")
                && r.get("event_type").and_then(|e| e.as_str()) == Some("task.assigned")
        })
        .collect();
    assert!(
        notification_reqs.is_empty(),
        "Notification webhook should have been filtered out for task.assigned"
    );
    drop(reqs);

    // Notification should still be unread (not auto-acked because webhook was never sent)
    let unread = unread_notifications(&s).await;
    assert!(
        !unread.is_empty(),
        "Notification should remain unread when webhook was filtered out"
    );
    // webhook_status should be null (delivery never attempted)
    let notif = unread
        .iter()
        .find(|n| n["event_type"].as_str() == Some("task.assigned"))
        .expect("task.assigned notification missing");
    assert!(
        notif["webhook_status"].is_null(),
        "webhook_status should be null when delivery was not attempted (filtered)"
    );
}

// ------ Test 3: failed delivery → notification stays unread, status = "failed" ------

/// NOTE: This test waits ~6 s for the 3-retry backoff to complete (1 s + 4 s).
#[tokio::test]
async fn test_webhook_failed_delivery_leaves_unread() {
    let s = TestServer::start().await;

    // Use a port that is (almost certainly) not listening → immediate connection refused
    let hook_url = "http://127.0.0.1:39876";

    set_agent_webhook(&s, hook_url, None).await;
    assign_task_to_self(&s, "failed-delivery").await;

    // Backoff: 1 s after attempt 1, 4 s after attempt 2, then done → wait ≥6 s
    tokio::time::sleep(tokio::time::Duration::from_secs(7)).await;

    // Notification should still be unread
    let unread = unread_notifications(&s).await;
    assert!(
        !unread.is_empty(),
        "Notification should remain unread after failed webhook delivery"
    );

    // webhook_status should be "failed"
    let all = all_notifications(&s).await;
    let notif = all
        .iter()
        .find(|n| n["event_type"].as_str() == Some("task.assigned"))
        .expect("task.assigned notification missing");
    assert_eq!(
        notif["webhook_status"].as_str(),
        Some("failed"),
        "webhook_status should be 'failed' after exhausting all retries"
    );
}

// ═══════════════════════════════════════════════════════
// v4: Task Dependencies — schema + API + cycle detection
// ═══════════════════════════════════════════════════════

#[tokio::test]
async fn test_task_dependencies_crud_and_auto_unblock() {
    let s = TestServer::start().await;
    let proj = s.create_project("deps-test").await;
    let pid = proj["id"].as_str().unwrap();

    // Create two tasks: A depends on B
    let a = s.create_task(pid, "Task A").await;
    let b = s.create_task(pid, "Task B").await;
    let a_id = a["id"].as_str().unwrap();
    let b_id = b["id"].as_str().unwrap();

    // POST /api/tasks/A/dependencies { depends_on: [B] }
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/dependencies", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "depends_on": [b_id] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "add dependency should succeed");
    let task_a: Value = resp.json().await.unwrap();
    assert!(
        task_a["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d.as_str() == Some(b_id)),
        "Task A should list B in its dependencies"
    );

    // GET /api/tasks/A/dependencies
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}/dependencies", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let deps: Value = resp.json().await.unwrap();
    assert_eq!(deps.as_array().unwrap().len(), 1);
    assert_eq!(deps[0]["id"].as_str(), Some(b_id));

    // GET /api/tasks/B/dependents
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}/dependents", s.base_url, b_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let dependents: Value = resp.json().await.unwrap();
    assert_eq!(dependents.as_array().unwrap().len(), 1);
    assert_eq!(dependents[0]["id"].as_str(), Some(a_id));

    // Attempt to move A to in_progress — should fail (B is not done)
    // First move A to todo (valid from backlog)
    s.client()
        .patch(format!("{}/api/tasks/{}", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "todo" }))
        .send()
        .await
        .unwrap();
    // Now try in_progress — should fail with 409 Conflict due to unmet dep
    let resp = s
        .client()
        .patch(format!("{}/api/tasks/{}", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "in_progress" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        409,
        "should block in_progress with 409 when dep not met"
    );

    // Claim B → in_progress, then complete it → A should auto-transition to todo
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, b_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    s.client()
        .post(format!("{}/api/tasks/{}/complete", s.base_url, b_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "summary": "B done" }))
        .send()
        .await
        .unwrap();

    // Check A is now todo
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let task_a_after: Value = resp.json().await.unwrap();
    assert_eq!(
        task_a_after["status"].as_str(),
        Some("todo"),
        "A should be auto-unblocked to todo after B completes"
    );

    // DELETE dependency
    let resp = s
        .client()
        .delete(format!(
            "{}/api/tasks/{}/dependencies/{}",
            s.base_url, a_id, b_id
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204, "delete dependency should return 204");
}

#[tokio::test]
async fn test_task_dependency_cycle_detection() {
    let s = TestServer::start().await;
    let proj = s.create_project("cycle-test").await;
    let pid = proj["id"].as_str().unwrap();

    let a = s.create_task(pid, "Cycle A").await;
    let b = s.create_task(pid, "Cycle B").await;
    let a_id = a["id"].as_str().unwrap();
    let b_id = b["id"].as_str().unwrap();

    // A depends on B
    s.client()
        .post(format!("{}/api/tasks/{}/dependencies", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "depends_on": [b_id] }))
        .send()
        .await
        .unwrap();

    // B depends on A → cycle! Should be rejected.
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/dependencies", s.base_url, b_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "depends_on": [a_id] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "cycle should be rejected");
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("cycle"),
        "error message should mention cycle"
    );

    // Self-dependency should also be rejected
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/dependencies", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "depends_on": [a_id] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "self-dependency should be rejected");
}

// ═══════════════════════════════════════════════════════
// v4: Task Scheduling — scheduled_at + auto-transition
// ═══════════════════════════════════════════════════════

#[tokio::test]
async fn test_task_scheduling_create_and_enforce() {
    let s = TestServer::start().await;
    let proj = s.create_project("sched-test").await;
    let pid = proj["id"].as_str().unwrap();

    // Create a task scheduled in the future
    let future_time = "2099-01-01T00:00:00Z";
    let resp = s
        .client()
        .post(format!("{}/api/projects/{}/tasks", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "title": "Future Task",
            "priority": "high",
            "scheduled_at": future_time
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let task: Value = resp.json().await.unwrap();
    let task_id = task["id"].as_str().unwrap();
    assert_eq!(
        task["scheduled_at"].as_str(),
        Some(future_time),
        "scheduled_at should be persisted"
    );

    // Cannot manually advance a future-scheduled task (to todo or in_progress)
    let resp = s
        .client()
        .patch(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "todo" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "future-scheduled task should block manual advance to todo"
    );
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap_or("").contains("scheduled"),
        "error should mention scheduling"
    );

    // Schedule listing: GET /api/projects/:id/schedule
    let resp = s
        .client()
        .get(format!("{}/api/projects/{}/schedule", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let schedule: Value = resp.json().await.unwrap();
    let entries = schedule.as_array().unwrap();
    assert!(
        !entries.is_empty(),
        "schedule should include the future task"
    );
    let entry = entries
        .iter()
        .find(|e| e["id"].as_str() == Some(task_id))
        .expect("task should be in schedule");
    assert_eq!(entry["scheduled_at"].as_str(), Some(future_time));
}

#[tokio::test]
async fn test_scheduled_task_auto_transition() {
    let s = TestServer::start().await;
    let proj = s.create_project("auto-sched-test").await;
    let pid = proj["id"].as_str().unwrap();

    // Create a task scheduled in the past (should be ready to transition)
    let past_time = "2020-01-01T00:00:00Z";
    let resp = s
        .client()
        .post(format!("{}/api/projects/{}/tasks", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "title": "Past Scheduled Task",
            "priority": "medium",
            "scheduled_at": past_time
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let task: Value = resp.json().await.unwrap();
    let task_id = task["id"].as_str().unwrap();

    // Trigger auto-transition
    let resp = s
        .client()
        .post(format!("{}/api/tasks/scheduled/transition", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let result: Value = resp.json().await.unwrap();
    assert!(
        result["transitioned"].as_i64().unwrap_or(0) >= 1,
        "should have transitioned at least one task"
    );

    // Task should now be todo
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let task_after: Value = resp.json().await.unwrap();
    assert_eq!(
        task_after["status"].as_str(),
        Some("todo"),
        "past-scheduled task should auto-transition to todo"
    );
}

// ═══════════════════════════════════════════════════════
// v4: Task Recurrence — rules + auto-creation on complete
// ═══════════════════════════════════════════════════════

#[tokio::test]
async fn test_task_recurrence_creates_next_on_complete() {
    let s = TestServer::start().await;
    let proj = s.create_project("recurrence-test").await;
    let pid = proj["id"].as_str().unwrap();

    // Create a daily recurring task
    let resp = s
        .client()
        .post(format!("{}/api/projects/{}/tasks", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "title": "Daily Standup",
            "priority": "medium",
            "scheduled_at": "2026-02-23T09:00:00Z",
            "recurrence_rule": {
                "frequency": "daily",
                "interval": 1
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let task: Value = resp.json().await.unwrap();
    let task_id = task["id"].as_str().unwrap();
    assert!(
        task["recurrence_rule"].is_object(),
        "recurrence_rule should be persisted"
    );

    // Claim task → in_progress, then complete it
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/complete", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "summary": "Standup done" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "complete should succeed");

    // Check that a new recurrence was created in the project
    let resp = s
        .client()
        .get(format!("{}/api/projects/{}/tasks", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let tasks: Value = resp.json().await.unwrap();
    let task_list = tasks.as_array().unwrap();

    let next_recurrence = task_list.iter().find(|t| {
        t["recurrence_parent_id"].as_str() == Some(task_id)
            || (t["id"].as_str() != Some(task_id)
                && t["title"].as_str() == Some("Daily Standup")
                && t["status"].as_str() != Some("done"))
    });
    assert!(
        next_recurrence.is_some(),
        "completing a recurring task should create the next occurrence"
    );
    let next = next_recurrence.unwrap();
    assert_eq!(
        next["status"].as_str(),
        Some("backlog"),
        "next recurrence should start in backlog"
    );
    assert!(
        next["scheduled_at"].is_string(),
        "next recurrence should have a scheduled_at"
    );
    // scheduled_at should be ~1 day after the original
    assert_ne!(
        next["scheduled_at"].as_str(),
        Some("2026-02-23T09:00:00Z"),
        "next occurrence should have a different scheduled_at"
    );
}

#[tokio::test]
async fn test_task_recurrence_respects_end_after() {
    let s = TestServer::start().await;
    let proj = s.create_project("recurrence-end-test").await;
    let pid = proj["id"].as_str().unwrap();

    // Create a task that recurs only 1 more time after first completion
    let resp = s
        .client()
        .post(format!("{}/api/projects/{}/tasks", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "title": "Limited Recurrence",
            "priority": "low",
            "scheduled_at": "2026-02-23T09:00:00Z",
            "recurrence_rule": {
                "frequency": "daily",
                "interval": 1,
                "end_after": 1
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let task: Value = resp.json().await.unwrap();
    let task_id = task["id"].as_str().unwrap();

    // Claim original → complete → should create 1 recurrence
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    s.client()
        .post(format!("{}/api/tasks/{}/complete", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "summary": "done" }))
        .send()
        .await
        .unwrap();

    let resp = s
        .client()
        .get(format!("{}/api/projects/{}/tasks", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let tasks: Value = resp.json().await.unwrap();
    let children: Vec<_> = tasks
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["recurrence_parent_id"].as_str() == Some(task_id))
        .collect();
    assert_eq!(
        children.len(),
        1,
        "should have exactly 1 recurrence after first completion"
    );
    let child_id = children[0]["id"].as_str().unwrap().to_string();

    // Claim + complete the first recurrence → end_after=1 means no more
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, child_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    s.client()
        .post(format!("{}/api/tasks/{}/complete", s.base_url, child_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "summary": "done again" }))
        .send()
        .await
        .unwrap();

    let resp = s
        .client()
        .get(format!("{}/api/projects/{}/tasks", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let tasks_after: Value = resp.json().await.unwrap();
    let children_after: Vec<_> = tasks_after
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["recurrence_parent_id"].as_str() == Some(task_id))
        .collect();
    assert_eq!(
        children_after.len(),
        1,
        "end_after=1 means no additional recurrences after the first child completes"
    );
}

// ===== Inbound Webhook Triggers =====

#[tokio::test]
async fn test_webhook_trigger_create_and_list() {
    let s = TestServer::start().await;
    let proj = s.create_project("trigger-test").await;
    let pid = proj["id"].as_str().unwrap();

    // Create a trigger
    let resp: Value = s
        .client()
        .post(format!("{}/api/projects/{}/triggers", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "name": "CI Failure",
            "action_type": "create_task",
            "action_config": {
                "title": "CI failed on {{payload.repo}}",
                "description": "Branch: {{payload.branch}}",
                "priority": "high",
                "tags": ["ci", "{{payload.repo}}"]
            }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(resp["trigger"]["name"].as_str(), Some("CI Failure"));
    assert_eq!(resp["trigger"]["action_type"].as_str(), Some("create_task"));
    let secret = resp["secret"].as_str().unwrap();
    assert!(!secret.is_empty(), "should return raw secret on creation");

    let trigger_id = resp["trigger"]["id"].as_str().unwrap().to_string();

    // List triggers
    let list: Value = s
        .client()
        .get(format!("{}/api/projects/{}/triggers", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"].as_str(), Some(trigger_id.as_str()));

    // Secret is not exposed in list
    assert!(arr[0].get("secret").is_none() || arr[0]["secret"].is_null());
}

#[tokio::test]
async fn test_webhook_trigger_receive_creates_task() {
    let s = TestServer::start().await;
    let proj = s.create_project("trigger-recv").await;
    let pid = proj["id"].as_str().unwrap();

    let resp: Value = s
        .client()
        .post(format!("{}/api/projects/{}/triggers", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "name": "Create Task Trigger",
            "action_type": "create_task",
            "action_config": {
                "title": "Alert: {{payload.service}} is down",
                "description": "Region: {{payload.region}}",
                "priority": "high"
            }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let secret = resp["secret"].as_str().unwrap().to_string();
    let trigger_id = resp["trigger"]["id"].as_str().unwrap().to_string();

    // Fire the webhook
    let fire_resp = s
        .client()
        .post(format!(
            "{}/api/webhooks/trigger/{}",
            s.base_url, trigger_id
        ))
        .header("X-Webhook-Secret", &secret)
        .json(&json!({ "service": "payments-api", "region": "eu-west-1" }))
        .send()
        .await
        .unwrap();

    assert_eq!(fire_resp.status(), 200);
    let result: Value = fire_resp.json().await.unwrap();
    assert!(result["task_id"].is_string(), "should return task_id");

    // Verify task was created with interpolated title
    let task_id = result["task_id"].as_str().unwrap();
    let task: Value = s
        .client()
        .get(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(task["title"].as_str(), Some("Alert: payments-api is down"));
}

#[tokio::test]
async fn test_webhook_trigger_invalid_secret_rejected() {
    let s = TestServer::start().await;
    let proj = s.create_project("trigger-auth").await;
    let pid = proj["id"].as_str().unwrap();

    let resp: Value = s
        .client()
        .post(format!("{}/api/projects/{}/triggers", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "name": "Auth Test",
            "action_type": "create_task",
            "action_config": { "title": "Test", "priority": "low" }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let trigger_id = resp["trigger"]["id"].as_str().unwrap().to_string();

    // Wrong secret → 401
    let fire_resp = s
        .client()
        .post(format!(
            "{}/api/webhooks/trigger/{}",
            s.base_url, trigger_id
        ))
        .header("X-Webhook-Secret", "wrong-secret")
        .json(&json!({ "foo": "bar" }))
        .send()
        .await
        .unwrap();

    assert_eq!(fire_resp.status(), 401);

    // Missing secret → 401
    let fire_resp2 = s
        .client()
        .post(format!(
            "{}/api/webhooks/trigger/{}",
            s.base_url, trigger_id
        ))
        .json(&json!({ "foo": "bar" }))
        .send()
        .await
        .unwrap();

    assert_eq!(fire_resp2.status(), 401);
}

#[tokio::test]
async fn test_webhook_trigger_delete() {
    let s = TestServer::start().await;
    let proj = s.create_project("trigger-del").await;
    let pid = proj["id"].as_str().unwrap();

    let resp: Value = s
        .client()
        .post(format!("{}/api/projects/{}/triggers", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "name": "Delete Me",
            "action_type": "create_task",
            "action_config": { "title": "Deleted", "priority": "low" }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let trigger_id = resp["trigger"]["id"].as_str().unwrap().to_string();

    // Delete
    let del_resp = s
        .client()
        .delete(format!(
            "{}/api/projects/{}/triggers/{}",
            s.base_url, pid, trigger_id
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(del_resp.status(), 204);

    // List should be empty
    let list: Value = s
        .client()
        .get(format!("{}/api/projects/{}/triggers", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_webhook_trigger_logs_recorded() {
    let s = TestServer::start().await;
    let proj = s.create_project("trigger-logs").await;
    let pid = proj["id"].as_str().unwrap();

    let resp: Value = s
        .client()
        .post(format!("{}/api/projects/{}/triggers", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "name": "Log Test",
            "action_type": "create_task",
            "action_config": { "title": "Logged Task", "priority": "medium" }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let secret = resp["secret"].as_str().unwrap().to_string();
    let trigger_id = resp["trigger"]["id"].as_str().unwrap().to_string();

    // Fire with correct secret
    s.client()
        .post(format!(
            "{}/api/webhooks/trigger/{}",
            s.base_url, trigger_id
        ))
        .header("X-Webhook-Secret", &secret)
        .json(&json!({ "x": 1 }))
        .send()
        .await
        .unwrap();

    // Fire with wrong secret
    s.client()
        .post(format!(
            "{}/api/webhooks/trigger/{}",
            s.base_url, trigger_id
        ))
        .header("X-Webhook-Secret", "bad")
        .json(&json!({ "x": 2 }))
        .send()
        .await
        .unwrap();

    // Check logs
    let logs: Value = s
        .client()
        .get(format!(
            "{}/api/projects/{}/triggers/{}/logs",
            s.base_url, pid, trigger_id
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let arr = logs.as_array().unwrap();
    assert_eq!(arr.len(), 2, "should have 2 log entries");
    // Most recent first
    assert_eq!(
        arr[0]["status"].as_str(),
        Some("rejected"),
        "last entry is rejected"
    );
    assert_eq!(
        arr[1]["status"].as_str(),
        Some("success"),
        "first entry is success"
    );
}

// ===== User Enrichment Tests =====

/// Helper: register a human user and return (user_id, jwt_token)
#[allow(dead_code)]
async fn register_user(s: &TestServer, username: &str, password: &str) -> (String, String) {
    let resp = s
        .client()
        .post(format!("{}/api/auth/register", s.base_url))
        .json(&json!({ "username": username, "password": password }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let id = body["user"]["id"].as_str().unwrap().to_string();
    let token = body["token"].as_str().unwrap().to_string();
    (id, token)
}

#[allow(dead_code)]
async fn test_list_questions_with_status_filter() {
    let s = TestServer::start().await;
    let project = s.create_project("Q - Filter").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Task question filter").await;
    let task_id = task["id"].as_str().unwrap();

    // Create two questions
    for q in &["Question 1?", "Question 2?"] {
        let resp = s
            .client()
            .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
            .header("Authorization", s.auth_header())
            .json(&json!({ "question": q }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);
    }

    // List all — should be 2
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let all: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(all.len(), 2);

    // Resolve first question
    let q_id = all[0]["id"].as_str().unwrap();
    let resp = s
        .client()
        .post(format!(
            "{}/api/tasks/{}/questions/{}/resolve",
            s.base_url, task_id, q_id
        ))
        .header("Authorization", s.auth_header())
        .json(&json!({ "resolution": "Use JSON" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let resolved: Value = resp.json().await.unwrap();
    assert_eq!(resolved["status"], "resolved");
    assert_eq!(resolved["resolution"], "Use JSON");

    // Filter open — should be 1
    let resp = s
        .client()
        .get(format!(
            "{}/api/tasks/{}/questions?status=open",
            s.base_url, task_id
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let open: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(open.len(), 1);

    // Filter resolved — should be 1
    let resp = s
        .client()
        .get(format!(
            "{}/api/tasks/{}/questions?status=resolved",
            s.base_url, task_id
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let resolved_list: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(resolved_list.len(), 1);
}

#[tokio::test]
async fn test_has_open_questions_flag() {
    let s = TestServer::start().await;
    let project = s.create_project("Q - Flag").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Task flag test").await;
    let task_id = task["id"].as_str().unwrap();

    // Initially, has_open_questions should be false
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let t: Value = resp.json().await.unwrap();
    assert_eq!(t["has_open_questions"], false);

    // Create a blocking question
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "question": "Blocking Q?", "blocking": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let q: Value = resp.json().await.unwrap();
    let q_id = q["id"].as_str().unwrap();

    // Now has_open_questions should be true
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let t: Value = resp.json().await.unwrap();
    assert_eq!(t["has_open_questions"], true);

    // Resolve the question
    let resp = s
        .client()
        .post(format!(
            "{}/api/tasks/{}/questions/{}/resolve",
            s.base_url, task_id, q_id
        ))
        .header("Authorization", s.auth_header())
        .json(&json!({ "resolution": "Answered" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Flag should be false again
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let t: Value = resp.json().await.unwrap();
    assert_eq!(t["has_open_questions"], false);
}

#[tokio::test]
async fn test_non_blocking_question_does_not_set_flag() {
    let s = TestServer::start().await;
    let project = s.create_project("Q - NonBlocking").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Task non-blocking Q").await;
    let task_id = task["id"].as_str().unwrap();

    // Create a non-blocking question
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "question": "FYI question?", "blocking": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // has_open_questions should still be false (only blocking questions count)
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let t: Value = resp.json().await.unwrap();
    assert_eq!(t["has_open_questions"], false);
}

#[tokio::test]
async fn test_stale_release_skips_tasks_with_open_questions() {
    // Direct DB test — create task with open question, simulate stale agent, verify not released
    let tmp = TempDir::new().expect("failed to create temp dir");
    let db_path = tmp.path().join("stale_q.db");
    let db_path_str = db_path.to_str().unwrap();

    let conn = db::init_db(db_path_str);
    let (agent, _api_key) = db_ops::create_agent(
        &conn,
        &CreateAgent::new("stale-q-agent").with_skills(vec!["rust".to_string()]),
    );

    // Create project + task
    let project = db_ops::create_project(
        &conn,
        &opengate_models::CreateProject {
            name: "Stale Q Project".to_string(),
            description: None,
        },
        &agent.id,
    );

    let task = db_ops::create_task(
        &conn,
        &project.id,
        &opengate_models::CreateTask {
            title: "Stale Q Task".to_string(),
            description: None,
            priority: None,
            tags: None,
            context: None,
            output: None,
            due_date: None,
            assignee_type: None,
            assignee_id: None,
            scheduled_at: None,
            recurrence_rule: None,
        },
        &agent.id,
    );

    // Claim the task (backlog → in_progress)
    db_ops::claim_task(&conn, &task.id, &agent.id, &agent.name).unwrap();

    // Create a blocking open question on this task
    db_ops::create_question(
        &conn,
        &task.id,
        &opengate_models::CreateQuestion {
            question: "Need clarification".to_string(),
            question_type: None,
            context: None,
            target_type: None,
            target_id: None,
            required_capability: None,
            blocking: Some(true),
        },
        "agent",
        &agent.id,
    );

    // Make the agent look stale (last_seen 2 hours ago)
    let stale_time = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    conn.execute(
        "UPDATE agents SET last_seen_at = ?1 WHERE id = ?2",
        rusqlite::params![stale_time, agent.id],
    )
    .unwrap();

    // Run stale release — task should NOT be released (has open questions)
    let released = db_ops::release_stale_tasks(&conn, 30);
    assert!(
        released.is_empty(),
        "Task with open questions should not be released"
    );

    // Verify task is still in_progress
    let task_after = db_ops::get_task(&conn, &task.id).unwrap();
    assert_eq!(task_after.status, "in_progress");
    assert_eq!(task_after.assignee_id.as_deref(), Some(&*agent.id));
}

#[tokio::test]
async fn test_agent_targeted_questions() {
    let s = TestServer::start().await;
    let project = s.create_project("Q - Agent Target").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Task for agent Q").await;
    let task_id = task["id"].as_str().unwrap();

    // Create a question targeted at the test agent
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "question": "Can you handle this?",
            "target_type": "agent",
            "target_id": s.agent_id()
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // GET /api/agents/me/questions should return it
    let resp = s
        .client()
        .get(format!("{}/api/agents/me/questions", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let questions: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0]["question"], "Can you handle this?");
    assert_eq!(questions[0]["target_id"].as_str().unwrap(), s.agent_id());
}

#[tokio::test]
async fn test_project_questions_and_unrouted() {
    let s = TestServer::start().await;
    let project = s.create_project("Q - Project").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Task proj Q").await;
    let task_id = task["id"].as_str().unwrap();

    // Create a routed question (has target_id)
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "question": "Routed Q?",
            "target_type": "agent",
            "target_id": s.agent_id()
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Create an unrouted question (no target_id)
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "question": "Unrouted Q?" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // GET /api/projects/:id/questions — should return both
    let resp = s
        .client()
        .get(format!("{}/api/projects/{}/questions", s.base_url, pid))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let all: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(all.len(), 2);

    // GET /api/projects/:id/questions?unrouted=true — should return 1
    let resp = s
        .client()
        .get(format!(
            "{}/api/projects/{}/questions?unrouted=true",
            s.base_url, pid
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let unrouted: Vec<Value> = resp.json().await.unwrap();
    assert_eq!(unrouted.len(), 1);
    assert_eq!(unrouted[0]["question"], "Unrouted Q?");
}

#[tokio::test]
async fn test_get_single_question() {
    let s = TestServer::start().await;
    let project = s.create_project("Q - Get Single").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Task single Q").await;
    let task_id = task["id"].as_str().unwrap();

    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "question": "Single Q?", "context": "some context" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let q: Value = resp.json().await.unwrap();
    let q_id = q["id"].as_str().unwrap();

    // GET single question
    let resp = s
        .client()
        .get(format!(
            "{}/api/tasks/{}/questions/{}",
            s.base_url, task_id, q_id
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let fetched: Value = resp.json().await.unwrap();
    assert_eq!(fetched["id"].as_str().unwrap(), q_id);
    assert_eq!(fetched["question"], "Single Q?");
    assert_eq!(fetched["context"], "some context");
}

// ===== Agent Inbox Tests =====

#[tokio::test]
async fn test_inbox_empty() {
    let s = TestServer::start().await;
    let resp = s
        .client()
        .get(format!("{}/api/agents/me/inbox", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let inbox: Value = resp.json().await.unwrap();
    assert!(inbox["summary"]
        .as_str()
        .unwrap()
        .contains("No actionable work"));
    assert_eq!(inbox["todo_tasks"].as_array().unwrap().len(), 0);
    assert_eq!(inbox["in_progress_tasks"].as_array().unwrap().len(), 0);
    assert!(inbox["capacity"]["has_capacity"].as_bool().unwrap());
}

#[tokio::test]
async fn test_inbox_assigned_todo_task() {
    let s = TestServer::start().await;
    let project = s.create_project("Inbox - Todo").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Inbox todo task").await;
    let task_id = task["id"].as_str().unwrap();

    // Move to todo
    s.client()
        .patch(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "todo" }))
        .send()
        .await
        .unwrap();

    // Assign to our agent
    s.client()
        .post(format!("{}/api/tasks/{}/assign", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "agent_id": s.agent_id() }))
        .send()
        .await
        .unwrap();

    let resp = s
        .client()
        .get(format!("{}/api/agents/me/inbox", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let inbox: Value = resp.json().await.unwrap();
    let todos = inbox["todo_tasks"].as_array().unwrap();
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0]["id"].as_str().unwrap(), task_id);
    assert_eq!(todos[0]["action"], "start_work");
    assert_eq!(todos[0]["item_type"], "task");
}

#[tokio::test]
async fn test_inbox_in_progress_task() {
    let s = TestServer::start().await;
    let project = s.create_project("Inbox - InProgress").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Inbox ip task").await;
    let task_id = task["id"].as_str().unwrap();

    // Move to todo then claim (→ in_progress)
    s.client()
        .patch(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "todo" }))
        .send()
        .await
        .unwrap();
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    let resp = s
        .client()
        .get(format!("{}/api/agents/me/inbox", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let inbox: Value = resp.json().await.unwrap();
    let ip = inbox["in_progress_tasks"].as_array().unwrap();
    assert_eq!(ip.len(), 1);
    assert_eq!(ip[0]["id"].as_str().unwrap(), task_id);
    assert_eq!(ip[0]["action"], "continue_work");
}

#[tokio::test]
async fn test_inbox_open_question() {
    let s = TestServer::start().await;
    let project = s.create_project("Inbox - Question").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Task with question").await;
    let task_id = task["id"].as_str().unwrap();

    // Create a question targeted at our agent
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "question": "What framework to use?",
            "target_type": "agent",
            "target_id": s.agent_id()
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    let resp = s
        .client()
        .get(format!("{}/api/agents/me/inbox", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let inbox: Value = resp.json().await.unwrap();
    let questions = inbox["open_questions"].as_array().unwrap();
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0]["action"], "resolve_question");
    assert_eq!(questions[0]["item_type"], "question");
    assert_eq!(questions[0]["title"], "What framework to use?");
}

#[tokio::test]
async fn test_inbox_capacity_calculation() {
    let s = TestServer::start().await;
    let project = s.create_project("Inbox - Capacity").await;
    let pid = project["id"].as_str().unwrap();

    // Create and claim a task
    let task = s.create_task(pid, "Capacity test task").await;
    let task_id = task["id"].as_str().unwrap();
    s.client()
        .patch(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "todo" }))
        .send()
        .await
        .unwrap();
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    let resp = s
        .client()
        .get(format!("{}/api/agents/me/inbox", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let inbox: Value = resp.json().await.unwrap();
    let capacity = &inbox["capacity"];
    assert_eq!(capacity["current_active_tasks"].as_i64().unwrap(), 1);
    assert!(capacity["max_concurrent_tasks"].as_i64().unwrap() >= 1);
}

// ===== Question Replies, Dismiss, Assign =====

#[tokio::test]
async fn test_question_replies() {
    let s = TestServer::start().await;
    let project = s.create_project("Reply Test Project").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Task for replies").await;
    let task_id = task["id"].as_str().unwrap();

    // Create a question
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "question": "What format?" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let q: Value = resp.json().await.unwrap();
    let q_id = q["id"].as_str().unwrap();

    // Post a reply
    let resp = s
        .client()
        .post(format!(
            "{}/api/tasks/{}/questions/{}/replies",
            s.base_url, task_id, q_id
        ))
        .header("Authorization", s.auth_header())
        .json(&json!({ "body": "Use JSON format", "is_resolution": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let reply: Value = resp.json().await.unwrap();
    assert_eq!(reply["body"], "Use JSON format");
    assert_eq!(reply["is_resolution"], false);

    // List replies
    let resp = s
        .client()
        .get(format!(
            "{}/api/tasks/{}/questions/{}/replies",
            s.base_url, task_id, q_id
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let replies: Value = resp.json().await.unwrap();
    assert_eq!(replies.as_array().unwrap().len(), 1);
    assert_eq!(replies[0]["body"], "Use JSON format");
}

#[tokio::test]
async fn test_reply_with_resolution_auto_resolves_question() {
    let s = TestServer::start().await;
    let project = s.create_project("Auto-resolve Project").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Task for auto-resolve").await;
    let task_id = task["id"].as_str().unwrap();

    // Create question
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "question": "Is this correct?", "blocking": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let q: Value = resp.json().await.unwrap();
    let q_id = q["id"].as_str().unwrap();

    // Post reply with is_resolution=true
    let resp = s
        .client()
        .post(format!(
            "{}/api/tasks/{}/questions/{}/replies",
            s.base_url, task_id, q_id
        ))
        .header("Authorization", s.auth_header())
        .json(&json!({ "body": "Yes, looks correct!", "is_resolution": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Question should now be answered
    let resp = s
        .client()
        .get(format!(
            "{}/api/tasks/{}/questions/{}",
            s.base_url, task_id, q_id
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let q: Value = resp.json().await.unwrap();
    assert_eq!(q["status"], "answered");
    assert_eq!(q["resolution"], "Yes, looks correct!");
}

#[tokio::test]
async fn test_dismiss_question() {
    let s = TestServer::start().await;
    let project = s.create_project("Dismiss Project").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Task for dismiss").await;
    let task_id = task["id"].as_str().unwrap();

    // Create question
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "question": "Obsolete question?", "blocking": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let q: Value = resp.json().await.unwrap();
    let q_id = q["id"].as_str().unwrap();

    // Dismiss it
    let resp = s
        .client()
        .post(format!(
            "{}/api/tasks/{}/questions/{}/dismiss",
            s.base_url, task_id, q_id
        ))
        .header("Authorization", s.auth_header())
        .json(&json!({ "reason": "No longer relevant" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let q: Value = resp.json().await.unwrap();
    assert_eq!(q["status"], "dismissed");

    // has_open_questions should be false
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let task: Value = resp.json().await.unwrap();
    assert_eq!(task["has_open_questions"], false);
}

#[tokio::test]
async fn test_assign_question() {
    let s = TestServer::start().await;
    let project = s.create_project("Assign Question Project").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Task for assign").await;
    let task_id = task["id"].as_str().unwrap();

    // Create question
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "question": "Who should handle this?" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let q: Value = resp.json().await.unwrap();
    let q_id = q["id"].as_str().unwrap();

    // Assign to a user
    let resp = s
        .client()
        .post(format!(
            "{}/api/tasks/{}/questions/{}/assign",
            s.base_url, task_id, q_id
        ))
        .header("Authorization", s.auth_header())
        .json(&json!({ "target_type": "user", "target_id": "user-123" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let q: Value = resp.json().await.unwrap();
    assert_eq!(q["target_type"], "user");
    assert_eq!(q["target_id"], "user-123");
}

// ===== Auto-targeting capability matching tests =====

#[tokio::test]
async fn test_auto_target_zero_matches() {
    // No agent/user has the required capability → question stays unrouted
    let tmp = TempDir::new().unwrap();
    let conn = db::init_db(tmp.path().join("at0.db").to_str().unwrap());

    let (agent, _) = db_ops::create_agent(
        &conn,
        &CreateAgent::new("agent-no-cap")
            .with_skills(vec!["rust".to_string()])
            .with_capabilities(vec!["coding:rust".to_string()]),
    );

    let project = db_ops::create_project(
        &conn,
        &opengate_models::CreateProject {
            name: "AT0 Project".to_string(),
            description: None,
        },
        &agent.id,
    );
    let task = db_ops::create_task(
        &conn,
        &project.id,
        &opengate_models::CreateTask {
            title: "AT0 Task".to_string(),
            description: None,
            priority: None,
            tags: None,
            context: None,
            output: None,
            due_date: None,
            assignee_type: None,
            assignee_id: None,
            scheduled_at: None,
            recurrence_rule: None,
        },
        &agent.id,
    );

    // Search for a capability nobody has
    let targets = db_ops::find_capability_targets(&conn, "devops:terraform");
    assert_eq!(targets.len(), 0, "Expected 0 matches for devops:terraform");

    // Create question with required_capability but no target
    let question = db_ops::create_question(
        &conn,
        &task.id,
        &opengate_models::CreateQuestion {
            question: "Who handles terraform?".to_string(),
            question_type: None,
            context: None,
            target_type: None,
            target_id: None,
            required_capability: Some("devops:terraform".to_string()),
            blocking: Some(true),
        },
        "agent",
        &agent.id,
    );

    // Auto-target returns empty
    let result = db_ops::auto_target_question(&conn, &question.id, "devops:terraform");
    assert_eq!(result.len(), 0);

    // Question should remain unrouted
    let q = db_ops::get_question(&conn, &question.id).unwrap();
    assert!(
        q.target_id.is_none(),
        "Question should remain unrouted with 0 matches"
    );
    assert!(q.target_type.is_none());
}

#[tokio::test]
async fn test_auto_target_single_exact_match() {
    // Exactly one agent has the required capability → auto-assigns
    let tmp = TempDir::new().unwrap();
    let conn = db::init_db(tmp.path().join("at1.db").to_str().unwrap());

    let (creator, _) = db_ops::create_agent(
        &conn,
        &CreateAgent::new("creator-agent").with_capabilities(vec!["coding:rust".to_string()]),
    );
    let (devops_agent, _) = db_ops::create_agent(
        &conn,
        &CreateAgent::new("devops-agent")
            .with_capabilities(vec!["devops:docker".to_string(), "devops:k8s".to_string()]),
    );

    let project = db_ops::create_project(
        &conn,
        &opengate_models::CreateProject {
            name: "AT1 Project".to_string(),
            description: None,
        },
        &creator.id,
    );
    let task = db_ops::create_task(
        &conn,
        &project.id,
        &opengate_models::CreateTask {
            title: "AT1 Task".to_string(),
            description: None,
            priority: None,
            tags: None,
            context: None,
            output: None,
            due_date: None,
            assignee_type: None,
            assignee_id: None,
            scheduled_at: None,
            recurrence_rule: None,
        },
        &creator.id,
    );

    let question = db_ops::create_question(
        &conn,
        &task.id,
        &opengate_models::CreateQuestion {
            question: "How to set up the Docker build?".to_string(),
            question_type: None,
            context: None,
            target_type: None,
            target_id: None,
            required_capability: Some("devops:docker".to_string()),
            blocking: Some(true),
        },
        "agent",
        &creator.id,
    );

    let result = db_ops::auto_target_question(&conn, &question.id, "devops:docker");
    assert_eq!(
        result.len(),
        1,
        "Expected exactly 1 match for devops:docker"
    );
    assert_eq!(result[0].target_type, "agent");
    assert_eq!(result[0].target_id, devops_agent.id);

    // Question should be auto-assigned
    let q = db_ops::get_question(&conn, &question.id).unwrap();
    assert_eq!(q.target_type.as_deref(), Some("agent"));
    assert_eq!(q.target_id.as_deref(), Some(devops_agent.id.as_str()));
}

#[tokio::test]
async fn test_auto_target_multiple_matches() {
    // Multiple agents match → question stays unrouted, all are returned
    let tmp = TempDir::new().unwrap();
    let conn = db::init_db(tmp.path().join("atm.db").to_str().unwrap());

    let (agent1, _) = db_ops::create_agent(
        &conn,
        &CreateAgent::new("devops-agent-1").with_capabilities(vec!["devops:docker".to_string()]),
    );
    let (_agent2, _) = db_ops::create_agent(
        &conn,
        &CreateAgent::new("devops-agent-2")
            .with_capabilities(vec!["devops:docker".to_string(), "devops:k8s".to_string()]),
    );

    let project = db_ops::create_project(
        &conn,
        &opengate_models::CreateProject {
            name: "ATM Project".to_string(),
            description: None,
        },
        &agent1.id,
    );
    let task = db_ops::create_task(
        &conn,
        &project.id,
        &opengate_models::CreateTask {
            title: "ATM Task".to_string(),
            description: None,
            priority: None,
            tags: None,
            context: None,
            output: None,
            due_date: None,
            assignee_type: None,
            assignee_id: None,
            scheduled_at: None,
            recurrence_rule: None,
        },
        &agent1.id,
    );

    let question = db_ops::create_question(
        &conn,
        &task.id,
        &opengate_models::CreateQuestion {
            question: "Docker build failing".to_string(),
            question_type: None,
            context: None,
            target_type: None,
            target_id: None,
            required_capability: Some("devops:docker".to_string()),
            blocking: Some(true),
        },
        "agent",
        &agent1.id,
    );

    let result = db_ops::auto_target_question(&conn, &question.id, "devops:docker");
    assert!(
        result.len() >= 2,
        "Expected 2+ matches for devops:docker, got {}",
        result.len()
    );

    // Question should remain unrouted (N > 1)
    let q = db_ops::get_question(&conn, &question.id).unwrap();
    assert!(
        q.target_id.is_none(),
        "Question should remain unrouted with N matches"
    );
}

#[tokio::test]
async fn test_auto_target_finds_agents_by_capability() {
    // Agents with matching capability should be returned
    let tmp = TempDir::new().unwrap();
    let conn = db::init_db(tmp.path().join("atu.db").to_str().unwrap());

    let (_agent, _) = db_ops::create_agent(
        &conn,
        &CreateAgent::new("review-agent").with_capabilities(vec!["review".to_string()]),
    );

    let targets = db_ops::find_capability_targets(&conn, "review");
    assert!(!targets.is_empty(), "Expected agent match");

    let agent_idx = targets.iter().position(|t| t.target_type == "agent");
    assert!(agent_idx.is_some(), "Agent should be in results");
}

#[tokio::test]
async fn test_auto_target_via_api_single_match() {
    // End-to-end: POST /api/tasks/:id/questions with required_capability
    // and one matching agent → question auto-targeted
    let s = TestServer::start().await;

    // Register a devops agent with capabilities via the register endpoint
    let resp = s
        .client()
        .post(format!("{}/api/agents/register", s.base_url))
        .json(&json!({
            "name": "devops-agent",
            "setup_token": "test-setup-token",
            "skills": ["devops"],
            "capabilities": ["devops:docker", "devops:k8s"]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let agent_resp: Value = resp.json().await.unwrap();
    let devops_agent_id = agent_resp["agent"]["id"].as_str().unwrap().to_string();

    let project = s.create_project("Auto-target API Project").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Auto-target API Task").await;
    let task_id = task["id"].as_str().unwrap();

    // Create question with required_capability but no target
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "question": "How to configure Docker?",
            "required_capability": "devops:docker"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let q: Value = resp.json().await.unwrap();

    // Should be auto-targeted to the devops agent
    assert_eq!(
        q["target_type"], "agent",
        "Should be auto-targeted to agent"
    );
    assert_eq!(
        q["target_id"], devops_agent_id,
        "Should be targeted to devops-agent"
    );
}

#[tokio::test]
async fn test_question_reply_notifications() {
    // When a reply is posted, the question asker (if agent) gets notified
    let s = TestServer::start().await;

    // Register a second agent
    let resp = s
        .client()
        .post(format!("{}/api/agents/register", s.base_url))
        .json(&json!({
            "name": "answerer-agent",
            "setup_token": "test-setup-token",
            "skills": ["devops"],
            "capabilities": ["devops:docker"]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let agent_resp: Value = resp.json().await.unwrap();
    let answerer_key = agent_resp["api_key"].as_str().unwrap().to_string();

    let project = s.create_project("Reply Notification Project").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Reply Notification Task").await;
    let task_id = task["id"].as_str().unwrap();

    // First agent asks a question
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "question": "How to deploy?" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let q: Value = resp.json().await.unwrap();
    let q_id = q["id"].as_str().unwrap();

    // Second agent replies
    let resp = s
        .client()
        .post(format!(
            "{}/api/tasks/{}/questions/{}/replies",
            s.base_url, task_id, q_id
        ))
        .header("Authorization", format!("Bearer {}", answerer_key))
        .json(&json!({ "body": "Use docker compose up" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // First agent should have a notification about the reply
    let resp = s
        .client()
        .get(format!(
            "{}/api/agents/me/notifications?unread=true",
            s.base_url
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let notifications: Vec<Value> = resp.json().await.unwrap();

    let reply_notif = notifications.iter().find(|n| {
        let event_type = n["event_type"].as_str().unwrap_or("");
        event_type == "question_replied"
    });
    assert!(
        reply_notif.is_some(),
        "Asker should receive question_replied notification"
    );
}

#[tokio::test]
async fn test_question_resolved_notification() {
    // When a question is resolved, the original asker gets notified
    let s = TestServer::start().await;

    // Register a second agent to resolve the question
    let resp = s
        .client()
        .post(format!("{}/api/agents/register", s.base_url))
        .json(&json!({
            "name": "resolver-agent",
            "setup_token": "test-setup-token",
            "skills": ["devops"]
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let agent_resp: Value = resp.json().await.unwrap();
    let resolver_key = agent_resp["api_key"].as_str().unwrap().to_string();

    let project = s.create_project("Resolved Notification Project").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Resolved Notification Task").await;
    let task_id = task["id"].as_str().unwrap();

    // First agent asks a question
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/questions", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "question": "What DB should we use?" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let q: Value = resp.json().await.unwrap();
    let q_id = q["id"].as_str().unwrap();

    // Second agent resolves the question
    let resp = s
        .client()
        .post(format!(
            "{}/api/tasks/{}/questions/{}/resolve",
            s.base_url, task_id, q_id
        ))
        .header("Authorization", format!("Bearer {}", resolver_key))
        .json(&json!({ "resolution": "Use PostgreSQL" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // First agent should have a question_resolved notification
    let resp = s
        .client()
        .get(format!(
            "{}/api/agents/me/notifications?unread=true",
            s.base_url
        ))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let notifications: Vec<Value> = resp.json().await.unwrap();

    let resolved_notif = notifications.iter().find(|n| {
        let event_type = n["event_type"].as_str().unwrap_or("");
        event_type == "question_resolved"
    });
    assert!(
        resolved_notif.is_some(),
        "Asker should receive question_resolved notification"
    );
}

// ===== Dependency Enforcement =====

#[tokio::test]
async fn test_cannot_start_task_with_unmet_deps() {
    let s = TestServer::start().await;
    let proj = s.create_project("dep-enforce-test").await;
    let pid = proj["id"].as_str().unwrap();

    let a = s.create_task(pid, "Task A (blocked)").await;
    let b = s.create_task(pid, "Task B (blocker)").await;
    let a_id = a["id"].as_str().unwrap();
    let b_id = b["id"].as_str().unwrap();

    // A depends on B
    s.client()
        .post(format!("{}/api/tasks/{}/dependencies", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "depends_on": [b_id] }))
        .send()
        .await
        .unwrap();

    // Move A to todo first (backlog → todo is always allowed)
    let r = s
        .client()
        .patch(format!("{}/api/tasks/{}", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "todo" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "backlog→todo should succeed");

    // Try to move A to in_progress — should fail (B not done)
    let resp = s
        .client()
        .patch(format!("{}/api/tasks/{}", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "in_progress" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        409,
        "Should reject with 409 Conflict when deps unmet"
    );
    let body: Value = resp.json().await.unwrap();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("dependencies not met"));
}

#[tokio::test]
async fn test_can_start_task_once_deps_done() {
    let s = TestServer::start().await;
    let proj = s.create_project("dep-enforce-start").await;
    let pid = proj["id"].as_str().unwrap();

    let a = s.create_task(pid, "Task A").await;
    let b = s.create_task(pid, "Task B").await;
    let a_id = a["id"].as_str().unwrap();
    let b_id = b["id"].as_str().unwrap();

    // A depends on B
    s.client()
        .post(format!("{}/api/tasks/{}/dependencies", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "depends_on": [b_id] }))
        .send()
        .await
        .unwrap();

    // Complete B: backlog → todo → in_progress → done
    for status in &["todo", "in_progress", "done"] {
        let r = s
            .client()
            .patch(format!("{}/api/tasks/{}", s.base_url, b_id))
            .header("Authorization", s.auth_header())
            .json(&json!({ "status": status }))
            .send()
            .await
            .unwrap();
        assert!(
            r.status().is_success(),
            "B transition to {} failed: {}",
            status,
            r.status()
        );
    }

    // Now A should be auto-unblocked to todo; can move to in_progress
    let resp = s
        .client()
        .patch(format!("{}/api/tasks/{}", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "in_progress" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "Should allow in_progress once dep is done"
    );
}

#[tokio::test]
async fn test_assign_with_pending_deps_adds_warning() {
    let s = TestServer::start().await;
    let proj = s.create_project("assign-dep-warn").await;
    let pid = proj["id"].as_str().unwrap();

    let a = s.create_task(pid, "Task A").await;
    let b = s.create_task(pid, "Task B (blocker)").await;
    let a_id = a["id"].as_str().unwrap();
    let b_id = b["id"].as_str().unwrap();

    // A depends on B (not done)
    s.client()
        .post(format!("{}/api/tasks/{}/dependencies", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "depends_on": [b_id] }))
        .send()
        .await
        .unwrap();

    // Assign A to the test agent (which exists in the test server)
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/assign", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "agent_id": s.agent_id() }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "Assign should succeed even with unmet deps"
    );

    // Check activity log has warning
    let activity_resp = s
        .client()
        .get(format!("{}/api/tasks/{}/activity", s.base_url, a_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(activity_resp.status(), 200);
    let activity: Value = activity_resp.json().await.unwrap();
    let has_warning = activity
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["content"].as_str().unwrap_or("").contains("unmet dep"));
    assert!(has_warning, "Activity should contain dep warning");
}

// --- Start Review tests ---

// Test start-review: sets started_review_at, only reviewer allowed, must be in review status
#[tokio::test]
async fn test_start_review_sets_timestamp() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Start Review").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Start Review").await;
    let task_id = task["id"].as_str().unwrap();

    // Claim (backlog -> in_progress)
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    // Move to review and set reviewer to our test agent
    s.client()
        .patch(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "review", "reviewer_type": "agent", "reviewer_id": s.agent_id() }))
        .send()
        .await
        .unwrap();

    // Verify started_review_at is null before start-review
    let resp = s
        .client()
        .get(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["started_review_at"].is_null(),
        "started_review_at should be null initially"
    );

    // Start review
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/start-review", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "review", "Status should remain review");
    assert!(
        body["started_review_at"].is_string(),
        "started_review_at should be set"
    );
}

// Test start-review: must be in review status
#[tokio::test]
async fn test_start_review_requires_review_status() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Start Review Status").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Start Review Status").await;
    let task_id = task["id"].as_str().unwrap();

    // Claim (backlog -> in_progress)
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    // Try start-review on in_progress task (should fail)
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/start-review", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "Should reject start-review when not in review status"
    );
}

// Test start-review: non-reviewer gets 403
#[tokio::test]
async fn test_start_review_non_reviewer_gets_403() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Non Reviewer").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Non Reviewer").await;
    let task_id = task["id"].as_str().unwrap();

    // Register a second agent to be the reviewer
    let resp = s
        .client()
        .post(format!("{}/api/agents/register", s.base_url))
        .header("Authorization", s.auth_header())
        .json(&json!({
            "name": "reviewer-agent",
            "skills": ["rust"],
            "setup_token": "test-setup-token"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let reviewer: Value = resp.json().await.unwrap();
    let reviewer_id = reviewer["agent"]["id"].as_str().unwrap();

    // Claim (backlog -> in_progress)
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    // Move to review with reviewer-agent as reviewer
    s.client()
        .patch(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "review", "reviewer_type": "agent", "reviewer_id": reviewer_id }))
        .send()
        .await
        .unwrap();

    // Try start-review as test-agent (not the reviewer) — should get 403
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/start-review", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "Non-reviewer should get 403");
    let body: Value = resp.json().await.unwrap();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("Only the assigned reviewer"));
}

// Test review_task_count appears in agent response
#[tokio::test]
async fn test_review_task_count_in_agent_response() {
    let s = TestServer::start().await;
    let project = s.create_project("Test Project - Review Count").await;
    let pid = project["id"].as_str().unwrap();

    // Create a task, claim it, move to review with our agent as reviewer
    let task = s.create_task(pid, "Task for Review Count").await;
    let task_id = task["id"].as_str().unwrap();

    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    s.client()
        .patch(format!("{}/api/tasks/{}", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "status": "review", "reviewer_type": "agent", "reviewer_id": s.agent_id() }))
        .send()
        .await
        .unwrap();

    // Fetch the agent and check review_task_count
    let resp = s
        .client()
        .get(format!("{}/api/agents/{}", s.base_url, s.agent_id()))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let agent: Value = resp.json().await.unwrap();
    assert_eq!(
        agent["review_task_count"], 1,
        "Agent should have 1 review task"
    );
    // current_task_count should be 0 since task moved to review (not in_progress)
    assert_eq!(
        agent["current_task_count"], 0,
        "current_task_count should be 0 (task is in review, not in_progress)"
    );
}

// Pre-assigned task: assign then claim should transition to in_progress
#[tokio::test]
async fn test_claim_preassigned_task() {
    let s = TestServer::start().await;

    // Heartbeat so agent is online
    s.client()
        .post(format!("{}/api/agents/heartbeat", s.base_url))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    let project = s.create_project("Test Project - Pre-assigned Claim").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Test Pre-assigned Claim").await;
    let task_id = task["id"].as_str().unwrap();

    // Step 1: Assign to agent (status stays "todo")
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/assign", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .json(&json!({ "agent_id": s.agent_id() }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "todo", "assign should keep status as todo");
    assert_eq!(body["assignee_id"].as_str().unwrap(), s.agent_id());

    // Step 2: Agent claims the pre-assigned task → should transition to in_progress
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["status"], "in_progress",
        "claim on pre-assigned task should transition to in_progress"
    );

    // Step 3: Claim again → idempotent, still in_progress
    let resp = s
        .client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["status"], "in_progress",
        "second claim should be idempotent"
    );
}

// ---------------------------------------------------------------------------
// WebSocket integration tests
// ---------------------------------------------------------------------------

// WS Auth: valid API key → auth_ok with identity
#[tokio::test]
async fn test_ws_auth_valid() {
    let s = TestServer::start().await;
    let (ws, _) = tokio_tungstenite::connect_async(&s.ws_url())
        .await
        .expect("WS connect failed");
    let (mut sink, mut stream) = ws.split();

    let auth_msg = json!({"type": "auth", "token": s.api_key}).to_string();
    sink.send(WsMessage::Text(auth_msg.into())).await.unwrap();

    let resp = recv_json(&mut stream, 2000)
        .await
        .expect("expected auth_ok");
    assert_eq!(resp["type"], "auth_ok");
    assert_eq!(resp["identity"]["type"], "agent");
    assert_eq!(resp["identity"]["id"], s.agent_id());
    assert!(resp["identity"]["name"].is_string());
}

// WS Auth: invalid API key → auth_failed error
#[tokio::test]
async fn test_ws_auth_invalid() {
    let s = TestServer::start().await;
    let (ws, _) = tokio_tungstenite::connect_async(&s.ws_url())
        .await
        .expect("WS connect failed");
    let (mut sink, mut stream) = ws.split();

    let auth_msg = json!({"type": "auth", "token": "bad-key-12345"}).to_string();
    sink.send(WsMessage::Text(auth_msg.into())).await.unwrap();

    let resp = recv_json(&mut stream, 2000).await.expect("expected error");
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "auth_failed");
}

// WS Auth: subscribe before auth → auth_required error
#[tokio::test]
async fn test_ws_auth_required() {
    let s = TestServer::start().await;
    let (ws, _) = tokio_tungstenite::connect_async(&s.ws_url())
        .await
        .expect("WS connect failed");
    let (mut sink, mut stream) = ws.split();

    let sub_msg = json!({"type": "subscribe", "events": ["task.*"]}).to_string();
    sink.send(WsMessage::Text(sub_msg.into())).await.unwrap();

    let resp = recv_json(&mut stream, 2000).await.expect("expected error");
    assert_eq!(resp["type"], "error");
    assert_eq!(resp["code"], "auth_required");
}

// WS Event roundtrip: subscribe task.*, create task via REST, receive event
#[tokio::test]
async fn test_ws_event_roundtrip() {
    let s = TestServer::start().await;
    let (mut sink, mut stream) = ws_auth(&s.ws_url(), &s.api_key).await;

    // Subscribe to task.*
    let sub_msg = json!({"type": "subscribe", "events": ["task.*"]}).to_string();
    sink.send(WsMessage::Text(sub_msg.into())).await.unwrap();
    let resp = recv_json(&mut stream, 2000)
        .await
        .expect("expected subscribed");
    assert_eq!(resp["type"], "subscribed");
    assert_eq!(resp["id"], "sub-1");

    // Create a task via REST → triggers task.created event
    let project = s.create_project("WS Roundtrip").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "WS Test Task").await;
    let task_id = task["id"].as_str().unwrap();

    // Should receive the event
    let event = recv_json(&mut stream, 2000).await.expect("expected event");
    assert_eq!(event["type"], "event");
    assert_eq!(event["sub"], "sub-1");
    assert_eq!(event["event"], "task.created");
    assert_eq!(event["data"]["id"].as_str().unwrap(), task_id);
}

// WS Multiple subscribers: two clients both receive the same event
#[tokio::test]
async fn test_ws_multiple_subscribers() {
    let s = TestServer::start().await;

    // Connect two WS clients
    let (mut sink1, mut stream1) = ws_auth(&s.ws_url(), &s.api_key).await;
    let (mut sink2, mut stream2) = ws_auth(&s.ws_url(), &s.api_key).await;

    // Both subscribe to task.*
    let sub_msg = json!({"type": "subscribe", "events": ["task.*"]}).to_string();
    sink1
        .send(WsMessage::Text(sub_msg.clone().into()))
        .await
        .unwrap();
    let r1 = recv_json(&mut stream1, 2000).await.unwrap();
    assert_eq!(r1["type"], "subscribed");

    sink2.send(WsMessage::Text(sub_msg.into())).await.unwrap();
    let r2 = recv_json(&mut stream2, 2000).await.unwrap();
    assert_eq!(r2["type"], "subscribed");

    // Create task via REST
    let project = s.create_project("WS Multi Sub").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Multi Sub Task").await;
    let task_id = task["id"].as_str().unwrap();

    // Both clients should receive the event
    let ev1 = recv_json(&mut stream1, 2000)
        .await
        .expect("client 1 should receive event");
    assert_eq!(ev1["event"], "task.created");
    assert_eq!(ev1["data"]["id"].as_str().unwrap(), task_id);

    let ev2 = recv_json(&mut stream2, 2000)
        .await
        .expect("client 2 should receive event");
    assert_eq!(ev2["event"], "task.created");
    assert_eq!(ev2["data"]["id"].as_str().unwrap(), task_id);
}

// WS No subscribers: creating a task with no WS clients doesn't panic
#[tokio::test]
async fn test_ws_no_subscribers_no_panic() {
    let s = TestServer::start().await;
    // No WS clients — just create a task via REST
    let project = s.create_project("No Subscribers").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "No Subscriber Task").await;
    assert!(
        task["id"].is_string(),
        "REST should succeed with no WS listeners"
    );
}

// WS Filter: agent_id "self" only receives events for own agent
#[tokio::test]
async fn test_ws_filter_agent_self() {
    let s = TestServer::start().await;
    let (mut sink, mut stream) = ws_auth(&s.ws_url(), &s.api_key).await;

    // Subscribe with agent_id: "self" filter
    let sub_msg = json!({
        "type": "subscribe",
        "events": ["task.*"],
        "filter": {"agent_id": "self"}
    })
    .to_string();
    sink.send(WsMessage::Text(sub_msg.into())).await.unwrap();
    let resp = recv_json(&mut stream, 2000).await.unwrap();
    assert_eq!(resp["type"], "subscribed");

    // Create a task (task.created has no assignee → agent_id is None) — should NOT match
    let project = s.create_project("Agent Filter").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Agent Filter Task").await;
    let task_id = task["id"].as_str().unwrap();

    // task.created has agent_id=None, won't match "self" filter
    let no_event = recv_json(&mut stream, 500).await;
    assert!(
        no_event.is_none(),
        "should not receive task.created with no assignee"
    );

    // Claim the task → assigns agent, emits task.claimed with agent_id = self
    s.client()
        .post(format!("{}/api/tasks/{}/claim", s.base_url, task_id))
        .header("Authorization", s.auth_header())
        .send()
        .await
        .unwrap();

    // Should receive the claimed event (agent_id matches self)
    let event = recv_json(&mut stream, 2000)
        .await
        .expect("expected claimed event");
    assert_eq!(event["event"], "task.claimed");
    assert_eq!(event["data"]["assignee_id"].as_str().unwrap(), s.agent_id());
}

// WS Filter: project_id only receives events for that project
#[tokio::test]
async fn test_ws_filter_project() {
    let s = TestServer::start().await;
    let (mut sink, mut stream) = ws_auth(&s.ws_url(), &s.api_key).await;

    // Create two projects
    let proj_a = s.create_project("Project A").await;
    let pid_a = proj_a["id"].as_str().unwrap();
    let proj_b = s.create_project("Project B").await;
    let pid_b = proj_b["id"].as_str().unwrap();

    // Subscribe with project_id filter for project A only
    let sub_msg = json!({
        "type": "subscribe",
        "events": ["task.*"],
        "filter": {"project_id": pid_a}
    })
    .to_string();
    sink.send(WsMessage::Text(sub_msg.into())).await.unwrap();
    let resp = recv_json(&mut stream, 2000).await.unwrap();
    assert_eq!(resp["type"], "subscribed");

    // Create task in project B → should NOT receive event
    s.create_task(pid_b, "Task in B").await;
    let no_event = recv_json(&mut stream, 500).await;
    assert!(no_event.is_none(), "should not receive event for project B");

    // Create task in project A → SHOULD receive event
    let task_a = s.create_task(pid_a, "Task in A").await;
    let event = recv_json(&mut stream, 2000)
        .await
        .expect("expected event for project A");
    assert_eq!(event["event"], "task.created");
    assert_eq!(
        event["data"]["id"].as_str().unwrap(),
        task_a["id"].as_str().unwrap()
    );
}

// WS Wildcard no match: subscribe task.*, trigger knowledge.updated → not received
#[tokio::test]
async fn test_ws_wildcard_no_match() {
    let s = TestServer::start().await;
    let (mut sink, mut stream) = ws_auth(&s.ws_url(), &s.api_key).await;

    // Subscribe to task.* only
    let sub_msg = json!({"type": "subscribe", "events": ["task.*"]}).to_string();
    sink.send(WsMessage::Text(sub_msg.into())).await.unwrap();
    let resp = recv_json(&mut stream, 2000).await.unwrap();
    assert_eq!(resp["type"], "subscribed");

    // Trigger a knowledge.updated event via REST
    let project = s.create_project("Wildcard NoMatch").await;
    let pid = project["id"].as_str().unwrap();
    s.client()
        .put(format!(
            "{}/api/projects/{}/knowledge/test-key",
            s.base_url, pid
        ))
        .header("Authorization", s.auth_header())
        .json(&json!({"title": "Test Knowledge", "body": "Some content"}))
        .send()
        .await
        .unwrap();

    // Should NOT receive the knowledge event on a task.* subscription
    let no_event = recv_json(&mut stream, 500).await;
    assert!(
        no_event.is_none(),
        "task.* should not match knowledge.updated"
    );
}

// WS Exact match: subscribe task.created, receive only task.created
#[tokio::test]
async fn test_ws_exact_match() {
    let s = TestServer::start().await;
    let (mut sink, mut stream) = ws_auth(&s.ws_url(), &s.api_key).await;

    // Subscribe to exact pattern task.created
    let sub_msg = json!({"type": "subscribe", "events": ["task.created"]}).to_string();
    sink.send(WsMessage::Text(sub_msg.into())).await.unwrap();
    let resp = recv_json(&mut stream, 2000).await.unwrap();
    assert_eq!(resp["type"], "subscribed");

    // Create task → should receive task.created
    let project = s.create_project("Exact Match").await;
    let pid = project["id"].as_str().unwrap();
    let task = s.create_task(pid, "Exact Match Task").await;
    let task_id = task["id"].as_str().unwrap();

    let event = recv_json(&mut stream, 2000)
        .await
        .expect("expected task.created");
    assert_eq!(event["event"], "task.created");
    assert_eq!(event["data"]["id"].as_str().unwrap(), task_id);
}

// WS Unsubscribe: receive event, unsubscribe, no longer receive
#[tokio::test]
async fn test_ws_unsubscribe() {
    let s = TestServer::start().await;
    let (mut sink, mut stream) = ws_auth(&s.ws_url(), &s.api_key).await;

    // Subscribe
    let sub_msg = json!({"type": "subscribe", "events": ["task.*"]}).to_string();
    sink.send(WsMessage::Text(sub_msg.into())).await.unwrap();
    let resp = recv_json(&mut stream, 2000).await.unwrap();
    assert_eq!(resp["type"], "subscribed");
    let sub_id = resp["id"].as_str().unwrap().to_string();

    // Create task → should receive event
    let project = s.create_project("Unsubscribe Test").await;
    let pid = project["id"].as_str().unwrap();
    s.create_task(pid, "Before Unsub").await;
    let event = recv_json(&mut stream, 2000)
        .await
        .expect("expected event before unsub");
    assert_eq!(event["event"], "task.created");

    // Unsubscribe
    let unsub_msg = json!({"type": "unsubscribe", "id": sub_id}).to_string();
    sink.send(WsMessage::Text(unsub_msg.into())).await.unwrap();
    let resp = recv_json(&mut stream, 2000).await.unwrap();
    assert_eq!(resp["type"], "unsubscribed");

    // Create another task → should NOT receive event
    s.create_task(pid, "After Unsub").await;
    let no_event = recv_json(&mut stream, 500).await;
    assert!(
        no_event.is_none(),
        "should not receive events after unsubscribe"
    );
}
