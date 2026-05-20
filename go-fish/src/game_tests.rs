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

// --- Deck tests ---

#[test]
fn deck_len_reflects_card_count() {
    let mut deck = Deck::new();
    assert_eq!(deck.len(), 52);
    deck.draw();
    assert_eq!(deck.len(), 51);
}

#[test]
fn shuffle_reorders_cards() {
    let ordered: Vec<Card> = {
        let mut d = Deck::new();
        std::iter::from_fn(|| d.draw()).collect()
    };
    let shuffled: Vec<Card> = {
        let mut d = Deck::new().shuffle();
        std::iter::from_fn(|| d.draw()).collect()
    };
    // With 52! permutations the probability of a random shuffle matching
    // the ordered sequence is negligible.
    assert_ne!(ordered, shuffled);
}

// --- GameResult tests ---

#[test]
fn game_result_winner_has_most_books() {
    let make_book = |rank: Rank| CompleteBook {
        rank,
        cards: [
            Card { rank, suit: Suit::Clubs },
            Card { rank, suit: Suit::Diamonds },
            Card { rank, suit: Suit::Hearts },
            Card { rank, suit: Suit::Spades },
        ],
    };
    let game = Game {
        deck: Deck { cards: vec![] },
        players: vec![],
        inactive_players: vec![
            InactivePlayer {
                id: PlayerId(1),
                completed_books: vec![make_book(Rank::Two), make_book(Rank::Three)],
            },
            InactivePlayer {
                id: PlayerId(2),
                completed_books: vec![make_book(Rank::Ace)],
            },
        ],
        player_turn: 0,
        is_finished: true,
    };
    let result = game.get_game_result().expect("game is finished");
    assert_eq!(result.winners.len(), 1);
    assert_eq!(result.winners[0].id, PlayerId(1));
    assert_eq!(result.losers.len(), 1);
    assert_eq!(result.losers[0].id, PlayerId(2));
}

// --- Turn-advance tests ---

fn player_with_cards(id: u8, rank: Rank, suits: &[Suit]) -> Player {
    Player {
        id: PlayerId(id),
        hand: Hand {
            books: vec![IncompleteBook {
                rank,
                cards: suits.iter().map(|&suit| Card { rank, suit }).collect(),
            }],
        },
        completed_books: vec![],
    }
}

fn player_with_two_books(
    id: u8,
    rank1: Rank, suits1: &[Suit],
    rank2: Rank, suits2: &[Suit],
) -> Player {
    Player {
        id: PlayerId(id),
        hand: Hand {
            books: vec![
                IncompleteBook {
                    rank: rank1,
                    cards: suits1.iter().map(|&suit| Card { rank: rank1, suit }).collect(),
                },
                IncompleteBook {
                    rank: rank2,
                    cards: suits2.iter().map(|&suit| Card { rank: rank2, suit }).collect(),
                },
            ],
        },
        completed_books: vec![],
    }
}

#[test]
fn turn_advances_to_next_player_on_go_fish() {
    // P1 (turn=0) GoFishes from P2. Turn must move to P2 (idx=1), not stay on P1.
    let p1 = player_with_cards(1, Rank::Two, &[Suit::Clubs]);
    let p2 = player_with_cards(2, Rank::King, &[Suit::Clubs]);
    let p3 = player_with_cards(3, Rank::King, &[Suit::Hearts]);
    let mut game = Game {
        deck: Deck { cards: vec![] },
        players: vec![p1, p2, p3],
        inactive_players: vec![],
        player_turn: 0,
        is_finished: false,
    };
    game.take_turn(Hook { target: PlayerId(2), rank: Rank::Two }).unwrap();
    let current = game.get_current_player().expect("game should not be finished");
    assert_eq!(current.id, PlayerId(2));
}

#[test]
fn turn_skips_eliminated_player_to_correct_next() {
    // P1 (turn=0) GoFishes from P4. P2 (next in order) has an empty hand and the deck
    // is empty, so P2 goes inactive mid-advance. This forces the loop-internal
    // turn update. The correct next active player is P4.
    let p1 = player_with_cards(1, Rank::Two, &[Suit::Clubs]);
    let p2 = Player { id: PlayerId(2), hand: Hand { books: vec![] }, completed_books: vec![] };
    let p3 = player_with_cards(3, Rank::King, &[Suit::Clubs]);
    let p4 = player_with_cards(4, Rank::King, &[Suit::Hearts]);
    let mut game = Game {
        deck: Deck { cards: vec![] },
        players: vec![p1, p2, p3, p4],
        inactive_players: vec![],
        player_turn: 0,
        is_finished: false,
    };
    game.take_turn(Hook { target: PlayerId(4), rank: Rank::Two }).unwrap();
    let current = game.get_current_player().expect("game should not be finished");
    assert_eq!(current.id, PlayerId(4));
}

#[test]
fn player_turn_adjusted_when_first_player_goes_inactive() {
    // P1 (idx=0) catches P2's last Two, completing 4 Twos. Empty deck → P1 inactive.
    // Remaining order: [P2, P3, P4]. Next turn should be P2.
    let p1 = player_with_cards(1, Rank::Two, &[Suit::Clubs, Suit::Diamonds, Suit::Hearts]);
    let p2 = player_with_two_books(2, Rank::Two, &[Suit::Spades], Rank::King, &[Suit::Clubs]);
    let p3 = player_with_cards(3, Rank::King, &[Suit::Diamonds]);
    let p4 = player_with_cards(4, Rank::King, &[Suit::Hearts]);
    let mut game = Game {
        deck: Deck { cards: vec![] },
        players: vec![p1, p2, p3, p4],
        inactive_players: vec![],
        player_turn: 0,
        is_finished: false,
    };
    game.take_turn(Hook { target: PlayerId(2), rank: Rank::Two }).unwrap();
    let current = game.get_current_player().expect("game should not be finished");
    assert_eq!(current.id, PlayerId(2));
}

#[test]
fn player_turn_adjusted_when_mid_player_goes_inactive() {
    // P2 (idx=1) catches P3's last Ace, completing 4 Aces. Empty deck → P2 inactive.
    // Remaining order: [P1, P3, P4]. Next turn should be P3 (the player after P2).
    let p1 = player_with_cards(1, Rank::King, &[Suit::Clubs]);
    let p2 = player_with_cards(2, Rank::Ace, &[Suit::Clubs, Suit::Diamonds, Suit::Hearts]);
    let p3 = player_with_two_books(3, Rank::Ace, &[Suit::Spades], Rank::King, &[Suit::Diamonds]);
    let p4 = player_with_cards(4, Rank::King, &[Suit::Hearts]);
    let mut game = Game {
        deck: Deck { cards: vec![] },
        players: vec![p1, p2, p3, p4],
        inactive_players: vec![],
        player_turn: 1,
        is_finished: false,
    };
    game.take_turn(Hook { target: PlayerId(3), rank: Rank::Ace }).unwrap();
    let current = game.get_current_player().expect("game should not be finished");
    assert_eq!(current.id, PlayerId(3));
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
