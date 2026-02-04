use super::{Card, Rank};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct IncompleteBook {
    pub rank: Rank,
    pub cards: Vec<Card>,
}

pub enum CombineResult {
    Combined(IncompleteBook),
    NotCombined(IncompleteBook, IncompleteBook),
    Completed(CompleteBook),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompleteBook {
    pub rank: Rank,
    pub cards: [Card; 4],
}

impl From<Card> for IncompleteBook {
    fn from(card: Card) -> Self {
        let rank = card.rank;
        let cards = vec![card];
        IncompleteBook { rank, cards }
    }
}

impl IncompleteBook {
    pub fn combine(self, other: IncompleteBook) -> CombineResult {
        if self.rank != other.rank {
            return CombineResult::NotCombined(self, other);
        }

        let rank = self.rank;
        let combined_cards = self
            .cards
            .into_iter()
            .chain(other.cards)
            .collect::<Vec<_>>();

        if combined_cards.len() == 4 {
            let cards = [
                combined_cards[0],
                combined_cards[1],
                combined_cards[2],
                combined_cards[3],
            ];
            let complete_book = CompleteBook { rank, cards };
            return CombineResult::Completed(complete_book);
        }

        let combined_book = IncompleteBook {
            rank,
            cards: combined_cards,
        };
        CombineResult::Combined(combined_book)
    }
}
