use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, State},
    response::IntoResponse,
    routing::get,
};
use serde_json::json;

use crate::{AppState, ResponseResult, ok_or_error_page};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/genres", get(index))
        .route("/genres/{id}", get(detail))
}

async fn index(State(state): State<Arc<AppState>>) -> ResponseResult {
    let genres = ok_or_error_page(&state, state.client.genres().await)?;

    Ok(state.render(
        "genres.html",
        &json!({"genres": genres}),
    ))
}

async fn detail(State(state): State<Arc<AppState>>, Path(id): Path<u32>) -> ResponseResult {
    let genres = ok_or_error_page(&state, state.client.genres().await)?;
    let genre = genres
        .into_iter()
        .find(|g| g.id == id)
        .ok_or_else(|| {
            state
                .templates
                .borrow()
                .render("error-page.html", &json!({"error": "Genre not found"}))
                .into_response()
        })?;

    let (albums, playlists) = ok_or_error_page(
        &state,
        tokio::try_join!(
            state.client.genre_albums(id),
            state.client.genre_playlists(id),
        ),
    )?;

    Ok(state.render(
        "genre-detail.html",
        &json!({"genre": genre, "albums": albums, "playlists": playlists}),
    ))
}
