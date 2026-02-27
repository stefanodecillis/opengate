use std::collections::HashMap;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::app::AppState;
use crate::events::Event;

// ---------------------------------------------------------------------------
// Protocol types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClientMessage {
    #[serde(rename = "auth")]
    Auth { token: String },
    #[serde(rename = "subscribe")]
    Subscribe {
        events: Vec<String>,
        filter: Option<SubscriptionFilter>,
    },
    #[serde(rename = "unsubscribe")]
    Unsubscribe { id: String },
    #[serde(rename = "ping")]
    Ping,
}

#[derive(Debug, Deserialize, Clone)]
struct SubscriptionFilter {
    agent_id: Option<String>,
    project_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ServerMessage {
    #[serde(rename = "auth_ok")]
    AuthOk { identity: serde_json::Value },
    #[serde(rename = "subscribed")]
    Subscribed { id: String },
    #[serde(rename = "unsubscribed")]
    Unsubscribed { id: String },
    #[serde(rename = "event")]
    Event {
        sub: String,
        event: String,
        data: serde_json::Value,
    },
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "error")]
    Error { code: String, message: String },
}

// ---------------------------------------------------------------------------
// Subscription bookkeeping
// ---------------------------------------------------------------------------

struct Subscription {
    patterns: Vec<String>,
    filter: Option<SubscriptionFilter>,
}

fn pattern_matches(pattern: &str, event_type: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix(".*") {
        event_type.starts_with(prefix)
            && event_type.len() > prefix.len()
            && event_type.as_bytes()[prefix.len()] == b'.'
    } else {
        pattern == event_type
    }
}

fn subscription_matches(sub: &Subscription, event: &Event, self_agent_id: &str) -> bool {
    // Check at least one pattern matches
    let pattern_ok = sub
        .patterns
        .iter()
        .any(|p| pattern_matches(p, &event.event_type));
    if !pattern_ok {
        return false;
    }

    // Check filters
    if let Some(ref filter) = sub.filter {
        if let Some(ref wanted_agent) = filter.agent_id {
            let resolved = if wanted_agent == "self" {
                self_agent_id
            } else {
                wanted_agent.as_str()
            };
            match &event.agent_id {
                Some(eid) if eid == resolved => {}
                _ => return false,
            }
        }
        if let Some(ref wanted_project) = filter.project_id {
            match &event.project_id {
                Some(pid) if pid == wanted_project => {}
                _ => return false,
            }
        }
    }

    true
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    // -----------------------------------------------------------------------
    // Phase 1: Auth — first message must be auth
    // -----------------------------------------------------------------------
    let (agent_id, agent_name) = match wait_for_auth(&mut socket, &state).await {
        Some(id) => id,
        None => return, // connection already closed with error
    };

    // Send auth_ok
    let identity_json = serde_json::json!({
        "type": "agent",
        "id": agent_id,
        "name": agent_name,
    });
    if send_msg(
        &mut socket,
        &ServerMessage::AuthOk {
            identity: identity_json,
        },
    )
    .await
    .is_err()
    {
        return;
    }

    // Heartbeat is updated only after auth_ok is confirmed sent, so we don't
    // record a heartbeat for a connection that never fully opened.
    state.storage.update_heartbeat(None, &agent_id);

    // -----------------------------------------------------------------------
    // Phase 2: Authenticated session
    // -----------------------------------------------------------------------
    let event_rx = state.event_bus.subscribe();
    run_session(socket, event_rx, agent_id).await;
}

/// Wait for the first message which must be an Auth message.
/// Returns (agent_id, agent_name) on success, or None if auth fails.
async fn wait_for_auth(socket: &mut WebSocket, state: &AppState) -> Option<(String, String)> {
    // Give client 10 seconds to authenticate
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(10));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => {
                let _ = send_msg(socket, &ServerMessage::Error {
                    code: "auth_timeout".to_string(),
                    message: "Authentication timeout".to_string(),
                }).await;
                let _ = send_close(socket).await;
                return None;
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(ClientMessage::Auth { token }) => {
                                let hash = state.storage.hash_api_key(&token);
                                if let Some(agent) = state.storage.get_agent_by_key_hash(None, &hash) {
                                    return Some((agent.id, agent.name));
                                } else {
                                    let _ = send_msg(socket, &ServerMessage::Error {
                                        code: "auth_failed".to_string(),
                                        message: "Invalid API key".to_string(),
                                    }).await;
                                    let _ = send_close(socket).await;
                                    return None;
                                }
                            }
                            Ok(_) => {
                                let _ = send_msg(socket, &ServerMessage::Error {
                                    code: "auth_required".to_string(),
                                    message: "First message must be auth".to_string(),
                                }).await;
                                let _ = send_close(socket).await;
                                return None;
                            }
                            Err(_) => {
                                let _ = send_msg(socket, &ServerMessage::Error {
                                    code: "invalid_message".to_string(),
                                    message: "Invalid JSON".to_string(),
                                }).await;
                                let _ = send_close(socket).await;
                                return None;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => return None,
                    // Ignore binary/ping/pong during auth phase
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => return None,
                }
            }
        }
    }
}

