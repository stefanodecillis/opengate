use axum::{extract::State, Json};

use crate::app::AppState;
use crate::db_ops;
use opengate_models::*;

pub async fn get_stats(State(state): State<AppState>, _identity: Identity) -> Json<DashboardStats> {
    let conn = state.db.lock().unwrap();
    Json(db_ops::get_stats(&conn))
}
