use async_trait::async_trait;
use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
};

use crate::app::AppState;
use crate::db_ops;
use opengate_models::Identity;

// Axum extractor for Identity — always succeeds.
// Returns AgentIdentity if a valid API key is provided, Anonymous otherwise.
#[async_trait]
impl FromRequestParts<AppState> for Identity {
    type Rejection = (StatusCode, String);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer ").map(|s| s.to_string()));

        if let Some(token) = token {
            let hash = db_ops::hash_api_key(&token);
            let conn = state.db.lock().unwrap();
            if let Some(agent) = db_ops::get_agent_by_key_hash(&conn, &hash) {
                db_ops::update_heartbeat(&conn, &agent.id);
                return Ok(Identity::AgentIdentity {
                    id: agent.id,
                    name: agent.name,
                });
            }
        }

        Ok(Identity::Anonymous)
    }
}
