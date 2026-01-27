use qobuz_player_controls::{
    database::Database, error::Error, ExitSender, Result,
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
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            state: TableState::default(),
        }
    }
}

impl SettingsState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let table = basic_list_table(vec![Row::new(vec!["Sign Out"])])
            .block(block(Some("Settings")));

        frame.render_stateful_widget(table, area, &mut self.state);
    }

    pub async fn handle_events(
        &mut self,
        event: Event,
        database: &Database,
        exit_sender: &ExitSender,
    ) -> Result<Output> {
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
                    KeyCode::Enter if self.state.selected() == Some(0) => {
                        database.refresh_database().await?;
                        exit_sender.send(true).map_err(|_| Error::Notification)?;
                        Ok(Output::Consumed)
                    }
                    _ => Ok(Output::NotConsumed),
                }
            }
            _ => Ok(Output::NotConsumed),
        }
    }
}
