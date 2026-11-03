//! One clock for everything done during an Ask: the words as they were said,
//! marks as they were drawn, captures, selections, what the pointer was on,
//! window switches. Times are milliseconds from the Ask key going down.
//!
//! "this", "here" and "that" are tied to whatever was done nearest to the
//! moment the word was said, so "when this talks to this" reaches Claude as
//! "when this [1] talks to this [2]".

/// Past this, a mark and a word are too far apart to be paired by time.
const MAX_GAP_MS: u64 = 3_000;
/// Selected text past this many characters is cut in the question line.
const MAX_INLINE: usize = 80;

/// A word as said, on the Ask clock.
#[derive(Debug, Clone, PartialEq)]
pub struct Spoken {
    pub at: u64,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MarkKind {
    Arrow,
    Box,
    Circle,
    Highlight,
    Pen,
    Text,
    Pin,
}

impl MarkKind {
    fn name(self) -> &'static str {
        match self {
            MarkKind::Arrow => "arrow",
            MarkKind::Box => "box",
            MarkKind::Circle => "circle",
            MarkKind::Highlight => "highlight",
            MarkKind::Pen => "pen",
            MarkKind::Text => "text",
            MarkKind::Pin => "pin",
        }
    }
}

/// Part of a capture, in percent of its width and height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub x0: u8,
    pub x1: u8,
    pub y0: u8,
    pub y1: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub enum What {
    /// Mark `n` (numbered across the whole question) on capture `on`.
    Mark {
        n: u32,
        kind: MarkKind,
        on: String,
        region: Option<Region>,
        /// An arrow's ends, when they start and finish on other marks.
        joins: Option<(u32, u32)>,
        /// Words inside the mark, read off the capture.
        encloses: Option<String>,
        /// What a text mark says.
        label: Option<String>,
    },
    /// A grab or snap was taken, e.g. "Snap 2" of "Terminal".
    Capture { name: String, of: String },
    /// Text selected in an app.
    Selected { text: String, app: String },
    /// What the pointer was on, e.g. a button's name.
    Pointed { label: String },
    /// Another window came to the front.
    Window { label: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub at: u64,
    pub what: What,
}

impl Event {
    /// Things a "this" can mean.
    fn is_pointable(&self) -> bool {
        matches!(
            self.what,
            What::Mark { kind, .. } if kind != MarkKind::Arrow
        ) || matches!(self.what, What::Selected { .. })
    }

    fn tag(&self) -> String {
        match &self.what {
            What::Mark { n, .. } => format!("[{n}]"),
            What::Capture { name, .. } => format!("[{name}]"),
            What::Selected { text, .. } => format!("[selected: \"{}\"]", clip(text, MAX_INLINE)),
            What::Pointed { label } => format!("[pointing at \"{}\"]", clip(label, MAX_INLINE)),
            What::Window { label } => format!("[now in {label}]"),
        }
    }
}

fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    format!("{cut}…")
}

/// "this", "here" and the like, ignoring case and punctuation.
fn is_deictic(word: &str) -> bool {
    let bare: String = word
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    matches!(
        bare.as_str(),
        "this" | "that" | "these" | "those" | "here" | "there"
    )
}

/// Which events go after which word: `after[i]` follows word `i`; `before`
/// comes ahead of the first word.
#[derive(Debug, Default, PartialEq)]
struct Placement {
    before: Vec<usize>,
    after: Vec<Vec<usize>>,
}

/// Ties marks and selections to the "this" words nearest them in time (one
/// each, closest pairs first); anything left is placed in order, so a mark
/// drawn well before it is mentioned still meets its word. Pointer targets
/// fill only words nothing else took. Arrows follow the mark they point to;
/// captures and window switches sit where they happened.
fn place(words: &[Spoken], events: &[Event]) -> Placement {
    let mut word_of: Vec<Option<usize>> = vec![None; events.len()];
    let mut taken = vec![false; words.len()];
    let deictic: Vec<usize> = (0..words.len())
        .filter(|&i| is_deictic(&words[i].text))
        .collect();
    let pointable: Vec<usize> = (0..events.len())
        .filter(|&e| events[e].is_pointable())
        .collect();

    let mut pairs: Vec<(u64, usize, usize)> = Vec::new();
    for &e in &pointable {
        for &w in &deictic {
            let gap = events[e].at.abs_diff(words[w].at);
            if gap <= MAX_GAP_MS {
                pairs.push((gap, e, w));
            }
        }
    }
    pairs.sort();
    for (_, e, w) in pairs {
        if word_of[e].is_none() && !taken[w] {
            word_of[e] = Some(w);
            taken[w] = true;
        }
    }
    let left_events: Vec<usize> = pointable
        .iter()
        .copied()
        .filter(|&e| word_of[e].is_none())
        .collect();
    let left_words: Vec<usize> = deictic.iter().copied().filter(|&w| !taken[w]).collect();
    for (e, w) in left_events.into_iter().zip(left_words) {
        word_of[e] = Some(w);
        taken[w] = true;
    }
    for (e, event) in events.iter().enumerate() {
        if let What::Pointed { .. } = event.what {
            let nearest = deictic
                .iter()
                .copied()
                .filter(|&w| !taken[w])
                .min_by_key(|&w| event.at.abs_diff(words[w].at));
            if let Some(w) = nearest {
                word_of[e] = Some(w);
                taken[w] = true;
            }
        }
    }
    // An arrow goes with the mark it points to, if that mark found a word.
    for e in 0..events.len() {
        if let What::Mark {
            joins: Some((_, to)),
            ..
        } = events[e].what
        {
            let target = events
                .iter()
                .position(|x| matches!(x.what, What::Mark { n, .. } if n == to));
            word_of[e] = target.and_then(|t| word_of[t]);
        }
    }

    let mut placement = Placement {
        before: Vec::new(),
        after: vec![Vec::new(); words.len()],
    };
    for (e, event) in events.iter().enumerate() {
        if matches!(event.what, What::Pointed { .. }) && word_of[e].is_none() {
            continue;
        }
        // By time: after the last word said before it.
        let w = word_of[e].or_else(|| words.iter().rposition(|w| w.at <= event.at));
        match w {
            Some(w) => placement.after[w].push(e),
            None => placement.before.push(e),
        }
    }
    for list in placement.after.iter_mut().chain([&mut placement.before]) {
        list.sort_by_key(|&e| events[e].at);
    }
    placement
}

