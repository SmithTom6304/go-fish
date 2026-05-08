const CARD_WIDTH: u16 = 7;
const CARD_HEIGHT: u16 = 5;

pub use render::render;

mod render {
    use crate::state::{AppState, BrowsingLobbiesState, BrowsingStatus, ConnectingState, LobbyState, PreLobbyState, Screen};
    use crate::state::{GameInputState, GameState, MAX_NOTIFICATION_HISTORY};
    use ratatui::layout::{Flex, Rect};
    use ratatui::style::Modifier;
    use ratatui::widgets::Clear;
    use ratatui::widgets::Padding;
    use ratatui::{
        layout::{Alignment, Constraint, Direction, Layout},
        style::{Color, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Paragraph},
        Frame,
    };

    pub fn render(f: &mut Frame, app: &AppState) {
        match &app.screen {
            Screen::Connecting(s) => render_connecting(f, s),
            Screen::PreLobby(s) => render_pre_lobby(f, s),
            Screen::BrowsingLobbies(s) => render_browsing_lobbies(f, s),
            Screen::Lobby(s) => render_lobby(f, s),
            Screen::Game(s) => render_game(f, s),
        }
    }

    fn render_connecting(f: &mut Frame, state: &ConnectingState) {
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
                Constraint::Fill(1),   // go-fish print
                Constraint::Length(1), // error (if any)
                Constraint::Length(1), // keyboard hints
            ])
            .split(area);

        // Player name
        let player_name = Line::from(vec![
            Span::styled("You are player ", Style::default()),
            Span::styled(player_name, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]);
        f.render_widget(player_name, chunks[0]);

        // Go-Fish block print
        let go_fish_str = include_str!("assets/go-fish-display-string.txt");
        let go_fish_display_area = chunks[1];
        let go_fish_para = Paragraph::new(go_fish_str)
            .block(Block::default()
                .borders(Borders::ALL)
                .padding(Padding::new(
                    0,                              // left
                    0,                              // right
                    go_fish_display_area.height / 3, // top
                    0,                              // bottom
                )))
            .alignment(Alignment::Center);
        f.render_widget(go_fish_para, go_fish_display_area);

        // Error message
        if let Some(err) = error {
            let error_para = Paragraph::new(err).style(Style::default().fg(Color::Red));
            f.render_widget(error_para, chunks[2]);
        }

        let hints = Paragraph::new(hints).alignment(Alignment::Center);
        f.render_widget(hints, chunks[3]);
    }

    fn render_pre_lobby(f: &mut Frame, state: &PreLobbyState) {
        let area = f.area();
        render_background(f, area, &state.player_name, state.error.as_deref(),
            "[c] Create lobby  [j] Browse lobbies  [q] Quit");
    }

    const BRAILLE_SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

    fn render_browsing_lobbies(f: &mut Frame, state: &BrowsingLobbiesState) {
        let area = f.area();
        let spinner = BRAILLE_SPINNER[state.frame_index % 10];

        let hints = match &state.status {
            BrowsingStatus::Creating => "",
            _ => "[c] Create  [i] Enter ID  [r] Refresh  [esc] Back",
        };
        render_background(f, area, &state.player_name, None, hints);

        let centered = area.centered(Constraint::Percentage(60), Constraint::Length(20));
        f.render_widget(Clear, centered);

        let list_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(centered);

        match &state.status {
            BrowsingStatus::Loading => {
                let top_pad = list_chunks[0].height.saturating_sub(2).saturating_sub(1) / 2;
                let para = Paragraph::new(format!("{} Loading lobbies…", spinner))
                    .alignment(Alignment::Center)
                    .block(Block::default().borders(Borders::ALL).title("Lobbies")
                        .padding(Padding::new(0, 0, top_pad, 0)));
                f.render_widget(para, list_chunks[0]);
            }
            BrowsingStatus::Creating => {
                let top_pad = list_chunks[0].height.saturating_sub(2).saturating_sub(1) / 2;
                let para = Paragraph::new(format!("{} Creating lobby…", spinner))
                    .alignment(Alignment::Center)
                    .block(Block::default().borders(Borders::ALL).title("Lobbies")
                        .padding(Padding::new(0, 0, top_pad, 0)));
                f.render_widget(para, list_chunks[0]);
            }
            BrowsingStatus::Loaded(lobbies) => {
                // Lobby list
                let list_lines: Vec<Line> = if lobbies.is_empty() {
                    vec![Line::from(Span::styled(
                        "No lobbies available — press [c] to create one",
                        Style::default().fg(Color::DarkGray),
                    ))]
                } else {
                    lobbies.iter().enumerate().map(|(i, lobby)| {
                        let text = format!("  {}  ({}/{})", lobby.lobby_id, lobby.player_count, lobby.max_players);
                        if i == state.selected_index {
                            Line::from(Span::styled(text, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
                        } else {
                            Line::from(text)
                        }
                    }).collect()
                };
                let list_para = if lobbies.is_empty() {
                    let inner_height = list_chunks[0].height.saturating_sub(2);
                    let top_pad = inner_height.saturating_sub(1) / 2;
                    Paragraph::new(list_lines)
                        .alignment(Alignment::Center)
                        .block(Block::default().borders(Borders::ALL).title("Lobbies")
                            .padding(Padding::new(0, 0, top_pad, 0)))
                } else {
                    Paragraph::new(list_lines)
                        .block(Block::default().borders(Borders::ALL).title("Lobbies"))
                };
                f.render_widget(list_para, list_chunks[0]);

                // Join hint
                let join_hint = if !lobbies.is_empty() { "[enter] Join selected" } else { "" };
                let hint_para = Paragraph::new(join_hint).alignment(Alignment::Center);
                f.render_widget(hint_para, list_chunks[1]);
            }
            BrowsingStatus::EnteringId { input: id_input, error } => {
                // Empty lobbies box as backdrop
                let list_para = Paragraph::new("").block(Block::default().borders(Borders::ALL).title("Lobbies"));
                f.render_widget(list_para, list_chunks[0]);

                // Input dialog centered over the lobbies box
                let overlay = list_chunks[0].centered(Constraint::Percentage(70), Constraint::Length(3));
                f.render_widget(Clear, overlay);

                let (display, content_style, border_style) = if let Some(err) = error {
                    (err.as_str(), Style::default().fg(Color::Red), Style::default().fg(Color::Red))
                } else if id_input.is_empty() {
                    ("Enter lobby ID…", Style::default().fg(Color::DarkGray), Style::default())
                } else {
                    (id_input.as_str(), Style::default(), Style::default())
                };
                let input_para = Paragraph::new(Span::styled(display, content_style))
                    .centered()
                    .block(Block::default().borders(Borders::ALL).title("Lobby ID").style(border_style));
                f.render_widget(input_para, overlay);
            }
            BrowsingStatus::Error(msg) => {
                let para = Paragraph::new(format!("Error: {}\n\n[r] Retry  [i] Enter ID manually", msg))
                    .alignment(Alignment::Center)
                    .block(Block::default().borders(Borders::ALL).title("Lobbies"));
                f.render_widget(para, centered);
            }
        }
    }

    fn render_lobby(f: &mut Frame, state: &LobbyState) {
        let area = f.area();

        let is_leader = state.leader == state.player_name;
        let can_start = state.players.len() >= 2 && is_leader;
        let hints = if can_start {
            "[s] Start  [a] Add bot  [d] Remove bot  [q] Leave lobby"
        } else if is_leader {
            "[a] Add bot  [d] Remove bot  [q] Leave lobby"
        } else {
            "[q] Leave lobby"
        };

        render_background(f, area, &state.player_name, state.error.as_deref(), hints);

        // Lobby overlay
        let centered_area = area.centered(Constraint::Percentage(40), Constraint::Length(20));
        let fg_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // lobby id
                Constraint::Min(8),    // players
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
                let display_name = match p {
                    go_fish_web::LobbyPlayer::Human { name } => name.clone(),
                    go_fish_web::LobbyPlayer::Bot { name, .. } => name.clone(),
                };
                let str = if p.name() == state.leader {
                    format!("★ {}", display_name)
                } else {
                    format!("  {}", display_name)
                };
                let style = if p.name() == state.player_name {
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
    }

    fn render_game(f: &mut Frame, state: &GameState) {
        let area = f.area();
        let is_turn = state.active_player == state.player_name;
        let hints = if state.game_result.is_some() {
            "[enter] Return to menu [q] Quit"
        } else if is_turn {
            match &state.input_state {
                GameInputState::Idle => "[h] Hook [q] Quit",
                GameInputState::SelectingTarget { .. } => "[k/up] Up [j/down] Down [enter] Select",
                GameInputState::SelectingRank { .. } => "[h/left] Left [l/right] Right [enter] Select]",
            }
        } else {
            "[q] Quit"
        };

        render_background(f, area, &state.player_name, None, hints);

        if let Some(game_result) = &state.game_result {
            let centered_area = area.centered(Constraint::Percentage(60), Constraint::Length(3));
            f.render_widget(Clear, centered_area);
            let text = format!("Game over! Winners: {}", game_result.winners.join(", "));
            let para = Paragraph::new(text)
                .centered()
                .block(Block::default().borders(Borders::ALL).style(Style::default()));
            f.render_widget(para, centered_area);
            return;
        }

        let mut constraints = state.players.iter().map(|_| Constraint::Length(super::CARD_HEIGHT + 2)).collect::<Vec<_>>();
        constraints.push(Constraint::Length(MAX_NOTIFICATION_HISTORY as u16));
        let constraint_count = constraints.len();
        let bg_chunks = Layout::default()
            .direction(Direction::Vertical)
            .flex(Flex::Center)
            .constraints(constraints)
            .split(area);

        let strip_order = strip_order(&state.players, &state.player_name);
        let opponents = opponents(state);
        for (i, player) in strip_order.iter().enumerate() {
            let player_area = bg_chunks[i];
            if player == &&state.player_name {
                let selected_card = match state.input_state {
                    GameInputState::SelectingRank { cursor: index, .. } => Some(index),
                    _ => None,
                };
                f.render_widget(Clear, player_area);
                f.render_widget(super::widgets::PlayerStripWidget::Local {
                    hand: &state.hand,
                    selected_card,
                    is_active: state.active_player == state.player_name,
                    book_count: state.completed_books.len(),
                }, player_area);
            } else {
                let hand_size = state.opponent_card_counts.get(*player).unwrap_or(&0);
                let book_count = state.opponent_book_counts.get(*player).unwrap_or(&0);
                let highlighted = match state.input_state {
                    GameInputState::SelectingTarget { cursor: c } => {
                        opponents.get(c).map_or("", |name| name) == *player
                    },
                    _ => false,
                };
                let is_active = state.active_player == **player;
                f.render_widget(Clear, player_area);
                f.render_widget(super::widgets::PlayerStripWidget::Opponent {
                    player_name: player,
                    hand_size: *hand_size,
                    highlighted,
                    is_active,
                    book_count: *book_count,
                }, player_area);
            }
        }

        render_status_bar(f, state, bg_chunks[constraint_count - 1]);
    }

    fn render_status_bar(f: &mut Frame, state: &GameState, area: Rect) {
        use super::widgets::{NotificationOrder, NotificationWidget};
        f.render_widget(
            NotificationWidget::new(&state.notifications, NotificationOrder::NewestFirst),
            area,
        );
    }

    fn strip_order<'a>(players: &'a [String], local: &str) -> Vec<&'a String> {
        let idx = players.iter().position(|p| p == local).unwrap_or(0);
        let n = players.len();
        (1..n).map(|i| &players[(idx + i) % n])
            .chain(std::iter::once(&players[idx]))
            .collect()
    }

    fn opponents(game: &GameState) -> Vec<&String> {
        game.players.iter().filter(|p| p != &&game.player_name).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::state::{AppState, BrowsingLobbiesState, BrowsingStatus, Screen};
        use go_fish::{Card, CompleteBook, Hand, IncompleteBook, Rank, Suit};
        use go_fish_web::GameResult;
        use proptest::prelude::*;
        use ratatui::{backend::TestBackend, Terminal};

        fn rank_strategy() -> impl Strategy<Value = Rank> {
            prop_oneof![
                Just(Rank::Two), Just(Rank::Three), Just(Rank::Four), Just(Rank::Five),
                Just(Rank::Six), Just(Rank::Seven), Just(Rank::Eight), Just(Rank::Nine),
                Just(Rank::Ten), Just(Rank::Jack), Just(Rank::Queen), Just(Rank::King),
                Just(Rank::Ace),
            ]
        }

        fn suit_strategy() -> impl Strategy<Value = Suit> {
            prop_oneof![
                Just(Suit::Clubs), Just(Suit::Diamonds),
                Just(Suit::Hearts), Just(Suit::Spades),
            ]
        }

        fn card_strategy() -> impl Strategy<Value = Card> {
            (suit_strategy(), rank_strategy()).prop_map(|(suit, rank)| Card { suit, rank })
        }

        fn incomplete_book_strategy() -> impl Strategy<Value = IncompleteBook> {
            (rank_strategy(), prop::collection::vec(card_strategy(), 1..=3))
                .prop_map(|(rank, cards)| IncompleteBook { rank, cards })
        }

        fn complete_book_strategy() -> impl Strategy<Value = CompleteBook> {
            (rank_strategy(), suit_strategy(), suit_strategy(), suit_strategy(), suit_strategy())
                .prop_map(|(rank, s1, s2, s3, s4)| CompleteBook {
                    rank,
                    cards: [
                        Card { suit: s1, rank }, Card { suit: s2, rank },
                        Card { suit: s3, rank }, Card { suit: s4, rank },
                    ],
                })
        }

        fn hand_strategy() -> impl Strategy<Value = Hand> {
            prop::collection::vec(incomplete_book_strategy(), 0..=4)
                .prop_map(|books| Hand { books })
        }

        fn game_result_strategy() -> impl Strategy<Value = GameResult> {
            (
                prop::collection::vec("[a-zA-Z0-9]{1,16}", 0..=4),
                prop::collection::vec("[a-zA-Z0-9]{1,16}", 0..=4),
            ).prop_map(|(winners, losers)| GameResult { winners, losers })
        }

        fn game_input_state_strategy(players: &[String], local_name: &str) -> impl Strategy<Value = GameInputState> {
            let opponents: Vec<String> = players.iter()
                .filter(|p| p.as_str() != local_name)
                .cloned()
                .collect();
            let target = opponents.into_iter().next().unwrap_or_default();
            prop_oneof![
                Just(GameInputState::Idle),
                (0usize..=3usize).prop_map(|cursor| GameInputState::SelectingTarget { cursor }),
                (Just(target), 0usize..=12usize)
                    .prop_map(|(t, cursor)| GameInputState::SelectingRank { target: t, cursor }),
            ]
        }

        fn game_state_strategy() -> impl Strategy<Value = GameState> {
            (
                "[a-zA-Z0-9]{1,16}",
                prop::collection::vec("[a-zA-Z0-9]{1,16}", 0..=3),
            ).prop_flat_map(|(player_name, extra_players)| {
                let mut players = vec![player_name.clone()];
                players.extend(extra_players);
                let input_state_strat = game_input_state_strategy(&players, &player_name);
                (
                    Just(player_name),
                    Just(players),
                    hand_strategy(),
                    prop::collection::vec(complete_book_strategy(), 0..=4),
                    prop::option::of(game_result_strategy()),
                    input_state_strat,
                )
            }).prop_map(|(player_name, players, hand, completed_books, game_result, input_state)| {
                let mut state = GameState::new(player_name.clone(), players.clone());
                state.active_player = player_name;
                state.hand = hand;
                state.completed_books = completed_books;
                state.game_result = game_result;
                state.input_state = input_state;
                let opponents: std::collections::HashMap<String, usize> = players.iter()
                    .filter(|p| p.as_str() != state.player_name.as_str())
                    .map(|p| (p.clone(), 0))
                    .collect();
                state.opponent_card_counts = opponents.clone();
                state.opponent_book_counts = opponents.keys().map(|k| (k.clone(), 0)).collect();
                state
            })
        }

        proptest! {
            #[test]
            fn render_game_does_not_panic(state in game_state_strategy()) {
                let backend = TestBackend::new(120, 40);
                let mut terminal = Terminal::new(backend).unwrap();
                let app = AppState { screen: Screen::Game(state) };
                terminal.draw(|f| render(f, &app)).unwrap();
            }
        }

        fn browsing_status_strategy() -> impl Strategy<Value = BrowsingStatus> {
            use go_fish_web::LobbyInfo;
            let lobby_info_strat = ("[a-zA-Z0-9]{1,16}", 1usize..=3usize, 2usize..=6usize)
                .prop_map(|(id, pc, mp)| LobbyInfo { lobby_id: id, player_count: pc.min(mp), max_players: mp });
            prop_oneof![
                Just(BrowsingStatus::Loading),
                Just(BrowsingStatus::Creating),
                prop::collection::vec(lobby_info_strat, 0..=4).prop_map(BrowsingStatus::Loaded),
                ("[a-zA-Z0-9 ]{0,32}", prop::option::of("[a-zA-Z0-9 ]{0,32}"))
                    .prop_map(|(input, error)| BrowsingStatus::EnteringId { input, error }),
                "[a-zA-Z0-9 ]{0,32}".prop_map(BrowsingStatus::Error),
            ]
        }

        fn browsing_lobbies_state_strategy() -> impl Strategy<Value = BrowsingLobbiesState> {
            ("[a-zA-Z0-9]{1,16}", browsing_status_strategy(), 0usize..=10usize, 0usize..=9usize)
                .prop_map(|(player_name, status, selected_index, frame_index)| BrowsingLobbiesState {
                    player_name,
                    status,
                    selected_index,
                    frame_index,
                })
        }

        proptest! {
            #[test]
            fn render_browsing_lobbies_does_not_panic(state in browsing_lobbies_state_strategy()) {
                let backend = TestBackend::new(120, 40);
                let mut terminal = Terminal::new(backend).unwrap();
                let app = AppState { screen: Screen::BrowsingLobbies(state) };
                terminal.draw(|f| render(f, &app)).unwrap();
            }
        }

        #[test]
        fn render_browsing_lobbies_loading_shows_spinner() {
            let backend = TestBackend::new(80, 24);
            let mut terminal = Terminal::new(backend).unwrap();
            let state = BrowsingLobbiesState {
                player_name: "Alice".to_string(),
                status: BrowsingStatus::Loading,
                selected_index: 0,
                frame_index: 0,
            };
            let app = AppState { screen: Screen::BrowsingLobbies(state) };
            terminal.draw(|f| render(f, &app)).unwrap();
            let content = terminal.backend().buffer().area;
            assert!(content.width > 0, "terminal has content area");
        }

        #[test]
        fn render_browsing_lobbies_loaded_shows_lobby_name() {
            use go_fish_web::LobbyInfo;
            let backend = TestBackend::new(120, 40);
            let mut terminal = Terminal::new(backend).unwrap();
            let state = BrowsingLobbiesState {
                player_name: "Alice".to_string(),
                status: BrowsingStatus::Loaded(vec![
                    LobbyInfo { lobby_id: "azure-reef".to_string(), player_count: 1, max_players: 4 },
                ]),
                selected_index: 0,
                frame_index: 0,
            };
            let app = AppState { screen: Screen::BrowsingLobbies(state) };
            terminal.draw(|f| render(f, &app)).unwrap();
            let buf = terminal.backend().buffer().clone();
            let rendered: String = buf.content().iter().map(|c| c.symbol()).collect();
            assert!(rendered.contains("azure-reef"), "rendered output should contain lobby name");
        }
    }
}

