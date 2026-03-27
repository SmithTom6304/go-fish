use crate::state::PreLobbyInputState;
use crate::state::{AppState, ConnectingState, LobbyState, PreLobbyState, Screen};
use futures_util::future::err;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Clear;
use ratatui::widgets::Padding;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::fmt::format;

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
            Constraint::Length(1), // player name
            Constraint::Fill(1), // go-fish print
            Constraint::Length(1), // error (if any)
            Constraint::Length(1), // keyboard hints
        ])
        .split(area);

    // Player name
    let player_name = Line::from(vec![
        Span::styled("You are player ", Style::default()),
        Span::styled(&state.player_name, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
    ]);
    f.render_widget(player_name, chunks[0]);

    // Go-Fish block print
    let go_fish_str = include_str!("assets/go-fish-display-string.txt");
    let go_fish_display_area = chunks[1];
    let go_fish_para = Paragraph::new(go_fish_str)
        .block(Block::default()
            .borders(Borders::ALL)
            .padding(Padding::new(
                0, // left
                0, // right
                go_fish_display_area.height / 3, // top
                0, // bottom
            )
            )
        ).alignment(Alignment::Center);
    f.render_widget(go_fish_para, go_fish_display_area);

    // Error message
    if let Some(err) = &state.error {
        let error_para = Paragraph::new(err.as_str())
            .style(Style::default().fg(Color::Red));
        f.render_widget(error_para, chunks[1]);
    }

    // Optional input overlay
    let hints = match &state.input_state {
        PreLobbyInputState::None => "[c] Create lobby  [j] Join lobby  [q] Quit",
        PreLobbyInputState::LobbyId(lobby_id_state) => {
            let lobby_id = &lobby_id_state.lobby_id;
            let error = &lobby_id_state.error;
            let error_text = error.as_deref().unwrap_or("");
            let centered_area = area.centered(Constraint::Percentage(60), Constraint::Length(3));
            f.render_widget(Clear, centered_area);
            let text = match error {
                Some(err) => err,
                None => {
                    match lobby_id.len() {
                        0 => "Enter a lobby ID",
                        _ => lobby_id,
                    }
                }
            };
            let border_style = match error.is_some() {
                true => Style::default().fg(Color::Red),
                false => Style::default(),
            };
            let lobby_id_para = Paragraph::new(text).centered()
                .block(Block::default().borders(Borders::ALL)
                    .style(border_style)
                );
            f.render_widget(lobby_id_para, centered_area);
            "[enter] Join lobby  [esc] Close"
        }
    };

    // Keybind hints
    let hints = Paragraph::new(hints)
        .alignment(Alignment::Center);
    f.render_widget(hints, chunks[3]);
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