/// The question with each tag where it belongs, e.g.
/// `when this [1] talks to this [2] [3], and …`.
pub fn question(words: &[Spoken], events: &[Event]) -> String {
    let placement = place(words, events);
    let mut parts: Vec<String> = placement.before.iter().map(|&e| events[e].tag()).collect();
    for (i, word) in words.iter().enumerate() {
        let tags: Vec<String> = placement.after[i]
            .iter()
            .map(|&e| events[e].tag())
            .collect();
        if tags.is_empty() {
            parts.push(word.text.clone());
            continue;
        }
        // Tags go inside trailing punctuation: "this [2]?" not "this? [2]".
        let cut = word
            .text
            .trim_end_matches(|c: char| c.is_ascii_punctuation())
            .len();
        let (bare, punct) = word.text.split_at(cut);
        parts.push(format!("{bare} {}{punct}", tags.join(" ")));
    }
    parts.join(" ")
}

/// One line per mark: what it is, where, and what it encloses.
pub fn marks(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.what {
            What::Mark {
                n,
                kind,
                on,
                region,
                joins,
                encloses,
                label,
            } => {
                let mut line = format!("{n}  {}  {on}", kind.name());
                if let Some((from, to)) = joins {
                    line.push_str(&format!("  from mark {from} to mark {to}"));
                } else if let Some(r) = region {
                    line.push_str(&format!(
                        "  around x {}-{}%, y {}-{}%",
                        r.x0, r.x1, r.y0, r.y1
                    ));
                }
                if let Some(text) = encloses.as_deref().filter(|t| !t.trim().is_empty()) {
                    line.push_str(&format!("  round \"{}\"", clip(text, MAX_INLINE)));
                }
                if let Some(text) = label.as_deref().filter(|t| !t.trim().is_empty()) {
                    line.push_str(&format!("  saying \"{}\"", clip(text, MAX_INLINE)));
                }
                Some(line)
            }
            _ => None,
        })
        .collect()
}

fn seconds(ms: u64) -> String {
    format!("{}.{}", ms / 1000, ms % 1000 / 100)
}

/// Said, drew, took, selected, pointed and switched, in order. Words run
/// together into one line until something else happens.
pub fn lines(words: &[Spoken], events: &[Event]) -> Vec<String> {
    let mut out = Vec::new();
    let mut said: Option<(u64, Vec<&str>)> = None;
    let mut events: Vec<&Event> = events.iter().collect();
    events.sort_by_key(|e| e.at);
    let mut next = events.into_iter().peekable();
    let flush = |said: &mut Option<(u64, Vec<&str>)>, out: &mut Vec<String>| {
        if let Some((at, text)) = said.take() {
            out.push(format!("{}  said  \"{}\"", seconds(at), text.join(" ")));
        }
    };
    for word in words {
        while let Some(event) = next.next_if(|e| e.at < word.at) {
            flush(&mut said, &mut out);
            out.push(event_line(event));
        }
        said.get_or_insert_with(|| (word.at, Vec::new()))
            .1
            .push(&word.text);
    }
    flush(&mut said, &mut out);
    out.extend(next.map(event_line));
    out
}

