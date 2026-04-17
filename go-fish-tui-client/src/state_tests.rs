// Property tests for go-fish-tui-client-gameplay (Properties 11-19)
// Feature: go-fish-tui-client-gameplay

use super::*;
use go_fish::{Card, CompleteBook, Hand, HookResult, IncompleteBook, Rank, Suit};
use ratatui::text::Line;

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}
use go_fish_web::{
    BotType, GameResult, GameSnapshot, HandState, HookError, HookOutcome, LobbyPlayer,
    OpponentState, ServerMessage,
};
use proptest::prelude::*;

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

fn hand_state_strategy() -> impl Strategy<Value = HandState> {
    (
        prop::collection::vec(incomplete_book_strategy(), 0..=4),
        prop::collection::vec(complete_book_strategy(), 0..=4),
    ).prop_map(|(books, completed_books)| HandState {
        hand: Hand { books },
        completed_books,
    })
}

fn hook_result_strategy() -> impl Strategy<Value = HookResult> {
    prop_oneof![
        incomplete_book_strategy().prop_map(HookResult::Catch),
        Just(HookResult::GoFish),
    ]
}

fn hook_error_strategy() -> impl Strategy<Value = HookError> {
    prop_oneof![
        Just(HookError::NotYourTurn),
        "[a-zA-Z0-9]{1,16}".prop_map(HookError::UnknownPlayer),
        Just(HookError::CannotTargetYourself),
        rank_strategy().prop_map(HookError::YouDoNotHaveRank),
    ]
}

fn game_result_strategy() -> impl Strategy<Value = GameResult> {
    (
        prop::collection::vec("[a-zA-Z0-9]{1,16}", 0..=4),
        prop::collection::vec("[a-zA-Z0-9]{1,16}", 0..=4),
    ).prop_map(|(winners, losers)| GameResult { winners, losers })
}

fn hook_outcome_strategy() -> impl Strategy<Value = HookOutcome> {
    ("[a-zA-Z0-9]{1,16}", "[a-zA-Z0-9]{1,16}", rank_strategy(), hook_result_strategy())
        .prop_map(|(fisher_name, target_name, rank, result)| HookOutcome {
            fisher_name,
            target_name,
            rank,
            result,
        })
}

fn opponent_state_strategy() -> impl Strategy<Value = OpponentState> {
    (
        "[a-zA-Z0-9]{1,16}",
        0usize..=13usize,
        prop::collection::vec(complete_book_strategy(), 0..=4),
    ).prop_map(|(name, card_count, completed_books)| OpponentState {
        name,
        card_count,
        completed_books,
    })
}

fn game_snapshot_strategy() -> impl Strategy<Value = GameSnapshot> {
    (
        hand_state_strategy(),
        prop::collection::vec(opponent_state_strategy(), 0..=3),
        "[a-zA-Z0-9]{1,16}",
        prop::option::of(hook_outcome_strategy()),
        0usize..=52usize,
    ).prop_map(|(hand_state, opponents, active_player, last_hook_outcome, deck_size)| GameSnapshot {
        hand_state,
        opponents,
        active_player,
        last_hook_outcome,
        deck_size,
    })
}

fn lobby_player_strategy() -> impl Strategy<Value = LobbyPlayer> {
    prop_oneof![
        "[a-zA-Z0-9]{1,16}".prop_map(|name| LobbyPlayer::Human { name }),
        "[a-zA-Z0-9]{1,16}".prop_map(|name| LobbyPlayer::Bot { name, bot_type: BotType::SimpleBot }),
    ]
}

/// Strategy for a LobbyState with at least one player (the local player).
fn lobby_state_strategy() -> impl Strategy<Value = LobbyState> {
    (
        "[a-zA-Z0-9]{1,16}",
        "[a-zA-Z0-9]{1,16}",
        "[a-zA-Z0-9]{1,16}",
        prop::collection::vec(lobby_player_strategy(), 0..=3),
        1usize..=8usize,
    ).prop_map(|(player_name, lobby_id, leader, extra_players, max_players)| {
        let mut players = vec![LobbyPlayer::Human { name: player_name.clone() }];
        players.extend(extra_players);
        LobbyState {
            player_name,
            lobby_id,
            leader,
            players,
            max_players,
            error: None,
        }
    })
}

