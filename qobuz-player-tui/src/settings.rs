use qobuz_player_controls::{
    database::Database, error::Error, ExitSender, Result, controls::Controls,
    list_audio_devices, AudioDevice,
};
use ratatui::{
    crossterm::event::{Event, KeyCode, KeyEventKind},
    prelude::*,
    widgets::*,
};

use crate::{
    app::Output,
    ui::{basic_list_table, block},
};

const TIME_STRETCH_OPTIONS: [f32; 16] =
    [0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 1.8, 1.9, 2.0];
const PITCH_OPTIONS: [i16; 25] = [
    -12, -11, -10, -9, -8, -7, -6, -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
];
const PITCH_CENTS_OPTIONS: [i16; 21] = [
    -100, -90, -80, -70, -60, -50, -40, -30, -20, -10, 0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100,
];

pub struct SettingsState {
    state: TableState,
    devices_state: TableState,
    devices: Vec<AudioDevice>,
    selected_device: Option<String>,
    showing_devices: bool,
    time_stretch_ratio: f32,
    pitch_semitones: i16,
    pitch_cents: i16,
    showing_time_stretch: bool,
    showing_pitch: bool,
    showing_pitch_cents: bool,
    time_stretch_state: TableState,
    pitch_state: TableState,
    pitch_cents_state: TableState,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            state: TableState::default(),
            devices_state: TableState::default(),
            devices: Vec::new(),
            selected_device: None,
            showing_devices: false,
            time_stretch_ratio: 1.0,
            pitch_semitones: 0,
            pitch_cents: 0,
            showing_time_stretch: false,
            showing_pitch: false,
            showing_pitch_cents: false,
            time_stretch_state: TableState::default(),
            pitch_state: TableState::default(),
            pitch_cents_state: TableState::default(),
        }
    }
}

impl SettingsState {
    pub async fn new(database: &Database) -> Result<Self> {
        let config = database.get_configuration().await?;
        let devices = list_audio_devices().unwrap_or_default();
        let time_stretch_ratio = config.time_stretch_ratio;
        let pitch_semitones = config.pitch_semitones;
        let pitch_cents = config.pitch_cents;
        Ok(Self {
            state: TableState::default(),
            devices_state: TableState::default(),
            devices,
            selected_device: config.audio_device_name,
            showing_devices: false,
            time_stretch_ratio,
            pitch_semitones,
            pitch_cents,
            showing_time_stretch: false,
            showing_pitch: false,
            showing_pitch_cents: false,
            time_stretch_state: TableState::default(),
            pitch_state: TableState::default(),
            pitch_cents_state: TableState::default(),
        })
    }

    pub async fn refresh_devices(&mut self) {
        tracing::info!("Refreshing audio device list");
        self.devices = list_audio_devices().unwrap_or_default();
        if let Some(selected) = &self.selected_device {
            if let Some(index) = self.devices.iter().position(|d| &d.name == selected) {
                self.devices_state.select(Some(index + 1));
            } else {
                self.devices_state.select(Some(0));
            }
        } else {
            self.devices_state.select(Some(0));
        }
    }

