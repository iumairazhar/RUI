//! The five owner-facing status words and how they look.

use super::character::CharState;
use crate::api::OwnerState;
use crate::theme::{Status, Theme};
use std::collections::HashMap;

/// One of the five status words shown everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Word {
    NeedsYou,
    Working,
    Waiting,
    Stopped,
    Done,
}

impl Word {
    /// Parse the daemon's word; anything unknown reads as Waiting.
    pub fn from_str(s: &str) -> Word {
        match s {
            "Needs you" => Word::NeedsYou,
            "Working" => Word::Working,
            "Stopped" => Word::Stopped,
            "Done" => Word::Done,
            _ => Word::Waiting,
        }
    }

    /// The word as shown to the owner.
    pub fn label(self) -> &'static str {
        match self {
            Word::NeedsYou => "Needs you",
            Word::Working => "Working",
            Word::Waiting => "Waiting",
            Word::Stopped => "Stopped",
            Word::Done => "Done",
        }
    }
}

/// A thread's status word from its daemon `state` (see `thread_state` in rui-daemon's thread.rs).
/// A failed step needs the owner; a step the owner took over counts as Working. Unknown reads as Waiting.
pub fn thread_word(state: &str) -> Word {
    match state {
        "needs_you" | "failed" => Word::NeedsYou,
        "running" | "owner" => Word::Working,
        "done" => Word::Done,
        _ => Word::Waiting,
    }
}

/// A task's status word from the daemon's states; a task with no state yet reads as Waiting.
pub fn word_of(states: &HashMap<i64, OwnerState>, id: i64) -> Word {
    states.get(&id).map_or(Word::Waiting, |o| Word::from_str(&o.word))
}

/// Colour family for a status word. Stopped is muted like Standing (the wait family).
pub fn style(w: Word, th: &Theme) -> &Status {
    match w {
        Word::NeedsYou => &th.block,
        Word::Working => &th.run,
        Word::Waiting | Word::Stopped => &th.wait,
        Word::Done => &th.done,
    }
}

/// The character pose for a status word.
pub fn char_state(w: Word) -> CharState {
    match w {
        Word::NeedsYou => CharState::Blocked,
        Word::Working => CharState::Doing,
        Word::Waiting | Word::Stopped => CharState::Standing,
        Word::Done => CharState::Landed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_words_and_defaults_to_waiting() {
        for w in [Word::NeedsYou, Word::Working, Word::Waiting, Word::Stopped, Word::Done] {
            assert_eq!(Word::from_str(w.label()), w);
        }
        assert_eq!(Word::from_str("Mystery"), Word::Waiting);
    }

    #[test]
    fn thread_states_map_to_words() {
        let cases = [
            ("needs_you", Word::NeedsYou),
            ("failed", Word::NeedsYou),
            ("running", Word::Working),
            ("owner", Word::Working),
            ("waiting", Word::Waiting),
            ("stalled", Word::Waiting),
            ("planned", Word::Waiting),
            ("done", Word::Done),
            ("", Word::Waiting),
        ];
        for (state, word) in cases {
            assert_eq!(thread_word(state), word, "{state}");
        }
    }

    #[test]
    fn styles_follow_the_mapping() {
        let th = Theme::dark();
        assert_eq!(style(Word::NeedsYou, &th).base, th.block.base);
        assert_eq!(style(Word::Working, &th).base, th.run.base);
        assert_eq!(style(Word::Stopped, &th).base, th.wait.base);
        assert_eq!(style(Word::Done, &th).base, th.done.base);
    }

    #[test]
    fn char_states_follow_the_mapping() {
        assert_eq!(char_state(Word::NeedsYou), CharState::Blocked);
        assert_eq!(char_state(Word::Working), CharState::Doing);
        assert_eq!(char_state(Word::Stopped), CharState::Standing);
        assert_eq!(char_state(Word::Done), CharState::Landed);
    }
}
