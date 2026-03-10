use crate::*;

#[test]
fn current_player_has_empty_hand_at_end_of_turn_then_draws() {
    // Arrange
    let player_1 = Player {
        id: PlayerId(1),
        hand: Hand {
            books: vec![IncompleteBook {
                rank: Rank::Ace,
                cards: vec![
                    Card {
                        rank: Rank::Ace,
                        suit: Suit::Clubs,
                    },
                    Card {
                        rank: Rank::Ace,
                        suit: Suit::Diamonds,
                    },
                    Card {
                        rank: Rank::Ace,
                        suit: Suit::Spades,
                    },
                ],
            }],
        },
        completed_books: vec![],
    };

    let player_2 = Player {
        id: PlayerId(2),
        hand: Hand {
            books: vec![IncompleteBook {
                rank: Rank::Ace,
                cards: vec![Card {
                    rank: Rank::Ace,
                    suit: Suit::Hearts,
                }],
            }],
        },
        completed_books: vec![],
    };

    let deck = Deck::from(vec![
        Card {
            rank: Rank::Two,
            suit: Suit::Clubs,
        },
        Card {
            rank: Rank::Two,
            suit: Suit::Diamonds,
        },
        Card {
            rank: Rank::Two,
            suit: Suit::Spades,
        },
    ]);

    let hook = Hook {
        target: player_2.id,
        rank: Rank::Ace,
    };
    let mut game = Game {
        deck,
        players: vec![player_1, player_2],
        inactive_players: Default::default(),
        player_turn: 0,
        is_finished: false,
    };

    // Act
    game.take_turn(hook).expect("Game state should be valid");

    // Assert
    assert_eq!(game.player_turn, 0); // Still player 1's turn
    assert_eq!(game.players.first().unwrap().completed_books.len(), 1); // Player 1 has completed book
    assert_eq!(game.players.first().unwrap().hand.books.len(), 1); // Importantly, Player 1 drew a new card before the end of their turn
}

#[test]
fn new_player_has_empty_hand_when_it_is_about_to_be_their_turn_then_draws() {
    // Arrange
    let player_1 = Player {
        id: PlayerId(1),
        hand: Hand {
            books: vec![IncompleteBook {
                rank: Rank::Ace,
                cards: vec![
                    Card {
                        rank: Rank::Ace,
                        suit: Suit::Clubs,
                    },
                    Card {
                        rank: Rank::Ace,
                        suit: Suit::Diamonds,
                    },
                    Card {
                        rank: Rank::Ace,
                        suit: Suit::Spades,
                    },
                ],
            }],
        },
        completed_books: vec![],
    };

    let player_2 = Player {
        id: PlayerId(2),
        hand: Hand {
            books: vec![IncompleteBook {
                rank: Rank::Ace,
                cards: vec![Card {
                    rank: Rank::Ace,
                    suit: Suit::Hearts,
                }],
            }],
        },
        completed_books: vec![],
    };

    let deck = Deck::from(vec![
        Card {
            rank: Rank::Two,
            suit: Suit::Clubs,
        },
        Card {
            rank: Rank::Two,
            suit: Suit::Diamonds,
        },
        Card {
            rank: Rank::Two,
            suit: Suit::Spades,
        },
    ]);

    let hook = Hook {
        target: PlayerId(2),
        rank: Rank::Ace,
    };
    let mut game = Game {
        deck,
        players: vec![player_1, player_2],
        inactive_players: Default::default(),
        player_turn: 0,
        is_finished: false,
    };

    game.take_turn(hook).expect("Game state should be valid"); // Catch, so still player 1's turn.
    // Player 2 now has no cards, but player 1 must still ask them
    // Player 2 will pick up when it becomes their turn
    let hook = Hook {
        target: PlayerId(2),
        rank: Rank::Two,
    };

    // Act
    game.take_turn(hook).expect("Game state should be valid");

    // Assert
    assert_eq!(game.player_turn, 1); // It's now player 2's turn
    assert_eq!(game.players.get(1).unwrap().hand.books.len(), 1); // Importantly, Player 2 has picked up a card now that it is their turn
}

