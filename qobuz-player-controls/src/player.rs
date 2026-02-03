use qobuz_player_models::{Album, Track, TrackStatus};
use rand::seq::SliceRandom;
use tokio::{
    select,
    sync::{
        mpsc,
        watch::{self, Receiver, Sender},
    },
};

use crate::{
    ExitReceiver, PositionReceiver, Result, Status, StatusReceiver, TracklistReceiver,
    VolumeReceiver,
    controls::{ControlCommand, Controls},
    database::Database,
    downloader::Downloader,
    notification::{Notification, NotificationBroadcast},
    sink::{PlaybackStretchConfig, QueryTrackResult, list_audio_devices},
    tracklist::{SingleTracklist, TracklistType},
};
use parking_lot::RwLock;
use std::{path::PathBuf, sync::Arc, time::Duration};

use crate::{
    client::Client,
    sink::Sink,
    tracklist::{self, Tracklist},
};

const INTERVAL_MS: u64 = 500;

pub struct Player {
    broadcast: Arc<NotificationBroadcast>,
    tracklist_tx: Sender<Tracklist>,
    tracklist_rx: Receiver<Tracklist>,
    target_status: Sender<Status>,
    client: Arc<Client>,
    sink: Sink,
    volume: Sender<f32>,
    position: Sender<Duration>,
    track_finished: Receiver<()>,
    done_buffering: Receiver<PathBuf>,
    controls_rx: mpsc::UnboundedReceiver<ControlCommand>,
    controls: Controls,
    database: Arc<Database>,
    next_track_is_queried: bool,
    next_track_in_sink_queue: bool,
    downloader: Downloader,
    playback_stretch: Arc<RwLock<PlaybackStretchConfig>>,
}

impl Player {
    pub fn new(
        tracklist: Tracklist,
        client: Arc<Client>,
        volume: f32,
        broadcast: Arc<NotificationBroadcast>,
        audio_cache_dir: PathBuf,
        database: Arc<Database>,
    ) -> Result<Self> {
        let (volume, volume_receiver) = watch::channel(volume);
        let playback_stretch = Arc::new(RwLock::new(PlaybackStretchConfig::default()));
        let sink = Sink::new(volume_receiver, playback_stretch.clone())?;

        let downloader = Downloader::new(audio_cache_dir, broadcast.clone(), database.clone());

        let track_finished = sink.track_finished();
        let done_buffering = downloader.done_buffering();

        let (position, _) = watch::channel(Default::default());
        let (target_status, _) = watch::channel(Default::default());
        let (tracklist_tx, tracklist_rx) = watch::channel(tracklist);

        let (controls_tx, controls_rx) = tokio::sync::mpsc::unbounded_channel();
        let controls = Controls::new(controls_tx);

        Ok(Self {
            broadcast,
            tracklist_tx,
            tracklist_rx,
            controls_rx,
            controls,
            target_status,
            client,
            sink,
            volume,
            position,
            track_finished,
            done_buffering,
            database,
            next_track_in_sink_queue: false,
            next_track_is_queried: false,
            downloader,
            playback_stretch,
        })
    }

    pub fn controls(&self) -> Controls {
        self.controls.clone()
    }

    pub fn status(&self) -> StatusReceiver {
        self.target_status.subscribe()
    }

    pub fn volume(&self) -> VolumeReceiver {
        self.volume.subscribe()
    }

    pub fn position(&self) -> PositionReceiver {
        self.position.subscribe()
    }

    pub fn tracklist(&self) -> TracklistReceiver {
        self.tracklist_tx.subscribe()
    }

    fn rescale_display_position(pos: Duration, old_ratio: f32, new_ratio: f32) -> Duration {
        let secs = pos.as_secs_f64() * old_ratio as f64 / new_ratio as f64;
        Duration::from_secs_f64(secs.max(0.0))
    }

