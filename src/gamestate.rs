use super::{Deck, Hand, IncompleteBook, Player};
use crate::player::PlayerId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct GameState {
    pub deck: Deck,
    pub players: Vec<Player>,
    pub player_turn: usize,
}

impl GameState {
    pub fn new(deck: Deck, player_count: u8) -> GameState {
        let hand_size = match player_count {
            2 | 3 => 7,
            _ => 5,
        };

        let mut players: Vec<Player> = vec![];
        let mut deck = deck;
        for n in 0..player_count {
            let (player, d) = Self::deal_player(PlayerId(n), hand_size, deck);
            deck = d;
            players.push(player);
        }

        GameState {
            deck,
            players,
            player_turn: 0,
        }
    }

    pub fn is_completed(&self) -> bool {
        self.players.iter().filter(|p| p.active).count() <= 1
    }

    fn deal_player(id: PlayerId, hand_size: usize, deck: Deck) -> (Player, Deck) {
        let mut hand = Hand::empty();
        let mut completed_books = vec![];
        let mut deck = deck;

        for _ in 0..hand_size {
            let (d, draw) = deck.draw();
            deck = d;
            let book = IncompleteBook::from(draw.unwrap());
            let (h, compl) = hand.add_book(book);
            hand = h;
            if let Some(c) = compl {
                completed_books.push(c);
            }
        }

        (
            Player {
                id,
                hand,
                active: true,
                completed_books,
            },
            deck,
        )
    }
}
