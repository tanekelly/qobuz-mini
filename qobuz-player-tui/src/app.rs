use crate::{
    discover::DiscoverState,
    library::LibraryState,
    now_playing::NowPlayingState,
    popup::{Popup, TrackPopupState},
    queue::QueueState,
    search::SearchState,
    settings::SettingsState,
};
use core::fmt;
use image::load_from_memory;
use qobuz_player_controls::{
    PositionReceiver, Result, Status, StatusReceiver, TracklistReceiver,
    client::Client,
    controls::Controls,
    database::Database,
    ExitSender,
    notification::{Notification, NotificationBroadcast},
    tracklist::Tracklist,
};
use qobuz_player_models::Track;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use futures::StreamExt;
use ratatui::{DefaultTerminal, widgets::*};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol};
use std::{io, sync::Arc, time::Instant};
use tokio::time::{self, Duration};

#[derive(Default)]
pub struct NotificationList {
    notifications: Vec<(Notification, Instant)>,
}

impl NotificationList {
    pub fn push(&mut self, notification: Notification) {
        self.notifications.push((notification, Instant::now()));
    }

    pub fn tick(&mut self) -> bool {
        let notifications_before_clean = self.notifications.len();
        self.notifications
            .retain(|notification| notification.1.elapsed() < Duration::from_secs(5));
        let notifications_after_clean = self.notifications.len();

        notifications_before_clean != notifications_after_clean
    }

    pub fn notifications(&self) -> Vec<&Notification> {
        self.notifications.iter().map(|x| &x.0).collect()
    }
}

pub struct App {
    pub client: Arc<Client>,
    pub controls: Controls,
    pub position: PositionReceiver,
    pub tracklist: TracklistReceiver,
    pub status: StatusReceiver,
    pub current_screen: Tab,
    pub exit: bool,
    pub should_draw: bool,
    pub app_state: AppState,
    pub now_playing: NowPlayingState,
    pub library: LibraryState,
    pub search: SearchState,
    pub queue: QueueState,
    pub discover: DiscoverState,
    pub settings: SettingsState,
    pub database: Arc<Database>,
    pub exit_sender: ExitSender,
    pub broadcast: Arc<NotificationBroadcast>,
    pub notifications: NotificationList,
    pub full_screen: bool,
    pub disable_tui_album_cover: bool,
    pub playback_config: (f32, i16, i16),
}

#[derive(Default)]
pub enum AppState {
    #[default]
    Normal,
    Popup(Vec<Popup>),
    Help,
}

pub enum Output {
    Consumed,
    NotConsumed,
    UpdateLibrary,
    Popup(Popup),
    PopPoputUpdateLibrary,
    AddTrackToPlaylist(Track),
    AddTrackToPlaylistAndPopPopup((u32, u32)),
}

#[derive(Default, PartialEq)]
pub enum Tab {
    #[default]
    Library,
    Search,
    Queue,
    Discover,
    Settings,
}

impl fmt::Display for Tab {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Tab::Library => write!(f, "Library"),
            Tab::Search => write!(f, "Search"),
            Tab::Queue => write!(f, "Queue"),
            Tab::Discover => write!(f, "Discover"),
            Tab::Settings => write!(f, "Settings"),
        }
    }
}

impl Tab {
    pub const VALUES: [Self; 5] = [
        Tab::Library,
        Tab::Search,
        Tab::Queue,
        Tab::Discover,
        Tab::Settings,
    ];
}

#[derive(Default)]
pub struct FilteredListState<T> {
    filter: Vec<T>,
    all_items: Vec<T>,
    pub state: TableState,
}

impl<T> FilteredListState<T>
where
    T: Clone,
{
    pub fn new(list: Vec<T>) -> Self {
        Self {
            filter: list.clone(),
            all_items: list,
            state: Default::default(),
        }
    }

    pub fn filter(&self) -> &Vec<T> {
        &self.filter
    }

    pub fn all_items(&self) -> &Vec<T> {
        &self.all_items
    }

    pub fn set_all_items(&mut self, items: Vec<T>) {
        self.all_items = items.clone();
        self.filter = items;
    }

    pub fn set_filter(&mut self, items: Vec<T>) {
        self.filter = items;
    }
}

impl App {
    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut tick_interval = time::interval(Duration::from_millis(100));
        let mut receiver = self.broadcast.subscribe();
        let mut event_stream = EventStream::new();