    pub async fn set_audio_device(&mut self, device_name: Option<String>) -> Result<()> {
        tracing::info!("Player: Setting audio device to: {:?}", device_name);
        
        if let Some(ref name) = device_name {
            let devices = list_audio_devices()?;
            if !devices.iter().any(|d| &d.name == name) {
                tracing::warn!("Player: Device '{}' not found, falling back to default", name);
                self.broadcast.send(Notification::Warning(
                    format!("Audio device '{}' not found. Using default device.", name)
                ));
                self.sink.set_device(None);
                return Ok(());
            }
        }
        
        self.sink.set_device(device_name.clone());
        
        let current_status = *self.target_status.borrow();
        let was_playing = current_status == Status::Playing || current_status == Status::Buffering;
        
        if was_playing {
            tracing::info!("Player: Device changed during playback, recreating stream");
            let tracklist = self.tracklist_rx.borrow();
            if let Some(current_track) = tracklist.current_track() {
                let current_position = self.sink.position();
                let position_ms = current_position.as_millis() as u64;
                
                let track_url = self.client.track_url(current_track.id).await?;
                if let Some(track_path) = self
                    .downloader
                    .ensure_track_is_downloaded(track_url, current_track)
                    .await
                {

                    if let Err(e) = self.sink.pause() {
                        tracing::warn!("Failed to pause sink during device change: {}", e);
                    }
                    
                    if let Err(e) = self.sink.clear() {
                        let error_msg = e.to_string();
                        if error_msg.contains("device") || error_msg.contains("no longer available") {
                            tracing::warn!("Device already removed during clear: {}", error_msg);
                        } else {
                            return Err(e);
                        }
                    }
                    
                    match self.sink.query_track(&track_path, None) {
                        Ok(_) => {
                            if let Err(e) = self.sink.play() {
                                tracing::warn!("Failed to play sink after device change: {}", e);
                            }
                            self.set_target_status(Status::Playing);
                            
                            if position_ms > 1000 {
                                tokio::time::sleep(Duration::from_millis(200)).await;
                                let restore_position = Duration::from_millis(position_ms);
                                if let Err(e) = self.sink.seek(restore_position) {
                                    tracing::warn!("Failed to restore position after device change: {}", e);
                                } else {
                                    tracing::info!("Restored playback position to {}ms after device change", position_ms);
                                    self.position.send(restore_position)?;
                                }
                            }
                            
                            tracing::info!("Player: Successfully switched to new audio device during playback");
                        }
                        Err(e) => {
                            let error_msg = e.to_string();
                            tracing::error!("Failed to recreate stream with new device: {}", error_msg);
                            self.set_target_status(Status::Paused);
                            self.broadcast.send(Notification::Warning(
                                "Failed to switch audio device. Please resume playback.".to_string()
                            ));
                        }
                    }
                } else {
                    if let Err(e) = self.sink.pause() {
                        tracing::warn!("Failed to pause sink during device change: {}", e);
                    }
                    if let Err(e) = self.sink.clear() {
                        let error_msg = e.to_string();
                        if error_msg.contains("device") || error_msg.contains("no longer available") {
                            tracing::warn!("Device already removed during clear: {}", error_msg);
                        } else {
                            return Err(e);
                        }
                    }
                    self.set_target_status(Status::Paused);
                    self.broadcast.send(Notification::Info(
                        "Audio device changed. Please resume playback.".to_string()
                    ));
                }
            }
        } else {
            tracing::info!("Player: Device changed while not playing");
            let tracklist = self.tracklist_rx.borrow();
            if let Some(_current_track) = tracklist.current_track() {
                let _current_position = self.sink.position();
                let _position_ms = _current_position.as_millis() as u64;
            }
            
            if let Err(e) = self.sink.clear() {
                let error_msg = e.to_string();
                if error_msg.contains("device") || error_msg.contains("no longer available") {
                    tracing::warn!("Device already removed during clear: {}", error_msg);
                } else {
                    tracing::warn!("Failed to clear stream during device change: {}", error_msg);
                }
            }
        }
        
        Ok(())
    }

