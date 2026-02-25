use axum::{Json};

use opengate_models::*;

pub async fn me(identity: Identity) -> Json<serde_json::Value> {
    match identity {
        Identity::AgentIdentity { id, name } => Json(serde_json::json!({
            "type": "agent",
            "id": id,
            "name": name,
        })),
        Identity::Anonymous => Json(serde_json::json!({
            "type": "anonymous",
        })),
    }
}
