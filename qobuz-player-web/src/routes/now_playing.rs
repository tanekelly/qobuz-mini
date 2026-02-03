use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;

use crate::AppState;

pub fn routes() -> Router<std::sync::Arc<crate::AppState>> {
    Router::new()
        .route("/", get(index))
        .route("/status", get(status_partial))
        .route("/now-playing", get(now_playing_partial))
}

async fn index(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let time_stretch_ratio = state
        .database
        .get_configuration()
        .await
        .ok()
        .map(|c| c.time_stretch_ratio)
        .unwrap_or(1.0);
    now_playing(&state, false, time_stretch_ratio)
}

async fn status_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    state.render("play-pause.html", &())
}

async fn now_playing_partial(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let time_stretch_ratio = state
        .database
        .get_configuration()
        .await
        .ok()
        .map(|c| c.time_stretch_ratio)
        .unwrap_or(1.0);
    now_playing(&state, true, time_stretch_ratio)
}

fn now_playing(state: &AppState, partial: bool, time_stretch_ratio: f32) -> Response {
    let tracklist = state.tracklist_receiver.borrow().clone();
    let current_track = tracklist.current_track().cloned();

    let position_mseconds = state.position_receiver.borrow().as_millis();
    let current_volume = state.volume_receiver.borrow();
    let current_volume = (*current_volume * 100.0) as u32;

    let current_position = tracklist.current_position() + 1;

    let (duration_mseconds, explicit, hires_available) =
        current_track
            .as_ref()
            .map_or((None, false, false), |track| {
                let base_ms = track.duration_seconds as u64 * 1000;
                let stretched_ms = (base_ms as f64 / time_stretch_ratio as f64).round() as u64;
                (
                    Some(stretched_ms),
                    track.explicit,
                    track.hires_available,
                )
            });

    let duration_mseconds = duration_mseconds.unwrap_or_default();

    let number_of_tracks = tracklist.total();

    let position_string = mseconds_to_mm_ss(position_mseconds);
    let duration_string = mseconds_to_mm_ss(duration_mseconds);

    state.render(
        "now-playing.html",
        &json! ({
            "partial": partial,
            "number_of_tracks": number_of_tracks,
            "current_volume": current_volume,
            "duration_mseconds": duration_mseconds,
            "position_mseconds": position_mseconds,
            "position_string": position_string,
            "duration_string": duration_string,
            "current_position": current_position,
            "explicit": explicit,
            "hires_available": hires_available,
        }),
    )
}

fn mseconds_to_mm_ss<T: Into<u128>>(mseconds: T) -> String {
    let seconds = mseconds.into() / 1000;

    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}