        while !self.exit {
            tokio::select! {
                // Prioritize keyboard events by checking them first with biased
                biased;

                Some(event_result) = event_stream.next() => {
                    if let Ok(event) = event_result {
                        self.handle_event(event).await.expect("infallible");
                    }
                }

                Ok(_) = self.position.changed() => {
                    self.now_playing.duration_ms = self.position.borrow_and_update().as_millis() as u32;
                    self.should_draw = true;
                },

                Ok(_) = self.tracklist.changed() => {
                    let tracklist = self.tracklist.borrow_and_update().clone();
                    self.queue.set_items(tracklist.queue().to_vec());
                    let status = self.now_playing.status;
                    self.now_playing = get_current_state(tracklist, status).await;
                    self.should_draw = true;
                },

                Ok(_) = self.status.changed() => {
                    let status = self.status.borrow_and_update();
                    self.now_playing.status = *status;
                    self.should_draw = true;
                }

                _ = tick_interval.tick() => {
                    // Tick is now only used for notification cleanup
                }

                notification = receiver.recv() => {
                    if let Ok(notification) = notification {
                        self.notifications.push(notification.clone());
                        self.should_draw = true;
                        
                        if let Notification::Warning(msg) = &notification {
                            if msg.contains("Audio device") && (msg.contains("was removed") || msg.contains("error") || msg.contains("Resetting")) {
                                self.settings.refresh_from_database(&self.database).await;
                                terminal.clear()?;
                                terminal.draw(|frame| self.render(frame))?;
                                self.should_draw = false;
                            }
                        }
                        
                        if let Notification::Info(msg) = &notification {
                            if msg.contains("Audio device list updated")
                                || msg.contains("Default audio device")
                                || msg.contains("Audio device changed to")
                                || msg.contains("was removed")
                                || msg.starts_with("Time stretch set to")
                                || msg.starts_with("Pitch set to")
                                || msg.starts_with("Pitch (cents) set to")
                            {
                                self.settings.refresh_from_database(&self.database).await;
                                if let Ok(cfg) = self.database.get_configuration().await {
                                    self.playback_config = (
                                        cfg.time_stretch_ratio,
                                        cfg.pitch_semitones,
                                        cfg.pitch_cents,
                                    );
                                }
                                terminal.clear()?;
                                terminal.draw(|frame| self.render(frame))?;
                                self.should_draw = false;
                            }
                        }
                    }
                }
            }

            if self.notifications.tick() {
                self.should_draw = true;
            };

            if self.should_draw {
                terminal.draw(|frame| self.render(frame))?;
                self.should_draw = false;
            }
        }