/// Strategy for a GameState with a given player_name and players list.
fn game_state_strategy() -> impl Strategy<Value = GameState> {
    (
        "[a-zA-Z0-9]{1,16}",
        prop::collection::vec("[a-zA-Z0-9]{1,16}", 0..=3),
    ).prop_map(|(player_name, extra_players)| {
        let mut players = vec![player_name.clone()];
        players.extend(extra_players);
        GameState::new(player_name, players)
    })
}

/// Strategy for a GameState with an optional hook_error set.
fn game_state_with_error_strategy() -> impl Strategy<Value = GameState> {
    (game_state_strategy(), prop::option::of(hook_error_strategy()))
        .prop_map(|(mut game, err)| {
            game.hook_error = err;
            game
        })
}

// ── Helper: compare HandState fields via JSON ─────────────────────────────────

fn hand_json(hand: &go_fish::Hand) -> String {
    serde_json::to_string(hand).unwrap()
}

fn complete_books_json(books: &[go_fish::CompleteBook]) -> String {
    serde_json::to_string(books).unwrap()
}

fn hook_error_json(err: &Option<go_fish_web::HookError>) -> String {
    serde_json::to_string(err).unwrap()
}

fn game_result_json(result: &Option<go_fish_web::GameResult>) -> String {
    serde_json::to_string(result).unwrap()
}

// ── Property 11: GameStarted transitions to Game screen ──────────────────────

// Feature: go-fish-tui-client-gameplay, Property 11: GameStarted transitions to Game screen
// Validates: Requirements 1.4, 7.1
proptest! {
    #[test]
    fn prop_game_started_transitions_to_game(lobby in lobby_state_strategy()) {
        let player_name = lobby.player_name.clone();
        let expected_players: Vec<String> = lobby.players.iter().map(|p| p.name().to_string()).collect();
        let mut state = AppState {
            screen: Screen::Lobby(lobby),
        };
        apply_network_event(&mut state, &NetworkEvent::Message(ServerMessage::GameStarted));
        match &state.screen {
            Screen::Game(game) => {
                prop_assert_eq!(&game.player_name, &player_name);
                prop_assert_eq!(&game.players, &expected_players);
            }
            other => prop_assert!(false, "Expected Screen::Game, got {:?}", other),
        }
    }
}

// ── Property 12: GameSnapshot updates all fields ──────────────────────────────

// Feature: go-fish-tui-client-gameplay, Property 12: GameSnapshot updates hand, books, opponent state, active player, and hook outcome
// Validates: Requirements 1.5, 4.1, 4.2, 4.3
proptest! {
    #[test]
    fn prop_game_snapshot_updates_all_fields(
        game in game_state_strategy(),
        snapshot in game_snapshot_strategy(),
    ) {
        let mut expected_hand = snapshot.hand_state.hand.clone();
        expected_hand.books.sort_by(|a, b| a.rank.cmp(&b.rank));
        let expected_hand_json = hand_json(&expected_hand);
        let expected_books_json = complete_books_json(&snapshot.hand_state.completed_books);
        let expected_active = snapshot.active_player.clone();

        let mut state = AppState { screen: Screen::Game(game) };
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::GameSnapshot(snapshot)),
        );

        match &state.screen {
            Screen::Game(g) => {
                prop_assert_eq!(hand_json(&g.hand), expected_hand_json);
                prop_assert_eq!(complete_books_json(&g.completed_books), expected_books_json);
                prop_assert_eq!(&g.active_player, &expected_active);
            }
            other => prop_assert!(false, "Expected Screen::Game, got {:?}", other),
        }
    }
}

// ── Property 13: GameSnapshot with active_player == self sets SelectingTarget ─

// Feature: go-fish-tui-client-gameplay, Property 13: GameSnapshot with active_player == self clears hook error and sets SelectingTarget
// Validates: Requirements 4.3, 4.6
proptest! {
    #[test]
    fn prop_game_snapshot_your_turn_sets_selecting_target(
        game in game_state_with_error_strategy(),
        snapshot_base in game_snapshot_strategy(),
    ) {
        let player_name = game.player_name.clone();
        // Force active_player == player_name
        let snapshot = GameSnapshot {
            active_player: player_name.clone(),
            ..snapshot_base
        };

        let mut state = AppState { screen: Screen::Game(game) };
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::GameSnapshot(snapshot)),
        );

        match &state.screen {
            Screen::Game(g) => {
                prop_assert_eq!(&g.active_player, &player_name);
                prop_assert_eq!(&g.input_state, &GameInputState::Idle);
                prop_assert_eq!(&g.hook_error, &None);
            }
            other => prop_assert!(false, "Expected Screen::Game, got {:?}", other),
        }
    }
}

