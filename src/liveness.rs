//! Is the TV there? A small state machine that turns probe and command
//! outcomes into Online / Unreachable without flapping.
//!
//! A Roku that is busy launching a channel can sit on `/query/device-info`
//! (and on the `/launch` POST itself) for several seconds, so one missed
//! request means nothing. The rules:
//!
//! - Unreachable only after [`FAILS_FOR_UNREACHABLE`] consecutive network
//!   failures, or two or more failures spread over [`FAILING_FOR_UNREACHABLE`].
//! - Any answer from the TV (a 2xx command, a probe, even an HTTP error
//!   status) is proof of life and resets the counter at once.
//! - For [`BUSY_QUIET`] after a command is sent the TV is known to be busy:
//!   the periodic probe is skipped, and a probe that was already in flight
//!   when the command went out does not count if it fails.
//! - "Search Again" is only worth offering once the TV has been Unreachable
//!   for [`SEARCH_OFFER_AFTER`].
//!
//! Pure logic with caller-supplied `Instant`s, so it is unit-testable and the
//! UI never blocks on it.

use std::time::{Duration, Instant};

use crate::ecp;

/// Consecutive network failures before the TV is shown as Unreachable.
pub const FAILS_FOR_UNREACHABLE: u32 = 3;
/// ...or this long since the first of at least two failures in a row.
pub const FAILING_FOR_UNREACHABLE: Duration = Duration::from_secs(15);
/// After a keypress or launch the TV is busy; do not probe it for this long.
pub const BUSY_QUIET: Duration = Duration::from_secs(5);
/// Unreachable for at least this long before a failed action offers
/// "Search Again".
pub const SEARCH_OFFER_AFTER: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Online,
    Unreachable,
}

#[derive(Debug, Clone, Default)]
pub struct Liveness {
    fails: u32,
    first_fail: Option<Instant>,
    unreachable_since: Option<Instant>,
    last_command: Option<Instant>,
}

impl Liveness {
    /// A freshly connected TV: online, no failures.
    pub fn new() -> Liveness {
        Liveness::default()
    }

    pub fn status(&self) -> Status {
        if self.unreachable_since.is_some() {
            Status::Unreachable
        } else {
            Status::Online
        }
    }

    pub fn is_online(&self) -> bool {
        self.status() == Status::Online
    }

    /// Consecutive network failures so far.
    pub fn failures(&self) -> u32 {
        self.fails
    }

    /// Back to a clean slate (after a successful (re)connect).
    pub fn reset(&mut self) {
        *self = Liveness::default();
    }

    /// Should the periodic probe run now? No while the TV is busy with a
    /// command we just sent.
    pub fn should_probe(&self, now: Instant) -> bool {
        match self.last_command {
            Some(t) => now.saturating_duration_since(t) >= BUSY_QUIET,
            None => true,
        }
    }

    /// A command is about to go out.
    pub fn command_sent(&mut self, now: Instant) {
        self.last_command = Some(now);
    }

    /// The TV answered something: proof of life.
    pub fn alive(&mut self) -> Status {
        self.fails = 0;
        self.first_fail = None;
        self.unreachable_since = None;
        Status::Online
    }

    /// A request got no answer (timeout, refused, unreachable).
    pub fn failure(&mut self, now: Instant) -> Status {
        self.fails = self.fails.saturating_add(1);
        let first = *self.first_fail.get_or_insert(now);
        let long_enough =
            self.fails >= 2 && now.saturating_duration_since(first) >= FAILING_FOR_UNREACHABLE;
        if self.fails >= FAILS_FOR_UNREACHABLE || long_enough {
            self.unreachable_since.get_or_insert(now);
        }
        self.status()
    }

    /// Feed the result of a probe that was started at `started`.
    pub fn probe_result<T>(
        &mut self,
        result: &ecp::Result<T>,
        started: Instant,
        now: Instant,
    ) -> Status {
        match result {
            Err(ecp::Error::Transport(_)) => {
                if self.last_command.is_some_and(|t| t >= started) {
                    // the TV got busy with a command mid-probe; not evidence
                    self.status()
                } else {
                    self.failure(now)
                }
            }
            // any HTTP answer, even an error status or junk, means it is there
            _ => self.alive(),
        }
    }

    /// Feed the result of a keypress / launch / text command. The TV is
    /// busy for [`BUSY_QUIET`] after it answers, too.
    pub fn command_result<T>(&mut self, result: &ecp::Result<T>, now: Instant) -> Status {
        self.last_command = Some(now);
        match result {
            Err(ecp::Error::Transport(_)) => self.failure(now),
            _ => self.alive(),
        }
    }

    /// How long the TV has been shown as Unreachable.
    pub fn unreachable_for(&self, now: Instant) -> Option<Duration> {
        self.unreachable_since
            .map(|t| now.saturating_duration_since(t))
    }

