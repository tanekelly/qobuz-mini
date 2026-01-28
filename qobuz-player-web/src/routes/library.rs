use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, State},
    routing::get,
};
use serde_json::json;

use crate::{AppState, ResponseResult, ok_or_error_page};

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum Tab {
    Albums,
    Artists,
    Playlists,
    Tracks,
}

pub fn routes() -> Router<std::sync::Arc<crate::AppState>> {
    Router::new().route("/library/{tab}", get(index))
}

async fn index(State(state): State<Arc<AppState>>, Path(tab): Path<Tab>) -> ResponseResult {
    let library = ok_or_error_page(&state, state.get_library().await)?;

    Ok(state.render(
        "library.html",
        &json!({"library": library, "tab": tab}),
    ))
}