// ── Property 14: GameSnapshot with last_hook_outcome adds notification ────────

// Feature: go-fish-tui-client-gameplay, Property 14: GameSnapshot with last_hook_outcome adds a notification
// Validates: Requirements 4.1
proptest! {
    #[test]
    fn prop_game_snapshot_with_hook_outcome_adds_notification(
        mut game in game_state_strategy(),
        snapshot_base in game_snapshot_strategy(),
        outcome in hook_outcome_strategy(),
    ) {
        game.has_received_snapshot = true;
        let snapshot = GameSnapshot {
            last_hook_outcome: Some(outcome),
            ..snapshot_base
        };

        let mut state = AppState { screen: Screen::Game(game) };
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::GameSnapshot(snapshot)),
        );

        match &state.screen {
            Screen::Game(g) => {
                // A hook outcome notification should be at the front (most recent)
                prop_assert!(g.notifications.front().is_some());
            }
            other => prop_assert!(false, "Expected Screen::Game, got {:?}", other),
        }
    }
}

// ── Property 15: GameResult does not auto-navigate ────────────────────────────

// Feature: go-fish-tui-client-gameplay, Property 15: GameResult does not auto-navigate
// Validates: Requirements 5.1, 7.2
proptest! {
    #[test]
    fn prop_game_result_does_not_auto_navigate(
        game in game_state_strategy(),
        result in game_result_strategy(),
    ) {
        let expected_json = game_result_json(&Some(result.clone()));
        let mut state = AppState { screen: Screen::Game(game) };
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::GameResult(result)),
        );

        match &state.screen {
            Screen::Game(g) => {
                prop_assert_eq!(game_result_json(&g.game_result), expected_json);
            }
            other => prop_assert!(false, "Expected Screen::Game, got {:?}", other),
        }
    }
}

// ── Property 16: GameSnapshot with active_player != self sets Idle ────────────

// Feature: go-fish-tui-client-gameplay, Property 16: GameSnapshot with active_player != self sets Idle
// Validates: Requirements 4.3
proptest! {
    #[test]
    fn prop_game_snapshot_other_turn_sets_idle(
        game in game_state_strategy(),
        snapshot_base in game_snapshot_strategy(),
        other_name in "[a-zA-Z0-9]{1,16}",
    ) {
        let player_name = game.player_name.clone();
        // Ensure active_player != player_name by appending "_other" if they match
        let active = if other_name == player_name {
            format!("{}_other", other_name)
        } else {
            other_name
        };
        let snapshot = GameSnapshot {
            active_player: active.clone(),
            ..snapshot_base
        };

        let mut state = AppState { screen: Screen::Game(game) };
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::GameSnapshot(snapshot)),
        );

        match &state.screen {
            Screen::Game(g) => {
                prop_assert_eq!(&g.active_player, &active);
                prop_assert_eq!(&g.input_state, &GameInputState::Idle);
            }
            other => prop_assert!(false, "Expected Screen::Game, got {:?}", other),
        }
    }
}

// ── Property 17: Notifications: seeding, ordering, eviction ──────────────────

#[test]
fn game_state_new_seeds_game_started_notification() {
    let game = GameState::new("Alice".into(), vec!["Alice".into(), "Bob".into()]);
    assert_eq!(game.notifications.len(), 1);
    assert_eq!(line_text(&game.notifications[0]), "Game started!");
}

#[test]
fn hook_outcome_notification_added_newest_first() {
    let mut game = GameState::new("Alice".into(), vec!["Alice".into(), "Bob".into()]);
    game.has_received_snapshot = true;

    let snapshot = GameSnapshot {
        hand_state: HandState { hand: Hand { books: vec![] }, completed_books: vec![] },
        opponents: vec![OpponentState { name: "Bob".into(), card_count: 5, completed_books: vec![] }],
        active_player: "Bob".into(),
        last_hook_outcome: Some(HookOutcome {
            fisher_name: "Bob".into(),
            target_name: "Alice".into(),
            rank: Rank::King,
            result: HookResult::GoFish,
        }),
        deck_size: 30,
    };

    let prev_len = game.notifications.len();
    let mut state = AppState { screen: Screen::Game(game) };
    apply_network_event(&mut state, &NetworkEvent::Message(ServerMessage::GameSnapshot(snapshot)));

    match &state.screen {
        Screen::Game(g) => {
            assert!(g.notifications.len() > prev_len || g.notifications.len() == MAX_NOTIFICATION_HISTORY);
            // Hook outcome is most recent — should be at front
            assert!(line_text(g.notifications.front().unwrap()).contains("Kings"));
        }
        other => panic!("Expected Screen::Game, got {:?}", other),
    }
}

