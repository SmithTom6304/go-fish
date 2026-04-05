use crate::state::PreLobbyInputState;
use crate::state::{AppState, ConnectingState, LobbyState, PreLobbyState, Screen};
use crate::state::{GameInputState, GameState};
use go_fish::HookResult;
use go_fish::Rank;
use go_fish::Suit;
use go_fish::{Card, IncompleteBook};
use go_fish_web::HookError;
use go_fish_web::HookOutcome;
use ratatui::buffer::Buffer;
use ratatui::layout::{Flex, Rect};
use ratatui::prelude::Stylize;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Padding;
use ratatui::widgets::{Clear, Widget};
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

fn render_background(f: &mut Frame, area: Rect, player_name: &str, error: Option<&str>, hints: &str) {
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
        Span::styled(player_name, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
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
    if let Some(err) = error {
        let error_para = Paragraph::new(err)
            .style(Style::default().fg(Color::Red));
        f.render_widget(error_para, chunks[1]);
    }

    let hints = Paragraph::new(hints)
        .alignment(Alignment::Center);
    f.render_widget(hints, chunks[3]);
}

pub fn render_pre_lobby(f: &mut Frame, state: &PreLobbyState) {
    let area = f.area();

    let hints = match &state.input_state {
        PreLobbyInputState::None => "[c] Create lobby  [j] Join lobby  [q] Quit",
        PreLobbyInputState::LobbyId(lobby_id_state) =>
            "[enter] Join lobby  [esc] Close"
    };

    render_background(f, area, &state.player_name, state.error.as_deref(), hints);

    // Optional input overlay
    match &state.input_state {
        PreLobbyInputState::None => {},
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
        }
    };
}