    async fn play_pause(&mut self) -> Result<()> {
        let target_status = *self.target_status.borrow();

        match target_status {
            Status::Playing | Status::Buffering => self.pause(),
            Status::Paused => self.play().await?,
        }

        Ok(())
    }

    async fn play(&mut self) -> Result<()> {
        tracing::info!("Play");

        let track = self.tracklist_rx.borrow().current_track().cloned();

        if self.sink.is_empty()
            && let Some(current_track) = track
        {
            tracing::info!("Sink is empty. Query track from play");
            self.set_target_status(Status::Buffering);
            self.query_track(&current_track, false).await?;
        } else {
            self.set_target_status(Status::Playing);
            self.sink.play()?;
        }

        Ok(())
    }

    fn pause(&mut self) {
        self.set_target_status(Status::Paused);
        if let Err(e) = self.sink.pause() {
            tracing::warn!("Failed to pause sink: {}", e);
        }
    }

    fn set_target_status(&self, status: Status) {
        self.target_status.send(status).expect("infallible");
    }

    async fn query_track(&mut self, track: &Track, next_track: bool) -> Result<()> {
        tracing::info!(
            "Querying {} track: {}",
            if next_track { "next" } else { "current" },
            &track.title
        );

        if next_track {
            self.next_track_is_queried = true;
        }

        let track_url = self.client.track_url(track.id).await?;
        if let Some(track_path) = self
            .downloader
            .ensure_track_is_downloaded(track_url, track)
            .await
        {
            match self.sink.query_track(&track_path, None) {
                Ok(query_result) => {
                    if next_track {
                        self.next_track_in_sink_queue = match query_result {
                            QueryTrackResult::Queued => {
                                tracing::info!("In queue");
                                true
                            }
                            QueryTrackResult::RecreateStreamRequired => {
                                tracing::info!("Not in queue");
                                false
                            }
                        };
                    }
                    if let Err(e) = self.sink.play() {
                        tracing::warn!("Failed to play sink: {}", e);
                    }
                    self.set_target_status(Status::Playing);
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    tracing::error!("Failed to query track: {}", error_msg);
                    
                    if error_msg.contains("device") || error_msg.contains("no longer available") {
                        tracing::warn!("Audio device error detected during play, pausing and resetting to default");
                        self.set_target_status(Status::Paused);
                        
                        self.sink.set_device(None);
                        
                        if let Err(db_err) = self.database.set_audio_device(None).await {
                            tracing::error!("Failed to update database after device error: {}", db_err);
                        }
                        
                        self.broadcast.send(Notification::Warning(
                            "Audio device error. Resetting to default device.".to_string()
                        ));
                        return Ok(());
                    }
                    return Err(e);
                }
            }
        } else {
            tracing::info!("Buffering track: {}", &track.title);
            self.set_target_status(Status::Buffering);
        }

        Ok(())
    }

    async fn set_volume(&self, volume: f32) -> Result<()> {
        self.volume.send(volume)?;
        self.sink.sync_volume();
        self.database.set_volume(volume).await?;
        Ok(())
    }

    async fn broadcast_tracklist(&self, tracklist: Tracklist) -> Result<()> {
        self.database.set_tracklist(&tracklist).await?;
        self.tracklist_tx.send(tracklist)?;
        Ok(())
    }

    fn seek(&mut self, duration: Duration) -> Result<()> {
        self.sink.seek(duration)?;
        self.position.send(self.sink.position())?;
        Ok(())
    }

    fn current_display_duration(&self) -> Option<Duration> {
        let ratio = self.playback_stretch.read().time_stretch_ratio.max(0.01) as f64;
        self.tracklist_rx
            .borrow()
            .current_track()
            .map(|x| Duration::from_secs_f64(x.duration_seconds as f64 / ratio))
    }

