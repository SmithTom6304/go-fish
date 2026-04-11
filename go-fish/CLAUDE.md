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

The entire library lives in a single file plus two test files:

| File | Purpose |
|------|---------|
| `src/lib.rs` | All public types, trait implementations, and game logic |
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

## Testing

- `rstest` with `#[values(...)]` for parameterized unit tests.
- `tests/complete_games.rs` runs up to 10,000 random turns for each of 2–6 player counts and asserts the game finishes.
- When adding new logic to `take_turn` or `advance_player_turn`, add a unit test in `src/game_tests.rs` covering the new edge case.