pub fn render_lobby(f: &mut Frame, state: &LobbyState) {
    let area = f.area();

    // Keybind hints
    let can_start = state.players.len() >= 2 && state.leader == state.player_name;
    let hints = match can_start {
        true => "[s] Start game [q] Leave lobby",
        false => "[q] Leave lobby",
    };

    render_background(f, area, &state.player_name, state.error.as_deref(), hints);

    // Lobby overlay
    let centered_area = area.centered(Constraint::Percentage(40), Constraint::Length(20));
    let fg_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // lobby id
            Constraint::Min(8), // players
        ])
        .split(centered_area);
    f.render_widget(Clear, centered_area);

    // Lobby info header
    let header = Paragraph::new(format!("Lobby ID: {}", state.lobby_id))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(header, fg_chunks[0]);

    // Player list
    let player_lines: Vec<Line> = state
        .players
        .iter()
        .map(|p| {
            let str = match p == &state.leader {
                true => format!("★ {}", p),
                false => format!("  {}", p),
            };
            let is_client = p == &state.player_name;
            let style = if is_client {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            Line::from(vec![Span::styled(str, style)])
        })
        .collect();
    let players_title = format!("Players: ({}/{})", state.players.len(), state.max_players);
    let player_list = Paragraph::new(player_lines)
        .block(Block::default().borders(Borders::ALL).title(players_title));
    f.render_widget(player_list, fg_chunks[1]);


    let player_name = Line::from(vec![
        Span::styled("You are player ", Style::default()),
        Span::styled(&state.player_name, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
    ]);
}

pub fn render_game(f: &mut Frame, state: &GameState) {
    let area = f.area();
    let is_turn = state.active_player == state.player_name;
    let hints = match state.game_result.is_some() {
        true => "[enter] Return to menu [q] Quit",
        false => match is_turn {
            true => match &state.input_state {
                GameInputState::Idle => "[h] Hook [q] Quit",
                GameInputState::SelectingTarget { .. } => "[k/up] Up [j/down] Down [enter] Select",
                GameInputState::SelectingRank { .. } => "[h/left] Left [l/right] Right [enter] Select]"
            },
            false => "[q] Quit",
        }
    };

    render_background(f, area, &state.player_name, None, hints);

    if let Some(game_result) = &state.game_result {
        let centered_area = area.centered(Constraint::Percentage(60), Constraint::Length(3));
        f.render_widget(Clear, centered_area);
        let text = format!("Game over! Winners: {}", game_result.winners.join(", "));
        let para = Paragraph::new(text).centered()
            .block(Block::default().borders(Borders::ALL).style(Style::default()));
        f.render_widget(para, centered_area);
        return;
    }

    // Fill constraint per player
    let mut constraints = state.players.iter().map(|_| Constraint::Length(7)).collect::<Vec<_>>();
    // Add status bar and keyboard hints
    constraints.push(Constraint::Length(2));
    constraints.push(Constraint::Length(2));
    let constraint_count = constraints.len();
    let bg_chunks = Layout::default()
        .direction(Direction::Vertical)
        .flex(Flex::Center)
        .constraints(constraints)
        .split(area);

    let strip_order = strip_order(&state.players, &state.player_name);
    for (i, player) in strip_order.iter().enumerate() {
        let player_area = bg_chunks[i];
        match player == &&state.player_name {
            true => {
                f.render_widget(Clear, player_area);
                render_local_player_strip(f, &state, player_area);
            },
            false => {
                let hand_size = state.opponent_card_counts.get(*player).unwrap_or(&0);
                let book_count = state.opponent_book_counts.get(*player).unwrap_or(&0);
                let highlighted = match state.input_state {
                    GameInputState::SelectingTarget { cursor: c } => c == i,
                    _ => false,
                };
                f.render_widget(Clear, player_area);
                render_opponent_player_strip(f, player, *hand_size, *book_count, player_area, highlighted);
            }
        }
    }

    render_status_bar(f, state, bg_chunks[constraint_count - 2]);
}

pub fn render(f: &mut Frame, app: &AppState) {
    match &app.screen {
        Screen::Connecting(s) => render_connecting(f, s),
        Screen::PreLobby(s) => render_pre_lobby(f, s),
        Screen::Lobby(s) => render_lobby(f, s),
        Screen::Game(s) => render_game(f, s)
    }
}

fn render_card_border(f: &mut Frame, area: Rect, highlighted: bool) {
    let rect = Rect::new(area.x, area.y, 7, 5);
    let col = if highlighted { Color::Yellow } else { Color::White };
    let block = Block::default().borders(Borders::ALL).style(Style::default().fg(col));
    f.render_widget(block, rect);
}

fn render_card_interior(f: &mut Frame, area: Rect, card: &Card) {
    let buf = f.buffer_mut();
    let suit_symbol = suit_symbol(card.suit);
    let suit_col = suit_colour(card.suit);
    let rank = rank_short(card.rank);

    buf.set_string(area.x + 2, area.y + 1, suit_symbol, Style::default().fg(suit_col));
    buf.set_string(area.x + 3, area.y + 2, rank, Style::default().fg(Color::White));
    buf.set_string(area.x + 4, area.y + 3, suit_symbol, Style::default().fg(suit_col));
}

fn render_book(f: &mut Frame, area: Rect, book: &IncompleteBook, highlighted: bool) {
    for (i, card) in book.cards.iter().enumerate() {
        let rect = Rect::new(area.x + (i as u16), area.y, 7, 5);
        f.render_widget(Clear, rect);
        render_card_border(f, rect, highlighted);
        render_card_interior(f, rect, card);
    }
}

fn render_local_player_strip(f: &mut Frame, game_state: &GameState, area: Rect) {
    let name = &game_state.player_name;
    let hand = &game_state.hand;
    let books = &game_state.completed_books;
    let highlighted = game_state.active_player == game_state.player_name;
    let selected_card = match game_state.input_state {
        GameInputState::SelectingRank { target: _, cursor: index } => Some(index),
        _ => None,
    };

    let border_style = match highlighted {
        true => Style::new().green(),
        false => Style::default(),
    };

    let you_title = Span::styled("you", Style::default().fg(Color::Green));
    let hand_block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(you_title);

    let strip_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(7), Constraint::Fill(1), Constraint::Length(14)])
        .split(hand_block.inner(area));

    let con = hand.books.iter().map(|book| Constraint::Length(6 + (book.cards.len()) as u16)).collect::<Vec<_>>();
    let cards_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(con)
        .split(strip_chunks[1]);

    // Render name
    // let name = Line::from(vec![Span::styled(name, Style::new().green())]);
    // f.render_widget(name, strip_chunks[0]);

    // Render cards
    //f.render_widget(cards_block, strip_chunks[1]);
    for (i, book) in hand.books.iter().enumerate() {
        let h = selected_card.map(|j| j == i).unwrap_or(false);
        render_book(f, cards_chunks[i], book, h);
    }

    // Render completed books
    let completed_books = Line::from(vec![Span::styled(format!("{} books", books.len()), Style::default().fg(Color::White))]);
    f.render_widget(completed_books, strip_chunks[2]);

    f.render_widget(hand_block, area);
}