    fn jump_forward(&mut self) -> Result<()> {
        let duration = self.current_display_duration();

        if let Some(duration) = duration {
            let ten_seconds = Duration::from_secs(10);
            let next_position = self.sink.position() + ten_seconds;

            if next_position < duration {
                self.seek(next_position)?;
            } else {
                self.seek(duration)?;
            }
        }

        Ok(())
    }

    fn jump_backward(&mut self) -> Result<()> {
        let current_position = self.sink.position();

        if current_position.as_millis() < 10000 {
            self.seek(Duration::default())?;
        } else {
            let ten_seconds = Duration::from_secs(10);
            let seek_position = current_position - ten_seconds;

            self.seek(seek_position)?;
        }
        Ok(())
    }

    async fn skip_to_position(&mut self, new_position: i32, force: bool) -> Result<()> {
        let mut tracklist = self.tracklist_rx.borrow().clone();
        let current_position = tracklist.current_position();

        // Typical previous skip functionality where if,
        // the track is greater than 1 second into playing,
        // then it goes to the beginning. If triggered again
        // within a second after playing, it will skip to the previous track.
        if !force
            && new_position < current_position as i32
            && self.position.borrow().as_millis() > 1000
        {
            self.seek(Duration::default())?;
            return Ok(());
        }

        self.position.send(Default::default())?;

        if tracklist.skip_to_track(new_position).is_some() {
            self.new_queue(tracklist).await?;
        } else {
            tracklist.reset();
            self.sink.clear()?;
            self.next_track_is_queried = false;
            self.set_target_status(Status::Paused);
            self.position.send(Default::default())?;
            self.broadcast_tracklist(tracklist).await?;
        }

        Ok(())
    }

    async fn next(&mut self) -> Result<()> {
        let current_position = self.tracklist_rx.borrow().current_position();
        self.skip_to_position((current_position + 1) as i32, true)
            .await
    }

    async fn previous(&mut self) -> Result<()> {
        let current_position = self.tracklist_rx.borrow().current_position();
        self.skip_to_position(current_position as i32 - 1, false)
            .await
    }

    async fn new_queue(&mut self, tracklist: Tracklist) -> Result<()> {
        self.sink.clear()?;
        self.next_track_is_queried = false;
        self.next_track_in_sink_queue = false;

        if let Some(first_track) = tracklist.current_track() {
            tracing::info!("New queue starting with: {}", first_track.title);
            self.query_track(first_track, false).await?;
        }

        self.broadcast_tracklist(tracklist).await?;

        Ok(())
    }

    async fn update_queue(&mut self, tracklist: Tracklist) -> Result<()> {
        self.next_track_is_queried = false;
        self.sink.clear_queue()?;
        self.broadcast_tracklist(tracklist).await?;
        Ok(())
    }

    async fn play_track(&mut self, track_id: u32) -> Result<()> {
        let mut track: Track = self.client.track(track_id).await?;
        track.status = TrackStatus::Playing;

        let tracklist = Tracklist {
            list_type: TracklistType::Track(SingleTracklist {
                track_title: track.title.clone(),
                album_id: track.album_id.clone(),
                image: track.image.clone(),
            }),
            queue: vec![track],
        };

        self.new_queue(tracklist).await
    }

    async fn play_album(&mut self, album_id: &str, index: usize) -> Result<()> {
        let album: Album = self.client.album(album_id).await?;

        let unstreamable_tracks_to_index = album
            .tracks
            .iter()
            .take(index)
            .filter(|t| !t.available)
            .count() as i32;

        let mut tracklist = Tracklist {
            queue: album.tracks.into_iter().filter(|t| t.available).collect(),
            list_type: TracklistType::Album(tracklist::AlbumTracklist {
                title: album.title,
                id: album.id,
                image: Some(album.image),
            }),
        };

        tracklist.skip_to_track(index as i32 - unstreamable_tracks_to_index);
        self.new_queue(tracklist).await
    }