fn event_line(event: &Event) -> String {
    let what = match &event.what {
        What::Mark {
            n, kind, on, joins, ..
        } => match joins {
            Some((from, to)) => format!("drew  {n}, {} from {from} to {to}", kind.name()),
            None => format!("drew  {n}, {} on {on}", kind.name()),
        },
        What::Capture { name, of } => format!("took  {name} ({of})"),
        What::Selected { text, app } => {
            format!("selected  \"{}\" in {app}", clip(text, MAX_INLINE))
        }
        What::Pointed { label } => format!("pointed at  \"{}\"", clip(label, MAX_INLINE)),
        What::Window { label } => format!("switched to  {label}"),
    };
    format!("{}  {what}", seconds(event.at))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spike's words: Parakeet on "Can I ask you about what happens when
    /// this … talks to this, … and then why does the log … say this?"
    fn spoken() -> Vec<Spoken> {
        [
            (640, "Can"),
            (720, "I"),
            (800, "ask"),
            (960, "you"),
            (1200, "about"),
            (1520, "what"),
            (1600, "happens"),
            (2160, "when"),
            (2240, "this"),
            (3840, "talks"),
            (4160, "to"),
            (4320, "this?"),
            (6640, "And"),
            (6720, "then"),
            (6960, "why"),
            (7120, "does"),
            (7280, "the"),
            (7440, "log"),
            (8880, "say"),
            (9120, "this?"),
        ]
        .iter()
        .map(|&(at, text)| Spoken {
            at,
            text: text.into(),
        })
        .collect()
    }

    fn mark(at: u64, n: u32, kind: MarkKind, joins: Option<(u32, u32)>) -> Event {
        Event {
            at,
            what: What::Mark {
                n,
                kind,
                on: "Grab 1".into(),
                region: Some(Region {
                    x0: 24,
                    x1: 52,
                    y0: 32,
                    y1: 65,
                }),
                joins,
                encloses: None,
                label: None,
            },
        }
    }

    fn circles_and_a_log_line() -> Vec<Event> {
        vec![
            mark(2600, 1, MarkKind::Circle, None),
            mark(4600, 2, MarkKind::Circle, None),
            mark(5000, 3, MarkKind::Arrow, Some((1, 2))),
            Event {
                at: 7600,
                what: What::Window {
                    label: "Terminal".into(),
                },
            },
            Event {
                at: 9300,
                what: What::Selected {
                    text: "queue full,\n dropping 2 chunks".into(),
                    app: "Terminal".into(),
                },
            },
        ]
    }

    #[test]
    fn each_this_meets_what_was_done_as_it_was_said() {
        assert_eq!(
            question(&spoken(), &circles_and_a_log_line()),
            "Can I ask you about what happens when this [1] talks to this [2] [3]? \
             And then why does the log [now in Terminal] say this \
             [selected: \"queue full, dropping 2 chunks\"]?"
        );
    }

    #[test]
    fn marks_drawn_before_talking_still_meet_their_words_in_order() {
        let early = vec![
            mark(100, 1, MarkKind::Circle, None),
            mark(200, 2, MarkKind::Box, None),
        ];
        let words: Vec<Spoken> = ["why", "is", "this", "slower", "than", "that"]
            .iter()
            .enumerate()
            .map(|(i, t)| Spoken {
                at: 20_000 + i as u64 * 300,
                text: (*t).into(),
            })
            .collect();
        assert_eq!(
            question(&words, &early),
            "why is this [1] slower than that [2]"
        );
    }

    #[test]
    fn pointing_only_fills_a_this_nothing_else_took() {
        let words = vec![
            Spoken {
                at: 1000,
                text: "what".into(),
            },
            Spoken {
                at: 1200,
                text: "does".into(),
            },
            Spoken {
                at: 1400,
                text: "this".into(),
            },
            Spoken {
                at: 1600,
                text: "do?".into(),
            },
        ];
        let events = vec![Event {
            at: 1400,
            what: What::Pointed {
                label: "Merge pull request".into(),
            },
        }];
        assert_eq!(
            question(&words, &events),
            "what does this [pointing at \"Merge pull request\"] do?"
        );
        let with_mark = vec![events[0].clone(), mark(1500, 1, MarkKind::Box, None)];
        assert_eq!(question(&words, &with_mark), "what does this [1] do?");
    }

    #[test]
    fn marks_are_listed_with_where_they_are() {
        let mut events = circles_and_a_log_line();
        if let What::Mark { encloses, .. } = &mut events[0].what {
            *encloses = Some("Ask runner".into());
        }
        assert_eq!(
            marks(&events),
            vec![
                "1  circle  Grab 1  around x 24-52%, y 32-65%  round \"Ask runner\"",
                "2  circle  Grab 1  around x 24-52%, y 32-65%",
                "3  arrow  Grab 1  from mark 1 to mark 2",
            ]
        );
    }

    #[test]
    fn the_timeline_runs_words_together_between_events() {
        let lines = lines(&spoken(), &circles_and_a_log_line());
        assert_eq!(
            lines,
            vec![
                "0.6  said  \"Can I ask you about what happens when this\"",
                "2.6  drew  1, circle on Grab 1",
                "3.8  said  \"talks to this?\"",
                "4.6  drew  2, circle on Grab 1",
                "5.0  drew  3, arrow from 1 to 2",
                "6.6  said  \"And then why does the log\"",
                "7.6  switched to  Terminal",
                "8.8  said  \"say this?\"",
                "9.3  selected  \"queue full, dropping 2 chunks\" in Terminal",
            ]
        );
    }

    #[test]
    fn nothing_done_leaves_the_words_alone() {
        let words = spoken();
        let plain: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(question(&words, &[]), plain.join(" "));
    }
}
