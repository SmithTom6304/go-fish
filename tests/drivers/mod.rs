use go_fish::*;
use rand::random_range;

pub fn _fish_from_ahead(game: &Game) -> Hook {
    let player = game.players.get(game.player_turn).unwrap();

    let mut rank = player
        .hand
        .books
        .iter()
        .map(|book| book.rank)
        .collect::<Vec<_>>();
    rank.sort();
    let rank = rank.first().unwrap();

    let player_ahead = _get_player_ahead(game.player_turn, &game.players);

    Hook {
        target: player_ahead,
        rank: *rank,
    }
}

pub fn fish_random_rank_and_player(game: &Game) -> Hook {
    let player = game.players.get(game.player_turn).unwrap();

    let mut rank = player
        .hand
        .books
        .iter()
        .map(|book| book.rank)
        .collect::<Vec<_>>();
    rank.sort();
    let r = random_range(0..rank.len());
    let rank = rank.get(r).unwrap();

    let mut r = random_range(0..game.players.len());
    if r == game.player_turn {
        r = (r + 1) % game.players.len();
    }
    let player_ahead = game.players.get(r).unwrap().id;

    Hook {
        target: player_ahead,
        rank: *rank,
    }
}

fn _get_player_ahead(index: usize, players: &[Player]) -> PlayerId {
    let new_index = (index + 1) % players.len();
    players.get(new_index).unwrap().id
}