    async fn play_top_tracks(&mut self, artist_id: u32, index: usize) -> Result<()> {
        let artist = self.client.artist_page(artist_id).await?;
        let tracks = artist.top_tracks;
        let unstreamable_tracks_to_index =
            tracks.iter().take(index).filter(|t| !t.available).count() as i32;

        let mut tracklist = Tracklist {
            queue: tracks.into_iter().filter(|t| t.available).collect(),
            list_type: TracklistType::TopTracks(tracklist::TopTracklist {
                artist_name: artist.name,
                id: artist_id,
                image: artist.image,
            }),
        };

        tracklist.skip_to_track(index as i32 - unstreamable_tracks_to_index);
        self.new_queue(tracklist).await
    }

    async fn play_playlist(&mut self, playlist_id: u32, index: usize, shuffle: bool) -> Result<()> {
        let playlist = self.client.playlist(playlist_id).await?;

        let unstreamable_tracks_to_index = playlist
            .tracks
            .iter()
            .take(index)
            .filter(|t| !t.available)
            .count() as i32;

        let mut tracks: Vec<Track> = playlist
            .tracks
            .into_iter()
            .filter(|t| t.available)
            .collect();

        if shuffle {
            tracks.shuffle(&mut rand::rng());
        }

        let mut tracklist = Tracklist {
            queue: tracks,
            list_type: TracklistType::Playlist(tracklist::PlaylistTracklist {
                title: playlist.title,
                id: playlist.id,
                image: playlist.image,
            }),
        };

        tracklist.skip_to_track(index as i32 - unstreamable_tracks_to_index);
        self.new_queue(tracklist).await
    }

    async fn remove_index_from_queue(&mut self, index: usize) -> Result<()> {
        let mut tracklist = self.tracklist_rx.borrow().clone();

        tracklist.queue.remove(index);
        self.update_queue(tracklist).await?;
        let notification = Notification::Info("Queue updated".into());
        self.broadcast.send(notification);
        Ok(())
    }

    async fn add_track_to_queue(&mut self, id: u32) -> Result<()> {
        let mut tracklist = self.tracklist_rx.borrow().clone();
        let track = self.client.track(id).await?;

        let notification = Notification::Info(format!("{} added to queue", track.title.clone()));

        tracklist.queue.push(track);
        self.update_queue(tracklist).await?;
        self.broadcast.send(notification);
        Ok(())
    }

    async fn play_track_next(&mut self, id: u32) -> Result<()> {
        let mut tracklist = self.tracklist_rx.borrow().clone();
        let track = self.client.track(id).await?;

        let notification = Notification::Info(format!("{} playing next", track.title.clone()));

        let current_index = tracklist.current_position();
        tracklist.queue.insert(current_index + 1, track);
        self.update_queue(tracklist).await?;
        self.broadcast.send(notification);
        Ok(())
    }

    async fn reorder_queue(&mut self, new_order: Vec<usize>) -> Result<()> {
        if new_order.iter().enumerate().all(|(i, &v)| i == v) {
            return Ok(());
        }

        let mut tracklist = self.tracklist_rx.borrow().clone();

        let reordered: Vec<_> = new_order
            .iter()
            .map(|&i| tracklist.queue[i].clone())
            .collect();

        tracklist.queue = reordered;

        self.update_queue(tracklist).await?;
        let notification = Notification::Info("Queue updated".into());
        self.broadcast.send(notification);
        Ok(())
    }

