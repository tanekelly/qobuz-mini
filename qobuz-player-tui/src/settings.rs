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

pub struct SettingsState {
    state: TableState,
    devices_state: TableState,
    devices: Vec<AudioDevice>,
    selected_device: Option<String>,
    showing_devices: bool,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            state: TableState::default(),
            devices_state: TableState::default(),
            devices: Vec::new(),
            selected_device: None,
            showing_devices: false,
        }
    }
}

impl SettingsState {
    pub async fn new(database: &Database) -> Result<Self> {
        let config = database.get_configuration().await?;
        let devices = list_audio_devices().unwrap_or_default();
        
        Ok(Self {
            state: TableState::default(),
            devices_state: TableState::default(),
            devices,
            selected_device: config.audio_device_name,
            showing_devices: false,
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
        } else {
            self.render_main(frame, area);
        }
    }

    fn render_main(&mut self, frame: &mut Frame, area: Rect) {
        if self.state.selected().is_none() {
            self.state.select(Some(0));
        }
        
        let rows = vec![
            Row::new(vec!["Audio Output", self.selected_device.as_ref().map(|s| s.as_str()).unwrap_or("Default")]),
            Row::new(vec!["Sign Out"]),
        ];
        
        let table = basic_list_table(rows)
            .block(block(Some("Settings")));

        frame.render_stateful_widget(table, area, &mut self.state);
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
    ) -> Result<Output> {
        if self.showing_devices {
            return self.handle_device_selection(event, database, controls).await;
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
