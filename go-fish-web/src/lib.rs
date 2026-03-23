use go_fish::Rank;
use go_fish::{CompleteBook, Hand, HookResult};
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    Hook(ClientHookRequest),
    RequestPlayerIdentity(PlayerIdentityRequest),
    Disconnect
}

#[derive(Debug, Serialize, Deserialize)]
pub enum PlayerIdentityRequest {
    RequestedName(String),
    RandomName
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ServerMessage {
    HookAndResult(HookAndResult),
    HookError(HookError),
    PlayerState(PlayerState),
    PlayerTurn(PlayerTurnValue),
    PlayerIdentity(PlayerIdentity),
    GameResult(GameResult),
    Disconnect
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum PlayerIdentity {
    RequestedPlayerIdentity(String),
    RandomPlayerIdentity(String, RandomPlayerIdentityReason)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum RandomPlayerIdentityReason {
    RandomIdentityRequested,
    RequestedIdentityAlreadyInUse(String)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
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
pub struct PlayerState {
    pub hand: Hand,
    pub completed_books: Vec<CompleteBook>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GameResult {
    pub winners: Vec<String>,
    pub losers: Vec<String>,
}