    async fn tick(&mut self) -> Result<()> {
        if *self.target_status.borrow() != Status::Playing {
            return Ok(());
        }

        let position = self.sink.position();
        self.position.send(position)?;

        let duration = self
            .tracklist_rx
            .borrow()
            .current_track()
            .map(|x| {
                x.duration_seconds as f64
                    / self.playback_stretch.read().time_stretch_ratio.max(0.01) as f64
            });

        if let Some(duration) = duration {
            let position = position.as_secs();

            let track_about_to_finish = (duration as i64 - position as i64) < 60;

            if track_about_to_finish && !self.next_track_is_queried {
                tracing::info!("Track about to finish");

                let tracklist = self.tracklist_rx.borrow().clone();

                if let Some(next_track) = tracklist.next_track() {
                    tracing::info!("Query next track: {} from tick", &next_track.title);
                    self.query_track(next_track, true).await?;
                }
            }
        }

        Ok(())
    }

    async fn handle_message(&mut self, notification: ControlCommand) -> Result<()> {
        match notification {
            ControlCommand::Album { id, index } => {
                self.play_album(&id, index).await?;
            }
            ControlCommand::Playlist { id, index, shuffle } => {
                self.play_playlist(id, index, shuffle).await?;
            }
            ControlCommand::ArtistTopTracks { artist_id, index } => {
                self.play_top_tracks(artist_id, index).await?;
            }
            ControlCommand::Track { id } => {
                self.play_track(id).await?;
            }
            ControlCommand::Next => {
                self.next().await?;
            }
            ControlCommand::Previous => {
                self.previous().await?;
            }
            ControlCommand::PlayPause => {
                self.play_pause().await?;
            }
            ControlCommand::Play => {
                self.play().await?;
            }
            ControlCommand::Pause => {
                self.pause();
            }
            ControlCommand::SkipToPosition {
                new_position,
                force,
            } => {
                self.skip_to_position(new_position as i32, force).await?;
            }
            ControlCommand::JumpForward => {
                self.jump_forward()?;
            }
            ControlCommand::JumpBackward => {
                self.jump_backward()?;
            }
            ControlCommand::Seek { time } => {
                self.seek(time)?;
            }
            ControlCommand::SetVolume { volume } => {
                self.set_volume(volume).await?;
            }
            ControlCommand::SetAudioDevice { device_name } => {
                tracing::info!("Player: Received SetAudioDevice command: {:?}", device_name);
                let device_display = device_name.as_deref().unwrap_or("Default").to_string();

                if let Err(e) = self.database.set_audio_device(device_name.clone()).await {
                    tracing::error!("Failed to save audio device to database: {}", e);
                    self.broadcast.send(Notification::Error(
                        format!("Failed to save audio device: {}", e)
                    ));
                } else if let Err(e) = self.set_audio_device(device_name).await {
                    tracing::error!("Failed to set audio device: {}", e);
                    self.broadcast.send(Notification::Error(
                        format!("Failed to set audio device: {}", e)
                    ));
                } else {
                    self.broadcast.send(Notification::Success(
                        format!("Output changed to '{}'.", device_display)
                    ));
                }
            }
            ControlCommand::SetTimeStretch { ratio } => {
                let ratio = ratio.clamp(0.5, 2.0);
                let old_ratio = self.playback_stretch.read().time_stretch_ratio;
                if let Err(e) = self.database.set_time_stretch_ratio(ratio).await {
                    tracing::error!("Failed to save time stretch: {}", e);
                } else {
                    let current_pos = self.sink.position();
                    self.playback_stretch.write().time_stretch_ratio = ratio;
                    self.broadcast.send(Notification::Info(
                        format!("Time stretch set to {:.1}x.", ratio)
                    ));
                    let live = self.sink.supports_live_stretch();
                    if live && current_pos > Duration::ZERO {
                        let desired_pos = Self::rescale_display_position(current_pos, old_ratio, ratio);
                        let delta_ms =
                            desired_pos.as_millis() as i64 - current_pos.as_millis() as i64;
                        self.sink.adjust_position_offset_ms(delta_ms);
                        self.position.send(desired_pos)?;
                    } else if !live {
                        let _ = self.reload_current_track_with_stretch(Some(old_ratio)).await;
                    }
                }
            }
            ControlCommand::SetPitch { semitones } => {
                let semitones = semitones.clamp(-12, 12);
                if let Err(e) = self.database.set_pitch_semitones(semitones).await {
                    tracing::error!("Failed to save pitch: {}", e);
                } else {
                    self.playback_stretch.write().pitch_semitones = semitones;
                    self.broadcast.send(Notification::Info(
                        format!("Pitch set to {} semitones.", semitones)
                    ));
                    if !self.sink.supports_live_stretch() {
                        let _ = self.reload_current_track_with_stretch(None).await;
                    }
                }
            }
            ControlCommand::SetPitchCents { cents } => {
                let cents = cents.clamp(-100, 100);
                if let Err(e) = self.database.set_pitch_cents(cents).await {
                    tracing::error!("Failed to save pitch cents: {}", e);
                } else {
                    self.playback_stretch.write().pitch_cents = cents;
                    self.broadcast.send(Notification::Info(
                        format!("Pitch (cents) set to {}.", cents)
                    ));
                    if !self.sink.supports_live_stretch() {
                        let _ = self.reload_current_track_with_stretch(None).await;
                    }
                }
            }
            ControlCommand::AddTrackToQueue { id } => self.add_track_to_queue(id).await?,
            ControlCommand::RemoveIndexFromQueue { index } => {
                self.remove_index_from_queue(index).await?
            }
            ControlCommand::PlayTrackNext { id } => self.play_track_next(id).await?,
            ControlCommand::ReorderQueue { new_order } => self.reorder_queue(new_order).await?,
        }
        Ok(())
    }

