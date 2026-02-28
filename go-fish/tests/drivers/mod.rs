use go_fish::*;
use rand::random_range;

pub fn fish_random_rank_and_player(game: &Game) -> Hook {
    let player = game.get_current_player();

    let mut rank = player
        .hand
        .books
        .iter()
        .map(|book| book.rank)
        .collect::<Vec<_>>();
    rank.sort();
    let r = random_range(0..rank.len());
    let rank = rank.get(r).unwrap();

    let r = random_range(0..game.players.len());
    let mut random_player = game.players.get(r).unwrap();
    if random_player.id == player.id {
        random_player = game.players.get((r + 1) % game.players.len()).unwrap();
    }

    Hook {
        target: random_player.id,
        rank: *rank,
    }
}