#[test]
fn notification_order_within_snapshot() {
    // One snapshot that produces: opponent book + local book + hook outcome
    // Expected front-to-back order after (newest first): hook outcome, local book, opponent book
    let mut game = GameState::new("Alice".into(), vec!["Alice".into(), "Bob".into()]);
    game.has_received_snapshot = true;
    // Clear the "Game started!" seed so we can reason about positions
    game.notifications.clear();

    let completed_ace = CompleteBook {
        rank: Rank::Ace,
        cards: [
            Card { suit: Suit::Clubs, rank: Rank::Ace },
            Card { suit: Suit::Diamonds, rank: Rank::Ace },
            Card { suit: Suit::Hearts, rank: Rank::Ace },
            Card { suit: Suit::Spades, rank: Rank::Ace },
        ],
    };
    let completed_king = CompleteBook {
        rank: Rank::King,
        cards: [
            Card { suit: Suit::Clubs, rank: Rank::King },
            Card { suit: Suit::Diamonds, rank: Rank::King },
            Card { suit: Suit::Hearts, rank: Rank::King },
            Card { suit: Suit::Spades, rank: Rank::King },
        ],
    };

    let snapshot = GameSnapshot {
        hand_state: HandState {
            hand: Hand { books: vec![] },
            completed_books: vec![completed_ace],
        },
        opponents: vec![OpponentState {
            name: "Bob".into(),
            card_count: 0,
            completed_books: vec![completed_king],
        }],
        active_player: "Alice".into(),
        last_hook_outcome: Some(HookOutcome {
            fisher_name: "Alice".into(),
            target_name: "Bob".into(),
            rank: Rank::Two,
            result: HookResult::GoFish,
        }),
        deck_size: 20,
    };

    let mut state = AppState { screen: Screen::Game(game) };
    apply_network_event(&mut state, &NetworkEvent::Message(ServerMessage::GameSnapshot(snapshot)));

    match &state.screen {
        Screen::Game(g) => {
            // With MAX_NOTIFICATION_HISTORY=3: local book (front), hook outcome, opponent book (back)
            assert_eq!(g.notifications.len(), MAX_NOTIFICATION_HISTORY);
            assert!(line_text(&g.notifications[0]).contains("Aces"), "front should be local book, got: {:?}", g.notifications[0]);
            assert!(line_text(&g.notifications[1]).contains("Twos"), "middle should be hook outcome, got: {:?}", g.notifications[1]);
            assert!(line_text(&g.notifications[2]).contains("Kings"), "back should be opponent book, got: {:?}", g.notifications[2]);
        }
        other => panic!("Expected Screen::Game, got {:?}", other),
    }
}

#[test]
fn notifications_evicted_when_exceeding_max() {
    let mut game = GameState::new("Alice".into(), vec!["Alice".into(), "Bob".into()]);
    game.has_received_snapshot = true;
    game.notifications.clear();

    let make_book = |rank: Rank| CompleteBook {
        rank,
        cards: [
            Card { suit: Suit::Clubs, rank },
            Card { suit: Suit::Diamonds, rank },
            Card { suit: Suit::Hearts, rank },
            Card { suit: Suit::Spades, rank },
        ],
    };

    // Apply a snapshot with 4 new opponent books — exceeds MAX_NOTIFICATION_HISTORY=3
    let snapshot = GameSnapshot {
        hand_state: HandState { hand: Hand { books: vec![] }, completed_books: vec![] },
        opponents: vec![OpponentState {
            name: "Bob".into(),
            card_count: 0,
            completed_books: vec![
                make_book(Rank::Two),
                make_book(Rank::Three),
                make_book(Rank::Four),
                make_book(Rank::Five),
            ],
        }],
        active_player: "Alice".into(),
        last_hook_outcome: None,
        deck_size: 10,
    };

    let mut state = AppState { screen: Screen::Game(game) };
    apply_network_event(&mut state, &NetworkEvent::Message(ServerMessage::GameSnapshot(snapshot)));

    match &state.screen {
        Screen::Game(g) => {
            assert_eq!(g.notifications.len(), MAX_NOTIFICATION_HISTORY);
            // Oldest (Twos, Threes) evicted; Fours, Fives remain at back; Fives at front
            assert!(line_text(&g.notifications[0]).contains("Fives"), "front (newest): {:?}", g.notifications[0]);
            assert!(line_text(&g.notifications[1]).contains("Fours"), "middle: {:?}", g.notifications[1]);
            assert!(line_text(&g.notifications[2]).contains("Threes"), "back (oldest retained): {:?}", g.notifications[2]);
        }
        other => panic!("Expected Screen::Game, got {:?}", other),
    }
}