fn render_opponent_player_strip(f: &mut Frame, name: &str, hand_size: usize, books: usize, area: Rect, highlighted: bool) {
    let strip_border = match highlighted {
        true => Style::default().fg(Color::Yellow),
        false => Style::default().fg(Color::White),
    };
    let hand_block = Block::default()
        .borders(Borders::ALL)
        .style(strip_border)
        .title(name);

    let strip_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(7), Constraint::Fill(1), Constraint::Length(14)])
        .split(hand_block.inner(area));

    let con = (0..hand_size).map(|_| Constraint::Length(7)).collect::<Vec<_>>();
    let cards_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(con)
        .split(strip_chunks[1]);

    // Render name
    // let name = Line::from(vec![Span::styled(name, Style::default().fg(Color::White))]);
    // f.render_widget(name, strip_chunks[0]);

    // Render cards
    //f.render_widget(cards_block, strip_chunks[1]);
    for i in 0..hand_size {
        render_card_border(f, cards_chunks[i], false);
    }

    // Render completed books
    let completed_books = Line::from(vec![Span::styled(format!("{} books", books), Style::default().fg(Color::White))]);
    f.render_widget(completed_books, strip_chunks[2]);

    f.render_widget(hand_block, area);
}

fn render_status_bar(f: &mut Frame, state: &GameState, area: Rect) {
    let status = Paragraph::new(status_message(state));
    f.render_widget(status, area);
}

// Card rendering helper functions
fn rank_short(rank: Rank) -> &'static str {
    match rank {
        Rank::Two => "2",
        Rank::Three => "3",
        Rank::Four => "4",
        Rank::Five => "5",
        Rank::Six => "6",
        Rank::Seven => "7",
        Rank::Eight => "8",
        Rank::Nine => "9",
        Rank::Ten => "10",
        Rank::Jack => "J",
        Rank::Queen => "Q",
        Rank::King => "K",
        Rank::Ace => "A"
    }
}

fn suit_symbol(suit: Suit) -> &'static str {
    match suit {
        Suit::Spades => "♠",
        Suit::Hearts => "♥",
        Suit::Diamonds => "♦",
        Suit::Clubs => "♣"
    }
}

fn suit_colour(suit: Suit) -> Color {
    match suit {
        Suit::Spades => Color::White,
        Suit::Hearts => Color::Red,
        Suit::Diamonds => Color::Red,
        Suit::Clubs => Color::White
    }
}

fn status_message(game_state: &'_ GameState) -> Line<'_> {
    if let Some(outcome) = &game_state.latest_hook_outcome {
        return hook_outcome_message(outcome, game_state.player_name.clone());
    }
    Line::styled("Game started!".to_string(), Style::default())
}

fn hook_outcome_message<'a>(outcome: &HookOutcome, local_name: String) -> Line<'a> {
    let rank = rank_short(outcome.rank);
    match &outcome.result {
        HookResult::Catch(book) => {
            Line::from(vec![
                format_name(outcome.fisher_name.clone(), local_name.clone()),
                " asked ".into(),
                format_name(outcome.target_name.clone(), local_name),
                " for ".into(),
                rank.into(),
                " — Caught ".into(),
                book.cards.len().to_string().into(),
                " cards!".into(),
            ])
        },
        HookResult::GoFish => {
            Line::from(vec![
                format_name(outcome.fisher_name.clone(), local_name.clone()),
                " asked ".into(),
                format_name(outcome.target_name.clone(), local_name),
                " for ".into(),
                rank.into(),
                " — Go Fish!".into(),
            ])
        }
    }
}

fn hook_error_message(err: &HookError) -> String {
    match err {
        HookError::NotYourTurn => "Not your turn".to_string(),
        HookError::UnknownPlayer(name) => format!("Unknown player: {}", name),
        HookError::CannotTargetYourself => "Cannot target yourself".to_string(),
        HookError::YouDoNotHaveRank(rank) => format!("You do not have rank: {}", rank),
    }
}

fn strip_order<'a>(players: &'a [String], local: &str) -> Vec<&'a String> {
    let idx = players.iter().position(|p| p == local).unwrap_or(0);
    let n = players.len();
    (1..n).map(|i| &players[(idx + i) % n])
        .chain(std::iter::once(&players[idx]))
        .collect()
}

fn format_name<'a>(name: String, local_name: String) -> Span<'a> {
    match name == local_name {
        true => Span::styled("you", Style::new().green()),
        false => Span::styled(name, Style::default()),
    }
}

fn opponents(game: &GameState) -> Vec<&String> {
    game.players.iter().filter(|p| p != &&game.player_name).collect()
}