#[test]
fn final_player_completes_final_book_by_drawing_then_game_is_finished() {
    let player_1 = Player {
        id: PlayerId(1),
        completed_books: vec![CompleteBook {
            rank: Rank::Ace,
            cards: [
                Card { suit: Suit::Clubs, rank: Rank::Ace },
                Card { suit: Suit::Diamonds, rank: Rank::Ace },
                Card { suit: Suit::Hearts, rank: Rank::Ace },
                Card { suit: Suit::Spades, rank: Rank::Ace }
            ]
        }],
        hand: Hand { books: vec![] },
    };
    let player_2 = Player {
        id: PlayerId(2),
        completed_books: vec![],
        hand: Hand {
            books: vec![IncompleteBook {
                rank: Rank::Two,
                cards: vec![
                    Card { suit: Suit::Clubs, rank: Rank::Two },
                    Card { suit: Suit::Diamonds, rank: Rank::Two },
                    Card { suit: Suit::Hearts, rank: Rank::Two },
                ],
            }]
        },
    };
    let deck = Deck { cards: vec![Card { suit: Suit::Spades, rank: Rank::Two }] };

    let mut game = Game {
        deck,
        players: vec![player_1, player_2],
        inactive_players: vec![],
        player_turn: 1,
        is_finished: false,
    };

    let hook = Hook {
        target: PlayerId(1),
        rank: Rank::Two,
    };

    game.take_turn(hook).expect("Game state should be valid");

    assert!(game.is_finished);
}

#[cfg(test)]
mod deck_tests {
    use crate::{Card, Deck};

    impl From<Vec<Card>> for Deck {
        fn from(cards: Vec<Card>) -> Deck {
            Deck { cards }
        }
    }

    #[test]
    fn can_draw_cards() {
        let mut deck = Deck::new();
        let card = deck.draw();
        assert!(card.is_some());
    }
}

#[cfg(test)]
mod hand_tests {
    use crate::{Card, Hand, IncompleteBook, Rank, Suit};

    #[test]
    fn add_book_adds_new_book() {
        let existing_books = vec![IncompleteBook::from(Card {
            suit: Suit::Spades,
            rank: Rank::Ace,
        })];
        let mut hand = Hand {
            books: existing_books,
        };
        let new_book = IncompleteBook::from(Card {
            suit: Suit::Spades,
            rank: Rank::Two,
        });

        let completed_book = hand.add_book(new_book);

        assert_eq!(hand.books.len(), 2);
        assert_eq!(hand.books[0].rank, Rank::Ace);
        assert_eq!(hand.books[0].cards.len(), 1);
        assert_eq!(hand.books[1].rank, Rank::Two);
        assert_eq!(hand.books[1].cards.len(), 1);
        assert!(completed_book.is_none());
    }

    #[test]
    fn add_book_combines_existing_book() {
        let existing_books = vec![IncompleteBook::from(Card {
            suit: Suit::Spades,
            rank: Rank::Ace,
        })];
        let mut hand = Hand {
            books: existing_books,
        };
        let new_book = IncompleteBook::from(Card {
            suit: Suit::Hearts,
            rank: Rank::Ace,
        });

        let completed_book = hand.add_book(new_book);

        assert_eq!(hand.books.len(), 1);
        assert_eq!(hand.books[0].rank, Rank::Ace);
        assert_eq!(hand.books[0].cards.len(), 2);
        assert!(completed_book.is_none());
    }

    #[test]
    fn add_book_completes_finished_book() {
        let nearly_finished_book = IncompleteBook {
            rank: Rank::Ace,
            cards: vec![
                Card {
                    suit: Suit::Spades,
                    rank: Rank::Ace,
                },
                Card {
                    suit: Suit::Clubs,
                    rank: Rank::Ace,
                },
                Card {
                    suit: Suit::Diamonds,
                    rank: Rank::Ace,
                },
            ],
        };
        let mut hand = Hand {
            books: vec![nearly_finished_book],
        };
        let new_book = IncompleteBook::from(Card {
            suit: Suit::Hearts,
            rank: Rank::Ace,
        });

        let completed_book = hand.add_book(new_book);

        assert_eq!(hand.books.len(), 0);
        assert!(completed_book.is_some());
    }

    #[test]
    fn loses_book_if_hook_catches() {
        let card = Card {
            suit: Suit::Spades,
            rank: Rank::Ace,
        };
        let existing_books = vec![IncompleteBook::from(card)];
        let mut hand = Hand {
            books: existing_books,
        };

        hand.receive_hook(Rank::Ace);

        assert_eq!(hand.books.len(), 0);
    }
}
