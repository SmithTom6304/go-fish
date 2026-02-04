use go_fish::GameState;
use go_fish::Player;
use go_fish::hook::Hook;
use go_fish::player::PlayerId;

pub fn fish_from_ahead(game_state: &GameState) -> Hook {
    let player = game_state.players.get(game_state.player_turn).unwrap();
    assert!(player.active);

    let mut rank = player
        .hand
        .books
        .iter()
        .map(|book| book.rank)
        .collect::<Vec<_>>();
    rank.sort();
    let rank = rank.first().unwrap();

    let player_ahead = get_player_ahead(game_state.player_turn, &game_state.players);

    Hook {
        fisher: player.id,
        target: player_ahead,
        rank: rank.clone(),
    }
}

fn get_player_ahead(index: usize, players: &Vec<Player>) -> PlayerId {
    let mut new_index = (index + 1) % players.len();
    for _ in players {
        let p = players.get(new_index).unwrap();
        if p.active {
            return p.id;
        }
        new_index = (new_index + 1) % players.len();
    }
    panic!("All players inactive!")
}
