use crate::storage::StorageBackend;
use opengate_models::*;
use std::sync::Arc;

/// Fire webhook notifications for a list of pending notification webhooks.
pub fn fire_notification_webhooks(
    storage: Arc<dyn StorageBackend>,
    pending: Vec<PendingNotifWebhook>,
) {
    for notif in pending {
        let agent = match storage.get_agent(None, &notif.agent_id) {
            Some(a) => a,
            None => continue,
        };
        let webhook_url = agent.webhook_url.clone();
        let webhook_events = agent.webhook_events.clone();

        let url = match webhook_url {
            Some(u) if !u.is_empty() => u,
            _ => continue,
        };

        if let Some(ref events) = webhook_events {
            if !events.is_empty() && !events.iter().any(|e| e == &notif.event_type) {
                continue;
            }
        }

        let payload = serde_json::json!({
            "event": "notification",
            "notification_id": notif.notification_id,
            "event_type": notif.event_type,
            "title": notif.title,
            "body": notif.body,
            "timestamp": chrono::Utc::now().to_rfc3339()
        });

        let storage_clone = storage.clone();
        let notification_id = notif.notification_id;
        let agent_id = notif.agent_id.clone();

        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let max_attempts: i64 = 3;

            for attempt in 1..=max_attempts {
                let result = client
                    .post(&url)
                    .json(&payload)
                    .timeout(std::time::Duration::from_secs(10))
                    .send()
                    .await;

                match result {
                    Ok(resp) => {
                        let status_code = resp.status().as_u16();
                        let success = (200..300).contains(&status_code);
                        let _ = resp.text().await;
                        if success {
                            storage_clone.ack_notification_system(None, notification_id);
                            storage_clone.update_notification_webhook_status(
                                None,
                                notification_id,
                                "delivered",
                            );
                            eprintln!(
                                "[webhook] notif {} delivered to agent {}, auto-acked",
                                notification_id, agent_id
                            );
                            return;
                        }
                        if attempt == max_attempts {
                            storage_clone.update_notification_webhook_status(
                                None,
                                notification_id,
                                "failed",
                            );
                            eprintln!("[webhook] notif {} failed for agent {} (HTTP {}); left unread for polling", notification_id, agent_id, status_code);
                        }
                    }
                    Err(e) => {
                        if attempt == max_attempts {
                            storage_clone.update_notification_webhook_status(
                                None,
                                notification_id,
                                "failed",
                            );
                            eprintln!("[webhook] notif {} failed for agent {} ({}); left unread for polling", notification_id, agent_id, e);
                        }
                    }
                }

                if attempt < max_attempts {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        attempt as u64 * attempt as u64,
                    ))
                    .await;
                }
            }
        });
    }
}

/// Fire a webhook event to an agent's webhook_url
pub fn fire_webhook(
    storage: Arc<dyn StorageBackend>,
    agent_id: &str,
    event_type: &str,
    task: &Task,
) {
    let agent = match storage.get_agent(None, agent_id) {
        Some(a) => a,
        None => return,
    };
    let webhook_url = match &agent.webhook_url {
        Some(url) if !url.is_empty() => url.clone(),
        _ => return,
    };

    let payload = serde_json::json!({
        "event": event_type,
        "task_id": task.id,
        "task": task,
        "timestamp": chrono::Utc::now().to_rfc3339()
    });

    let log_id = storage.create_webhook_log(None, agent_id, event_type, &payload);

    let storage_clone = storage.clone();
    let log_id_clone = log_id;
    let url = webhook_url;
    let payload_clone = payload;

    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let max_attempts: i64 = 3;

        for attempt in 1..=max_attempts {
            let result: Result<reqwest::Response, reqwest::Error> = client
                .post(&url)
                .json(&payload_clone)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await;

            match result {
                Ok(resp) => {
                    let status_code: i64 = resp.status().as_u16() as i64;
                    let body: String = resp.text().await.unwrap_or_default();
                    let delivery_status = if (200..300).contains(&(status_code as u16)) {
                        "delivered"
                    } else {
                        "failed"
                    };
                    storage_clone.update_webhook_log(
                        None,
                        &log_id_clone,
                        delivery_status,
                        attempt,
                        Some(status_code),
                        Some(&body),
                    );
                    if delivery_status == "delivered" {
                        return;
                    }
                }
                Err(e) => {
                    let err_str: String = e.to_string();
                    let delivery_status = if attempt == max_attempts {
                        "failed"
                    } else {
                        "pending"
                    };
                    storage_clone.update_webhook_log(
                        None,
                        &log_id_clone,
                        delivery_status,
                        attempt,
                        None,
                        Some(&err_str),
                    );
                }
            }

            if attempt < max_attempts {
                tokio::time::sleep(std::time::Duration::from_secs(
                    attempt as u64 * attempt as u64,
                ))
                .await;
            }
        }
    });
}

/// Fire webhook events related to task assignment
pub fn fire_assignment_webhook(storage: Arc<dyn StorageBackend>, task: &Task) {
    if let Some(ref agent_id) = task.assignee_id {
        if task.assignee_type.as_deref() == Some("agent") {
            fire_webhook(storage, agent_id, "task.assigned", task);
        }
    }
}

/// Fire webhook when task is updated
pub fn fire_update_webhook(storage: Arc<dyn StorageBackend>, task: &Task) {
    if let Some(ref agent_id) = task.assignee_id {
        if task.assignee_type.as_deref() == Some("agent") {
            fire_webhook(storage, agent_id, "task.updated", task);
        }
    }
}

/// Fire webhook when dependencies are ready
pub fn fire_dependency_ready_webhook(storage: Arc<dyn StorageBackend>, task: &Task) {
    if let Some(ref agent_id) = task.assignee_id {
        if task.assignee_type.as_deref() == Some("agent") {
            fire_webhook(storage, agent_id, "task.dependency_ready", task);
        }
    }
}

/// Fire webhook to reviewer when task enters review
pub fn fire_review_requested_webhook(storage: Arc<dyn StorageBackend>, task: &Task) {
    if let Some(ref reviewer_id) = task.reviewer_id {
        fire_webhook(storage, reviewer_id, "task.review_requested", task);
    }
}
