//! # go-fish
//!
//! `go-fish` is a library providing core functionality for the classic [Go Fish card game](https://en.wikipedia.org/wiki/Go_Fish).

use enum_iterator::{all, Sequence};
use rand::prelude::SliceRandom;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use tracing::debug;
use tracing::warn;

/// A playing card
#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub struct Card {
    pub suit: Suit,
    pub rank: Rank,
}

/// The suit of a [Card]
#[derive(Debug, PartialEq, Sequence, Clone, Copy, Serialize, Deserialize)]
pub enum Suit {
    Clubs,
    Diamonds,
    Hearts,
    Spades,
}

/// The rank (or value) of a [Card]
#[derive(
    Debug,
    PartialEq,
    Eq,
    Sequence,
    Clone,
    Copy,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize
)]
pub enum Rank {
    Two,
    Three,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Ten,
    Jack,
    Queen,
    King,
    Ace,
}

impl Display for Rank {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let s = match self {
            Rank::Two => "Two",
            Rank::Three => "Three",
            Rank::Four => "Four",
            Rank::Five => "Five",
            Rank::Six => "Six",
            Rank::Seven => "Seven",
            Rank::Eight => "Eight",
            Rank::Nine => "Nine",
            Rank::Ten => "Ten",
            Rank::Jack => "Jack",
            Rank::Queen => "Queen",
            Rank::King => "King",
            Rank::Ace => "Ace",
        };
        write!(f, "{}", s)
    }
}

/// A deck of [Cards](Card)
#[derive(Debug, Serialize, Deserialize)]
pub struct Deck {
    pub cards: Vec<Card>,
}

/// A collection of three or fewer [Cards](Card) of the same [Rank]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IncompleteBook {
    pub rank: Rank,
    pub cards: Vec<Card>,
}

/// A collection of four [Cards](Card) of the same [Rank]
#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub struct CompleteBook {
    pub rank: Rank,
    pub cards: [Card; 4],
}

/// A players hand
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Hand {
    pub books: Vec<IncompleteBook>,
}

/// A player actively trying to win the game
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Player {
    pub id: PlayerId,
    pub hand: Hand,
    pub completed_books: Vec<CompleteBook>,
}

/// A player who no longer has any viable moves. They can still win the game, if they have
/// more [Completed Books](CompleteBook) than any other player at the end of the game
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InactivePlayer {
    pub id: PlayerId,
    pub completed_books: Vec<CompleteBook>,
}

#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq, Eq, Hash)]
pub struct PlayerId(pub u8);

/// A request for another players cards
/// > Player 2, got any three's?
#[derive(Debug, Serialize, Deserialize)]
pub struct Hook {
    pub target: PlayerId,
    pub rank: Rank,
}

#[derive(Debug)]
pub enum TurnError {
    /// The [Hooks](Hook) target is not a player in the game
    TargetNotFound(PlayerId),
    GameIsFinished
}

/// Handling for a game of Go Fish
#[derive(Debug, Serialize, Deserialize)]
pub struct Game {
    pub deck: Deck,
    pub players: Vec<Player>,
    pub inactive_players: Vec<InactivePlayer>,
    pub is_finished: bool,
    player_turn: usize,
}

impl Deck {
    /// Creates a new, ordered deck of [Cards](Card)
    pub fn new() -> Deck {
        let cards = Self::ordered_cards();
        Deck { cards }
    }

