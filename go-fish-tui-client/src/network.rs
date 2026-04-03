use go_fish_web::ServerMessage;

/// Network events produced by the network task and consumed by the event loop.
#[derive(Debug, Clone)]
pub enum NetworkEvent {
    /// A successfully deserialised ServerMessage from the server.
    Message(ServerMessage),
    /// The server sent a WebSocket Close frame; connection is terminated.
    Closed,
    /// The connection was lost unexpectedly.
    Error(String),
}

// ── Native (non-WASM) ────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
pub use native::run_network_task;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpStream;
    use tokio::sync::mpsc;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
    use tokio_tungstenite::tungstenite::Message;

    use go_fish_web::ServerMessage;

    use super::NetworkEvent;

    /// Runs the network task: reads WebSocket frames and forwards them as
    /// NetworkEvents, and writes ClientMessages as JSON text frames.
    pub async fn run_network_task(
        mut ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
        network_event_tx: mpsc::Sender<NetworkEvent>,
        mut client_msg_rx: mpsc::Receiver<go_fish_web::ClientMessage>,
    ) {
        loop {
            tokio::select! {
                frame = ws.next() => {
                    match frame {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<ServerMessage>(&text) {
                                Ok(msg) => {
                                    if network_event_tx.send(NetworkEvent::Message(msg)).await.is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    tracing::debug!("Failed to deserialise server frame: {e}. Raw: {text}");
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) => {
                            let _ = network_event_tx.send(NetworkEvent::Closed).await;
                            return;
                        }
                        Some(Err(e)) => {
                            let _ = network_event_tx.send(NetworkEvent::Error(e.to_string())).await;
                            return;
                        }
                        None => {
                            let _ = network_event_tx.send(NetworkEvent::Closed).await;
                            return;
                        }
                        Some(Ok(_)) => {} // Binary, Ping, Pong — silently ignore
                    }
                }
                msg = client_msg_rx.recv() => {
                    match msg {
                        Some(client_msg) => {
                            match serde_json::to_string(&client_msg) {
                                Ok(json) => {
                                    if ws.send(Message::Text(json.into())).await.is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    tracing::debug!("Failed to serialise client message: {e}");
                                }
                            }
                        }
                        None => return,
                    }
                }
            }
        }
    }
}

// ── WASM ─────────────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
pub use wasm::run_network_task;

#[cfg(target_arch = "wasm32")]
mod wasm {
    use futures_util::{SinkExt, StreamExt};
    use gloo_net::websocket::futures::WebSocket;
    use gloo_net::websocket::Message;
    use tokio::sync::mpsc;

    use go_fish_web::ServerMessage;

    use super::NetworkEvent;

    /// WASM network task: same contract as the native version but uses
    /// `gloo_net::websocket` instead of `tokio_tungstenite`.
    pub async fn run_network_task(
        ws: WebSocket,
        network_event_tx: mpsc::Sender<NetworkEvent>,
        mut client_msg_rx: mpsc::Receiver<go_fish_web::ClientMessage>,
    ) {
        let (mut sink, mut stream) = ws.split();

        loop {
            tokio::select! {
                frame = stream.next() => {
                    match frame {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<ServerMessage>(&text) {
                                Ok(msg) => {
                                    if network_event_tx.send(NetworkEvent::Message(msg)).await.is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    tracing::debug!("Failed to deserialise server frame: {e}. Raw: {text}");
                                }
                            }
                        }
                        Some(Ok(Message::Bytes(_))) => {} // ignore binary frames
                        Some(Err(e)) => {
                            let _ = network_event_tx.send(NetworkEvent::Error(e.to_string())).await;
                            return;
                        }
                        None => {
                            let _ = network_event_tx.send(NetworkEvent::Closed).await;
                            return;
                        }
                    }
                }
                msg = client_msg_rx.recv() => {
                    match msg {
                        Some(client_msg) => {
                            match serde_json::to_string(&client_msg) {
                                Ok(json) => {
                                    if sink.send(Message::Text(json)).await.is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    tracing::debug!("Failed to serialise client message: {e}");
                                }
                            }
                        }
                        None => return,
                    }
                }
            }
        }
    }
}

