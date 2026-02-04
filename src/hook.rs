use super::{IncompleteBook, Rank};
use crate::player::PlayerId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Hook {
    pub fisher: PlayerId,
    pub target: PlayerId,
    pub rank: Rank,
}

pub enum Result {
    Catch(IncompleteBook),
    GoFish,
}
