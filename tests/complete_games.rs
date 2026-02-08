mod drivers;

use go_fish::*;
use rstest::rstest;

#[rstest]
fn completes_a_game(#[values(2, 3, 4, 5, 6)] players: u8) {
    let max_turns = 10_000;
    let deck = Deck::new();
    let mut game = Game::new(deck, players);

    for _ in 1..max_turns {
        let hook = drivers::fish_random_rank_and_player(&game);
        game.take_turn(hook).expect("Game state should be valid");

        if game.players.len() <= 1 {
            break;
        }
    }

    assert!(
        game.players.len() <= 1,
        "Game with {} players did not complete in {} turns",
        players,
        max_turns
    );
}
