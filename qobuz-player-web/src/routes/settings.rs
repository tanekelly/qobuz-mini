use std::sync::Arc;

use axum::{
    extract::State,
    response::Response,
    routing::{get, post},
    Router,
    Form,
};
use serde::Deserialize;
use serde_json::json;

use qobuz_player_controls::{list_audio_devices, notification::Notification};

use crate::{AppState, ResponseResult, hx_redirect, ok_or_error_page};

#[derive(Deserialize)]
struct SetDeviceForm {
    device_name: Option<String>,
}

#[derive(Deserialize)]
struct SetPreferredGenreForm {
    preferred_genre_id: Option<String>,
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/settings", get(index))
        .route("/settings/partial", get(settings_partial))
        .route("/settings/sign-out", post(sign_out))
        .route("/settings/devices", get(get_devices))
        .route("/settings/set-device", post(set_device))
        .route("/settings/set-preferred-genre", post(set_preferred_genre))
        .route("/disconnected", get(disconnected))
}

async fn index(State(state): State<Arc<AppState>>) -> ResponseResult {
    let devices = list_audio_devices().unwrap_or_default();
    let config = state.database.get_configuration().await.ok();
    let selected_device = config.as_ref().and_then(|c| c.audio_device_name.clone());
    let selected_device_name = selected_device.as_deref().unwrap_or("");
    let preferred_genre_id = config.and_then(|c| c.preferred_genre_id);
    let is_discover = preferred_genre_id.is_none();
    let genres = state.client.genres().await.unwrap_or_default();

    Ok(state.render("settings.html", &json!({
        "devices": devices,
        "selected_device": selected_device,
        "selected_device_name": selected_device_name,
        "genres": genres,
        "preferred_genre_id": preferred_genre_id,
        "is_discover": is_discover,
    })))
}

async fn settings_partial(State(state): State<Arc<AppState>>) -> ResponseResult {
    let devices = list_audio_devices().unwrap_or_default();
    let config = state.database.get_configuration().await.ok();
    let selected_device = config.as_ref().and_then(|c| c.audio_device_name.clone());
    let selected_device_name = selected_device.as_deref().unwrap_or("");
    let preferred_genre_id = config.and_then(|c| c.preferred_genre_id);
    let is_discover = preferred_genre_id.is_none();
    let genres = state.client.genres().await.unwrap_or_default();

    Ok(state.render("settings-content.html", &json!({
        "devices": devices,
        "selected_device": selected_device,
        "selected_device_name": selected_device_name,
        "genres": genres,
        "preferred_genre_id": preferred_genre_id,
        "is_discover": is_discover,
    })))
}

async fn get_devices(State(state): State<Arc<AppState>>) -> ResponseResult {
    let devices = list_audio_devices().unwrap_or_default();
    Ok(state.render("settings-devices.html", &json!({
        "devices": devices,
    })))
}

async fn set_device(
    State(state): State<Arc<AppState>>,
    Form(form): Form<SetDeviceForm>,
) -> ResponseResult {
    let device_name = form.device_name.filter(|s| !s.is_empty());
    
    tracing::info!("Setting audio device to: {:?}", device_name);
    
    if let Some(ref name) = device_name {
        let devices = list_audio_devices().unwrap_or_default();
        if !devices.iter().any(|d| &d.name == name) {
            tracing::warn!("Device '{}' not found", name);
            let devices = list_audio_devices().unwrap_or_default();
            let config = state.database.get_configuration().await.ok();
            let selected_device = config.as_ref().and_then(|c| c.audio_device_name.clone());
            let selected_device_name = selected_device.as_deref().unwrap_or("");
            let preferred_genre_id = config.and_then(|c| c.preferred_genre_id);
            let is_discover = preferred_genre_id.is_none();
            let genres = state.client.genres().await.unwrap_or_default();
            return Ok(state.render("settings-content.html", &json!({
                "devices": devices,
                "selected_device": selected_device,
                "selected_device_name": selected_device_name,
                "genres": genres,
                "preferred_genre_id": preferred_genre_id,
                "is_discover": is_discover,
                "error": format!("Device '{}' not found", name),
            })));
        }
    }

    if let Err(e) = state.database.set_audio_device(device_name.clone()).await {
        tracing::error!("Failed to set audio device: {}", e);
        return ok_or_error_page(&state, Err(e.into()));
    }

    state.controls.set_audio_device(device_name.clone());

    let device_display = device_name.as_ref().map(|s| s.as_str()).unwrap_or("Default");
    state.broadcast.send(Notification::Info(
        format!("Audio device changed to '{}'.", device_display)
    ));
    state.send_sse("device".to_string(), "changed".to_string());

    let devices = list_audio_devices().unwrap_or_default();
    let config = state.database.get_configuration().await.ok();
    let selected_device = config.as_ref().and_then(|c| c.audio_device_name.clone());
    let selected_device_name = selected_device.as_deref().unwrap_or("");
    let preferred_genre_id = config.and_then(|c| c.preferred_genre_id);
    let is_discover = preferred_genre_id.is_none();
    let genres = state.client.genres().await.unwrap_or_default();
    Ok(state.render("settings-content.html", &json!({
        "devices": devices,
        "selected_device": selected_device,
        "selected_device_name": selected_device_name,
        "genres": genres,
        "preferred_genre_id": preferred_genre_id,
        "is_discover": is_discover,
    })))
}

async fn set_preferred_genre(
    State(state): State<Arc<AppState>>,
    Form(form): Form<SetPreferredGenreForm>,
) -> ResponseResult {
    let preferred_genre_id = form
        .preferred_genre_id
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<i64>().ok());

    if let Err(e) = state.database.set_preferred_genre_id(preferred_genre_id).await {
        tracing::error!("Failed to set preferred genre: {}", e);
        return ok_or_error_page(&state, Err(e.into()));
    }

    let genres = state.client.genres().await.unwrap_or_default();
    let label = preferred_genre_id
        .and_then(|id| genres.iter().find(|g| g.id == id).map(|g| g.name.as_str()))
        .unwrap_or("Discover");
    state.broadcast.send(Notification::Info(
        format!("Search default set to '{}'.", label)
    ));

    let devices = list_audio_devices().unwrap_or_default();
    let config = state.database.get_configuration().await.ok();
    let selected_device = config.as_ref().and_then(|c| c.audio_device_name.clone());
    let selected_device_name = selected_device.as_deref().unwrap_or("");
    let preferred_genre_id = config.and_then(|c| c.preferred_genre_id);
    let is_discover = preferred_genre_id.is_none();
    Ok(state.render("settings-content.html", &json!({
        "devices": devices,
        "selected_device": selected_device,
        "selected_device_name": selected_device_name,
        "genres": genres,
        "preferred_genre_id": preferred_genre_id,
        "is_discover": is_discover,
    })))
}

async fn sign_out(State(state): State<Arc<AppState>>) -> Response {
    match state.database.refresh_database().await {
        Ok(_) => {
            let exit_sender = state.exit_sender.clone();
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                _ = exit_sender.send(true);
            });
            hx_redirect("/disconnected")
        }
        Err(err) => ok_or_error_page::<()>(&state, Err(err.into())).unwrap_err(),
    }
}

async fn disconnected(State(state): State<Arc<AppState>>) -> ResponseResult {
    Ok(state.render("disconnected.html", &json!({})))
}