// ── Tests (native only) ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use go_fish::Rank;
    use go_fish_web::{
        ClientMessage, ClientHookRequest,
        ServerMessage, HookAndResult, HookError, HandState, PlayerTurnValue,
        FullHookRequest, GameResult, GameSnapshot, HookOutcome, OpponentState,
    };
    use go_fish::{Hand, IncompleteBook, CompleteBook, Card, Suit, HookResult};

    fn rank_strategy() -> impl Strategy<Value = Rank> {
        prop_oneof![
            Just(Rank::Two), Just(Rank::Three), Just(Rank::Four),
            Just(Rank::Five), Just(Rank::Six), Just(Rank::Seven),
            Just(Rank::Eight), Just(Rank::Nine), Just(Rank::Ten),
            Just(Rank::Jack), Just(Rank::Queen), Just(Rank::King),
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

    fn hook_result_strategy() -> impl Strategy<Value = HookResult> {
        prop_oneof![
            incomplete_book_strategy().prop_map(HookResult::Catch),
            Just(HookResult::GoFish),
        ]
    }

    fn client_hook_request_strategy() -> impl Strategy<Value = ClientHookRequest> {
        ("[a-zA-Z0-9]{1,16}", rank_strategy())
            .prop_map(|(target_name, rank)| ClientHookRequest { target_name, rank })
    }

    fn client_message_strategy() -> impl Strategy<Value = ClientMessage> {
        prop_oneof![
            client_hook_request_strategy().prop_map(ClientMessage::Hook),
            Just(()).prop_map(|_| ClientMessage::Identity),
            Just(()).prop_map(|_| ClientMessage::CreateLobby),
            "[a-zA-Z0-9]{1,16}".prop_map(ClientMessage::JoinLobby),
            Just(()).prop_map(|_| ClientMessage::LeaveLobby),
            Just(()).prop_map(|_| ClientMessage::StartGame),
        ]
    }

    fn hook_error_strategy() -> impl Strategy<Value = HookError> {
        prop_oneof![
            Just(()).prop_map(|_| HookError::NotYourTurn),
            "[a-zA-Z0-9]{1,16}".prop_map(HookError::UnknownPlayer),
            Just(()).prop_map(|_| HookError::CannotTargetYourself),
            rank_strategy().prop_map(HookError::YouDoNotHaveRank),
        ]
    }

    fn player_turn_value_strategy() -> impl Strategy<Value = PlayerTurnValue> {
        prop_oneof![
            Just(()).prop_map(|_| PlayerTurnValue::YourTurn),
            "[a-zA-Z0-9]{1,16}".prop_map(PlayerTurnValue::OtherTurn),
        ]
    }

    fn full_hook_request_strategy() -> impl Strategy<Value = FullHookRequest> {
        ("[a-zA-Z0-9]{1,16}", "[a-zA-Z0-9]{1,16}", rank_strategy())
            .prop_map(|(fisher_name, target_name, rank)| FullHookRequest {
                fisher_name, target_name, rank,
            })
    }

    fn hook_and_result_strategy() -> impl Strategy<Value = HookAndResult> {
        (full_hook_request_strategy(), hook_result_strategy())
            .prop_map(|(hook_request, hook_result)| HookAndResult { hook_request, hook_result })
    }

    fn hand_state_strategy() -> impl Strategy<Value = HandState> {
        (hand_strategy(), prop::collection::vec(complete_book_strategy(), 0..=4))
            .prop_map(|(hand, completed_books)| HandState { hand, completed_books })
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
                fisher_name, target_name, rank, result,
            })
    }

    fn opponent_state_strategy() -> impl Strategy<Value = OpponentState> {
        ("[a-zA-Z0-9]{1,16}", 0usize..=13usize, 0usize..=13usize)
            .prop_map(|(name, card_count, completed_book_count)| OpponentState {
                name, card_count, completed_book_count,
            })
    }

    fn game_snapshot_strategy() -> impl Strategy<Value = GameSnapshot> {
        (
            hand_state_strategy(),
            prop::collection::vec(opponent_state_strategy(), 0..=3),
            "[a-zA-Z0-9]{1,16}",
            prop::option::of(hook_outcome_strategy()),
        ).prop_map(|(hand_state, opponents, active_player, last_hook_outcome)| GameSnapshot {
            hand_state, opponents, active_player, last_hook_outcome,
        })
    }

    fn server_message_strategy() -> impl Strategy<Value = ServerMessage> {
        prop_oneof![
            hook_and_result_strategy().prop_map(ServerMessage::HookAndResult),
            hook_error_strategy().prop_map(ServerMessage::HookError),
            hand_state_strategy().prop_map(ServerMessage::HandState),
            player_turn_value_strategy().prop_map(ServerMessage::PlayerTurn),
            "[a-zA-Z0-9]{1,32}".prop_map(ServerMessage::PlayerIdentity),
            game_result_strategy().prop_map(ServerMessage::GameResult),
            (
                "[a-zA-Z0-9]{1,16}",
                "[a-zA-Z0-9]{1,16}",
                prop::collection::vec("[a-zA-Z0-9]{1,16}", 0..=4),
                1usize..=8usize,
            ).prop_map(|(lobby_id, leader, players, max_players)| ServerMessage::LobbyJoined {
                lobby_id, leader, players, max_players,
            }),
            (
                "[a-zA-Z0-9]{1,16}",
                prop::collection::vec("[a-zA-Z0-9]{1,16}", 0..=4),
            ).prop_map(|(leader, players)| ServerMessage::LobbyUpdated { leader, players }),
            Just(()).prop_map(|_| ServerMessage::GameStarted),
            "[a-zA-Z0-9 ]{1,32}".prop_map(ServerMessage::Error),
            game_snapshot_strategy().prop_map(ServerMessage::GameSnapshot),
        ]
    }

    proptest! {
        #[test]
        fn prop_client_message_round_trip(msg in client_message_strategy()) {
            let json = serde_json::to_string(&msg).unwrap();
            let back: ClientMessage = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&back).unwrap();
            prop_assert_eq!(json, json2);
        }

        #[test]
        fn prop_server_message_round_trip(msg in server_message_strategy()) {
            let json = serde_json::to_string(&msg).unwrap();
            let back: ServerMessage = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&back).unwrap();
            prop_assert_eq!(json, json2);
        }

        #[test]
        fn prop_rank_round_trip(rank in rank_strategy()) {
            let json = serde_json::to_string(&rank).unwrap();
            let back: go_fish::Rank = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&back).unwrap();
            prop_assert_eq!(json, json2);
        }

        #[test]
        fn prop_invalid_frames_discarded(
            s in any::<String>().prop_filter(
                "must not be valid ServerMessage JSON",
                |s| serde_json::from_str::<go_fish_web::ServerMessage>(s).is_err(),
            )
        ) {
            let result = serde_json::from_str::<go_fish_web::ServerMessage>(&s);
            prop_assert!(result.is_err());
        }

        #[test]
        fn prop_identity_is_first_message(_dummy in Just(())) {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<go_fish_web::ClientMessage>(32);
            tx.try_send(go_fish_web::ClientMessage::Identity).unwrap();
            let first = rx.try_recv().expect("channel should have a message");
            let json = serde_json::to_string(&first).unwrap();
            let expected = serde_json::to_string(&go_fish_web::ClientMessage::Identity).unwrap();
            prop_assert_eq!(json, expected);
        }
    }
}
