use crate::Deck;
use crate::hook::Hook;
use crate::player::PlayerId;
use crate::{GameState, Player};
use crate::{IncompleteBook, hook};

pub fn take_turn(game_state: GameState, hook: Hook) -> GameState {
    let player_order = game_state
        .players
        .iter()
        .map(|p| p.id)
        .collect::<Vec<PlayerId>>();
    let (fisher, target, others) = find_hook_players(game_state.players, hook.fisher, hook.target);
    let fisher = fisher.unwrap_or_else(|| {
        panic!(
            "Could not find fisher in gamestate with id {:?}",
            hook.fisher
        )
    });
    let target = target.unwrap_or_else(|| {
        panic!(
            "Could not find target in gamestate with id {:?}",
            hook.target
        )
    });

    // TODO Check fisher is actually current player

    let (target, result) = target.receive_hook(hook.rank);

    let game_state: GameState = match result {
        hook::Result::Catch(catch) => {
            let fisher = fisher.add_book(catch);
            let (fisher, deck) = match fisher.hand.books.is_empty() {
                true => handle_active_player_has_empty_hand(fisher, game_state.deck),
                false => (fisher, game_state.deck),
            };
            let players = recombine_players(vec![fisher, target], others, &player_order);

            // In here, need to handle player becoming inactive after draw
            // Also need to handle multiple draws making complete books

            let player_turn = game_state.player_turn;
            let (new_player, others) = find_current_player(players, player_turn);
            let (new_player, deck) = match new_player.hand.books.is_empty() {
                true => handle_active_player_has_empty_hand(new_player, deck),
                false => (new_player, deck),
            };
            let players = recombine_players(vec![new_player], others, &player_order);

            GameState {
                deck,
                players,
                player_turn,
            }
        }
        hook::Result::GoFish => {
            let (deck, draw) = game_state.deck.draw();
            let fisher = match draw {
                Some(card) => fisher.add_book(IncompleteBook::from(card)),
                None => fisher,
            };

            let players = recombine_players(vec![fisher, target], others, &player_order);

            let gs = GameState {
                deck,
                players,
                player_turn: game_state.player_turn,
            };
            let gs = move_player_turn(gs);
            assert!(gs.players.get(gs.player_turn).unwrap().active);
            gs
        }
    };

    game_state
}

fn move_player_turn(game_state: GameState) -> GameState {
    let current_turn = game_state.player_turn;
    let mut new_turn = (current_turn + 1) % game_state.players.len();
    let mut deck = game_state.deck;
    let mut players = game_state.players;
    let player_order = players.iter().map(|p| p.id).collect::<Vec<PlayerId>>();

    for _ in 1..players.len() {
        let p = players.get(new_turn).unwrap();
        if !p.active {
            new_turn = (new_turn + 1) % players.len();
            continue;
        }

        let (new_player, others) = find_current_player(players, new_turn);
        let (new_player, d) = match new_player.hand.books.is_empty() {
            true => handle_active_player_has_empty_hand(new_player, deck),
            false => (new_player, deck),
        };

        deck = d;
        players = recombine_players(vec![new_player], others, &player_order);

        let p = players.get(new_turn).unwrap();
        if p.active {
            break;
        }

        new_turn = (new_turn + 1) % players.len();
    }

    GameState {
        deck,
        players,
        player_turn: new_turn,
    }
}

fn handle_active_player_has_empty_hand(player: Player, deck: Deck) -> (Player, Deck) {
    let (deck, draw) = deck.draw();
    let fisher = match draw {
        Some(card) => player.add_book(IncompleteBook::from(card)),
        None => Player {
            id: player.id,
            hand: player.hand,
            active: false,
            completed_books: player.completed_books,
        },
    };
    (fisher, deck)
}

fn find_current_player(players: Vec<Player>, current_index: usize) -> (Player, Vec<Player>) {
    let mut found = None;
    let mut others = vec![];

    for (i, player) in players.into_iter().enumerate() {
        if i == current_index {
            found = Some(player);
        } else {
            others.push(player);
        }
    }

    (found.unwrap(), others)
}

fn find_hook_players(
    players: Vec<Player>,
    fisher_id: PlayerId,
    target_id: PlayerId,
) -> (Option<Player>, Option<Player>, Vec<Player>) {
    let mut fisher = None;
    let mut target = None;
    let mut others = vec![];

    for player in players.into_iter() {
        if player.id == fisher_id {
            fisher = Some(player);
        } else if player.id == target_id {
            target = Some(player);
        } else {
            others.push(player);
        }
    }

    (fisher, target, others)
}