    async fn track_finished(&mut self) -> Result<()> {
        let mut tracklist = self.tracklist_rx.borrow().clone();

        let current_position = tracklist.current_position();
        let new_position = current_position + 1;

        let next_track = tracklist.skip_to_track(new_position as i32);

        match next_track {
            Some(next_track) => {
                if !self.next_track_in_sink_queue {
                    tracing::info!(
                        "Track finished and next track is not in queue. Resetting queue"
                    );
                    if let Err(e) = self.sink.clear() {
                        let error_msg = e.to_string();
                        if error_msg.contains("device") || error_msg.contains("no longer available") {
                            tracing::warn!("Device error during clear: {}", error_msg);
                        } else {
                            return Err(e);
                        }
                    }
                    if let Err(e) = self.query_track(next_track, false).await {
                        let error_msg = e.to_string();
                        if error_msg.contains("device") || error_msg.contains("no longer available") {
                            tracing::warn!("Device error during query_track: {} - already handled", error_msg);
                            return Ok(());
                        }
                        return Err(e);
                    }
                }
            }
            None => {
                tracklist.reset();
                self.set_target_status(Status::Paused);
                if let Err(e) = self.sink.pause() {
                    tracing::warn!("Failed to pause sink: {}", e);
                }
                self.sink.clear()?;
                self.position.send(Default::default())?;
            }
        }
        self.next_track_is_queried = false;
        self.broadcast_tracklist(tracklist).await?;
        Ok(())
    }

    async fn reload_current_track_with_stretch(
        &mut self,
        old_time_stretch_ratio: Option<f32>,
    ) -> Result<()> {
        if *self.target_status.borrow() != Status::Playing {
            return Ok(());
        }
        let track = match self.tracklist_rx.borrow().current_track() {
            Some(t) => t.clone(),
            None => return Ok(()),
        };
        let track_url = self.client.track_url(track.id).await.ok();
        let track_url = match track_url {
            Some(u) => u,
            None => return Ok(()),
        };
        let path = self
            .downloader
            .ensure_track_is_downloaded(track_url, &track)
            .await;
        let path = match path {
            Some(p) => p,
            None => return Ok(()),
        };
        let current_pos = self.sink.position();
        self.sink.clear()?;
        let new_ratio = self.playback_stretch.read().time_stretch_ratio;
        let start_at = if current_pos > Duration::ZERO {
            if let Some(old_ratio) = old_time_stretch_ratio {
                Some(Self::rescale_display_position(current_pos, old_ratio, new_ratio))
            } else {
                Some(current_pos)
            }
        } else {
            None
        };
        match self.sink.query_track(&path, start_at) {
            Ok(_) => {
                let _ = self.sink.play();
                if let Some(pos) = start_at {
                    self.position.send(pos)?;
                } else {
                    self.position.send(self.sink.position())?;
                }
                Ok(())
            }
            Err(e) => {
                tracing::error!("Failed to reload track with new stretch: {}", e);
                Err(e)
            }
        }
    }

