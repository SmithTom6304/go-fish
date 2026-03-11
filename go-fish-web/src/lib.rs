use go_fish::Rank;
use go_fish::{CompleteBook, Hand, HookResult};
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    Hook(ClientHookRequest),
    PlayerNameChangeRequest(String),
    Disconnect
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ServerMessage {
    HookAndResult(HookAndResult),
    HookError(HookError),
    PlayerState(PlayerState),
    PlayerTurn(PlayerTurnValue),
    PlayerIdentity(String),
    GameResult(GameResult)
}

#[derive(Debug, Serialize, Deserialize)]
pub enum HookError {
    NotYourTurn,
    UnknownPlayer(String),
    CannotTargetYourself,
    YouDoNotHaveRank(Rank),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum PlayerTurnValue {
    YourTurn,
    OtherTurn(String)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClientHookRequest {
    pub target_name: String,
    pub rank: Rank
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FullHookRequest {
    pub fisher_name: String,
    pub target_name: String,
    pub rank: Rank,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HookAndResult {
    pub hook_request: FullHookRequest,
    pub hook_result: HookResult
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PlayerState {
    pub hand: Hand,
    pub completed_books: Vec<CompleteBook>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GameResult {
    pub winners: Vec<String>,
    pub losers: Vec<String>,
}