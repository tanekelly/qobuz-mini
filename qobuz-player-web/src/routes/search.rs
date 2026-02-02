use std::sync::Arc;

use axum::{
    Form, Router,
    extract::{Path, Query, State},
    routing::get,
};
use qobuz_player_models::SearchResults;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::try_join;

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Tab {
    Albums,
    Artists,
    Playlists,
    Tracks,
}

use crate::{AppState, Discover, ResponseResult, ok_or_error_page, ok_or_send_error_toast};

pub fn routes() -> Router<std::sync::Arc<crate::AppState>> {
    Router::new()
        .route("/discover", get(redirect_to_search))
        .route("/search/{tab}", get(index).post(search))
}

async fn redirect_to_search() -> axum::response::Redirect {
    axum::response::Redirect::to("/search/albums")
}

#[derive(Deserialize)]
struct SearchParameters {
    query: Option<String>,
}

async fn index(
    State(state): State<Arc<AppState>>,
    Path(tab): Path<Tab>,
    Query(parameters): Query<SearchParameters>,
) -> ResponseResult {
    let query = parameters
        .query
        .and_then(|s| if s.is_empty() { None } else { Some(s) });
    let (search_results, discover) = match &query {
        Some(q) => {
            let results = ok_or_error_page(&state, state.client.search(q.clone()).await)?;
            (results, None as Option<Discover>)
        }
        None => {
            let preferred_genre_id = state
                .database
                .get_configuration()
                .await
                .ok()
                .and_then(|c| c.preferred_genre_id)
                .filter(|&id| id != 0);

            let (albums, playlists) = if let Some(genre_id) = preferred_genre_id {
                let albums = ok_or_error_page(&state, state.client.genre_albums(genre_id).await)?;
                (albums, vec![])
            } else {
                ok_or_error_page(
                    &state,
                    try_join!(
                        state.client.featured_albums(),
                        state.client.featured_playlists(),
                    ),
                )?
            };
            (
                SearchResults::default(),
                Some(Discover { albums, playlists }),
            )
        }
    };

    Ok(state.render(
        "search.html",
        &json!({"search_results": search_results, "tab": tab, "discover": discover}),
    ))
}

async fn search(
    State(state): State<Arc<AppState>>,
    Path(tab): Path<Tab>,
    Form(parameters): Form<SearchParameters>,
) -> ResponseResult {
    let query = parameters
        .query
        .and_then(|s| if s.is_empty() { None } else { Some(s) });
    let (search_results, discover) = match &query {
        Some(q) => {
            let results = ok_or_send_error_toast(&state, state.client.search(q.clone()).await)?;
            (results, None as Option<Discover>)
        }
        None => {
            let preferred_genre_id = state
                .database
                .get_configuration()
                .await
                .ok()
                .and_then(|c| c.preferred_genre_id)
                .filter(|&id| id != 0);

            let (albums, playlists) = if let Some(genre_id) = preferred_genre_id {
                let albums =
                    ok_or_send_error_toast(&state, state.client.genre_albums(genre_id).await)?;
                (albums, vec![])
            } else {
                ok_or_send_error_toast(
                    &state,
                    try_join!(
                        state.client.featured_albums(),
                        state.client.featured_playlists(),
                    ),
                )?
            };
            (
                SearchResults::default(),
                Some(Discover { albums, playlists }),
            )
        }
    };

    Ok(state.render(
        "search-content.html",
        &json!({"search_results": search_results, "tab": tab, "discover": discover}),
    ))
}