    /// Shuffles the remaining cards
    pub fn shuffle(&mut self) {
        let mut rng = rand::rng();
        self.cards.shuffle(&mut rng);
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// Draws a card from the deck´
    /// ```
    /// use go_fish::Deck;
    /// let mut deck = Deck::new();
    /// let card = deck.draw();
    /// assert!(card.is_some())
    /// ```
    pub fn draw(&mut self) -> Option<Card> {
        self.cards.pop()
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

impl IncompleteBook {
    /// Try to combine one [IncompleteBook] with another. This can result in a [CompleteBook].
    pub(crate) fn combine(self, other: IncompleteBook) -> CombineBookResult {
        if self.rank != other.rank {
            return CombineBookResult::NotCombined(self, other);
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
            return CombineBookResult::Completed(complete_book);
        }

        let combined_book = IncompleteBook {
            rank,
            cards: combined_cards,
        };
        CombineBookResult::Combined(combined_book)
    }
}

impl From<Card> for IncompleteBook {
    fn from(card: Card) -> Self {
        let rank = card.rank;
        let cards = vec![card];
        IncompleteBook { rank, cards }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GameResult {
    pub winners: Vec<InactivePlayer>,
    pub losers: Vec<InactivePlayer>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum HookResult {
    Catch(IncompleteBook),
    GoFish,
}

impl Hand {
    /// Create a new, empty Hand.
    pub fn empty() -> Hand {
        let books = vec![];
        Hand { books }
    }

    /// Add an [IncompleteBook] to the Hand. This may produce an [CompleteBook].
    pub fn add_book(&mut self, book: IncompleteBook) -> Option<CompleteBook> {
        let position = self.books.iter().position(|b| b.rank == book.rank);
        let position = match position {
            Some(position) => position,
            None => {
                self.books.push(book);
                return None;
            }
        };

        let existing_book = self.books.remove(position);
        let combined_result = existing_book.combine(book);

        match combined_result {
            CombineBookResult::NotCombined(a, b) => {
                panic!(
                    "Books {:?} and {:?} failed to combine, but we expect them to have the same rank",
                    a, b
                )
            }
            CombineBookResult::Combined(combined_book) => {
                self.books.push(combined_book);
                None
            }
            CombineBookResult::Completed(completed_book) => Some(completed_book),
        }
    }

    pub(crate) fn receive_hook(&mut self, rank: Rank) -> HookResult {
        let position = self.books.iter().position(|b| b.rank == rank);
        let position = match position {
            None => return HookResult::GoFish,
            Some(pos) => pos,
        };

        let catch = self.books.remove(position);
        HookResult::Catch(catch)
    }
}

impl Player {
    pub(crate) fn add_book(&mut self, book: IncompleteBook) {
        let completed_book = self.hand.add_book(book);

        if let Some(completed_book) = completed_book {
            self.completed_books.push(completed_book);
        }
    }

    pub(crate) fn receive_hook(&mut self, rank: Rank) -> HookResult {
        self.hand.receive_hook(rank)
    }
}

impl Game {
    /// Create a new Game of Go Fish, with the given [Deck] and number of players
    pub fn new(deck: Deck, player_count: u8) -> Game {
        debug!(?deck, player_count, "Creating new Game");
        let hand_size = match player_count {
            2 | 3 => 7,
            _ => 5,
        };

        let mut players: Vec<Player> = vec![];
        let mut deck = deck;
        for n in 0..player_count {
            let player = Self::deal_player(PlayerId(n), hand_size, &mut deck);
            players.push(player);
        }

        Game {
            deck,
            players,
            is_finished: false,
            inactive_players: vec![],
            player_turn: 0,
        }
    }

    /// Take a turn in the game
    pub fn take_turn(&mut self, hook: Hook) -> Result<HookResult, TurnError> {
        debug!(game.players = ?self.players,
            game.inactive_players = ?self.inactive_players,
            game.is_deck_empty = self.deck.is_empty(),
            game.player_turn = self.player_turn,
            game.is_finished = self.is_finished,
            ?hook,
            "Taking turn");
        if self.is_finished {
            warn!("Taking turn when game is finished");
            return Err(TurnError::GameIsFinished);
        }

        let player_order = self.players.iter().map(|p| p.id).collect::<Vec<PlayerId>>();

        let (mut fisher, target) =
            Self::find_hook_players(&mut self.players, self.player_turn, hook.target);

        let mut target = match target {
            Some(target) => target,
            None => {
                self.players.push(fisher);
                Self::reorder_players(&mut self.players, &player_order);
                debug!(hook.target = hook.target.0, "Target player was not found");
                return Err(TurnError::TargetNotFound(hook.target));
            }
        };

        let result = target.receive_hook(hook.rank);
        debug!(?target, ?hook, ?result, "Target received hook");

        match result.clone() {
            HookResult::Catch(catch) => {
                fisher.add_book(catch);
                let fisher = match fisher.hand.books.is_empty() {
                    true => Self::handle_active_player_has_empty_hand(fisher, &mut self.deck),
                    false => PlayerEmptyHandOutcome::Active(fisher),
                };

                debug!(?fisher, "Handled fisher hand state");
                match fisher {
                    PlayerEmptyHandOutcome::Active(fisher) => {
                        self.players.push(fisher);
                        self.players.push(target);
                        Self::reorder_players(&mut self.players, &player_order);
                    }
                    PlayerEmptyHandOutcome::Inactive(fisher) => {
                        self.players.push(target);
                        Self::reorder_players(&mut self.players, &player_order);
                        self.inactive_players.push(fisher);
                        self.player_turn = match self.player_turn {
                            0 => self.players.len() - 1,
                            n => n - 1,
                        };

                        self.advance_player_turn()
                    }
                }
            }
            HookResult::GoFish => {
                let draw = self.deck.draw();
                if let Some(card) = draw {
                    fisher.add_book(card.into())
                }

                self.players.push(fisher);
                self.players.push(target);
                Self::reorder_players(&mut self.players, &player_order);
                self.advance_player_turn()
            }
        };

        if self.players.is_empty() {
            self.is_finished = true;
        }

        debug!(game.players = ?self.players,
            game.inactive_players = ?self.inactive_players,
            game.is_deck_empty = self.deck.is_empty(),
            game.player_turn = self.player_turn,
            game.is_finished = self.is_finished,
            "Finished taking turn");

        Ok(result)
    }

    /// Get the current player
    pub fn get_current_player(&self) -> Option<&Player> {
        if self.is_finished {
            return None;
        }
        if self.players.is_empty() {
            warn!("Current game has no current player, is not finished");
            return None;
        }

        let player = self.players.get(self.player_turn);
        if player.is_none() {
            warn!(player_turn = self.player_turn, players = ?self.players, "player_turn index is out of bounds");
        }

        player
    }

    pub fn get_game_result(&self) -> Option<GameResult> {
        if !self.is_finished {
            return None;
        }

        let max_books = self.inactive_players.iter().map(|p| p.completed_books.len()).max().unwrap();
        let mut winners = vec![];
        let mut losers = vec![];
        for player in self.inactive_players.clone().into_iter() {
            if player.completed_books.len() == max_books {
                winners.push(player);
            } else {
                losers.push(player);
            }
        };
        Some(GameResult { winners, losers })
    }

    fn deal_player(id: PlayerId, hand_size: usize, deck: &mut Deck) -> Player {
        let mut hand = Hand::empty();
        let mut completed_books = vec![];

        for _ in 0..hand_size {
            let draw = deck.draw();
            let book = IncompleteBook::from(draw.unwrap());
            let completed_book = hand.add_book(book);
            if let Some(c) = completed_book {
                completed_books.push(c);
            }
        }

        Player {
            id,
            hand,
            completed_books,
        }
    }

    fn advance_player_turn(&mut self) {
        if self.players.is_empty() {
            return;
        }

        if self.players.len() == 1 {
            let player = self.players.remove(0);
            if !player.hand.books.is_empty() {
                panic!("Shouldn't get here I don't think")
            }
            let player = InactivePlayer { id: player.id, completed_books: player.completed_books };
            self.inactive_players.push(player);
            return;
        }

        let mut new_turn = (self.player_turn + 1) % self.players.len();
        let player_order = self.players.iter().map(|p| p.id).collect::<Vec<PlayerId>>();

        for _ in 1..self.players.len() {
            let current_player = self.players.remove(new_turn);
            let result = match current_player.hand.books.is_empty() {
                true => Self::handle_active_player_has_empty_hand(current_player, &mut self.deck),
                false => PlayerEmptyHandOutcome::Active(current_player),
            };
            let found_new_player = match result {
                PlayerEmptyHandOutcome::Active(player) => {
                    self.players.push(player);
                    Self::reorder_players(&mut self.players, &player_order);
                    true
                }
                PlayerEmptyHandOutcome::Inactive(player) => {
                    self.inactive_players.push(player);
                    Self::reorder_players(&mut self.players, &player_order);
                    false
                }
            };

            if found_new_player {
                break;
            }

            new_turn = (new_turn + 1) % self.players.len();
        }

        if self.players.len() == 1 {
            let player = self.players.remove(0);
            if !player.hand.books.is_empty() {
                panic!("Shouldn't get here I don't think")
            }
            let player = InactivePlayer { id: player.id, completed_books: player.completed_books };
            self.inactive_players.push(player);
            return;
        }

        self.player_turn = new_turn;
    }

    fn handle_active_player_has_empty_hand(
        mut player: Player,
        deck: &mut Deck,
    ) -> PlayerEmptyHandOutcome {
        let draw = deck.draw();
        match draw {
            Some(card) => {
                player.add_book(IncompleteBook::from(card));
                PlayerEmptyHandOutcome::Active(player)
            }
            None => PlayerEmptyHandOutcome::Inactive(InactivePlayer {
                id: player.id,
                completed_books: player.completed_books,
            }),
        }
    }

    fn find_hook_players(
        players: &mut Vec<Player>,
        current_player_index: usize,
        target_id: PlayerId,
    ) -> (Player, Option<Player>) {
        let fisher = players.remove(current_player_index);

        let target_index = players.iter().position(|p| p.id == target_id);
        let target = match target_index {
            Some(index) => players.remove(index),
            None => return (fisher, None),
        };

        (fisher, Some(target))
    }

    fn reorder_players(players: &mut [Player], order: &[PlayerId]) {
        players.sort_by_key(|p| order.iter().position(|pos| &p.id == pos).unwrap());
    }
}

#[derive(Debug)]
pub(crate) enum CombineBookResult {
    Combined(IncompleteBook),
    NotCombined(IncompleteBook, IncompleteBook),
    Completed(CompleteBook),
}

#[derive(Debug)]
enum PlayerEmptyHandOutcome {
    Active(Player),
    Inactive(InactivePlayer),
}

#[cfg(test)]
mod game_tests;