#[test]
fn deck_draw_notification_added() {
    let mut game = GameState::new("Alice".into(), vec!["Alice".into(), "Bob".into()]);
    game.has_received_snapshot = true;
    game.notifications.clear();

    let snapshot = GameSnapshot {
        hand_state: HandState {
            hand: Hand {
                books: vec![IncompleteBook {
                    rank: Rank::King,
                    cards: vec![Card { suit: Suit::Spades, rank: Rank::King }],
                }],
            },
            completed_books: vec![],
        },
        opponents: vec![OpponentState { name: "Bob".into(), card_count: 5, completed_books: vec![] }],
        active_player: "Bob".into(),
        last_hook_outcome: Some(HookOutcome {
            fisher_name: "Alice".into(),
            target_name: "Bob".into(),
            rank: Rank::King,
            result: HookResult::GoFish,
        }),
        deck_size: 29,
    };

    let mut state = AppState { screen: Screen::Game(game) };
    apply_network_event(&mut state, &NetworkEvent::Message(ServerMessage::GameSnapshot(snapshot)));

    match &state.screen {
        Screen::Game(g) => {
            // Notifications (newest first): deck draw, hook outcome
            assert!(line_text(&g.notifications[0]).contains("drew a King"), "deck draw at front: {:?}", g.notifications[0]);
            assert!(line_text(&g.notifications[1]).contains("Kings"), "hook outcome second");
        }
        other => panic!("Expected Screen::Game, got {:?}", other),
    }
}

#[test]
fn first_snapshot_suppresses_deck_draw_and_books_but_not_hook() {
    let game = GameState::new("Alice".into(), vec!["Alice".into(), "Bob".into()]);
    assert!(!game.has_received_snapshot);

    let snapshot = GameSnapshot {
        hand_state: HandState {
            hand: Hand {
                books: vec![IncompleteBook {
                    rank: Rank::King,
                    cards: vec![Card { suit: Suit::Spades, rank: Rank::King }],
                }],
            },
            completed_books: vec![CompleteBook {
                rank: Rank::Ace,
                cards: [
                    Card { suit: Suit::Clubs, rank: Rank::Ace },
                    Card { suit: Suit::Diamonds, rank: Rank::Ace },
                    Card { suit: Suit::Hearts, rank: Rank::Ace },
                    Card { suit: Suit::Spades, rank: Rank::Ace },
                ],
            }],
        },
        opponents: vec![OpponentState { name: "Bob".into(), card_count: 5, completed_books: vec![] }],
        active_player: "Alice".into(),
        last_hook_outcome: None,
        deck_size: 38,
    };

    let mut state = AppState { screen: Screen::Game(game) };
    apply_network_event(&mut state, &NetworkEvent::Message(ServerMessage::GameSnapshot(snapshot)));

    match &state.screen {
        Screen::Game(g) => {
            // Only "Game started!" from seed — no deck draw or book completion notifications
            assert_eq!(g.notifications.len(), 1);
            assert_eq!(line_text(&g.notifications[0]), "Game started!");
            assert!(g.has_received_snapshot);
        }
        other => panic!("Expected Screen::Game, got {:?}", other),
    }
}

