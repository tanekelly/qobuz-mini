use assets::static_handler;
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response, Sse, sse::Event},
    routing::{get, post},
    Form,
};
use futures::stream::Stream;
use qobuz_player_client::client::AudioQuality;
use qobuz_player_controls::{
    ExitSender, PositionReceiver, Result, Status, StatusReceiver, TracklistReceiver, VolumeReceiver,
    client::Client,
    controls::Controls,
    database::Database,
    error::Error,
    notification::{Notification, NotificationBroadcast},
};
use qobuz_player_models::{Album, AlbumSimple, Playlist};
use qobuz_player_rfid::RfidState;
use serde_json::json;
use skabelon::Templates;
use std::{convert::Infallible, env, sync::Arc};
use tokio::sync::{
    broadcast::{self, Receiver, Sender},
    watch, RwLock,
};
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::BroadcastStream;

use crate::{
    app_state::AppState,
    routes::{
        album, api, artist, auth, controls, discover, genre, library, now_playing, playlist, queue,
        search, settings,
    },
    views::templates,
};

mod app_state;
mod assets;
mod routes;
mod views;

pub struct AuthState {
    pub database: Arc<Database>,
    pub templates: watch::Receiver<Templates>,
}

impl AuthState {
    pub fn render<T>(&self, view: &str, context: &T) -> Response
    where
        T: serde::Serialize,
    {
        let context_value = serde_json::to_value(context).unwrap_or_default();
        let templates = self.templates.borrow();
        let render = templates.render(view, &context_value);
        Html(render).into_response()
    }
}

pub const AUTH_SUCCESS_HTML: &str = r#"<!doctype html>
<html lang="en" class="h-full dark">
  <head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Auth Success</title>
    <style>
      body {
        display: flex;
        justify-content: center;
        align-items: center;
        height: 100vh;
        margin: 0;
        background-color: #000;
        color: #f3f4f6;
        font-family: system-ui, -apple-system, sans-serif;
      }
      .message {
        text-align: center;
        font-size: 1.5rem;
      }
    </style>
  </head>
  <body>
    <div class="message">
      Auth complete, you can now close this page
    </div>
  </body>
</html>"#;

pub async fn validate_credentials(
    username: &str,
    password: &str,
    database: &Database,
) -> Result<()> {
    let max_audio_quality = database
        .get_configuration()
        .await
        .ok()
        .and_then(|config| config.max_audio_quality.try_into().ok())
        .unwrap_or(AudioQuality::HIFI192);

    qobuz_player_client::client::new(username, password, max_audio_quality).await?;
    Ok(())
}

pub async fn save_credentials(
    database: &Database,
    username: String,
    password: String,
) -> Result<()> {
    database.set_username(username).await?;
    database.set_password(password).await?;
    Ok(())
}

pub async fn init_auth_only(
    port: u16,
    database: Arc<Database>,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> Result<()> {
    let interface = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&interface)
        .await
        .or(Err(Error::PortInUse { port }))?;

    let template_path = {
        let current_dir = env::current_dir().expect("Failed to get current directory");
        current_dir.join("qobuz-player-web/templates")
    };

    let templates = templates(&template_path);
    let (_templates_tx, templates_rx) = watch::channel(templates);

    let auth_state = Arc::new(AuthState {
        database,
        templates: templates_rx,
    });

    let router = axum::Router::new()
        .route("/auth", get(auth_index))
        .route("/auth/login", post(auth_login))
        .route("/assets/{*file}", get(static_handler))
        .with_state(auth_state);

    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            shutdown.await.ok();
        })
        .await
        .expect("infallible");
    Ok(())
}

async fn auth_index(State(state): State<Arc<AuthState>>) -> impl IntoResponse {
    state.render("qobuz-login-page.html", &json!({}))
}

#[derive(serde::Deserialize)]
struct LoginParameters {
    username: String,
    password: String,
}

async fn auth_login(
    State(state): State<Arc<AuthState>>,
    Form(parameters): Form<LoginParameters>,
) -> impl IntoResponse {
    match validate_credentials(
        &parameters.username,
        &parameters.password,
        &state.database,
    )
    .await
    {
        Ok(()) => {
            match save_credentials(
                &state.database,
                parameters.username,
                parameters.password,
            )
            .await
            {
                Ok(()) => create_auth_success_response(),
                Err(_) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to save credentials",
                )
                    .into_response(),
            }
        }
        Err(_) => {
            let error_html = state.render("qobuz-login-page.html", &json!({
                "error": "Invalid credentials."
            }));
            (StatusCode::UNAUTHORIZED, error_html).into_response()
        }
    }
}