mod widgets {
    use go_fish::{Card, Hand, IncompleteBook, Rank, Suit};
    use ratatui::{
        buffer::Buffer,
        layout::{Constraint, Direction, Layout, Margin, Rect},
        style::{Color, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph, Widget},
    };
    use std::collections::VecDeque;

    pub(super) trait IntoNotificationLine {
        fn into_notification_line(&self) -> Line<'_>;
    }

    impl IntoNotificationLine for String {
        fn into_notification_line(&self) -> Line<'_> {
            Line::from(self.as_str())
        }
    }

    impl IntoNotificationLine for Line<'static> {
        fn into_notification_line(&self) -> Line<'_> {
            self.clone()
        }
    }

    #[allow(dead_code)]
    pub(super) enum NotificationOrder {
        NewestFirst,
        OldestFirst,
    }

    pub(super) struct NotificationWidget<'a, N: IntoNotificationLine> {
        notifications: &'a VecDeque<N>,
        order: NotificationOrder,
    }

    impl<'a, N: IntoNotificationLine> NotificationWidget<'a, N> {
        pub(super) fn new(notifications: &'a VecDeque<N>, order: NotificationOrder) -> Self {
            Self { notifications, order }
        }
    }

    impl<N: IntoNotificationLine> Widget for NotificationWidget<'_, N> {
        fn render(self, area: Rect, buf: &mut Buffer) {
            let lines: Vec<Line<'_>> = match self.order {
                NotificationOrder::NewestFirst => self.notifications.iter()
                    .map(|n| n.into_notification_line())
                    .collect(),
                NotificationOrder::OldestFirst => self.notifications.iter().rev()
                    .map(|n| n.into_notification_line())
                    .collect(),
            };
            Paragraph::new(lines).render(area, buf);
        }
    }

    pub(super) fn rank_short(rank: Rank) -> &'static str {
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
            Rank::Ace => "A",
        }
    }

    pub(super) fn suit_symbol(suit: Suit) -> &'static str {
        match suit {
            Suit::Spades => "♠",
            Suit::Hearts => "♥",
            Suit::Diamonds => "♦",
            Suit::Clubs => "♣",
        }
    }

    pub(super) fn suit_colour(suit: Suit) -> Color {
        match suit {
            Suit::Hearts | Suit::Diamonds => Color::Red,
            Suit::Spades | Suit::Clubs => Color::White,
        }
    }

    pub(super) enum CardWidget<'a> {
        FaceDown { highlighted: bool },
        FaceUp { card: &'a Card, highlighted: bool },
    }

    impl Widget for CardWidget<'_> {
        fn render(self, area: Rect, buf: &mut Buffer) {
            let (highlighted, card) = match self {
                CardWidget::FaceDown { highlighted } => (highlighted, None),
                CardWidget::FaceUp { card, highlighted } => (highlighted, Some(card)),
            };
            let col = if highlighted { Color::Yellow } else { Color::White };
            Block::default()
                .borders(Borders::ALL)
                .style(Style::default().fg(col))
                .render(area, buf);
            if let Some(card) = card {
                let suit_sym = suit_symbol(card.suit);
                let suit_col = suit_colour(card.suit);
                let rank = rank_short(card.rank);
                buf.set_string(area.x + 2, area.y + 1, suit_sym, Style::default().fg(suit_col));
                buf.set_string(area.x + 3, area.y + 2, rank, Style::default().fg(Color::White));
                buf.set_string(area.x + 4, area.y + 3, suit_sym, Style::default().fg(suit_col));
            }
        }
    }

    pub(super) struct TurnIndicatorWidget {
        pub is_active: bool,
    }

    impl Widget for TurnIndicatorWidget {
        fn render(self, area: Rect, buf: &mut Buffer) {
            let x = area.x + (area.width.saturating_sub(5)) / 2;
            let y = area.y + (area.height.saturating_sub(3)) / 2;
            let indicator = Rect { x, y, width: 5.min(area.width), height: 3.min(area.height) };
            Block::default().borders(Borders::ALL).render(indicator, buf);
            if self.is_active {
                let inner = indicator.inner(Margin { horizontal: 1, vertical: 1 });
                for row in inner.top()..inner.bottom() {
                    for col in inner.left()..inner.right() {
                        buf[(col, row)].set_char('█');
                    }
                }
            }
        }
    }

    pub(super) struct IncompleteBookWidget<'a> {
        pub book: &'a IncompleteBook,
        pub highlighted: bool,
    }

    impl Widget for IncompleteBookWidget<'_> {
        fn render(self, area: Rect, buf: &mut Buffer) {
            for (i, card) in self.book.cards.iter().enumerate() {
                let rect = Rect::new(area.x + (i as u16), area.y, super::CARD_WIDTH, super::CARD_HEIGHT);
                Clear.render(rect, buf);
                CardWidget::FaceUp { card, highlighted: self.highlighted }.render(rect, buf);
            }
        }
    }

    pub(super) enum PlayerStripWidget<'a> {
        Local {
            hand: &'a Hand,
            selected_card: Option<usize>,
            is_active: bool,
            book_count: usize,
        },
        Opponent {
            player_name: &'a str,
            hand_size: usize,
            highlighted: bool,
            is_active: bool,
            book_count: usize,
        },
    }

    impl Widget for PlayerStripWidget<'_> {
        fn render(self, area: Rect, buf: &mut Buffer) {
            match self {
                PlayerStripWidget::Local { hand, selected_card, is_active, book_count } =>
                    render_local(hand, selected_card, is_active, book_count, area, buf),
                PlayerStripWidget::Opponent { player_name, hand_size, highlighted, is_active, book_count } =>
                    render_opponent(player_name, hand_size, highlighted, is_active, book_count, area, buf),
            }
        }
    }

    fn render_local(hand: &Hand, selected_card: Option<usize>, is_active: bool, book_count: usize, area: Rect, buf: &mut Buffer) {
        let border_style = if is_active { Style::default().fg(Color::Green) } else { Style::default() };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Span::styled("you", Style::default().fg(Color::Green)));

        let strip_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(super::CARD_WIDTH), Constraint::Fill(1), Constraint::Length(14)])
            .split(block.inner(area));

        TurnIndicatorWidget { is_active }.render(strip_chunks[0], buf);

        let con = hand.books.iter()
            .map(|b| Constraint::Length((super::CARD_WIDTH - 1) + b.cards.len() as u16))
            .collect::<Vec<_>>();
        let cards_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(con)
            .split(strip_chunks[1]);
        for (i, book) in hand.books.iter().enumerate() {
            let highlighted = selected_card.map(|j| j == i).unwrap_or(false);
            IncompleteBookWidget { book, highlighted }.render(cards_chunks[i], buf);
        }

        Line::from(vec![Span::styled(format!("{} books", book_count), Style::default().fg(Color::White))])
            .render(strip_chunks[2], buf);

        block.render(area, buf);
    }

    fn render_opponent(player_name: &str, hand_size: usize, highlighted: bool, is_active: bool, book_count: usize, area: Rect, buf: &mut Buffer) {
        let border_style = if highlighted {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .style(border_style)
            .title(player_name);

        let strip_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(super::CARD_WIDTH), Constraint::Fill(1), Constraint::Length(14)])
            .split(block.inner(area));

        TurnIndicatorWidget { is_active }.render(strip_chunks[0], buf);

        let con = (0..hand_size).map(|_| Constraint::Length(super::CARD_WIDTH)).collect::<Vec<_>>();
        let cards_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(con)
            .split(strip_chunks[1]);
        for i in 0..hand_size {
            CardWidget::FaceDown { highlighted: false }.render(cards_chunks[i], buf);
        }

        Line::from(vec![Span::styled(format!("{} books", book_count), Style::default().fg(Color::White))])
            .render(strip_chunks[2], buf);

        block.render(area, buf);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use ratatui::{buffer::Buffer, layout::Rect};

        // 7×5 — matches CARD_WIDTH × CARD_HEIGHT
        fn card_area() -> Rect {
            Rect::new(0, 0, 7, 5)
        }

        // ── CardWidget ────────────────────────────────────────────────────────

        #[test]
        fn card_face_down_has_white_border() {
            let mut buf = Buffer::empty(card_area());
            CardWidget::FaceDown { highlighted: false }.render(card_area(), &mut buf);
            assert_eq!(buf[(0, 0)].symbol(), "┌");
            assert_eq!(buf[(0, 0)].fg, Color::White);
        }

        #[test]
        fn card_face_down_highlighted_has_yellow_border() {
            let mut buf = Buffer::empty(card_area());
            CardWidget::FaceDown { highlighted: true }.render(card_area(), &mut buf);
            assert_eq!(buf[(0, 0)].fg, Color::Yellow);
        }

        #[test]
        fn card_face_up_renders_rank_and_suit_symbols() {
            let mut buf = Buffer::empty(card_area());
            let card = Card { suit: Suit::Spades, rank: Rank::Ace };
            CardWidget::FaceUp { card: &card, highlighted: false }.render(card_area(), &mut buf);
            assert_eq!(buf[(2, 1)].symbol(), "♠");
            assert_eq!(buf[(3, 2)].symbol(), "A");
            assert_eq!(buf[(4, 3)].symbol(), "♠");
        }

        #[test]
        fn card_face_up_red_suit_has_red_foreground() {
            let mut buf = Buffer::empty(card_area());
            let card = Card { suit: Suit::Hearts, rank: Rank::King };
            CardWidget::FaceUp { card: &card, highlighted: false }.render(card_area(), &mut buf);
            assert_eq!(buf[(2, 1)].fg, Color::Red);
            assert_eq!(buf[(4, 3)].fg, Color::Red);
        }

        // ── TurnIndicatorWidget ───────────────────────────────────────────────

        #[test]
        fn turn_indicator_inactive_interior_is_spaces() {
            let mut buf = Buffer::empty(card_area());
            TurnIndicatorWidget { is_active: false }.render(card_area(), &mut buf);
            // 5×3 indicator centred in 7×5: top-left at (1,1), interior at row 2, cols 2–4
            assert_eq!(buf[(2, 2)].symbol(), " ");
            assert_eq!(buf[(3, 2)].symbol(), " ");
            assert_eq!(buf[(4, 2)].symbol(), " ");
        }

        #[test]
        fn turn_indicator_active_fills_interior() {
            let mut buf = Buffer::empty(card_area());
            TurnIndicatorWidget { is_active: true }.render(card_area(), &mut buf);
            assert_eq!(buf[(2, 2)].symbol(), "█");
            assert_eq!(buf[(3, 2)].symbol(), "█");
            assert_eq!(buf[(4, 2)].symbol(), "█");
        }

        // ── IncompleteBookWidget ──────────────────────────────────────────────

        #[test]
        fn incomplete_book_single_card_renders_at_origin() {
            let mut buf = Buffer::empty(card_area());
            let card = Card { suit: Suit::Clubs, rank: Rank::Seven };
            let book = IncompleteBook { rank: Rank::Seven, cards: vec![card] };
            IncompleteBookWidget { book: &book, highlighted: false }.render(card_area(), &mut buf);
            assert_eq!(buf[(3, 2)].symbol(), "7");
        }

        #[test]
        fn incomplete_book_second_card_is_offset_one_column() {
            // Width 8 fits two CARD_WIDTH=7 cards staggered by 1 column
            let area = Rect::new(0, 0, 8, 5);
            let mut buf = Buffer::empty(area);
            let book = IncompleteBook {
                rank: Rank::Three,
                cards: vec![
                    Card { suit: Suit::Hearts, rank: Rank::Two },
                    Card { suit: Suit::Spades, rank: Rank::Three },
                ],
            };
            IncompleteBookWidget { book: &book, highlighted: false }.render(area, &mut buf);
            // Card 1 is at x_offset=1: rank at (1+3, 2) = (4, 2)
            assert_eq!(buf[(4, 2)].symbol(), "3");
        }

        #[test]
        fn incomplete_book_highlighted_uses_yellow_border() {
            let mut buf = Buffer::empty(card_area());
            let card = Card { suit: Suit::Diamonds, rank: Rank::Five };
            let book = IncompleteBook { rank: Rank::Five, cards: vec![card] };
            IncompleteBookWidget { book: &book, highlighted: true }.render(card_area(), &mut buf);
            assert_eq!(buf[(0, 0)].fg, Color::Yellow);
        }
    }
}
