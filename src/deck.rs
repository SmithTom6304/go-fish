use super::{Card, Rank, Suit};
use enum_iterator::all;
use rand;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Deck {
    cards: Vec<Card>,
}

impl Deck {
    pub fn new() -> Deck {
        let cards = Self::ordered_cards();
        Deck { cards }
    }

    pub fn shuffle(self) -> Deck {
        let mut rng = rand::rng();
        let mut cards = self.cards;
        cards.shuffle(&mut rng);
        Deck { cards }
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    pub fn draw(self) -> (Deck, Option<Card>) {
        if self.is_empty() {
            return (self, None);
        }

        let card = self.cards[0];
        (
            Deck {
                cards: self.cards[1..].to_vec(),
            },
            Some(card),
        )
    }

    fn ordered_cards() -> Vec<Card> {
        let cards_from_suit = |suit| {
            all::<Rank>()
                .map(|rank| Card { suit, rank })
                .collect::<Vec<_>>()
        };
        all::<Suit>().flat_map(cards_from_suit).collect::<Vec<_>>()
    }
}

impl Default for Deck {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Vec<Card>> for Deck {
    fn from(cards: Vec<Card>) -> Deck {
        Deck { cards }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn can_draw_cards() {
        let deck = Deck::new();
        let (_, card) = deck.draw();
        assert!(card.is_some());
    }
}