fn create_auth_success_response() -> Response {
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static(mime::TEXT_HTML_UTF_8.as_ref()),
        )],
        AUTH_SUCCESS_HTML,
    )
        .into_response()
}

#[allow(clippy::too_many_arguments)]
pub async fn init(
    controls: Controls,
    position_receiver: PositionReceiver,
    tracklist_receiver: TracklistReceiver,
    volume_receiver: VolumeReceiver,
    status_receiver: StatusReceiver,
    port: u16,
    web_secret: Option<String>,
    rfid_state: Option<RfidState>,
    broadcast: Arc<NotificationBroadcast>,
    client: Arc<Client>,
    database: Arc<Database>,
    exit_sender: ExitSender,
) -> Result<()> {
    let interface = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&interface)
        .await
        .or(Err(Error::PortInUse { port }))?;

    let router = create_router(
        controls,
        position_receiver,
        tracklist_receiver,
        volume_receiver,
        status_receiver,
        web_secret,
        rfid_state,
        broadcast,
        client,
        database,
        exit_sender,
    )
    .await;

    axum::serve(listener, router).await.expect("infallible");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn create_router(
    controls: Controls,
    position_receiver: PositionReceiver,
    tracklist_receiver: TracklistReceiver,
    volume_receiver: VolumeReceiver,
    status_receiver: StatusReceiver,
    web_secret: Option<String>,
    rfid_state: Option<RfidState>,
    broadcast: Arc<NotificationBroadcast>,
    client: Arc<Client>,
    database: Arc<Database>,
    exit_sender: ExitSender,
) -> Router {
    let (tx, _rx) = broadcast::channel::<ServerSentEvent>(100);
    let broadcast_subscribe = broadcast.subscribe();

    let template_path = {
        let current_dir = env::current_dir().expect("Failed to get current directory");
        current_dir.join("qobuz-player-web/templates")
    };

    let templates = templates(&template_path);

    #[allow(unused_variables)]
    let (templates_tx, templates_rx) = watch::channel(templates);

    #[cfg(all(debug_assertions, target_os = "linux"))]
    {
        let templates_clone = templates_rx.clone();
        let watcher_sender = tx.clone();
        let watcher = filesentry::Watcher::new().unwrap();
        watcher.add_root(&template_path, true, |_| ()).unwrap();

        watcher.add_handler(move |events| {
            for event in &*events {
                match event.ty {
                    filesentry::EventType::Modified | filesentry::EventType::Create => {
                        let mut templates = templates_clone.borrow().clone();
                        templates.reload();
                        templates_tx.send(templates).unwrap();

                        let event = ServerSentEvent {
                            event_name: "reload".into(),
                            event_data: "template changed".into(),
                        };

                        _ = watcher_sender.send(event);
                    }
                    _ => (),
                }
            }
            true
        });
        watcher.start();
    }

    let shared_state = Arc::new(AppState {
        controls,
        web_secret,
        rfid_state,
        broadcast,
        client,
        tx: tx.clone(),
        position_receiver: position_receiver.clone(),
        tracklist_receiver: tracklist_receiver.clone(),
        volume_receiver: volume_receiver.clone(),
        status_receiver: status_receiver.clone(),
        templates: templates_rx.clone(),
        database,
        exit_sender,
        library_cache: Arc::new(RwLock::new(app_state::LibraryCache::new())),
    });

    tokio::spawn(background_task(
        tx,
        broadcast_subscribe,
        position_receiver,
        tracklist_receiver,
        volume_receiver,
        status_receiver,
        templates_rx,
    ));

    axum::Router::new()
        .route("/sse", get(sse_handler))
        .merge(now_playing::routes())
        .merge(queue::routes())
        .merge(api::routes())
        .merge(search::routes())
        .merge(discover::routes())
        .merge(album::routes())
        .merge(artist::routes())
        .merge(playlist::routes())
        .merge(genre::routes())
        .merge(library::routes())
        .merge(controls::routes())
        .merge(settings::routes())
        .layer(axum::middleware::from_fn_with_state(
            shared_state.clone(),
            auth::auth_middleware,
        ))
        .route("/assets/{*file}", get(static_handler))
        .merge(auth::routes())
        .with_state(shared_state.clone())
}

