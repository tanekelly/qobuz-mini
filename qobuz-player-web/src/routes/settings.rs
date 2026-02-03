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

#[derive(Deserialize)]
struct SetTimeStretchForm {
    time_stretch_ratio: Option<String>,
}

#[derive(Deserialize)]
struct SetPitchForm {
    pitch_semitones: Option<String>,
}

#[derive(Deserialize)]
struct SetPitchCentsForm {
    pitch_cents: Option<String>,
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/settings", get(index))
        .route("/settings/partial", get(settings_partial))
        .route("/settings/sign-out", post(sign_out))
        .route("/settings/devices", get(get_devices))
        .route("/settings/set-device", post(set_device))
        .route("/settings/set-preferred-genre", post(set_preferred_genre))
        .route("/settings/set-time-stretch", post(set_time_stretch))
        .route("/settings/set-pitch", post(set_pitch))
        .route("/settings/set-pitch-cents", post(set_pitch_cents))
        .route("/disconnected", get(disconnected))
}

fn settings_context(
    devices: Vec<qobuz_player_controls::AudioDevice>,
    config: Option<qobuz_player_controls::database::DatabaseConfiguration>,
    genres: Vec<qobuz_player_models::Genre>,
) -> serde_json::Value {
    let selected_device = config.as_ref().and_then(|c| c.audio_device_name.clone());
    let selected_device_name = selected_device.as_deref().unwrap_or("");
    let preferred_genre_id = config.as_ref().and_then(|c| c.preferred_genre_id);
    let is_discover = preferred_genre_id.is_none();
    let time_stretch_ratio = (config
        .as_ref()
        .map(|c| c.time_stretch_ratio)
        .unwrap_or(1.0)
        * 10.0)
        .round()
        / 10.0;
    let time_stretch_ratio_display = format!("{:.1}", time_stretch_ratio);
    let pitch_semitones = config.as_ref().map(|c| c.pitch_semitones).unwrap_or(0);
    let pitch_cents = config.as_ref().map(|c| c.pitch_cents).unwrap_or(0);
    json!({
        "devices": devices,
        "selected_device": selected_device,
        "selected_device_name": selected_device_name,
        "genres": genres,
        "preferred_genre_id": preferred_genre_id,
        "is_discover": is_discover,
        "time_stretch_ratio": time_stretch_ratio,
        "time_stretch_ratio_display": time_stretch_ratio_display,
        "pitch_semitones": pitch_semitones,
        "pitch_cents": pitch_cents,
    })
}

async fn index(State(state): State<Arc<AppState>>) -> ResponseResult {
    let devices = list_audio_devices().unwrap_or_default();
    let config = state.database.get_configuration().await.ok();
    let genres = state.client.genres().await.unwrap_or_default();
    let context = settings_context(devices, config, genres);
    Ok(state.render("settings.html", &context))
}

async fn settings_partial(State(state): State<Arc<AppState>>) -> ResponseResult {
    let devices = list_audio_devices().unwrap_or_default();
    let config = state.database.get_configuration().await.ok();
    let genres = state.client.genres().await.unwrap_or_default();
    let context = settings_context(devices, config, genres);
    Ok(state.render("settings-content.html", &context))
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
            let genres = state.client.genres().await.unwrap_or_default();
            let mut ctx = settings_context(devices, config, genres);
            if let Some(obj) = ctx.as_object_mut() {
                obj.insert("error".to_string(), json!(format!("Device '{}' not found", name)));
            }
            return Ok(state.render("settings-content.html", &ctx));
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
    let genres = state.client.genres().await.unwrap_or_default();
    let context = settings_context(devices, config, genres);
    Ok(state.render("settings-content.html", &context))
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
        .and_then(|id: i64| id.try_into().ok())
        .and_then(|id: u32| genres.iter().find(|g| g.id == id).map(|g| g.name.as_str()))
        .unwrap_or("Discover");
    state.broadcast.send(Notification::Info(
        format!("Search default set to '{}'.", label)
    ));

    let devices = list_audio_devices().unwrap_or_default();
    let config = state.database.get_configuration().await.ok();
    let genres = state.client.genres().await.unwrap_or_default();
    let context = settings_context(devices, config, genres);
    Ok(state.render("settings-content.html", &context))
}

async fn set_time_stretch(
    State(state): State<Arc<AppState>>,
    Form(form): Form<SetTimeStretchForm>,
) -> ResponseResult {
    let ratio = form
        .time_stretch_ratio
        .and_then(|s| s.parse::<f32>().ok())
        .map(|r| (r.clamp(0.5, 2.0) * 10.0).round() / 10.0)
        .unwrap_or(1.0);
    if let Err(e) = state.database.set_time_stretch_ratio(ratio).await {
        tracing::error!("Failed to set time stretch: {}", e);
        return ok_or_error_page(&state, Err(e.into()));
    }
    state.controls.set_time_stretch(ratio);
    let devices = list_audio_devices().unwrap_or_default();
    let config = state.database.get_configuration().await.ok();
    let genres = state.client.genres().await.unwrap_or_default();
    let context = settings_context(devices, config, genres);
    Ok(state.render("settings-content.html", &context))
}

async fn set_pitch(
    State(state): State<Arc<AppState>>,
    Form(form): Form<SetPitchForm>,
) -> ResponseResult {
    let semitones = form
        .pitch_semitones
        .and_then(|s| s.parse::<i16>().ok())
        .map(|s| s.clamp(-12, 12))
        .unwrap_or(0);
    if let Err(e) = state.database.set_pitch_semitones(semitones).await {
        tracing::error!("Failed to set pitch: {}", e);
        return ok_or_error_page(&state, Err(e.into()));
    }
    state.controls.set_pitch(semitones);
    let devices = list_audio_devices().unwrap_or_default();
    let config = state.database.get_configuration().await.ok();
    let genres = state.client.genres().await.unwrap_or_default();
    let context = settings_context(devices, config, genres);
    Ok(state.render("settings-content.html", &context))
}

async fn set_pitch_cents(
    State(state): State<Arc<AppState>>,
    Form(form): Form<SetPitchCentsForm>,
) -> ResponseResult {
    let cents = form
        .pitch_cents
        .and_then(|s| s.parse::<i16>().ok())
        .map(|s| s.clamp(-100, 100))
        .unwrap_or(0);
    if let Err(e) = state.database.set_pitch_cents(cents).await {
        tracing::error!("Failed to set pitch cents: {}", e);
        return ok_or_error_page(&state, Err(e.into()));
    }
    state.controls.set_pitch_cents(cents);
    let devices = list_audio_devices().unwrap_or_default();
    let config = state.database.get_configuration().await.ok();
    let genres = state.client.genres().await.unwrap_or_default();
    let context = settings_context(devices, config, genres);
    Ok(state.render("settings-content.html", &context))
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
