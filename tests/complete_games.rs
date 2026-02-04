mod drivers;

use go_fish::{Deck, GameState, game};
use rstest::rstest;

#[rstest]
fn completes_a_game(#[values(2, 3, 4, 5, 6)] players: u8) {
    let max_turns = 100;
    let deck = Deck::new();
    let mut game_state = GameState::new(deck, players);

    for turn in 1..max_turns {
        let hook = drivers::fish_from_ahead(&game_state);
        game_state = game::take_turn(game_state, hook);

        if game_state.is_completed() {
            break;
        }
    }

    assert!(
        game_state.is_completed(),
        "Game with {} players did not complete in {} turns",
        players,
        max_turns
    );
}
