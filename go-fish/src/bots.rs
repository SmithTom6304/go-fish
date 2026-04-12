use std::collections::VecDeque;

use rand::{RngExt as _, SeedableRng};
use rand::rngs::SmallRng;
use rand_distr::{Distribution, Normal};

use crate::{CompleteBook, Hook, HookOutcome, HookResult, IncompleteBook, PlayerId, Rank};
use enum_iterator::all;

/// The partial view of game state a bot receives after each turn.
/// Mirrors what a human player can observe: own hand fully visible,
/// opponents expose only hand size and completed books.
pub struct BotObservation {
    pub my_hand: Vec<IncompleteBook>,
    pub my_completed_books: Vec<CompleteBook>,
    pub opponents: Vec<OpponentView>,
    pub deck_size: usize,
    pub active_player_id: PlayerId,
    pub last_hook_outcome: Option<HookOutcome>,
}

/// An opponent as seen by a bot.
pub struct OpponentView {
    pub id: PlayerId,
    pub hand_size: usize,
    pub completed_books: Vec<CompleteBook>,
}

/// A bot that can observe game state and generate hooks.
pub trait Bot: Send {
    fn observe(&mut self, observation: BotObservation);
    fn generate_hook(&mut self, valid_targets: &[PlayerId]) -> Hook;
}

/// A probability-table bot with configurable memory depth and noise.
pub struct SimpleBot {
    my_id: PlayerId,
    memory_limit: u8,
    error_margin: f32,
    rng: SmallRng,
    observations: VecDeque<BotObservation>,
    /// Current hand — always updated on observe regardless of memory_limit,
    /// so generate_hook can always produce a valid rank.
    current_hand: Vec<IncompleteBook>,
}

impl SimpleBot {
    pub fn new(my_id: PlayerId, memory_limit: u8, error_margin: f32, seed: u64) -> Self {
        SimpleBot {
            my_id,
            memory_limit,
            error_margin,
            rng: SmallRng::seed_from_u64(seed),
            observations: VecDeque::new(),
            current_hand: Vec::new(),
        }
    }

    pub fn my_id(&self) -> PlayerId {
        self.my_id
    }
}

impl SimpleBot {
    /// Build a probability table: for every (opponent_id, rank) pair, estimate
    /// the probability that the opponent currently holds that rank.
    /// Iterates observations oldest→newest so later evidence overrides earlier.
    fn build_probability_table(&mut self, opponents: &[OpponentView]) -> Vec<(PlayerId, Rank, f32)> {
        let mut table: std::collections::HashMap<(PlayerId, Rank), f32> = std::collections::HashMap::new();

        // Compute baseline probabilities from the most recent observation.
        // Baseline = opponent.hand_size / total_remaining_cards_of_rank_in_unknown_hands
        if let Some(latest) = self.observations.back() {
            // Total cards remaining in all opponents' hands (unknown to us)
            let total_unknown: usize = latest.opponents.iter().map(|o| o.hand_size).sum();
            let ranks_in_deck: usize = latest.deck_size;
            let total_unknown_pool = total_unknown + ranks_in_deck;

            for rank in all::<Rank>() {
                // Cards of this rank we can account for: our own hand + our completed books
                let known_ours = latest.my_hand.iter().filter(|b| b.rank == rank).map(|b| b.cards.len()).sum::<usize>()
                    + latest.my_completed_books.iter().filter(|b| b.rank == rank).count() * 4
                    + opponents.iter().flat_map(|o| o.completed_books.iter()).filter(|b| b.rank == rank).count() * 4;
                let remaining = 4usize.saturating_sub(known_ours);

                for opp in opponents {
                    if total_unknown_pool > 0 {
                        let prob = (opp.hand_size as f32 * remaining as f32) / total_unknown_pool as f32;
                        table.insert((opp.id, rank), prob.min(1.0));
                    } else {
                        table.insert((opp.id, rank), 0.0);
                    }
                }
            }
        }

        // Iterate observations oldest→newest and apply inference rules.
        for obs in &self.observations {
            if let Some(outcome) = &obs.last_hook_outcome {
                match &outcome.result {
                    HookResult::Catch(_) => {
                        // Target lost these cards: set P=0 for target, P=1 for fisher (if opponent)
                        if outcome.target != self.my_id {
                            table.insert((outcome.target, outcome.rank), 0.0);
                        }
                        if outcome.fisher != self.my_id {
                            table.insert((outcome.fisher, outcome.rank), 1.0);
                        }
                    }
                    HookResult::GoFish => {
                        // Fisher asked for the rank, so they hold it
                        if outcome.fisher != self.my_id {
                            table.insert((outcome.fisher, outcome.rank), 1.0);
                        }
                        // Target doesn't have it
                        if outcome.target != self.my_id {
                            table.insert((outcome.target, outcome.rank), 0.0);
                        }
                    }
                }
            }
        }

        // Apply N(0, error_margin) noise to each entry and clamp to [0.0, 1.0].
        let noise_stddev = self.error_margin;
        table
            .into_iter()
            .map(|((id, rank), prob)| {
                let noisy = if noise_stddev > 0.0 {
                    let normal = Normal::new(0.0f32, noise_stddev).unwrap();
                    (prob + normal.sample(&mut self.rng)).clamp(0.0, 1.0)
                } else {
                    prob
                };
                (id, rank, noisy)
            })
            .collect()
    }
}

