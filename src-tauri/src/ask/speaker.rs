//! Speaks an answer's spoken part a sentence at a time while it streams, so
//! the first words play before Claude has finished.

use tauri::AppHandle;

use super::answer;
use crate::speech::SpeakRefusal;

/// Claude is asked for one to three sentences; past these, the rest is left
/// to the panel.
const MAX_SENTENCES: usize = 4;
const MAX_WORDS: usize = 90;
const REST_IN_PANEL: &str = "The rest is in the panel.";

/// Where the next whole sentence in `text[from..]` ends: after `.`, `!`, `?`
/// or `…` followed by white space, or at a line break. With `finished`, the
/// end of the text ends a sentence too.
pub fn sentence_end(text: &str, from: usize, finished: bool) -> Option<usize> {
    let rest = &text[from..];
    let mut chars = rest.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\n' {
            return Some(from + i + 1);
        }
        if matches!(c, '.' | '!' | '?' | '…') {
            match chars.peek() {
                Some((_, next)) if next.is_whitespace() => return Some(from + i + c.len_utf8()),
                None if finished => return Some(text.len()),
                _ => {}
            }
        }
    }
    (finished && !rest.trim().is_empty()).then_some(text.len())
}

pub struct Speaker {
    app: AppHandle,
    /// How far into the current message has been spoken.
    spoken_to: usize,
    sentences: usize,
    words: usize,
    /// The speech manager's stop count when speaking began.
    stops: Option<u64>,
    done: bool,
    refusal: Option<SpeakRefusal>,
}

impl Speaker {
    pub fn new(app: &AppHandle, enabled: bool) -> Self {
        Self {
            app: app.clone(),
            spoken_to: 0,
            sentences: 0,
            words: 0,
            stops: crate::speech::manager(app).map(|s| s.stops()),
            done: !enabled,
            refusal: None,
        }
    }

    /// A new message began (Claude used a tool): its text starts over.
    /// Whether anything was queued to be read.
    pub fn spoke(&self) -> bool {
        self.sentences > 0
    }

    pub fn reset(&mut self) {
        self.spoken_to = 0;
    }

    /// `text` is the current message so far.
    pub fn feed(&mut self, text: &str) {
        self.speak_from(text, false);
    }

    /// `text` is the whole answer: speaks what's left of its spoken part.
    pub fn finish(&mut self, text: &str) -> Option<SpeakRefusal> {
        self.speak_from(text, true);
        self.refusal
    }

    fn speak_from(&mut self, text: &str, finished: bool) {
        if self.done {
            return;
        }
        let (spoken, detail) = answer::split(text);
        let finished = finished || detail.is_some();
        // The final text can be shorter than the streamed one (narration
        // dropped): never speak past what is there.
        self.spoken_to = self.spoken_to.min(spoken.len());
        while let Some(end) = sentence_end(spoken, self.spoken_to, finished) {
            let sentence = spoken[self.spoken_to..end].trim();
            self.spoken_to = end;
            if sentence.is_empty() {
                continue;
            }
            if self.sentences >= MAX_SENTENCES || self.words >= MAX_WORDS {
                self.say(REST_IN_PANEL);
                self.done = true;
                return;
            }
            self.sentences += 1;
            self.words += sentence.split_whitespace().count();
            if !self.say(sentence) {
                return;
            }
        }
    }

    /// Queues `sentence`; `false` once speaking should stop for good.
    fn say(&mut self, sentence: &str) -> bool {
        let Some(speech) = crate::speech::manager(&self.app) else {
            self.done = true;
            return false;
        };
        // The user pressed stop: the rest of this answer stays quiet.
        if self.stops.is_some_and(|stops| stops != speech.stops()) {
            self.done = true;
            return false;
        }
        match speech.speak(sentence) {
            Ok(()) => true,
            // Leftover markdown, say; the next sentence may be fine.
            Err(SpeakRefusal::NothingToRead) => true,
            Err(refusal) => {
                self.refusal = Some(refusal);
                self.done = true;
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sentences(text: &str, finished: bool) -> Vec<&str> {
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(end) = sentence_end(text, from, finished) {
            out.push(text[from..end].trim());
            from = end;
        }
        out
    }

    #[test]
    fn splits_whole_sentences_only_while_streaming() {
        let text = "It takes the transcript. Then it runs the prompt! Two places call";
        assert_eq!(
            sentences(text, false),
            ["It takes the transcript.", "Then it runs the prompt!"]
        );
        assert_eq!(sentences(text, true).last(), Some(&"Two places call"));
    }

    #[test]
    fn numbers_and_files_are_not_sentence_ends() {
        assert_eq!(
            sentences("Version 3.5 of actions.rs works. Yes", false),
            ["Version 3.5 of actions.rs works."]
        );
    }

    #[test]
    fn a_line_break_ends_a_sentence() {
        assert_eq!(sentences("First line\nSecond", false), ["First line"]);
    }

    #[test]
    fn a_trailing_full_stop_waits_for_more_text() {
        // "3." may still become "3.5".
        assert!(sentences("Up to 3.", false).is_empty());
        assert_eq!(sentences("Up to 3.", true), ["Up to 3."]);
    }
}
