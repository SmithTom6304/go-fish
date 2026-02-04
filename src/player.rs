use super::{CompleteBook, Hand, IncompleteBook, Rank, hook};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Player {
    pub id: PlayerId,
    pub hand: Hand,
    pub active: bool,
    pub completed_books: Vec<CompleteBook>,
}

#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq, Eq)]
pub struct PlayerId(pub u8);

impl Player {
    pub fn add_book(self, book: IncompleteBook) -> Player {
        let (hand, completed_book) = self.hand.add_book(book);

        let mut completed_books = self.completed_books;
        if let Some(completed_book) = completed_book {
            completed_books.push(completed_book);
        }

        Player {
            id: self.id,
            hand,
            active: true,
            completed_books,
        }
    }

    pub fn receive_hook(self, rank: Rank) -> (Player, hook::Result) {
        let (hand, result) = self.hand.receive_hook(rank);
        let player = Player {
            id: self.id,
            hand,
            active: self.active,
            completed_books: self.completed_books,
        };
        (player, result)
    }
}
