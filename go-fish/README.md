# go-fish

A pure Rust library implementing the core engine for the classic Go Fish card game.

## Features

- Complete game logic: dealing, turn resolution, book completion, player elimination, and win detection
- Serialization-ready — all public types derive `serde::Serialize` / `Deserialize`
- Instrumented with `tracing` for debug-level turn logging
- No async, no I/O — plain library crate, suitable for embedding in any host (server, TUI, WASM, etc.)

## Quick start

```rust
use go_fish::{Deck, Game, Hook, PlayerId, Rank};

let deck = Deck::new().shuffle();
let mut game = Game::new(deck, /* player_count */ 2);

while !game.is_finished {
    let current = game.get_current_player().unwrap();
    // ... choose a target and rank ...
    let hook = Hook { target: PlayerId::new(1), rank: Rank::Seven };
    match game.take_turn(hook) {
        Ok(result) => { /* handle HookResult::Catch or HookResult::GoFish */ }
        Err(e)     => { /* handle TurnError */ }
    }
}

let result = game.get_game_result().unwrap();
println!("Winners: {:?}", result.winners);
```

## API overview

### Types

| Type | Description |
|------|-------------|
| `Card` | A single playing card (`suit`, `rank`). `Copy`. |
| `Suit` | `Clubs`, `Diamonds`, `Hearts`, `Spades`. |
| `Rank` | `Two` through `Ace`. Implements `Ord` and `Display`. |
| `Deck` | A 52-card deck. Call `.shuffle()` before passing to `Game::new`. |
| `Hand` | A player's in-hand cards, grouped into `IncompleteBook`s by rank. |
| `IncompleteBook` | 1–3 cards of the same rank. |
| `CompleteBook` | All 4 cards of a rank. Contributes to a player's score. |
| `Player` | An active player: ID, hand, and completed books. |
| `InactivePlayer` | An eliminated player (empty hand, deck exhausted). Still scored at game end. |
| `PlayerId` | Newtype wrapper around `u8`. |
| `Hook` | A turn action: ask `target` for all cards of `rank`. |
| `Game` | Top-level game state. |

### `Game::new(deck: Deck, player_count: u8) -> Game`

Creates a game and deals initial hands (7 cards for 2–3 players, 5 cards otherwise). Completes any initial books before returning.

### `Game::take_turn(hook: Hook) -> Result<HookResult, TurnError>`

The single method that advances game state. Returns:
- `Ok(HookResult::Catch(book))` — the target held cards of the requested rank; they're transferred to the current player.
- `Ok(HookResult::GoFish)` — the target had nothing; the current player draws from the deck.
- `Err(TurnError::TargetNotFound(id))` — the target ID doesn't refer to an active player.
- `Err(TurnError::GameIsFinished)` — the game is already over.

After each turn the engine automatically handles book completion, player elimination (empty hand → draw or become inactive), and win detection.

### `Game::get_current_player() -> Option<&Player>`

Returns the player whose turn it is, or `None` if the game is finished.

### `Game::get_game_result() -> Option<GameResult>`

Returns winners and losers by completed-book count once `game.is_finished` is `true`.

## Testing

```bash
cargo test --package go-fish                    # unit + integration tests
cargo test --package go-fish complete_games     # full-game simulation (2–6 players)
```

Integration tests in `tests/complete_games.rs` simulate up to 10,000 random turns for each player count (2–6) and assert the game terminates correctly.

## License

MIT