    fn done_buffering(&mut self, path: PathBuf) -> Result<()> {
        if *self.target_status.borrow() != Status::Playing {
            self.set_target_status(Status::Playing);
        }

        tracing::info!("Done buffering track: {}", path.to_string_lossy());

        match self.sink.query_track(&path, None) {
            Ok(result) => {
                self.next_track_in_sink_queue = match result {
                    QueryTrackResult::Queued => true,
                    QueryTrackResult::RecreateStreamRequired => false,
                };
            }
            Err(e) => {
                let error_msg = e.to_string();
                tracing::error!("Failed to query track: {}", error_msg);
                
                if error_msg.contains("device") || error_msg.contains("no longer available") {
                    tracing::warn!("Audio device error detected, pausing playback and resetting to default");
                    if let Err(e) = self.sink.pause() {
                        tracing::warn!("Failed to pause sink: {}", e);
                    }
                    self.set_target_status(Status::Paused);
                    
                    self.sink.set_device(None);
                    
                    let database = self.database.clone();
                    tokio::spawn(async move {
                        if let Err(db_err) = database.set_audio_device(None).await {
                            tracing::error!("Failed to update database after device error: {}", db_err);
                        }
                    });
                    
                    self.broadcast.send(Notification::Warning(
                        "Audio device error. Resetting to default device.".to_string()
                    ));
                    return Ok(());
                }
                return Err(e);
            }
        }
        Ok(())
    }

    pub async fn player_loop(&mut self, mut exit_receiver: ExitReceiver) -> Result<()> {
        if let Ok(config) = self.database.get_configuration().await {
            if let Some(device_name) = config.audio_device_name {
                tracing::info!("Setting initial audio device from database: {}", device_name);
                self.sink.set_device(Some(device_name));
            }
            *self.playback_stretch.write() = PlaybackStretchConfig {
                time_stretch_ratio: config.time_stretch_ratio,
                pitch_semitones: config.pitch_semitones,
                pitch_cents: config.pitch_cents,
            };
        }

        let mut interval = tokio::time::interval(Duration::from_millis(INTERVAL_MS));

        loop {
            select! {
                _ = interval.tick() => {
                    if let Err(err) = self.tick().await {
                        self.broadcast.send_error(err.to_string());
                    };
                }

                Some(notification) = self.controls_rx.recv() => {
                    if let Err(err) = self.handle_message(notification).await {
                        self.broadcast.send_error(err.to_string());
                    };
                }

                Ok(_) = self.track_finished.changed() => {
                    if let Err(err) = self.track_finished().await {
                        let error_msg = err.to_string();
                        if !error_msg.contains("device") && !error_msg.contains("no longer available") {
                            self.broadcast.send_error(error_msg);
                        }
                    };
                }

                Ok(_) = self.done_buffering.changed() => {
                    let path = self.done_buffering.borrow_and_update().clone();
                    if let Err(err) = self.done_buffering(path) {
                        let error_msg = err.to_string();
                        if error_msg.contains("device") || error_msg.contains("no longer available") {
                            tracing::warn!("Device error in done_buffering: {} - already handled", error_msg);
                        } else {
                            self.broadcast.send_error(error_msg);
                        }
                    };
                }
                Ok(exit) = exit_receiver.recv() => {
                    if exit {
                        break Ok(());
                    }
                }
            }
        }
    }
}