    /// After a user action failed with a network error: is it time to offer
    /// "Search Again"? Only once the TV has been gone for a while.
    pub fn offer_search(&self, now: Instant) -> bool {
        self.unreachable_for(now)
            .is_some_and(|d| d >= SEARCH_OFFER_AFTER)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net() -> ecp::Result<()> {
        Err(ecp::Error::Transport("timed out".into()))
    }

    fn secs(t0: Instant, s: u64) -> Instant {
        t0 + Duration::from_secs(s)
    }

    #[test]
    fn one_miss_is_not_offline() {
        let t0 = Instant::now();
        let mut lv = Liveness::new();
        assert_eq!(lv.probe_result(&net(), t0, t0), Status::Online);
        assert_eq!(
            lv.probe_result(&Ok(()), secs(t0, 10), secs(t0, 10)),
            Status::Online
        );
        assert_eq!(lv.failures(), 0);
    }

    #[test]
    fn three_misses_in_a_row_are_offline() {
        let t0 = Instant::now();
        let mut lv = Liveness::new();
        assert_eq!(lv.failure(t0), Status::Online);
        assert_eq!(lv.failure(secs(t0, 1)), Status::Online);
        assert_eq!(lv.failure(secs(t0, 2)), Status::Unreachable);
    }

    #[test]
    fn two_misses_fifteen_seconds_apart_are_offline() {
        let t0 = Instant::now();
        let mut lv = Liveness::new();
        assert_eq!(lv.failure(t0), Status::Online);
        assert_eq!(lv.failure(secs(t0, 14)), Status::Online);
        let mut lv = Liveness::new();
        lv.failure(t0);
        assert_eq!(lv.failure(secs(t0, 15)), Status::Unreachable);
    }

    #[test]
    fn single_miss_never_offline_however_long() {
        let t0 = Instant::now();
        let mut lv = Liveness::new();
        lv.failure(t0);
        lv.alive();
        assert_eq!(lv.failure(secs(t0, 60)), Status::Online);
    }

    #[test]
    fn command_success_resets_and_recovers() {
        let t0 = Instant::now();
        let mut lv = Liveness::new();
        lv.failure(t0);
        lv.failure(secs(t0, 1));
        assert_eq!(lv.command_result(&Ok(()), secs(t0, 2)), Status::Online);
        assert_eq!(lv.failures(), 0);
        // needs three fresh misses again
        assert_eq!(lv.failure(secs(t0, 3)), Status::Online);
        // and from Unreachable a 2xx brings it straight back
        lv.failure(secs(t0, 4));
        assert_eq!(lv.failure(secs(t0, 5)), Status::Unreachable);
        assert_eq!(lv.command_result(&Ok(()), secs(t0, 6)), Status::Online);
    }

    #[test]
    fn http_error_status_is_proof_of_life() {
        let t0 = Instant::now();
        let mut lv = Liveness::new();
        lv.failure(t0);
        lv.failure(t0);
        let rejected: ecp::Result<()> = Err(ecp::Error::Status {
            code: 404,
            path: "/launch/x".into(),
        });
        assert_eq!(lv.command_result(&rejected, t0), Status::Online);
        assert_eq!(lv.failures(), 0);
    }

    #[test]
    fn probe_is_skipped_while_tv_is_busy() {
        let t0 = Instant::now();
        let mut lv = Liveness::new();
        assert!(lv.should_probe(t0));
        lv.command_sent(t0);
        assert!(!lv.should_probe(secs(t0, 1)));
        assert!(!lv.should_probe(t0 + Duration::from_millis(4999)));
        assert!(lv.should_probe(secs(t0, 5)));
    }

    #[test]
    fn busy_window_runs_from_the_answer_of_a_slow_launch() {
        let t0 = Instant::now();
        let mut lv = Liveness::new();
        lv.command_sent(t0);
        // the launch took 8 s to answer
        lv.command_result(&Ok(()), secs(t0, 8));
        assert!(!lv.should_probe(secs(t0, 12)));
        assert!(lv.should_probe(secs(t0, 13)));
    }

    #[test]
    fn probe_failure_overlapping_a_command_is_ignored() {
        let t0 = Instant::now();
        let mut lv = Liveness::new();
        lv.failure(t0);
        lv.failure(t0);
        // probe started at 10, a launch went out at 11, probe timed out at 13
        lv.command_sent(secs(t0, 11));
        assert_eq!(
            lv.probe_result(&net(), secs(t0, 10), secs(t0, 13)),
            Status::Online
        );
        assert_eq!(lv.failures(), 2);
    }

    #[test]
    fn search_offer_waits_fifteen_seconds_of_unreachable() {
        let t0 = Instant::now();
        let mut lv = Liveness::new();
        assert!(!lv.offer_search(t0));
        lv.failure(t0);
        lv.failure(t0);
        lv.failure(secs(t0, 20));
        assert_eq!(lv.status(), Status::Unreachable);
        assert!(!lv.offer_search(secs(t0, 21)));
        assert!(!lv.offer_search(secs(t0, 34)));
        assert!(lv.offer_search(secs(t0, 35)));
        lv.alive();
        assert!(!lv.offer_search(secs(t0, 60)));
    }
}
