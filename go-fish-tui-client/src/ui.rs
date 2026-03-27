use crate::state::PreLobbyInputState;
use crate::state::{AppState, ConnectingState, LobbyState, PreLobbyState, Screen};
use ratatui::widgets::Clear;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn render_connecting(f: &mut Frame, state: &ConnectingState) {
    let area = f.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(45),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);

    let paragraph = Paragraph::new(state.status.as_str())
        .alignment(Alignment::Center)
        .block(Block::default());

    f.render_widget(paragraph, vertical[1]);
}

pub fn render_pre_lobby(f: &mut Frame, state: &PreLobbyState) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // player name
            Constraint::Length(3), // keybind hints
            Constraint::Length(3), // error (if any)
            Constraint::Min(0),
        ])
        .split(area);

    // Player name
    let name_para = Paragraph::new(format!("Player: {}", state.player_name))
        .block(Block::default().borders(Borders::ALL).title("Identity"));
    f.render_widget(name_para, chunks[0]);

    match &state.input_state {
        PreLobbyInputState::None => {}
        PreLobbyInputState::LobbyId(lobby_id) => {
            let centered_area = area.centered(Constraint::Percentage(60), Constraint::Percentage(20));
            f.render_widget(Clear, centered_area);
            let lobby_id_para = Paragraph::new(format!("Lobby ID: {}", lobby_id))
                .block(Block::default().borders(Borders::ALL).title("Lobby ID"));
            f.render_widget(lobby_id_para, centered_area);
        }
    }

    // Keybind hints
    let hints = Paragraph::new("[c] Create lobby  [Enter] Join lobby  [q] Quit")
        .alignment(Alignment::Center);
    f.render_widget(hints, chunks[1]);

    // Error message
    if let Some(err) = &state.error {
        let error_para = Paragraph::new(err.as_str())
            .style(Style::default().fg(Color::Red))
            .alignment(Alignment::Center);
        f.render_widget(error_para, chunks[2]);
    }
}

pub fn render_lobby(f: &mut Frame, state: &LobbyState) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // lobby info header
            Constraint::Min(5),    // player list
            Constraint::Length(3), // keybind hints
            Constraint::Length(3), // error (if any)
        ])
        .split(area);

    // Lobby info header
    let header = Paragraph::new(format!(
        "Lobby: {}  |  Max players: {}",
        state.lobby_id, state.max_players
    ))
    .block(Block::default().borders(Borders::ALL).title("Lobby"));
    f.render_widget(header, chunks[0]);

    // Player list
    let player_lines: Vec<String> = state
        .players
        .iter()
        .map(|p| {
            if p == &state.leader {
                format!("★ {}", p)
            } else {
                format!("  {}", p)
            }
        })
        .collect();
    let player_list = Paragraph::new(player_lines.join("\n"))
        .block(Block::default().borders(Borders::ALL).title("Players"));
    f.render_widget(player_list, chunks[1]);

    // Keybind hints
    let hints = Paragraph::new("[l] Leave lobby  [q] Quit").alignment(Alignment::Center);
    f.render_widget(hints, chunks[2]);

    // Error message
    if let Some(err) = &state.error {
        let error_para = Paragraph::new(err.as_str())
            .style(Style::default().fg(Color::Red))
            .alignment(Alignment::Center);
        f.render_widget(error_para, chunks[3]);
    }
}

pub fn render(f: &mut Frame, app: &AppState) {
    match &app.screen {
        Screen::Connecting(s) => render_connecting(f, s),
        Screen::PreLobby(s) => render_pre_lobby(f, s),
        Screen::Lobby(s) => render_lobby(f, s),
    }
}