fn recombine_players(
    players: Vec<Player>,
    separated_players: Vec<Player>,
    order: &[PlayerId],
) -> Vec<Player> {
    let mut players = players
        .into_iter()
        .chain(separated_players)
        .collect::<Vec<_>>();
    players.sort_by_key(|p| order.iter().position(|pos| &p.id == pos).unwrap());
    players
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rank::Ace;
    use crate::Rank::Two;
    use crate::Suit::Clubs;
    use crate::Suit::Spades;
    use crate::Suit::{Diamonds, Hearts};
    use crate::{Card, Deck, game};
    use crate::{Hand, IncompleteBook};

    #[test]
    fn current_player_has_empty_hand_at_end_of_turn_then_draws() {
        // Arrange
        let player_1 = Player {
            id: PlayerId(1),
            hand: Hand {
                books: vec![IncompleteBook {
                    rank: Ace,
                    cards: vec![
                        Card {
                            rank: Ace,
                            suit: Clubs,
                        },
                        Card {
                            rank: Ace,
                            suit: Diamonds,
                        },
                        Card {
                            rank: Ace,
                            suit: Spades,
                        },
                    ],
                }],
            },
            active: true,
            completed_books: vec![],
        };

        let player_2 = Player {
            id: PlayerId(2),
            hand: Hand {
                books: vec![IncompleteBook {
                    rank: Ace,
                    cards: vec![Card {
                        rank: Ace,
                        suit: Hearts,
                    }],
                }],
            },
            active: true,
            completed_books: vec![],
        };

        let deck = Deck::from(vec![
            Card {
                rank: Two,
                suit: Clubs,
            },
            Card {
                rank: Two,
                suit: Diamonds,
            },
            Card {
                rank: Two,
                suit: Spades,
            },
        ]);

        let hook = Hook {
            fisher: player_1.id,
            target: player_2.id,
            rank: Ace,
        };
        let game_state = GameState {
            deck,
            players: vec![player_1, player_2],
            player_turn: 0,
        };

        // Act
        let game_state = game::take_turn(game_state, hook);

        // Assert
        assert_eq!(game_state.player_turn, 0); // Still player 1's turn
        assert_eq!(game_state.players.get(0).unwrap().completed_books.len(), 1); // Player 1 has completed book
        assert_eq!(game_state.players.get(0).unwrap().hand.books.len(), 1); // Importantly, Player 1 drew a new card before the end of their turn
    }

    #[test]
    fn new_player_has_empty_hand_when_it_is_about_to_be_their_turn_then_draws() {
        // Arrange
        let player_1 = Player {
            id: PlayerId(1),
            hand: Hand {
                books: vec![IncompleteBook {
                    rank: Ace,
                    cards: vec![
                        Card {
                            rank: Ace,
                            suit: Clubs,
                        },
                        Card {
                            rank: Ace,
                            suit: Diamonds,
                        },
                        Card {
                            rank: Ace,
                            suit: Spades,
                        },
                    ],
                }],
            },
            active: true,
            completed_books: vec![],
        };

        let player_2 = Player {
            id: PlayerId(2),
            hand: Hand {
                books: vec![IncompleteBook {
                    rank: Ace,
                    cards: vec![Card {
                        rank: Ace,
                        suit: Hearts,
                    }],
                }],
            },
            active: true,
            completed_books: vec![],
        };

        let deck = Deck::from(vec![
            Card {
                rank: Two,
                suit: Clubs,
            },
            Card {
                rank: Two,
                suit: Diamonds,
            },
            Card {
                rank: Two,
                suit: Spades,
            },
        ]);

        let hook = Hook {
            fisher: PlayerId(1),
            target: PlayerId(2),
            rank: Ace,
        };
        let game_state = GameState {
            deck,
            players: vec![player_1, player_2],
            player_turn: 0,
        };

        let game_state = game::take_turn(game_state, hook); // Catch, so still player 1's turn.
        // Player 2 now has no cards, but player 1 must still ask them
        // Player 2 will pick up when it becomes their turn
        let hook = Hook {
            fisher: PlayerId(1),
            target: PlayerId(2),
            rank: Two,
        };

        // Act
        let game_state = game::take_turn(game_state, hook);

        // Assert
        assert_eq!(game_state.player_turn, 1); // It's now player 2's turn
        assert_eq!(game_state.players.get(1).unwrap().hand.books.len(), 1); // Importantly, Player 2 has picked up a card now that it is their turn
    }
}
