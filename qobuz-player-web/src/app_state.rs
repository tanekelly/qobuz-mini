use axum::response::{Html, IntoResponse, Response};
use qobuz_player_controls::{
    ExitSender, PositionReceiver, Result, Status, StatusReceiver, TracklistReceiver, VolumeReceiver,
    client::Client,
    controls::Controls,
    database::Database,
    notification::{Notification, NotificationBroadcast},
    AudioQuality,
};
use qobuz_player_models::Library;
use qobuz_player_rfid::RfidState;
use serde_json::json;
use skabelon::Templates;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::{sync::{broadcast::Sender, RwLock, watch}, try_join};

use crate::{AlbumData, ServerSentEvent};

pub(crate) struct LibraryCache {
    value: Option<Library>,
    created: Option<Instant>,
}

impl LibraryCache {
    pub(crate) fn new() -> Self {
        Self {
            value: None,
            created: None,
        }
    }

    fn get(&self, ttl: Duration) -> Option<Library> {
        if let (Some(value), Some(created)) = (&self.value, &self.created) {
            if created.elapsed() < ttl {
                return Some(value.clone());
            }
        }
        None
    }

    fn set(&mut self, value: Library) {
        self.value = Some(value);
        self.created = Some(Instant::now());
    }

    fn clear(&mut self) {
        self.value = None;
        self.created = None;
    }
}

pub struct AppState {
    pub tx: Sender<ServerSentEvent>,
    pub web_secret: Option<String>,
    pub rfid_state: Option<RfidState>,
    pub broadcast: Arc<NotificationBroadcast>,
    pub client: Arc<Client>,
    pub controls: Controls,
    pub position_receiver: PositionReceiver,
    pub tracklist_receiver: TracklistReceiver,
    pub status_receiver: StatusReceiver,
    pub volume_receiver: VolumeReceiver,
    pub templates: watch::Receiver<Templates>,
    pub database: Arc<Database>,
    pub exit_sender: ExitSender,
    pub library_cache: Arc<RwLock<LibraryCache>>,
}

impl AppState {
    pub fn render<T>(&self, view: &str, context: &T) -> Response
    where
        T: serde::Serialize,
    {
        let tracklist = self.tracklist_receiver.borrow();
        let current_track = tracklist.current_track();
        let status = *self.status_receiver.borrow();

        let (title, artist_link, artist_name, hires_available) = current_track
            .map(|track| (
                track.title.clone(),
                track.artist_id.map(|id| format!("/artist/{id}")),
                track.artist_name.clone(),
                track.hires_available,
            ))
            .unwrap_or_else(|| (String::default(), None, None, false));

        let entity = tracklist.entity_playing();
        let now_playing_id = tracklist.currently_playing();

        let max_quality = self.client.max_audio_quality();
        let effective_quality = match (current_track, hires_available) {
            (None, _) => max_quality.clone(),
            (Some(_), true) => max_quality.clone(),
            (Some(_), false) => match max_quality {
                AudioQuality::HIFI96 | AudioQuality::HIFI192 => AudioQuality::CD,
                _ => max_quality.clone(),
            },
        };
        let audio_quality_display = audio_quality_display(effective_quality);

        let playing_info = PlayingInfo {
            title,
            now_playing_id,
            artist_link,
            artist_name,
            entity_title: entity.title,
            entity_link: entity.link,
            status,
            cover_image: entity.cover_link,
            hires_available,
            audio_quality_display,
        };

        let playing_info_json = json!({"playing_info": playing_info});
        let context = merge_serialized(&playing_info_json, context).unwrap();
        let templates = self.templates.borrow();
        let rendered = templates.render(view, &context);

        Html(rendered).into_response()
    }

    pub fn send_toast(&self, message: Notification) -> Response {
        let (message_string, severity) = match &message {
            Notification::Error(message) => (message, 1),
            Notification::Warning(message) => (message, 2),
            Notification::Success(message) => (message, 3),
            Notification::Info(message) => (message, 4),
        };

        self.render(
            "send-toast.html",
            &json!({"message": message_string, "severity": severity}),
        )
    }

    pub fn send_sse(&self, event: String, data: String) {
        let event = ServerSentEvent {
            event_name: event,
            event_data: data,
        };

        _ = self.tx.send(event);
    }

    pub async fn get_library(&self) -> Result<Library> {
        const CACHE_TTL: Duration = Duration::from_secs(30);
        
        {
            let cache = self.library_cache.read().await;
            if let Some(cached) = cache.get(CACHE_TTL) {
                return Ok(cached);
            }
        }

        let library = self.client.library().await?;
        
        {
            let mut cache = self.library_cache.write().await;
            cache.set(library.clone());
        }
        
        Ok(library)
    }

    pub async fn clear_library_cache(&self) {
        let mut cache = self.library_cache.write().await;
        cache.clear();
    }

    pub async fn get_album(&self, id: &str) -> Result<AlbumData> {
        let (album, suggested_albums) =
            try_join!(self.client.album(id), self.client.suggested_albums(id))?;

        Ok(AlbumData {
            album,
            suggested_albums,
        })
    }

    pub async fn is_album_favorite(&self, id: &str) -> Result<bool> {
        let library = self.get_library().await?;
        Ok(library.albums.iter().any(|album| album.id == id))
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PlayingInfo {
    title: String,
    now_playing_id: Option<u32>,
    artist_link: Option<String>,
    artist_name: Option<String>,
    entity_title: Option<String>,
    entity_link: Option<String>,
    status: Status,
    cover_image: Option<String>,
    hires_available: bool,
    audio_quality_display: AudioQualityDisplay,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AudioQualityDisplay {
    icon: String,
    line1: String,
    line2: String,
}

fn audio_quality_display(quality: AudioQuality) -> AudioQualityDisplay {
    match quality {
        AudioQuality::Mp3 => AudioQualityDisplay {
            icon: "/assets/svg/mp3.svg".into(),
            line1: "MP3 320 kbps".into(),
            line2: String::new(),
        },
        AudioQuality::CD => AudioQualityDisplay {
            icon: "/assets/svg/cd.svg".into(),
            line1: "CD 16 bit".into(),
            line2: "44.1kHz".into(),
        },
        AudioQuality::HIFI96 => AudioQualityDisplay {
            icon: "/assets/logo-hires.png".into(),
            line1: "Hi-Res 24-Bit".into(),
            line2: "96kHz".into(),
        },
        AudioQuality::HIFI192 => AudioQualityDisplay {
            icon: "/assets/logo-hires.png".into(),
            line1: "Hi-Res 24-Bit".into(),
            line2: "192kHz".into(),
        },
    }
}

fn merge_serialized<T: serde::Serialize, Y: serde::Serialize>(
    info: &T,
    extra: &Y,
) -> serde_json::Result<serde_json::Value> {
    let mut info_value = serde_json::to_value(info)?;
    let extra_value = serde_json::to_value(extra)?;

    if let (serde_json::Value::Object(info_map), serde_json::Value::Object(extra_map)) =
        (&mut info_value, extra_value)
    {
        info_map.extend(extra_map);
    }

    Ok(info_value)
}
