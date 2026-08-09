//! Totals that outlive the server process.
//!
//! `/metrics` counters restart at zero every time `llama-server` does — which
//! this app makes happen, every time you go and play something. To show what
//! the card has actually chewed through, the counters have to be banked across
//! restarts.
//!
//! The rule is the one any counter-scraper uses: a counter that went *down*
//! means the process behind it restarted, so whatever was last seen belongs to
//! a finished run and gets added to the running total.
//!
//! Undercounting is possible and deliberate. Sampling only happens while the
//! menu is open, so a run that starts and ends entirely between two openings
//! contributes whatever was last observed rather than its true final value.
//! Guessing at the gap would be worse than being slightly conservative.

use serde::{Deserialize, Serialize};

/// The cumulative counters `/metrics` exposes, as of one observation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Counters {
    pub prompt_tokens: u64,
    pub predicted_tokens: u64,
    pub generating_seconds: f64,
}

impl std::ops::AddAssign for Counters {
    fn add_assign(&mut self, other: Self) {
        self.prompt_tokens += other.prompt_tokens;
        self.predicted_tokens += other.predicted_tokens;
        self.generating_seconds += other.generating_seconds;
    }
}

/// Everything ever, split into runs that have finished and the one in progress.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lifetime {
    /// Runs that have ended.
    banked: Counters,
    /// The most recent reading from the run that is still going.
    current: Counters,
}

impl Lifetime {
    /// Fold in one observation. `None` means the server is not up.
    pub fn observe(&mut self, seen: Option<Counters>) {
        match seen {
            // Counters went backwards: a new process is behind them, so the
            // previous run is over and its last reading is final.
            Some(counters) if counters.predicted_tokens < self.current.predicted_tokens => {
                self.bank();
                self.current = counters;
            }
            Some(counters) => self.current = counters,
            None => self.bank(),
        }
    }

    fn bank(&mut self) {
        self.banked += self.current;
        self.current = Counters::default();
    }

    /// Everything, finished runs and the live one together.
    pub fn total(&self) -> Counters {
        let mut total = self.banked;
        total += self.current;
        total
    }
}

/// `9_234_567` as `"9.2M"`.
///
/// A panel menu has room for a number, not for nine digits and four separators.
pub fn compact(value: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")];

    for (scale, suffix) in UNITS {
        if value >= scale {
            let scaled = value as f64 / scale as f64;
            // 9.2M, but 12M rather than 12.3M — three significant figures is
            // more precision than anyone reads off a menu.
            return if scaled < 10.0 {
                format!("{scaled:.1}{suffix}")
            } else {
                format!("{scaled:.0}{suffix}")
            };
        }
    }
    value.to_string()
}

/// `86_400` seconds as `"1d 0h"`. Always two units, largest first.
pub fn duration(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (days, hours, minutes) = (total / 86_400, (total % 86_400) / 3600, (total % 3600) / 60);

    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

/// Roughly how many English words a token count is worth.
///
/// The usual rule of thumb, ~0.75 words per token. Approximate on purpose —
/// it is there to make the magnitude legible, not to be audited.
pub fn words(tokens: u64) -> u64 {
    (tokens as f64 * 0.75) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counters(prompt: u64, predicted: u64, seconds: f64) -> Counters {
        Counters {
            prompt_tokens: prompt,
            predicted_tokens: predicted,
            generating_seconds: seconds,
        }
    }

    #[test]
    fn a_rising_counter_replaces_rather_than_accumulates() {
        // The counter is already cumulative within a run; adding each reading
        // would multiply the total by the number of times the menu was opened.
        let mut lifetime = Lifetime::default();
        lifetime.observe(Some(counters(10, 20, 1.0)));
        lifetime.observe(Some(counters(30, 60, 3.0)));
        assert_eq!(lifetime.total(), counters(30, 60, 3.0));
    }

    #[test]
    fn a_counter_that_went_backwards_banks_the_finished_run() {
        // Stop for a game, start again: the new process counts from zero, and
        // the 60 tokens of the old one must not vanish.
        let mut lifetime = Lifetime::default();
        lifetime.observe(Some(counters(30, 60, 3.0)));
        lifetime.observe(Some(counters(5, 10, 0.5)));
        assert_eq!(lifetime.total(), counters(35, 70, 3.5));
    }

    #[test]
    fn the_server_going_away_banks_the_run() {
        let mut lifetime = Lifetime::default();
        lifetime.observe(Some(counters(30, 60, 3.0)));
        lifetime.observe(None);
        assert_eq!(lifetime.total(), counters(30, 60, 3.0));
    }

    #[test]
    fn a_run_is_never_banked_twice_while_the_server_stays_down() {
        // The menu can be opened any number of times with nothing running.
        let mut lifetime = Lifetime::default();
        lifetime.observe(Some(counters(30, 60, 3.0)));
        lifetime.observe(None);
        lifetime.observe(None);
        lifetime.observe(None);
        assert_eq!(lifetime.total(), counters(30, 60, 3.0));
    }

    #[test]
    fn a_full_stop_and_restart_cycle_adds_up() {
        let mut lifetime = Lifetime::default();
        lifetime.observe(Some(counters(100, 200, 10.0)));
        lifetime.observe(None); // stopped to play something
        lifetime.observe(Some(counters(7, 9, 0.5))); // started again
        lifetime.observe(Some(counters(70, 90, 5.0)));
        assert_eq!(lifetime.total(), counters(170, 290, 15.0));
    }

    #[test]
    fn nothing_observed_yet_totals_zero() {
        assert_eq!(Lifetime::default().total(), Counters::default());
    }

    #[test]
    fn totals_survive_a_round_trip_through_the_state_file() {
        let mut lifetime = Lifetime::default();
        lifetime.observe(Some(counters(100, 200, 10.0)));
        lifetime.observe(None);
        lifetime.observe(Some(counters(5, 6, 0.5)));

        let json = serde_json::to_string(&lifetime).unwrap();
        let restored: Lifetime = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, lifetime);
        assert_eq!(restored.total(), counters(105, 206, 10.5));
    }

    #[test]
    fn large_numbers_are_shortened_to_something_readable() {
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1_000), "1.0K");
        assert_eq!(compact(56_886), "57K");
        assert_eq!(compact(9_234_567), "9.2M");
        assert_eq!(compact(12_300_000), "12M");
        assert_eq!(compact(4_000_000_000), "4.0B");
    }

    #[test]
    fn a_count_below_a_thousand_is_shown_exactly() {
        // Early on the number is small, and "0.1K" would look broken.
        assert_eq!(compact(0), "0");
        assert_eq!(compact(7), "7");
    }

    #[test]
    fn durations_show_the_two_largest_units() {
        assert_eq!(duration(0.0), "0m");
        assert_eq!(duration(90.0), "1m");
        assert_eq!(duration(3_600.0), "1h 0m");
        assert_eq!(duration(83_700.0), "23h 15m");
        assert_eq!(duration(90_000.0), "1d 1h");
    }

    #[test]
    fn a_negative_duration_does_not_wrap_around() {
        // f64 -> u64 on a negative value saturates rather than wrapping, but
        // the clamp makes that explicit rather than incidental.
        assert_eq!(duration(-5.0), "0m");
    }

    #[test]
    fn tokens_convert_to_a_rough_word_count() {
        assert_eq!(words(1_000_000), 750_000);
        assert_eq!(words(0), 0);
    }
}
