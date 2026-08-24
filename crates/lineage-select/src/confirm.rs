//! The share confirmation: what it is asking, and which answer is focused.

use std::time::{Duration, Instant};

use crate::orb;

/// Which answer the cursor is on.
///
/// Starts on [`Choice::GoBack`] deliberately: the modal exists because sharing
/// is easy to trigger by accident, so the key already under the finger must be
/// the one that does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    GoBack,
    DoIt,
}

impl Choice {
    pub fn label(self) -> &'static str {
        match self {
            Choice::GoBack => "go back",
            Choice::DoIt => "do it",
        }
    }

    fn other(self) -> Self {
        match self {
            Choice::GoBack => Choice::DoIt,
            Choice::DoIt => Choice::GoBack,
        }
    }
}

/// What the confirmation is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Waiting for an answer.
    Asking,
    /// The share is running; the orb fills and tightens while it does.
    Working,
    /// The share succeeded and the burst is playing.
    Celebrating,
}

/// The share confirmation modal's state.
pub struct Confirm {
    answer: Choice,
    stage: Stage,
    started: Option<Instant>,
}

impl Default for Confirm {
    fn default() -> Self {
        Self::new()
    }
}

impl Confirm {
    pub fn new() -> Self {
        Self {
            answer: Choice::DoIt,
            stage: Stage::Asking,
            started: None,
        }
    }

    pub fn answer(&self) -> Choice {
        self.answer
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    /// Left and right are the only movement: the two answers sit side by side,
    /// and mapping vertical keys onto a horizontal choice reads as wrong without
    /// being obviously so.
    pub fn move_focus(&mut self) {
        if self.stage == Stage::Asking {
            self.answer = self.answer.other();
        }
    }

    pub fn focus(&mut self, answer: Choice) {
        if self.stage == Stage::Asking {
            self.answer = answer;
        }
    }

    /// Begin the share, starting the animation at the same moment.
    ///
    /// The two are deliberately uncoordinated: waiting for the network before
    /// moving leaves a dead pause exactly where the feedback should be. The
    /// animation is the acknowledgement that the key was pressed, and the share
    /// runs underneath it.
    pub fn begin_work(&mut self, now: Instant) {
        self.stage = Stage::Working;
        self.started = Some(now);
    }

    /// The share landed. The clock is already running, so this only records
    /// that the modal may close once the animation finishes.
    pub fn celebrate(&mut self, _now: Instant) {
        if self.stage == Stage::Working {
            self.stage = Stage::Celebrating;
        }
    }

    /// How far into the animation the modal is.
    pub fn elapsed(&self, now: Instant) -> Duration {
        self.started
            .map(|start| now.duration_since(start))
            .unwrap_or_default()
    }

    /// Whether the celebration has played out and the modal can close.
    pub fn is_finished(&self, now: Instant) -> bool {
        self.stage == Stage::Celebrating && self.elapsed(now) >= orb::duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_share_is_focused_first() {
        let confirm = Confirm::new();
        assert_eq!(confirm.answer(), Choice::DoIt);
    }

    #[test]
    fn focus_moves_between_the_two_answers() {
        let mut confirm = Confirm::new();
        confirm.move_focus();
        assert_eq!(confirm.answer(), Choice::GoBack);
        confirm.move_focus();
        assert_eq!(confirm.answer(), Choice::DoIt);
    }

    #[test]
    fn focus_is_locked_once_the_share_is_running() {
        let mut confirm = Confirm::new();
        confirm.focus(Choice::DoIt);
        confirm.begin_work(Instant::now());
        confirm.move_focus();
        assert_eq!(
            confirm.answer(),
            Choice::DoIt,
            "the answer cannot change under a share already sent"
        );
    }

    #[test]
    fn only_a_running_share_can_be_celebrated() {
        let mut confirm = Confirm::new();
        confirm.celebrate(Instant::now());
        assert_eq!(
            confirm.stage(),
            Stage::Asking,
            "a share that never started has nothing to celebrate"
        );

        confirm.begin_work(Instant::now());
        confirm.celebrate(Instant::now());
        assert_eq!(confirm.stage(), Stage::Celebrating);
    }

    #[test]
    fn the_animation_starts_when_the_key_is_pressed() {
        let mut confirm = Confirm::new();
        let pressed = Instant::now();
        confirm.begin_work(pressed);
        // Running from the press, not from the share landing: the animation is
        // the acknowledgement, so it cannot wait on the network.
        assert!(confirm.elapsed(pressed + orb::duration()) >= orb::duration());
    }

    #[test]
    fn the_modal_closes_only_after_the_burst_has_played() {
        let mut confirm = Confirm::new();
        let start = Instant::now();
        confirm.begin_work(start);
        confirm.celebrate(start);

        assert!(!confirm.is_finished(start));
        assert!(confirm.is_finished(start + orb::duration()));
    }

    #[test]
    fn a_share_still_asking_never_reports_finished() {
        let confirm = Confirm::new();
        assert!(!confirm.is_finished(Instant::now() + orb::duration()));
    }
}
