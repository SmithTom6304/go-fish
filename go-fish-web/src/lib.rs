//! # go-fish-web
//! 
//! `go-fish-web` provides the protocol message types used by the `go-fish` web client and server.

use go_fish::Rank;
use go_fish::{CompleteBook, Hand, HookResult};
use serde::Deserialize;
use serde::Serialize;

/// Messages sent from the client to the server.
#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    Hook(ClientHookRequest),
    Identity,
    CreateLobby,
    JoinLobby(String),
    LeaveLobby,
    StartGame,
}

/// Messages sent from the server to the client.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ServerMessage {
    HookAndResult(HookAndResult),
    HookError(HookError),
    HandState(HandState),
    PlayerTurn(PlayerTurnValue),
    PlayerIdentity(String),
    GameResult(GameResult),
    LobbyJoined {
        lobby_id: String,
        leader: String,
        players: Vec<String>,
        max_players: usize,
    },
    LobbyUpdated {
        leader: String,
        players: Vec<String>,
    },
    LobbyLeft(LobbyLeftReason),
    GameStarted,
    GameSnapshot(GameSnapshot),
    Error(String),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum HookError {
    NotYourTurn,
    UnknownPlayer(String),
    CannotTargetYourself,
    YouDoNotHaveRank(Rank),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum PlayerTurnValue {
    YourTurn,
    OtherTurn(String)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClientHookRequest {
    pub target_name: String,
    pub rank: Rank
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FullHookRequest {
    pub fisher_name: String,
    pub target_name: String,
    pub rank: Rank,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HookAndResult {
    pub hook_request: FullHookRequest,
    pub hook_result: HookResult
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HandState {
    pub hand: Hand,
    pub completed_books: Vec<CompleteBook>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GameResult {
    pub winners: Vec<String>,
    pub losers: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum LobbyLeftReason {
    RequestedByPlayer
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HookOutcome {
    pub fisher_name: String,
    pub target_name: String,
    pub rank: Rank,
    pub result: HookResult,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OpponentState {
    pub name: String,
    pub card_count: usize,
    pub completed_book_count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GameSnapshot {
    pub hand_state: HandState,
    pub opponents: Vec<OpponentState>,
    pub active_player: String,
    pub last_hook_outcome: Option<HookOutcome>,
}
