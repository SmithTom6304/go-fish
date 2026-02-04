use super::{CompleteBook, IncompleteBook, Rank, hook};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Hand {
    pub books: Vec<IncompleteBook>,
}

impl Hand {
    pub fn empty() -> Hand {
        let books = vec![];
        Hand { books }
    }

    pub fn add_book(self, book: IncompleteBook) -> (Self, Option<CompleteBook>) {
        // TODO This function stinks - use it as an exercise in better FP
        // Should try use IncompleteBook.combine with a reducer or something
        let mut books: Vec<IncompleteBook> = vec![];
        let mut combined = false;
        let mut completed_book = None;

        for b in self.books {
            if b.rank != book.rank {
                books.push(b);
                continue;
            }

            let rank = book.rank;
            let combined_cards = book
                .cards
                .clone()
                .into_iter()
                .chain(b.cards)
                .collect::<Vec<_>>();
            combined = true;

            if combined_cards.len() == 4 {
                let cards = [
                    combined_cards[0],
                    combined_cards[1],
                    combined_cards[2],
                    combined_cards[3],
                ];
                completed_book = Some(CompleteBook { rank, cards });
            } else {
                let incomplete_book = IncompleteBook {
                    rank,
                    cards: combined_cards,
                };
                books.push(incomplete_book);
            }
        }

        if !combined {
            books.push(book);
        }

        let hand = Hand { books };
        (hand, completed_book)
    }

    pub fn receive_hook(self, rank: Rank) -> (Self, hook::Result) {
        let mut books = vec![];
        let mut catch = None;

        for book in self.books {
            if book.rank == rank {
                catch = Some(book);
            } else {
                books.push(book);
            }
        }

        let result = match catch {
            Some(catch) => hook::Result::Catch(catch),
            None => hook::Result::GoFish,
        };

        (Hand { books }, result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Card;
    use crate::Rank::{Ace, Two};
    use crate::Suit::{Clubs, Diamonds, Hearts, Spades};

    #[test]
    fn add_book_adds_new_book() {
        let existing_books = vec![IncompleteBook::from(Card {
            suit: Spades,
            rank: Ace,
        })];
        let hand = Hand {
            books: existing_books,
        };
        let new_book = IncompleteBook::from(Card {
            suit: Spades,
            rank: Two,
        });

        let (hand, completed_book) = hand.add_book(new_book);

        assert_eq!(hand.books.len(), 2);
        assert_eq!(hand.books[0].rank, Ace);
        assert_eq!(hand.books[0].cards.len(), 1);
        assert_eq!(hand.books[1].rank, Two);
        assert_eq!(hand.books[1].cards.len(), 1);
        assert!(completed_book.is_none());
    }

    #[test]
    fn add_book_combines_existing_book() {
        let existing_books = vec![IncompleteBook::from(Card {
            suit: Spades,
            rank: Ace,
        })];
        let hand = Hand {
            books: existing_books,
        };
        let new_book = IncompleteBook::from(Card {
            suit: Hearts,
            rank: Ace,
        });

        let (hand, completed_book) = hand.add_book(new_book);

        assert_eq!(hand.books.len(), 1);
        assert_eq!(hand.books[0].rank, Ace);
        assert_eq!(hand.books[0].cards.len(), 2);
        assert!(completed_book.is_none());
    }

    #[test]
    fn add_book_completes_finished_book() {
        let nearly_finished_book = IncompleteBook {
            rank: Ace,
            cards: vec![
                Card {
                    suit: Spades,
                    rank: Ace,
                },
                Card {
                    suit: Clubs,
                    rank: Ace,
                },
                Card {
                    suit: Diamonds,
                    rank: Ace,
                },
            ],
        };
        let hand = Hand {
            books: vec![nearly_finished_book],
        };
        let new_book = IncompleteBook::from(Card {
            suit: Hearts,
            rank: Ace,
        });

        let (hand, completed_book) = hand.add_book(new_book);

        assert_eq!(hand.books.len(), 0);
        assert!(completed_book.is_some());
    }

    #[test]
    fn loses_book_if_hook_catches() {
        let card = Card {
            suit: Spades,
            rank: Ace,
        };
        let existing_books = vec![IncompleteBook::from(card)];
        let hand = Hand {
            books: existing_books,
        };

        let (hand, result) = hand.receive_hook(Ace);

        assert_eq!(hand.books.len(), 0);
        let caught_card = match result {
            hook::Result::Catch(card) => card,
            hook::Result::GoFish => panic!("Should not be GoFish"),
        }
        .cards
        .first()
        .unwrap()
        .clone();
        assert_eq!(caught_card, card);
    }
}