    pub async fn refresh_from_database(&mut self, database: &Database) {
        tracing::info!("Refreshing settings state from database");
        if let Ok(config) = database.get_configuration().await {
            let old_device = self.selected_device.clone();
            self.selected_device = config.audio_device_name;
            self.time_stretch_ratio = config.time_stretch_ratio;
            self.pitch_semitones = config.pitch_semitones;
            self.pitch_cents = config.pitch_cents;
            self.time_stretch_state.select(None);
            self.pitch_state.select(None);
            self.pitch_cents_state.select(None);
            self.devices = list_audio_devices().unwrap_or_default();
            if old_device != self.selected_device {
                if let Some(selected) = &self.selected_device {
                    if let Some(index) = self.devices.iter().position(|d| &d.name == selected) {
                        self.devices_state.select(Some(index + 1));
                    } else {
                        self.devices_state.select(Some(0));
                    }
                } else {
                    self.devices_state.select(Some(0));
                }
            }
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if self.showing_devices {
            self.render_devices(frame, area);
        } else if self.showing_time_stretch {
            self.render_time_stretch(frame, area);
        } else if self.showing_pitch {
            self.render_pitch(frame, area);
        } else if self.showing_pitch_cents {
            self.render_pitch_cents(frame, area);
        } else {
            self.render_main(frame, area);
        }
    }

    fn render_main(&mut self, frame: &mut Frame, area: Rect) {
        if self.state.selected().is_none() {
            self.state.select(Some(0));
        }
        let time_str = format!("{:.1}x", self.time_stretch_ratio);
        let pitch_str = format!("{} semitones", self.pitch_semitones);
        let pitch_cents_str = format!("{} cents", self.pitch_cents);
        let rows = vec![
            Row::new(vec![
                "Audio Output",
                self.selected_device.as_ref().map(|s| s.as_str()).unwrap_or("Default"),
            ]),
            Row::new(vec!["Time stretch", time_str.as_str()]),
            Row::new(vec!["Pitch (semitones)", pitch_str.as_str()]),
            Row::new(vec!["Pitch (cents)", pitch_cents_str.as_str()]),
            Row::new(vec!["Sign Out"]),
        ];
        let table = basic_list_table(rows).block(block(Some("Settings")));
        frame.render_stateful_widget(table, area, &mut self.state);
    }

    fn render_time_stretch(&mut self, frame: &mut Frame, area: Rect) {
        if self.time_stretch_state.selected().is_none() {
            let idx = TIME_STRETCH_OPTIONS
                .iter()
                .position(|&r| (r - self.time_stretch_ratio).abs() < 0.01)
                .unwrap_or(5);
            self.time_stretch_state.select(Some(idx));
        }
        let labels: Vec<String> = TIME_STRETCH_OPTIONS
            .iter()
            .map(|&r| {
                let marker = if (r - self.time_stretch_ratio).abs() < 0.01 {
                    "✓ "
                } else {
                    "  "
                };
                format!("{}{:.1}x", marker, r)
            })
            .collect();
        let rows: Vec<Row> = labels.iter().map(|s| Row::new(vec![s.as_str()])).collect();
        let table = basic_list_table(rows).block(block(Some("Time stretch (0.5–2.0)")));
        frame.render_stateful_widget(table, area, &mut self.time_stretch_state);
    }

    fn render_pitch(&mut self, frame: &mut Frame, area: Rect) {
        if self.pitch_state.selected().is_none() {
            let idx = PITCH_OPTIONS
                .iter()
                .position(|&s| s == self.pitch_semitones)
                .unwrap_or(12);
            self.pitch_state.select(Some(idx));
        }
        let labels: Vec<String> = PITCH_OPTIONS
            .iter()
            .map(|&s| {
                let marker = if s == self.pitch_semitones { "✓ " } else { "  " };
                if s >= 0 {
                    format!("{}+{}", marker, s)
                } else {
                    format!("{}{}", marker, s)
                }
            })
            .collect();
        let rows: Vec<Row> = labels.iter().map(|s| Row::new(vec![s.as_str()])).collect();
        let table = basic_list_table(rows).block(block(Some("Pitch (-12 to +12 semitones)")));
        frame.render_stateful_widget(table, area, &mut self.pitch_state);
    }

    fn render_pitch_cents(&mut self, frame: &mut Frame, area: Rect) {
        if self.pitch_cents_state.selected().is_none() {
            let idx = PITCH_CENTS_OPTIONS
                .iter()
                .position(|&c| c == self.pitch_cents)
                .unwrap_or(10);
            self.pitch_cents_state.select(Some(idx));
        }
        let labels: Vec<String> = PITCH_CENTS_OPTIONS
            .iter()
            .map(|&c| {
                let marker = if c == self.pitch_cents { "✓ " } else { "  " };
                if c >= 0 {
                    format!("{}+{}", marker, c)
                } else {
                    format!("{}{}", marker, c)
                }
            })
            .collect();
        let rows: Vec<Row> = labels.iter().map(|s| Row::new(vec![s.as_str()])).collect();
        let table = basic_list_table(rows).block(block(Some("Pitch (-100 to +100 cents)")));
        frame.render_stateful_widget(table, area, &mut self.pitch_cents_state);
    }

    fn render_devices(&mut self, frame: &mut Frame, area: Rect) {
        if self.devices_state.selected().is_none() {
            self.devices_state.select(Some(0));
        }
        
        let mut device_texts: Vec<String> = Vec::new();
        
        let default_marker = if self.selected_device.is_none() {
            "✓ "
        } else {
            "  "
        };
        device_texts.push(format!("{}{}", default_marker, "Default"));
        
        device_texts.extend(self.devices.iter().map(|d| {
            let marker = if self.selected_device.as_ref().map(|s| s == &d.name).unwrap_or(false) {
                "✓ "
            } else {
                "  "
            };
            format!("{}{}", marker, d.name)
        }));
        
        let rows: Vec<Row> = device_texts.iter().map(|text| Row::new(vec![text.as_str()])).collect();
        
        let table = basic_list_table(rows)
            .block(block(Some("Select Audio Output")));

        frame.render_stateful_widget(table, area, &mut self.devices_state);
    }

    pub async fn handle_events(
        &mut self,
        event: Event,
        database: &Database,
        exit_sender: &ExitSender,
        controls: &Controls,
        playback_config: &mut (f32, i16, i16),
    ) -> Result<Output> {
        if self.showing_devices {
            return self.handle_device_selection(event, database, controls).await;
        }
        if self.showing_time_stretch {
            return self
                .handle_time_stretch_selection(event, database, controls, playback_config)
                .await;
        }
        if self.showing_pitch {
            return self
                .handle_pitch_selection(event, database, controls, playback_config)
                .await;
        }
        if self.showing_pitch_cents {
            return self
                .handle_pitch_cents_selection(event, database, controls, playback_config)
                .await;
        }

        match event {
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                match key_event.code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.state.select_next();
                        Ok(Output::Consumed)
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.state.select_previous();
                        Ok(Output::Consumed)
                    }
                    KeyCode::Enter => {
                        match self.state.selected() {
                            Some(0) => {
                                self.showing_devices = true;
                                self.refresh_devices().await;
                                Ok(Output::Consumed)
                            }
                            Some(1) => {
                                self.showing_time_stretch = true;
                                let idx = TIME_STRETCH_OPTIONS
                                    .iter()
                                    .position(|&r| (r - self.time_stretch_ratio).abs() < 0.01)
                                    .unwrap_or(5);
                                self.time_stretch_state.select(Some(idx));
                                Ok(Output::Consumed)
                            }
                            Some(2) => {
                                self.showing_pitch = true;
                                let idx = PITCH_OPTIONS
                                    .iter()
                                    .position(|&s| s == self.pitch_semitones)
                                    .unwrap_or(12);
                                self.pitch_state.select(Some(idx));
                                Ok(Output::Consumed)
                            }
                            Some(3) => {
                                self.showing_pitch_cents = true;
                                let idx = PITCH_CENTS_OPTIONS
                                    .iter()
                                    .position(|&c| c == self.pitch_cents)
                                    .unwrap_or(10);
                                self.pitch_cents_state.select(Some(idx));
                                Ok(Output::Consumed)
                            }
                            Some(4) => {
                                database.refresh_database().await?;
                                exit_sender.send(true).map_err(|_| Error::Notification)?;
                                Ok(Output::Consumed)
                            }
                            _ => Ok(Output::NotConsumed),
                        }
                    }
                    KeyCode::Esc => Ok(Output::NotConsumed),
                    _ => Ok(Output::NotConsumed),
                }
            }
            _ => Ok(Output::NotConsumed),
        }
    }

    async fn handle_time_stretch_selection(
        &mut self,
        event: Event,
        database: &Database,
        controls: &Controls,
        playback_config: &mut (f32, i16, i16),
    ) -> Result<Output> {
        match event {
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                match key_event.code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        if let Some(selected) = self.time_stretch_state.selected() {
                            if selected + 1 < TIME_STRETCH_OPTIONS.len() {
                                self.time_stretch_state.select(Some(selected + 1));
                            }
                        }
                        Ok(Output::Consumed)
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if let Some(selected) = self.time_stretch_state.selected() {
                            if selected > 0 {
                                self.time_stretch_state.select(Some(selected - 1));
                            }
                        }
                        Ok(Output::Consumed)
                    }
                    KeyCode::Enter => {
                        if let Some(selected) = self.time_stretch_state.selected() {
                            if let Some(&ratio) = TIME_STRETCH_OPTIONS.get(selected) {
                                self.time_stretch_ratio = ratio;
                                playback_config.0 = ratio;
                                database.set_time_stretch_ratio(ratio).await?;
                                controls.set_time_stretch(ratio);
                                self.showing_time_stretch = false;
                                Ok(Output::Consumed)
                            } else {
                                Ok(Output::NotConsumed)
                            }
                        } else {
                            Ok(Output::NotConsumed)
                        }
                    }
                    KeyCode::Esc => {
                        self.showing_time_stretch = false;
                        Ok(Output::Consumed)
                    }
                    _ => Ok(Output::NotConsumed),
                }
            }
            _ => Ok(Output::NotConsumed),
        }
    }

    async fn handle_pitch_selection(
        &mut self,
        event: Event,
        database: &Database,
        controls: &Controls,
        playback_config: &mut (f32, i16, i16),
    ) -> Result<Output> {
        match event {
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                match key_event.code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        if let Some(selected) = self.pitch_state.selected() {
                            if selected + 1 < PITCH_OPTIONS.len() {
                                self.pitch_state.select(Some(selected + 1));
                            }
                        }
                        Ok(Output::Consumed)
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if let Some(selected) = self.pitch_state.selected() {
                            if selected > 0 {
                                self.pitch_state.select(Some(selected - 1));
                            }
                        }
                        Ok(Output::Consumed)
                    }
                    KeyCode::Enter => {
                        if let Some(selected) = self.pitch_state.selected() {
                            if let Some(&semitones) = PITCH_OPTIONS.get(selected) {
                                self.pitch_semitones = semitones;
                                playback_config.1 = semitones;
                                database.set_pitch_semitones(semitones).await?;
                                controls.set_pitch(semitones);
                                self.showing_pitch = false;
                                Ok(Output::Consumed)
                            } else {
                                Ok(Output::NotConsumed)
                            }
                        } else {
                            Ok(Output::NotConsumed)
                        }
                    }
                    KeyCode::Esc => {
                        self.showing_pitch = false;
                        Ok(Output::Consumed)
                    }
                    _ => Ok(Output::NotConsumed),
                }
            }
            _ => Ok(Output::NotConsumed),
        }
    }

    async fn handle_pitch_cents_selection(
        &mut self,
        event: Event,
        database: &Database,
        controls: &Controls,
        playback_config: &mut (f32, i16, i16),
    ) -> Result<Output> {
        match event {
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                match key_event.code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        if let Some(selected) = self.pitch_cents_state.selected() {
                            if selected + 1 < PITCH_CENTS_OPTIONS.len() {
                                self.pitch_cents_state.select(Some(selected + 1));
                            }
                        }
                        Ok(Output::Consumed)
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if let Some(selected) = self.pitch_cents_state.selected() {
                            if selected > 0 {
                                self.pitch_cents_state.select(Some(selected - 1));
                            }
                        }
                        Ok(Output::Consumed)
                    }
                    KeyCode::Enter => {
                        if let Some(selected) = self.pitch_cents_state.selected() {
                            if let Some(&cents) = PITCH_CENTS_OPTIONS.get(selected) {
                                self.pitch_cents = cents;
                                playback_config.2 = cents;
                                database.set_pitch_cents(cents).await?;
                                controls.set_pitch_cents(cents);
                                self.showing_pitch_cents = false;
                                Ok(Output::Consumed)
                            } else {
                                Ok(Output::NotConsumed)
                            }
                        } else {
                            Ok(Output::NotConsumed)
                        }
                    }
                    KeyCode::Esc => {
                        self.showing_pitch_cents = false;
                        Ok(Output::Consumed)
                    }
                    _ => Ok(Output::NotConsumed),
                }
            }
            _ => Ok(Output::NotConsumed),
        }
    }

    async fn handle_device_selection(
        &mut self,
        event: Event,
        database: &Database,
        controls: &Controls,
    ) -> Result<Output> {
        match event {
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                match key_event.code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        if let Some(selected) = self.devices_state.selected() {
                            if selected < self.devices.len() {
                                self.devices_state.select(Some(selected + 1));
                            }
                        }
                        Ok(Output::Consumed)
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if let Some(selected) = self.devices_state.selected() {
                            if selected > 0 {
                                self.devices_state.select(Some(selected - 1));
                            }
                        }
                        Ok(Output::Consumed)
                    }
                    KeyCode::Enter => {
                        if let Some(selected) = self.devices_state.selected() {
                            let device_name = if selected == 0 {
                                None
                            } else {
                                self.devices.get(selected - 1).map(|d| d.name.clone())
                            };
                            
                            tracing::info!("User selected audio device: {:?}", device_name);
                            self.selected_device = device_name.clone();
                            controls.set_audio_device(device_name.clone());
                            database.set_audio_device(device_name).await?;
                            self.showing_devices = false;
                            self.refresh_devices().await;
                            Ok(Output::Consumed)
                        } else {
                            Ok(Output::NotConsumed)
                        }
                    }
                    KeyCode::Esc => {
                        self.showing_devices = false;
                        Ok(Output::Consumed)
                    }
                    _ => Ok(Output::NotConsumed),
                }
            }
            _ => Ok(Output::NotConsumed),
        }
    }
}