/// Run the authenticated WebSocket session.
async fn run_session(
    mut socket: WebSocket,
    mut event_rx: broadcast::Receiver<Event>,
    agent_id: String,
) {
    let mut subscriptions: HashMap<String, Subscription> = HashMap::new();
    let mut sub_counter: u64 = 0;
    let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(30));
    // First tick completes immediately; skip it.
    ping_interval.tick().await;

    loop {
        tokio::select! {
            // Server ping keepalive
            _ = ping_interval.tick() => {
                if send_msg(&mut socket, &ServerMessage::Ping).await.is_err() {
                    break;
                }
            }

            // Events from the bus
            event_result = event_rx.recv() => {
                match event_result {
                    Ok(event) => {
                        for (sub_id, sub) in &subscriptions {
                            if subscription_matches(sub, &event, &agent_id) {
                                let msg = ServerMessage::Event {
                                    sub: sub_id.clone(),
                                    event: event.event_type.clone(),
                                    data: event.data.clone(),
                                };
                                if send_msg(&mut socket, &msg).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        let _ = send_msg(&mut socket, &ServerMessage::Error {
                            code: "events_lagged".to_string(),
                            message: format!("Missed {} events", n),
                        }).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            // Client messages
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(ClientMessage::Ping) => {
                                if send_msg(&mut socket, &ServerMessage::Pong).await.is_err() {
                                    break;
                                }
                            }
                            Ok(ClientMessage::Subscribe { events, filter }) => {
                                sub_counter += 1;
                                let id = format!("sub-{}", sub_counter);
                                subscriptions.insert(id.clone(), Subscription {
                                    patterns: events,
                                    filter,
                                });
                                if send_msg(&mut socket, &ServerMessage::Subscribed { id }).await.is_err() {
                                    break;
                                }
                            }
                            Ok(ClientMessage::Unsubscribe { id }) => {
                                subscriptions.remove(&id);
                                if send_msg(&mut socket, &ServerMessage::Unsubscribed { id }).await.is_err() {
                                    break;
                                }
                            }
                            Ok(ClientMessage::Auth { .. }) => {
                                let _ = send_msg(&mut socket, &ServerMessage::Error {
                                    code: "already_authenticated".to_string(),
                                    message: "Already authenticated".to_string(),
                                }).await;
                            }
                            Err(_) => {
                                let _ = send_msg(&mut socket, &ServerMessage::Error {
                                    code: "invalid_message".to_string(),
                                    message: "Invalid JSON".to_string(),
                                }).await;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // ignore binary/ping/pong frames
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

/// Serialize and send a ServerMessage as text.
async fn send_msg(socket: &mut WebSocket, msg: &ServerMessage) -> Result<(), ()> {
    let json = serde_json::to_string(msg).map_err(|_| ())?;
    socket.send(Message::Text(json)).await.map_err(|_| ())
}

/// Send a close frame without consuming the socket.
async fn send_close(socket: &mut WebSocket) -> Result<(), ()> {
    socket.send(Message::Close(None)).await.map_err(|_| ())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_pattern_exact_match() {
        assert!(pattern_matches("task.created", "task.created"));
        assert!(!pattern_matches("task.created", "task.updated"));
    }

    #[test]
    fn test_pattern_wildcard_match() {
        assert!(pattern_matches("task.*", "task.created"));
        assert!(pattern_matches("task.*", "task.updated"));
        assert!(!pattern_matches("task.*", "project.created"));
        // Should not match bare prefix without dot separator
        assert!(!pattern_matches("task.*", "taskfoo"));
    }

    #[test]
    fn test_subscription_matches_no_filter() {
        let sub = Subscription {
            patterns: vec!["task.*".to_string()],
            filter: None,
        };
        let event = Event {
            event_type: "task.created".to_string(),
            project_id: Some("p1".to_string()),
            agent_id: Some("a1".to_string()),
            data: serde_json::Value::Null,
            timestamp: Utc::now(),
        };
        assert!(subscription_matches(&sub, &event, "a1"));
    }

    #[test]
    fn test_subscription_matches_agent_filter_self() {
        let sub = Subscription {
            patterns: vec!["task.*".to_string()],
            filter: Some(SubscriptionFilter {
                agent_id: Some("self".to_string()),
                project_id: None,
            }),
        };
        let event_match = Event {
            event_type: "task.assigned".to_string(),
            project_id: None,
            agent_id: Some("agent-42".to_string()),
            data: serde_json::Value::Null,
            timestamp: Utc::now(),
        };
        let event_no_match = Event {
            event_type: "task.assigned".to_string(),
            project_id: None,
            agent_id: Some("other-agent".to_string()),
            data: serde_json::Value::Null,
            timestamp: Utc::now(),
        };
        assert!(subscription_matches(&sub, &event_match, "agent-42"));
        assert!(!subscription_matches(&sub, &event_no_match, "agent-42"));
    }

    #[test]
    fn test_subscription_matches_project_filter() {
        let sub = Subscription {
            patterns: vec!["task.created".to_string()],
            filter: Some(SubscriptionFilter {
                agent_id: None,
                project_id: Some("proj-1".to_string()),
            }),
        };
        let event_match = Event {
            event_type: "task.created".to_string(),
            project_id: Some("proj-1".to_string()),
            agent_id: None,
            data: serde_json::Value::Null,
            timestamp: Utc::now(),
        };
        let event_no_match = Event {
            event_type: "task.created".to_string(),
            project_id: Some("proj-2".to_string()),
            agent_id: None,
            data: serde_json::Value::Null,
            timestamp: Utc::now(),
        };
        assert!(subscription_matches(&sub, &event_match, "x"));
        assert!(!subscription_matches(&sub, &event_no_match, "x"));
    }

    #[test]
    fn test_subscription_no_pattern_match() {
        let sub = Subscription {
            patterns: vec!["project.*".to_string()],
            filter: None,
        };
        let event = Event {
            event_type: "task.created".to_string(),
            project_id: None,
            agent_id: None,
            data: serde_json::Value::Null,
            timestamp: Utc::now(),
        };
        assert!(!subscription_matches(&sub, &event, "x"));
    }
}