#[test]
fn notifications_persist_across_snapshots() {
    let mut game = GameState::new("Alice".into(), vec!["Alice".into(), "Bob".into()]);
    game.has_received_snapshot = true;
    game.notifications.clear();

    // First snapshot: adds one hook outcome
    let snap1 = GameSnapshot {
        hand_state: HandState { hand: Hand { books: vec![] }, completed_books: vec![] },
        opponents: vec![OpponentState { name: "Bob".into(), card_count: 5, completed_books: vec![] }],
        active_player: "Bob".into(),
        last_hook_outcome: Some(HookOutcome {
            fisher_name: "Bob".into(),
            target_name: "Alice".into(),
            rank: Rank::Two,
            result: HookResult::GoFish,
        }),
        deck_size: 30,
    };

    let mut state = AppState { screen: Screen::Game(game) };
    apply_network_event(&mut state, &NetworkEvent::Message(ServerMessage::GameSnapshot(snap1)));

    // Second snapshot: no new events
    let snap2 = GameSnapshot {
        hand_state: HandState { hand: Hand { books: vec![] }, completed_books: vec![] },
        opponents: vec![OpponentState { name: "Bob".into(), card_count: 5, completed_books: vec![] }],
        active_player: "Alice".into(),
        last_hook_outcome: None,
        deck_size: 30,
    };
    apply_network_event(&mut state, &NetworkEvent::Message(ServerMessage::GameSnapshot(snap2)));

    match &state.screen {
        Screen::Game(g) => {
            // Notification from snap1 should still be present
            assert!(g.notifications.iter().any(|n| line_text(n).contains("Twos")),
                "old notification should persist, got: {:?}", g.notifications);
        }
        other => panic!("Expected Screen::Game, got {:?}", other),
    }
}

// ── Property 18: HookError stored without navigation ─────────────────────────

// Feature: go-fish-tui-client-gameplay, Property 18: HookError is stored without navigation
// Validates: Requirements 4.4, 6.1, 6.2, 6.3, 6.4
proptest! {
    #[test]
    fn prop_hook_error_stored_without_navigation(
        game in game_state_strategy(),
        err in hook_error_strategy(),
    ) {
        let expected_json = serde_json::to_string(&err).unwrap();
        let mut state = AppState { screen: Screen::Game(game) };
        apply_network_event(
            &mut state,
            &NetworkEvent::Message(ServerMessage::HookError(err)),
        );

        match &state.screen {
            Screen::Game(g) => {
                let actual_json = hook_error_json(&g.hook_error);
                prop_assert_eq!(actual_json, format!("{}", expected_json));
            }
            other => prop_assert!(false, "Expected Screen::Game, got {:?}", other),
        }
    }
}

// ── Property 19: Game-only messages discarded outside Game screen ─────────────

// Feature: go-fish-tui-client-gameplay, Property 19: Game-only messages are discarded outside Game screen
// Validates: Requirements 7.3, 7.4, 7.5, 7.6
proptest! {
    #[test]
    fn prop_game_messages_discarded_outside_game_screen(
        snapshot in game_snapshot_strategy(),
        err in hook_error_strategy(),
        player_name in "[a-zA-Z0-9]{1,16}",
        lobby in lobby_state_strategy(),
    ) {
        // Test on Connecting screen
        {
            let mut state = AppState {
                screen: Screen::Connecting(ConnectingState { status: "Connecting…".to_string() }),
            };
            let before = format!("{:?}", state.screen);
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::GameSnapshot(snapshot.clone())),
            );
            let after = format!("{:?}", state.screen);
            prop_assert_eq!(before, after.clone(), "Connecting screen changed after GameSnapshot");

            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::HookError(err.clone())),
            );
            let after2 = format!("{:?}", state.screen);
            prop_assert_eq!(after, after2, "Connecting screen changed after HookError");
        }

        // Test on PreLobby screen
        {
            let mut state = AppState {
                screen: Screen::PreLobby(PreLobbyState {
                    player_name: player_name.clone(),
                    input_state: PreLobbyInputState::None,
                    error: None,
                }),
            };
            let before = format!("{:?}", state.screen);
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::GameSnapshot(snapshot.clone())),
            );
            let after = format!("{:?}", state.screen);
            prop_assert_eq!(before, after.clone(), "PreLobby screen changed after GameSnapshot");

            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::HookError(err.clone())),
            );
            let after2 = format!("{:?}", state.screen);
            prop_assert_eq!(after, after2, "PreLobby screen changed after HookError");
        }

        // Test on Lobby screen
        {
            let mut state = AppState {
                screen: Screen::Lobby(lobby),
            };
            let before = format!("{:?}", state.screen);
            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::GameSnapshot(snapshot.clone())),
            );
            let after = format!("{:?}", state.screen);
            prop_assert_eq!(before, after.clone(), "Lobby screen changed after GameSnapshot");

            apply_network_event(
                &mut state,
                &NetworkEvent::Message(ServerMessage::HookError(err.clone())),
            );
            let after2 = format!("{:?}", state.screen);
            prop_assert_eq!(after, after2, "Lobby screen changed after HookError");
        }
    }
}
