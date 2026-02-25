use axum::{extract::State, Json};

use crate::app::AppState;
use opengate_models::*;

pub async fn get_stats(State(state): State<AppState>, _identity: Identity) -> Json<DashboardStats> {
    Json(state.storage.get_stats(None))
}
