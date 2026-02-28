use async_trait::async_trait;
use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
};

use crate::app::AppState;
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
        // If identity was pre-resolved by product middleware, use it directly
        if let Some(identity) = parts.extensions.get::<Identity>() {
            return Ok(identity.clone());
        }

        let token = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer ").map(|s| s.to_string()));

        if let Some(token) = token {
            let hash = state.storage.hash_api_key(&token);
            if let Some(agent) = state.storage.get_agent_by_key_hash(None, &hash) {
                state.storage.update_heartbeat(None, &agent.id);
                return Ok(Identity::AgentIdentity {
                    id: agent.id,
                    name: agent.name,
                    tenant_id: agent.owner_id,
                });
            }
        }

        Ok(Identity::Anonymous)
    }
}