impl Bot for SimpleBot {
    fn observe(&mut self, observation: BotObservation) {
        // Always track the current hand so generate_hook can produce valid ranks
        // even when memory_limit == 0.
        self.current_hand = observation.my_hand.clone();
        if self.memory_limit == 0 {
            return;
        }
        self.observations.push_back(observation);
        while self.observations.len() > self.memory_limit as usize {
            self.observations.pop_front();
        }
    }

    fn generate_hook(&mut self, valid_targets: &[PlayerId]) -> Hook {
        // Derive hand and opponents from the most recent stored observation,
        // falling back to current_hand when memory_limit == 0.
        let (my_hand_ranks, opponents): (Vec<Rank>, Vec<OpponentView>) = match self.observations.back() {
            Some(obs) => {
                let ranks = obs.my_hand.iter().map(|b| b.rank).collect();
                let opps = obs.opponents.iter().map(|o| OpponentView {
                    id: o.id,
                    hand_size: o.hand_size,
                    completed_books: o.completed_books.clone(),
                }).collect();
                (ranks, opps)
            }
            None => {
                // memory_limit==0 or generate_hook called before any observe.
                // Use current_hand for a valid rank; no opponent info for probability table.
                let ranks: Vec<Rank> = self.current_hand.iter().map(|b| b.rank).collect();
                if ranks.is_empty() {
                    // Truly no information — return first valid target with a placeholder rank.
                    // take_turn will reject this if invalid, but this state should not occur
                    // in a well-driven game (active player always has cards).
                    return Hook { target: valid_targets[0], rank: Rank::Two };
                }
                (ranks, vec![])
            }
        };

        let table = self.build_probability_table(&opponents);

        // Filter to (target, rank) pairs where:
        // - we hold the rank
        // - the target is in valid_targets
        let best = table
            .iter()
            .filter(|(id, rank, _)| valid_targets.contains(id) && my_hand_ranks.contains(rank))
            .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

        match best {
            Some((target, rank, _)) => Hook { target: *target, rank: *rank },
            None => {
                // No table entry matched (zero-memory or no observations yet).
                // Pick randomly to avoid deterministic cycles when the deck empties.
                let target = valid_targets[self.rng.random_range(0..valid_targets.len())];
                let rank = my_hand_ranks[self.rng.random_range(0..my_hand_ranks.len())];
                Hook { target, rank }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Card, Rank, Suit};

    fn make_incomplete_book(rank: Rank, suits: &[Suit]) -> IncompleteBook {
        IncompleteBook {
            rank,
            cards: suits.iter().map(|&suit| Card { rank, suit }).collect(),
        }
    }

    fn make_observation(
        my_hand: Vec<IncompleteBook>,
        opponents: Vec<OpponentView>,
        deck_size: usize,
        last_hook_outcome: Option<HookOutcome>,
    ) -> BotObservation {
        BotObservation {
            my_hand,
            my_completed_books: vec![],
            opponents,
            deck_size,
            active_player_id: PlayerId::new(0),
            last_hook_outcome,
        }
    }

    // --- observe tests ---

    #[test]
    fn observe_adds_to_deque() {
        let mut bot = SimpleBot::new(PlayerId::new(0), 5, 0.0, 42);
        bot.observe(make_observation(vec![], vec![], 0, None));
        assert_eq!(bot.observations.len(), 1);
    }

    #[test]
    fn observe_drops_oldest_when_limit_exceeded() {
        let mut bot = SimpleBot::new(PlayerId::new(0), 3, 0.0, 42);
        for _ in 0..5 {
            bot.observe(make_observation(vec![], vec![], 0, None));
        }
        assert_eq!(bot.observations.len(), 3);
    }

    #[test]
    fn observe_memory_limit_zero_always_empty() {
        let mut bot = SimpleBot::new(PlayerId::new(0), 0, 0.0, 42);
        bot.observe(make_observation(vec![], vec![], 0, None));
        bot.observe(make_observation(vec![], vec![], 0, None));
        assert_eq!(bot.observations.len(), 0);
    }

    #[test]
    fn observe_memory_limit_one_retains_only_latest() {
        let mut bot = SimpleBot::new(PlayerId::new(0), 1, 0.0, 42);
        bot.observe(make_observation(
            vec![make_incomplete_book(Rank::Two, &[Suit::Clubs])],
            vec![],
            0,
            None,
        ));
        bot.observe(make_observation(
            vec![make_incomplete_book(Rank::Three, &[Suit::Clubs])],
            vec![],
            0,
            None,
        ));
        assert_eq!(bot.observations.len(), 1);
        assert_eq!(bot.observations[0].my_hand[0].rank, Rank::Three);
    }

    // --- probability table tests ---

    fn hook_outcome(fisher: u8, target: u8, rank: Rank, result: HookResult) -> HookOutcome {
        HookOutcome {
            fisher: PlayerId::new(fisher),
            target: PlayerId::new(target),
            rank,
            result,
        }
    }

    fn opponent(id: u8, hand_size: usize) -> OpponentView {
        OpponentView { id: PlayerId::new(id), hand_size, completed_books: vec![] }
    }

    fn prob_for(table: &[(PlayerId, Rank, f32)], id: u8, rank: Rank) -> Option<f32> {
        table.iter().find(|(p, r, _)| *p == PlayerId::new(id) && *r == rank).map(|(_, _, p)| *p)
    }

    #[test]
    fn probability_table_opponent_asks_sets_p1() {
        // Opponent 1 asked for Rank::Ace (GoFish) — they hold Ace
        let mut bot = SimpleBot::new(PlayerId::new(0), 5, 0.0, 42);
        let obs = make_observation(
            vec![make_incomplete_book(Rank::Two, &[Suit::Clubs])],
            vec![opponent(1, 3)],
            20,
            Some(hook_outcome(1, 2, Rank::Ace, HookResult::GoFish)),
        );
        bot.observe(obs);
        let table = bot.build_probability_table(&[opponent(1, 3)]);
        assert_eq!(prob_for(&table, 1, Rank::Ace), Some(1.0));
    }

    #[test]
    fn probability_table_successful_catch_target_p0_fisher_p1() {
        let book = make_incomplete_book(Rank::Ace, &[Suit::Hearts]);
        let mut bot = SimpleBot::new(PlayerId::new(0), 5, 0.0, 42);
        let obs = make_observation(
            vec![make_incomplete_book(Rank::Two, &[Suit::Clubs])],
            vec![opponent(1, 3), opponent(2, 3)],
            20,
            Some(hook_outcome(1, 2, Rank::Ace, HookResult::Catch(book))),
        );
        bot.observe(obs);
        let opps = vec![opponent(1, 3), opponent(2, 3)];
        let table = bot.build_probability_table(&opps);
        assert_eq!(prob_for(&table, 2, Rank::Ace), Some(0.0)); // target lost cards
        assert_eq!(prob_for(&table, 1, Rank::Ace), Some(1.0)); // fisher gained cards
    }

    #[test]
    fn probability_table_latest_state_wins() {
        // Opponent 1 catches Ace from opponent 2, then loses Ace back to opponent 2
        let book = make_incomplete_book(Rank::Ace, &[Suit::Hearts]);
        let mut bot = SimpleBot::new(PlayerId::new(0), 5, 0.0, 42);
        let obs1 = make_observation(
            vec![make_incomplete_book(Rank::Two, &[Suit::Clubs])],
            vec![opponent(1, 3), opponent(2, 3)],
            20,
            Some(hook_outcome(1, 2, Rank::Ace, HookResult::Catch(book.clone()))),
        );
        // Now opp2 fishes from opp1 successfully
        let obs2 = make_observation(
            vec![make_incomplete_book(Rank::Two, &[Suit::Clubs])],
            vec![opponent(1, 3), opponent(2, 3)],
            20,
            Some(hook_outcome(2, 1, Rank::Ace, HookResult::Catch(book))),
        );
        bot.observe(obs1);
        bot.observe(obs2);
        let opps = vec![opponent(1, 3), opponent(2, 3)];
        let table = bot.build_probability_table(&opps);
        assert_eq!(prob_for(&table, 1, Rank::Ace), Some(0.0)); // lost it second
        assert_eq!(prob_for(&table, 2, Rank::Ace), Some(1.0)); // gained it second
    }

    #[test]
    fn probability_table_ignores_outside_memory_window() {
        // memory_limit=1, so only last observation counts
        let book = make_incomplete_book(Rank::Ace, &[Suit::Hearts]);
        let mut bot = SimpleBot::new(PlayerId::new(0), 1, 0.0, 42);
        // Old obs: opp1 asked for Ace (GoFish), so P=1
        let old_obs = make_observation(
            vec![make_incomplete_book(Rank::Two, &[Suit::Clubs])],
            vec![opponent(1, 3)],
            20,
            Some(hook_outcome(1, 2, Rank::Ace, HookResult::GoFish)),
        );
        // New obs overrides: opp1 lost Ace (caught by opp2), so P=0
        let new_obs = make_observation(
            vec![make_incomplete_book(Rank::Two, &[Suit::Clubs])],
            vec![opponent(1, 3)],
            20,
            Some(hook_outcome(2, 1, Rank::Ace, HookResult::Catch(book))),
        );
        bot.observe(old_obs);
        bot.observe(new_obs); // evicts old_obs
        let opps = vec![opponent(1, 3)];
        let table = bot.build_probability_table(&opps);
        assert_eq!(prob_for(&table, 1, Rank::Ace), Some(0.0));
    }

    // --- generate_hook tests ---

    #[test]
    fn generate_hook_returns_rank_in_hand() {
        let mut bot = SimpleBot::new(PlayerId::new(0), 5, 0.0, 42);
        bot.observe(make_observation(
            vec![make_incomplete_book(Rank::Seven, &[Suit::Clubs])],
            vec![opponent(1, 3)],
            20,
            None,
        ));
        let hook = bot.generate_hook(&[PlayerId::new(1)]);
        assert_eq!(hook.rank, Rank::Seven);
    }

    #[test]
    fn generate_hook_returns_valid_target() {
        let mut bot = SimpleBot::new(PlayerId::new(0), 5, 0.0, 42);
        bot.observe(make_observation(
            vec![make_incomplete_book(Rank::Seven, &[Suit::Clubs])],
            vec![opponent(1, 3), opponent(2, 3)],
            20,
            None,
        ));
        let valid = [PlayerId::new(1), PlayerId::new(2)];
        let hook = bot.generate_hook(&valid);
        assert!(valid.contains(&hook.target));
    }

    #[test]
    fn generate_hook_zero_memory_still_valid() {
        let mut bot = SimpleBot::new(PlayerId::new(0), 0, 0.0, 42);
        // With memory_limit=0 observe is a no-op; we need to give it at least one observation
        // to know the hand. We can't — so the fallback path kicks in when no observations exist.
        // Instead test with memory_limit=1:
        let mut bot2 = SimpleBot::new(PlayerId::new(0), 1, 0.0, 42);
        bot2.observe(make_observation(
            vec![make_incomplete_book(Rank::King, &[Suit::Spades])],
            vec![opponent(1, 2)],
            10,
            None,
        ));
        let hook = bot2.generate_hook(&[PlayerId::new(1)]);
        assert_eq!(hook.rank, Rank::King);
        assert_eq!(hook.target, PlayerId::new(1));

        // memory_limit=0 bot: generate_hook fallback returns without panicking
        let _ = bot.generate_hook(&[PlayerId::new(1)]);
    }

    #[test]
    fn generate_hook_behavioural_asks_informed_target() {
        // Opp1 previously asked for Rank::Queen (GoFish) so they hold it.
        // We also hold Queen. With error_margin=0, bot should ask opp1 for Queen.
        let mut bot = SimpleBot::new(PlayerId::new(0), 5, 0.0, 42);
        bot.observe(make_observation(
            vec![make_incomplete_book(Rank::Queen, &[Suit::Clubs])],
            vec![opponent(1, 3), opponent(2, 3)],
            20,
            Some(hook_outcome(1, 2, Rank::Queen, HookResult::GoFish)),
        ));
        let hook = bot.generate_hook(&[PlayerId::new(1), PlayerId::new(2)]);
        assert_eq!(hook.rank, Rank::Queen);
        assert_eq!(hook.target, PlayerId::new(1));
    }

    #[test]
    fn generate_hook_noise_produces_varied_choices() {
        // With a high error_margin, repeated calls should not all return the same target.
        // Use two opponents so there's actually a choice.
        let mut bot = SimpleBot::new(PlayerId::new(0), 5, 2.0, 99);
        bot.observe(make_observation(
            vec![
                make_incomplete_book(Rank::Two, &[Suit::Clubs]),
                make_incomplete_book(Rank::Three, &[Suit::Hearts]),
            ],
            vec![opponent(1, 3), opponent(2, 3)],
            20,
            None,
        ));
        let valid = [PlayerId::new(1), PlayerId::new(2)];
        let results: Vec<_> = (0..50).map(|_| {
            // re-observe each time so the hand is fresh
            bot.observe(make_observation(
                vec![
                    make_incomplete_book(Rank::Two, &[Suit::Clubs]),
                    make_incomplete_book(Rank::Three, &[Suit::Hearts]),
                ],
                vec![opponent(1, 3), opponent(2, 3)],
                20,
                None,
            ));
            bot.generate_hook(&valid)
        }).collect();

        // Verify all results are valid
        for h in &results {
            assert!(valid.contains(&h.target));
        }

        // With noise there should be at least two distinct (target, rank) combos
        let distinct: std::collections::HashSet<(u8, Rank)> = results
            .iter()
            .map(|h| (h.target.0, h.rank))
            .collect();
        assert!(distinct.len() > 1, "Expected varied choices with high noise");
    }
}