async fn background_task(
    tx: Sender<ServerSentEvent>,
    mut receiver: Receiver<Notification>,
    mut position: PositionReceiver,
    mut tracklist: TracklistReceiver,
    mut volume: VolumeReceiver,
    mut status: StatusReceiver,
    templates: watch::Receiver<Templates>,
) {
    loop {
        tokio::select! {
            Ok(_) = position.changed() => {
                let position_duration = position.borrow_and_update();
                let event = ServerSentEvent {
                    event_name: "position".into(),
                    event_data: position_duration.as_millis().to_string(),
                };

                _ = tx.send(event);
            },
            Ok(_) = tracklist.changed() => {
                _ = tracklist.borrow_and_update();
                let event = ServerSentEvent {
                    event_name: "tracklist".into(),
                    event_data: "new tracklist".into(),
                };
                _ = tx.send(event);
            },
            Ok(_) = volume.changed() => {
                let volume_value = *volume.borrow_and_update();
                let volume_percent = (volume_value * 100.0) as u32;
                let event = ServerSentEvent {
                    event_name: "volume".into(),
                    event_data: volume_percent.to_string(),
                };
                _ = tx.send(event);
            }
            Ok(_) = status.changed() => {
                let current_status = *status.borrow_and_update();
                let status_string = match current_status {
                    Status::Paused => "pause",
                    Status::Playing => "play",
                    Status::Buffering => "buffering",
                };

                let event = ServerSentEvent {
                    event_name: "status".into(),
                    event_data: status_string.into(),
                };
                _ = tx.send(event);
            }
            notification_result = receiver.recv() => {
                if let Ok(notification) = notification_result {
                    let (message_string, severity) = match &notification {
                        Notification::Error(message) => (message, 1),
                        Notification::Warning(message) => (message, 2),
                        Notification::Success(message) => (message, 3),
                        Notification::Info(message) => (message, 4),
                    };

                    let toast = templates.borrow().render("toast.html", &json!({"message": message_string, "severity": severity}));
                    let event_name = match notification {
                        Notification::Error(_) => "error",
                        Notification::Warning(_) => "warn",
                        Notification::Success(_) => "success",
                        Notification::Info(_) => "info",
                    };

                    let event = ServerSentEvent {
                        event_name: event_name.into(),
                        event_data: toast,
                    };
                    _ = tx.send(event);
                }
            }
        }
    }
}

async fn sse_handler(
    State(state): State<Arc<AppState>>,
) -> (
    axum::http::HeaderMap,
    Sse<impl Stream<Item = Result<Event, Infallible>>>,
) {
    let rx = state.tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(event) => Some(Ok(Event::default()
            .event(event.event_name)
            .data(event.event_data))),
        Err(_) => None,
    });

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("X-Accel-Buffering", "no".parse().expect("infallible"));

    (headers, Sse::new(stream))
}

#[derive(Clone)]
pub struct AlbumData {
    pub album: Album,
    pub suggested_albums: Vec<AlbumSimple>,
}

#[derive(Clone)]
pub struct ServerSentEvent {
    event_name: String,
    event_data: String,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct Discover {
    pub albums: Vec<(String, Vec<AlbumSimple>)>,
    pub playlists: Vec<(String, Vec<Playlist>)>,
}

type ResponseResult = std::result::Result<axum::response::Response, axum::response::Response>;

#[allow(clippy::result_large_err)]
fn ok_or_send_error_toast<T>(
    state: &AppState,
    value: Result<T, qobuz_player_controls::error::Error>,
) -> Result<T, axum::response::Response> {
    match value {
        Ok(value) => Ok(value),
        Err(err) => Err(state.send_toast(Notification::Error(err.to_string()))),
    }
}

#[allow(clippy::result_large_err)]
fn ok_or_error_page<T>(
    state: &AppState,
    value: Result<T, qobuz_player_controls::error::Error>,
) -> Result<T, axum::response::Response> {
    match value {
        Ok(value) => Ok(value),
        Err(err) => Err(Html(
            state
                .templates
                .borrow()
                .render("error-page.html", &json!({"error": err.to_string()})),
        )
        .into_response()),
    }
}

#[allow(clippy::result_large_err, unused)]
fn ok_or_broadcast<T>(
    broadcast: &NotificationBroadcast,
    value: Result<T, qobuz_player_controls::error::Error>,
) -> Result<T, axum::response::Response> {
    match value {
        Ok(value) => Ok(value),
        Err(err) => {
            broadcast.send(Notification::Error(err.to_string()));

            let mut response = Html("<div></div>".to_string()).into_response();
            let headers = response.headers_mut();
            headers.insert("HX-Reswap", "none".try_into().expect("infallible"));

            Err(response)
        }
    }
}

pub fn hx_redirect(url: &str) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert("HX-Redirect", url.parse().unwrap());
    (StatusCode::OK, headers).into_response()
}
