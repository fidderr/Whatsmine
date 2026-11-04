//! When to try someone again. At once on a network change, on opening the app and on seeing them;
//! between those it follows this ladder, so a phone that is off for days costs next to nothing.

use std::time::Duration;

const LADDER: [Duration; 6] = [
    Duration::from_secs(30),
    Duration::from_secs(2 * 60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(30 * 60),
    Duration::from_secs(60 * 60),
];

/// While the app is in use the radio is on anyway, and someone is looking at the screen.
const LADDER_IN_USE: [Duration; 3] = [
    Duration::from_secs(3),
    Duration::from_secs(10),
    Duration::from_secs(30),
];

/// How long to wait after the `failures`th failed try in a row (the first is 1).
pub fn delay_after(failures: u32) -> Duration {
    step(&LADDER, failures)
}

/// The same while the app is in use.
pub fn delay_in_use(failures: u32) -> Duration {
    step(&LADDER_IN_USE, failures)
}

fn step(ladder: &[Duration], failures: u32) -> Duration {
    let step = failures.saturating_sub(1) as usize;
    ladder[step.min(ladder.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_climbs_and_stays_at_an_hour() {
        let minutes: Vec<u64> = (1..=8).map(|n| delay_after(n).as_secs()).collect();
        assert_eq!(minutes, [30, 120, 300, 900, 1800, 3600, 3600, 3600]);
    }

    #[test]
    fn in_use_it_stays_quick() {
        let seconds: Vec<u64> = (1..=5).map(|n| delay_in_use(n).as_secs()).collect();
        assert_eq!(seconds, [3, 10, 30, 30, 30]);
    }

    #[test]
    fn nothing_failed_yet_is_the_first_step() {
        assert_eq!(delay_after(0), Duration::from_secs(30));
        assert_eq!(delay_after(u32::MAX), Duration::from_secs(3600));
    }
}
