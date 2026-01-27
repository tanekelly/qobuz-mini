use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::State,
    response::Response,
    routing::{get, post},
    Router,
};
use qobuz_player_controls::error::Error;
use serde_json::json;
use tokio::time::sleep;

use crate::{AppState, ResponseResult, ok_or_error_page};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/settings", get(index))
        .route("/settings/sign-out", post(sign_out))
        .route("/disconnected", get(disconnected))
}

async fn index(State(state): State<Arc<AppState>>) -> ResponseResult {
    Ok(state.render("settings.html", &json!({})))
}

async fn sign_out(State(state): State<Arc<AppState>>) -> Response {
    match state.database.refresh_database().await {
        Ok(_) => {
            let exit_sender = state.exit_sender.clone();
            tokio::spawn(async move {
                sleep(Duration::from_secs(3)).await;
                _ = exit_sender.send(true).map_err(|_| Error::Notification);
            });
            crate::hx_redirect("/disconnected")
        }
        Err(err) => ok_or_error_page::<()>(&state, Err(err.into())).unwrap_err(),
    }
}

async fn disconnected(State(state): State<Arc<AppState>>) -> ResponseResult {
    Ok(state.render("disconnected.html", &json!({})))
}
