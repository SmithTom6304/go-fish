# go-fish

A Rust package for the classic Go Fish game.

## Example

```rust
let deck = Deck::new().shuffle();
let player_count = 3;
let mut game = Game::new(deck, player_count);

let hook = Hook{target: PlayerId(2), rank: Rank::Ace};

game.take_turn(hook);
```

## Roadmap

- Add a server and client binary to allow playing a game with peers

## Licence

This project is licenced under the MIT licence. See the LICENSE.md for more details.