        Ok(())
    }

    async fn update_library(&mut self) {
        let library = self.client.library().await;
        let Ok(library) = library else {
            return;
        };

        self.library.albums.set_all_items(library.albums);
        self.library.artists.set_all_items(library.artists);
        self.library.playlists.set_all_items(library.playlists);
        self.library.tracks.set_all_items(library.tracks);
        self.library.filter.reset();
    }

    async fn handle_output(&mut self, key_code: KeyCode, output: Result<Output>) {
        let output = match output {
            Ok(res) => res,
            Err(err) => {
                self.notifications
                    .push(Notification::Error(err.to_string()));
                return;
            }
        };

        match output {
            Output::Consumed => {
                self.should_draw = true;
            }
            Output::UpdateLibrary => {
                self.update_library().await;
                self.should_draw = true;
            }
            Output::NotConsumed => match key_code {
                KeyCode::Char('?') => {
                    self.app_state = AppState::Help;
                    self.should_draw = true;
                }
                KeyCode::Char('q') => {
                    self.should_draw = true;
                    self.exit()
                }
                KeyCode::Char('1') => {
                    self.navigate_to_library();
                    self.should_draw = true;
                }
                KeyCode::Char('2') => {
                    self.navigate_to_search();
                    self.should_draw = true;
                }
                KeyCode::Char('3') => {
                    self.navigate_to_queue();
                    self.should_draw = true;
                }
                KeyCode::Char('4') => {
                    self.navigate_to_discover();
                    self.should_draw = true;
                }
                KeyCode::Char('5') => {
                    self.navigate_to_settings().await;
                    self.should_draw = true;
                }
                KeyCode::Char(' ') => {
                    self.controls.play_pause();
                    self.should_draw = true;
                }
                KeyCode::Char('n') => {
                    self.controls.next();
                    self.should_draw = true;
                }
                KeyCode::Char('p') => {
                    self.controls.previous();
                    self.should_draw = true;
                }
                KeyCode::Char('f') => {
                    self.controls.jump_forward();
                    self.should_draw = true;
                }
                KeyCode::Char('b') => {
                    self.controls.jump_backward();
                    self.should_draw = true;
                }
                KeyCode::Char('F') => {
                    self.full_screen = !self.full_screen;
                    self.should_draw = true;
                }
                _ => {}
            },
            Output::Popup(popup) => {
                let mut popups = match std::mem::take(&mut self.app_state) {
                    AppState::Popup(popups) => popups,
                    _ => Vec::new(),
                };

                popups.push(popup);

                self.app_state = AppState::Popup(popups);
                self.should_draw = true;
            }
            Output::PopPoputUpdateLibrary => {
                if let AppState::Popup(popups) = &mut self.app_state {
                    popups.pop();
                    if popups.is_empty() {
                        self.app_state = AppState::Normal;
                    }
                    self.update_library().await;
                    self.should_draw = true;
                }
            }
            Output::AddTrackToPlaylist(track) => {
                let playlists_res = self.client.library().await.map(|lib| {
                    lib.playlists
                        .into_iter()
                        .filter(|p| p.is_owned)
                        .collect::<Vec<_>>()
                });

                if let Ok(playlists) = playlists_res {
                    let mut popups = match std::mem::take(&mut self.app_state) {
                        AppState::Popup(v) => v,
                        other => {
                            self.app_state = other;
                            Vec::new()
                        }
                    };

                    popups.push(Popup::Track(TrackPopupState::new(track, playlists)));

                    self.app_state = AppState::Popup(popups);
                    self.should_draw = true;
                }
            }
            Output::AddTrackToPlaylistAndPopPopup((track_id, playlist_id)) => {
                match self
                    .client
                    .playlist_add_track(playlist_id, &[track_id])
                    .await
                {
                    Ok(_) => {
                        if let AppState::Popup(popups) = &mut self.app_state {
                            popups.pop();
                            if popups.is_empty() {
                                self.app_state = AppState::Normal;
                            }
                            self.update_library().await;
                        }
                        self.notifications
                            .push(Notification::Info("Added to playlist".into())); // Add track and playlist name
                    }
                    Err(err) => {
                        self.notifications
                            .push(Notification::Error(err.to_string()));
                    }
                };
                self.should_draw = true;
            }
        }
    }

    async fn handle_event(&mut self, event: Event) -> io::Result<()> {
        match event {
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                match &mut self.app_state {
                    AppState::Help => {
                        self.app_state = AppState::Normal;
                        self.should_draw = true;
                        return Ok(());
                    }
                    AppState::Popup(popups) => {
                        if key_event.code == KeyCode::Esc {
                            _ = popups.pop();
                            if popups.is_empty() {
                                self.app_state = AppState::Normal;
                            }
                            self.should_draw = true;
                            return Ok(());
                        }

                        let outcome_opt = {
                            if let AppState::Popup(popups) = &mut self.app_state {
                                if let Some(popup) = popups.last_mut() {
                                    popup
                                        .handle_event(
                                            event,
                                            &self.client,
                                            &self.controls,
                                            &mut self.notifications,
                                        )
                                        .await
                                } else {
                                    Ok(Output::NotConsumed)
                                }
                            } else {
                                Ok(Output::NotConsumed)
                            }
                        };

                        self.handle_output(key_event.code, outcome_opt).await;

                        self.should_draw = true;
                        return Ok(());
                    }
                    _ => {}
                };

                let screen_output = match self.current_screen {
                    Tab::Library => {
                        self.library
                            .handle_events(
                                event,
                                &self.client,
                                &self.controls,
                                &mut self.notifications,
                            )
                            .await
                    }
                    Tab::Search => {
                        self.search
                            .handle_events(
                                event,
                                &self.client,
                                &self.controls,
                                &mut self.notifications,
                            )
                            .await
                    }
                    Tab::Queue => Ok(self.queue.handle_events(event, &self.controls).await),
                    Tab::Discover => {
                        self.discover
                            .handle_events(event, &self.client, &mut self.notifications)
                            .await
                    }
                    Tab::Settings => {
                        self.settings
                            .handle_events(
                                event,
                                &self.database,
                                &self.exit_sender,
                                &self.controls,
                                &mut self.playback_config,
                            )
                            .await
                    }
                };

                self.handle_output(key_event.code, screen_output).await;
            }

            Event::Resize(_, _) => self.should_draw = true,
            _ => {}
        };
        Ok(())
    }

    fn navigate_to_library(&mut self) {
        self.current_screen = Tab::Library;
    }

    fn navigate_to_search(&mut self) {
        self.search.editing = true;
        self.current_screen = Tab::Search;
    }

    fn navigate_to_queue(&mut self) {
        self.current_screen = Tab::Queue;
    }

    fn navigate_to_discover(&mut self) {
        self.current_screen = Tab::Discover;
    }

    async fn navigate_to_settings(&mut self) {
        self.current_screen = Tab::Settings;
        self.settings.refresh_from_database(&self.database).await;
    }

    fn exit(&mut self) {
        self.exit = true;
    }
}

async fn fetch_image(image_url: &str) -> Option<(StatefulProtocol, f32)> {
    let client = reqwest::Client::new();
    let response = client.get(image_url).send().await.ok()?;
    let img_bytes = response.bytes().await.ok()?;

    let image = load_from_memory(&img_bytes).ok()?;
    let ratio = image.width() as f32 / image.height() as f32;

    let picker = Picker::from_query_stdio().ok()?;
    Some((picker.new_resize_protocol(image), ratio))
}

pub async fn get_current_state(tracklist: Tracklist, status: Status) -> NowPlayingState {
    let entity = tracklist.entity_playing();
    let track = tracklist.current_track().cloned();
    let image = if let Some(image_url) = entity.cover_link {
        Some(fetch_image(&image_url).await)
    } else {
        None
    }
    .flatten();

    let tracklist_length = tracklist.total();

    NowPlayingState {
        image,
        entity_title: entity.title,
        playing_track: track,
        tracklist_length,
        status,
        tracklist_position: tracklist.current_position(),
        duration_ms: 0,
    }
}
