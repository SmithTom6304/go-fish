mod drivers;

use go_fish::*;
use go_fish::bots::{Bot, BotObservation, OpponentView, SimpleBot};
use proptest::prelude::*;
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(20))]
    #[test]
    fn prop_simple_bot_game_always_terminates(
        player_count in 2u8..=6u8,
        seeds in proptest::collection::vec(any::<u64>(), 6),
        memory_limits in proptest::collection::vec(0u8..=10u8, 6),
        error_margins in proptest::collection::vec(0.0f32..=1.0f32, 6),
    ) {
        let max_turns = 10_000;
        let deck = Deck::new();
        let mut game = Game::new(deck, player_count);

        let mut bots: Vec<SimpleBot> = (0..player_count as usize)
            .map(|i| SimpleBot::new(
                PlayerId::new(i as u8),
                memory_limits[i],
                error_margins[i],
                seeds[i],
            ))
            .collect();

        let mut last_outcome: Option<HookOutcome> = None;

        for _ in 0..max_turns {
            let current_player = match game.get_current_player() {
                Some(p) => p.clone(),
                None => break,
            };

            let active_id = current_player.id;
            let deck_size = game.deck.cards.len();

            for bot in bots.iter_mut() {
                let bot_player = game.players.iter().find(|p| p.id == bot.my_id());
                let (my_hand, my_completed_books) = match bot_player {
                    Some(p) => (p.hand.books.clone(), p.completed_books.clone()),
                    None => (vec![], vec![]),
                };
                let opponents: Vec<OpponentView> = game.players.iter()
                    .filter(|p| p.id != bot.my_id())
                    .map(|p| OpponentView {
                        id: p.id,
                        hand_size: p.hand.books.iter().map(|b| b.cards.len()).sum(),
                        completed_books: p.completed_books.clone(),
                    })
                    .collect();
                bot.observe(BotObservation {
                    my_hand,
                    my_completed_books,
                    opponents,
                    deck_size,
                    active_player_id: active_id,
                    last_hook_outcome: last_outcome.clone(),
                });
            }

            let active_bot = bots.iter_mut().find(|b| b.my_id() == active_id).unwrap();
            let valid_targets: Vec<PlayerId> = game.players.iter()
                .filter(|p| p.id != active_id)
                .map(|p| p.id)
                .collect();

            let hook = active_bot.generate_hook(&valid_targets);
            let outcome = game.take_turn(hook).expect("Bot generated an invalid hook");
            last_outcome = Some(outcome);

            if game.players.len() <= 1 {
                break;
            }
        }

        prop_assert!(
            game.players.len() <= 1,
            "Bot game with {} players did not complete in {} turns",
            player_count,
            max_turns
        );
        prop_assert!(game.get_game_result().is_some(), "Game should have a result");
    }
}
