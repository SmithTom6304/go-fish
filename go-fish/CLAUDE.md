# CLAUDE.md — go-fish (core engine)

## Commands

```bash
# Build / check
cargo build --package go-fish
cargo check --package go-fish
cargo clippy --package go-fish

# Test
cargo test --package go-fish                        # all tests
cargo test --package go-fish complete_games         # integration tests only
```

## Module overview

| File | Purpose |
|------|---------|
| `src/lib.rs` | All public types, trait implementations, and game logic |
| `src/bots.rs` | `Bot` trait, `BotObservation`, `OpponentView`, and `SimpleBot` implementation |
| `src/game_tests.rs` | Unit tests for `Deck`, `Hand`, and `Game` (included via `mod game_tests`) |
| `tests/complete_games.rs` | Integration tests: runs full games with a random AI for 2–6 players |
| `tests/drivers/mod.rs` | Random AI driver used by integration tests |

## Key types

| Type | Description |
|------|-------------|
| `Card` | A `suit: Suit` + `rank: Rank`. Copy. |
| `Suit` | `Clubs / Diamonds / Hearts / Spades`. Implements `Sequence` for iteration. |
| `Rank` | `Two` through `Ace` (13 values). Implements `Sequence`, `Ord`, `Display`. |
| `Deck` | `Vec<Card>` with `new()`, `shuffle()`, `draw()`, `is_empty()`. |
| `IncompleteBook` | 1–3 cards of the same rank. `combine()` merges two, optionally producing a `CompleteBook`. |
| `CompleteBook` | Exactly 4 cards of the same rank (`[Card; 4]`). Copy. |
| `Hand` | `Vec<IncompleteBook>`. `add_book()` adds/combines books; `receive_hook()` yields matching cards. |
| `Player` | Active player: `id: PlayerId`, `hand: Hand`, `completed_books: Vec<CompleteBook>`. |
| `InactivePlayer` | Eliminated player: `id: PlayerId`, `completed_books: Vec<CompleteBook>`. Still wins if they have the most books. |
| `PlayerId` | Newtype `u8`. Copy, Hash. |
| `Hook` | A turn request: `target: PlayerId` + `rank: Rank`. |
| `Game` | Top-level game state. `player_turn` is private. |

## Main API

```rust
// Create and start a game
let deck = Deck::new().shuffle();
let mut game = Game::new(deck, player_count);

// Execute a turn (the only way to mutate game state)
let result: Result<HookResult, TurnError> = game.take_turn(Hook { target, rank });

// Inspect state
let current: Option<&Player> = game.get_current_player();
let result: Option<GameResult>  = game.get_game_result(); // Some only when is_finished
```

`HookResult` variants: `Catch(IncompleteBook)` — target had the rank; `GoFish` — they didn't.
`TurnError` variants: `TargetNotFound(PlayerId)`, `GameIsFinished`.

## turn flow

`take_turn()` → validates game not finished → locates fisher + target via `find_hook_players()` → calls `Hand::receive_hook()` → on Catch: fisher adds book, may complete it → on GoFish: fisher draws from deck → `advance_player_turn()` handles player elimination (empty hand → draw or become inactive) → `is_finished = true` when all players are inactive.

`advance_player_turn()` and `handle_active_player_has_empty_hand()` are the most complex methods; they temporarily remove players from `self.players`, then restore order via `reorder_players()`.

## Dealing rules

`Game::new()` deals 7 cards per player for 2–3 players, 5 cards otherwise. Initial books are completed before play begins.

## Bots

`src/bots.rs` defines the bot interface and the bundled `SimpleBot` implementation.

### `Bot` trait

```rust
pub trait Bot: Send {
    fn observe(&mut self, observation: BotObservation);
    fn generate_hook(&mut self, valid_targets: &[PlayerId]) -> Hook;
}
```

`BotObservation` is the partial-information view a bot receives after every turn: own hand + completed books, opponent hand sizes + completed books, deck size, active player, and the last hook outcome. `OpponentView` carries per-opponent data.

### `SimpleBot`

A probability-table bot with two tunable parameters:

| Parameter | Type | Effect |
|-----------|------|--------|
| `memory_limit` | `u8` | Number of past observations retained. `0` = memoryless; picks a random valid move. |
| `error_margin` | `f32` | Std-dev of Gaussian noise added to each probability entry. `0.0` = deterministic best move. |

Algorithm: build a `(opponent, rank) → probability` table from retained observations using baseline proportional priors, then update with inference rules (Catch: target loses, fisher gains; GoFish: fisher holds, target lacks). Add `N(0, error_margin)` noise, clamp to `[0, 1]`, then pick the highest-scoring `(target, rank)` pair where the rank is in the bot's current hand.

`SimpleBot::new(my_id, memory_limit, error_margin, seed)` — seeded with `SmallRng` for deterministic replay.

## Testing

- `rstest` with `#[values(...)]` for parameterized unit tests.
- `tests/complete_games.rs` runs up to 10,000 random turns for each of 2–6 player counts and asserts the game finishes.
- `src/bots.rs` has unit tests for `observe` memory management, probability table updates, and `generate_hook` correctness.
- When adding new logic to `take_turn` or `advance_player_turn`, add a unit test in `src/game_tests.rs` covering the new edge case.
