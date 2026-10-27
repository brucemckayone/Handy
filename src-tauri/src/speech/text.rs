//! Turns text worth reading, often markdown from a coding agent, into short
//! pieces worth hearing: code, tables and URLs are left out, markup is
//! dropped, paths shrink to their file name, and long text stops at a word
//! limit. The pieces are sentences (long ones split at commas), so speech
//! can start after the first and stop between any two.

use once_cell::sync::Lazy;
use regex::Regex;

/// Longest piece handed to the voice model at once.
const MAX_CHARS: usize = 220;
const CODE_OMITTED: &str = "Code omitted.";
const TABLE_OMITTED: &str = "Table omitted.";
const CUT_SHORT: &str = "The rest is too long to read.";
/// Words that end in a full stop without ending the sentence.
const ABBREVIATIONS: &[&str] = &[
    "e.g", "i.e", "etc", "vs", "mr", "mrs", "ms", "dr", "st", "approx", "no", "fig",
];

static IMAGE: Lazy<Regex> = Lazy::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
static LINK: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[([^\]]+)\]\([^)]*\)").unwrap());
static URL: Lazy<Regex> = Lazy::new(|| Regex::new(r"<?https?://[^\s>)]+>?").unwrap());
static HTML_TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"</?[a-zA-Z][^>]*>").unwrap());
static INLINE_CODE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`+([^`]+)`+").unwrap());
static BOLD: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\*\*|__)([^*_]+)(\*\*|__)").unwrap());
static ITALIC_STAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*([^*\s][^*]*)\*").unwrap());
static ITALIC_UNDERSCORE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(^|[^\w])_([^_\s][^_]*)_([^\w]|$)").unwrap());
static STRIKE: Lazy<Regex> = Lazy::new(|| Regex::new(r"~~([^~]+)~~").unwrap());
/// `dir/dir/file.ext` (optionally `:line`), forward or back slashes.
static PATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:[A-Za-z]:[/\\])?(?:[\w.-]+[/\\])+([\w-]+(?:\.[\w-]+)+)(?::(\d+))?").unwrap()
});
static FILE_LINE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b([\w-]+\.[A-Za-z]{1,5}):(\d+)\b").unwrap());
/// `name.ext` for the extensions an agent mentions most.
static FILE_EXTENSION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b([\w-]+)\.(rs|ts|tsx|js|jsx|json|md|py|toml|yaml|yml|txt|css|html|cpp|hpp|h|c|cs|go|java|sh|ps1|sql|wav|lock)\b").unwrap()
});
static HEADING: Lazy<Regex> = Lazy::new(|| Regex::new(r"^#{1,6}\s+").unwrap());
static LIST_MARKER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s+)?").unwrap());
static RULE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?:[-*_]\s*){3,}$").unwrap());
static SPACES: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());

/// The pieces to read aloud, in order, at most about `max_words` words.
pub fn prepare(text: &str, max_words: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut words = 0;
    for block in blocks(text) {
        for sentence in sentences(&block) {
            for piece in split_long(&sentence) {
                let count = piece.split_whitespace().count();
                if !pieces.is_empty() && words + count > max_words {
                    pieces.push(CUT_SHORT.to_string());
                    return pieces;
                }
                words += count;
                pieces.push(piece);
            }
        }
    }
    pieces
}

/// Plain-text paragraphs, each ending in sentence punctuation.
fn blocks(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut paragraph = String::new();
    let mut fence: Option<&str> = None;
    let mut in_table = false;

    for raw in text.lines() {
        let line = raw.trim();
        if let Some(marker) = fence {
            if line.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if line.starts_with("```") || line.starts_with("~~~") {
            finish(&mut paragraph, &mut blocks);
            fence = Some(&line[..3]);
            if blocks.last().map(String::as_str) != Some(CODE_OMITTED) {
                blocks.push(CODE_OMITTED.to_string());
            }
            continue;
        }
        if line.starts_with('|') {
            if !in_table {
                finish(&mut paragraph, &mut blocks);
                blocks.push(TABLE_OMITTED.to_string());
                in_table = true;
            }
            continue;
        }
        in_table = false;
        if line.is_empty() || RULE.is_match(line) {
            finish(&mut paragraph, &mut blocks);
            continue;
        }
        let line = line.trim_start_matches('>').trim_start();
        // Headings and list items are sentences of their own.
        let own_sentence = HEADING.is_match(line) || LIST_MARKER.is_match(line);
        if own_sentence {
            finish(&mut paragraph, &mut blocks);
        }
        let line = HEADING.replace(line, "");
        let line = LIST_MARKER.replace(&line, "");
        paragraph.push(' ');
        paragraph.push_str(&inline(&line));
        if own_sentence {
            finish(&mut paragraph, &mut blocks);
        }
    }
    finish(&mut paragraph, &mut blocks);
    blocks
}

/// Drops inline markup and shortens what's tedious to hear.
fn inline(line: &str) -> String {
    let line = IMAGE.replace_all(line, "");
    let line = LINK.replace_all(&line, "$1");
    let line = URL.replace_all(&line, "link");
    let line = HTML_TAG.replace_all(&line, "");
    let line = INLINE_CODE.replace_all(&line, "$1");
    let line = PATH.replace_all(&line, |caps: &regex::Captures| match caps.get(2) {
        Some(number) => format!("{} line {}", &caps[1], number.as_str()),
        None => caps[1].to_string(),
    });
    let line = FILE_LINE.replace_all(&line, "$1 line $2");
    // Short extensions are said letter by letter: "mod dot RS".
    let line = FILE_EXTENSION.replace_all(&line, |caps: &regex::Captures| {
        let ext = &caps[2];
        let ext = if ext.len() <= 3 {
            ext.to_uppercase()
        } else {
            ext.to_string()
        };
        format!("{} dot {}", &caps[1], ext)
    });
    let line = BOLD.replace_all(&line, "$2");
    let line = ITALIC_STAR.replace_all(&line, "$1");
    let line = ITALIC_UNDERSCORE.replace_all(&line, "$1$2$3");
    STRIKE.replace_all(&line, "$1").into_owned()
}

/// Ends the paragraph being built as a block of its own.
fn finish(paragraph: &mut String, blocks: &mut Vec<String>) {
    let text = SPACES.replace_all(paragraph.trim(), " ").into_owned();
    if !text.is_empty() {
        blocks.push(end_sentence(text));
    }
    paragraph.clear();
}

fn end_sentence(mut text: String) -> String {
    let last = text.chars().last();
    if let Some(':') | Some(';') | Some(',') = last {
        text.pop();
    }
    if !matches!(text.chars().last(), Some('.' | '!' | '?')) {
        text.push('.');
    }
    text
}

/// Splits a paragraph after `.`, `!` or `?` followed by a space, except
/// after abbreviations and initials.
fn sentences(paragraph: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = paragraph.char_indices().collect();
    for (i, &(at, c)) in chars.iter().enumerate() {
        if !matches!(c, '.' | '!' | '?') {
            continue;
        }
        let next = chars.get(i + 1).map(|&(_, c)| c);
        if !next.is_some_and(char::is_whitespace) {
            continue;
        }
        let word = paragraph[start..at]
            .rsplit(char::is_whitespace)
            .next()
            .unwrap_or("")
            .trim_start_matches(['(', '"', '\''])
            .to_lowercase();
        let initial = word.chars().count() == 1 && word.chars().all(char::is_alphabetic);
        if c == '.' && (initial || ABBREVIATIONS.contains(&word.as_str())) {
            continue;
        }
        let end = at + c.len_utf8();
        let sentence = paragraph[start..end].trim();
        if !sentence.is_empty() {
            sentences.push(sentence.to_string());
        }
        start = end;
    }
    let rest = paragraph[start..].trim();
    if !rest.is_empty() {
        sentences.push(rest.to_string());
    }
    sentences
}

/// Splits a sentence longer than [`MAX_CHARS`] at a comma or similar.
fn split_long(sentence: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut rest = sentence.trim();
    while rest.len() > MAX_CHARS {
        let mut limit = MAX_CHARS;
        while !rest.is_char_boundary(limit) {
            limit -= 1;
        }
        let head = &rest[..limit];
        let cut = [", ", "; ", ": ", " - ", " – "]
            .iter()
            .filter_map(|sep| head.rfind(sep).map(|i| i + 1))
            .filter(|&i| i > MAX_CHARS / 3)
            .max()
            .or_else(|| head.rfind(' '))
            .unwrap_or(limit);
        pieces.push(rest[..cut].trim().to_string());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        pieces.push(rest.to_string());
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Vec<String> {
        prepare(text, 1000)
    }

    #[test]
    fn splits_sentences_but_not_abbreviations_or_decimals() {
        assert_eq!(
            read("It took 3.5 seconds, e.g. on this laptop. Then it stopped! Done?"),
            [
                "It took 3.5 seconds, e.g. on this laptop.",
                "Then it stopped!",
                "Done?"
            ]
        );
    }

    #[test]
    fn leaves_code_blocks_out() {
        let reply = "Here is the fix:\n\n```rust\nfn main() {}\n```\n\nIt compiles now.";
        assert_eq!(
            read(reply),
            ["Here is the fix.", CODE_OMITTED, "It compiles now."]
        );
    }

    #[test]
    fn reads_link_text_not_urls() {
        assert_eq!(
            read("See [the docs](https://example.com/x) or https://example.com/y for more."),
            ["See the docs or link for more."]
        );
    }

    #[test]
    fn headings_and_list_items_are_their_own_sentences() {
        let reply = "## Summary\n\n- Fixed the bug\n- Added a test\n1. Run it";
        assert_eq!(
            read(reply),
            ["Summary.", "Fixed the bug.", "Added a test.", "Run it."]
        );
    }

    #[test]
    fn paths_become_file_names() {
        assert_eq!(
            read("I changed `src-tauri/src/meeting/mod.rs:42` and C:\\dev\\Handy\\notes.md, plus lib.rs:7."),
            ["I changed mod dot RS line 42 and notes dot MD, plus lib dot RS line 7."]
        );
    }

    #[test]
    fn markup_goes_but_snake_case_stays() {
        assert_eq!(
            read("This is **bold**, *soft*, _quiet_ and ~~gone~~ in snake_case_name."),
            ["This is bold, soft, quiet and gone in snake_case_name."]
        );
    }

    #[test]
    fn tables_and_html_are_left_out() {
        let reply = "Results:\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nAll <b>green</b>.";
        assert_eq!(read(reply), ["Results.", TABLE_OMITTED, "All green."]);
    }

    #[test]
    fn stops_at_the_word_limit() {
        let pieces = prepare("One two three. Four five six. Seven eight nine.", 6);
        assert_eq!(pieces, ["One two three.", "Four five six.", CUT_SHORT]);
    }

    #[test]
    fn long_sentences_split_at_commas() {
        let sentence = format!(
            "{}, and then {}.",
            "word ".repeat(30).trim(),
            "more ".repeat(30).trim()
        );
        let pieces = read(&sentence);
        assert_eq!(pieces.len(), 2);
        assert!(pieces.iter().all(|p| p.len() <= MAX_CHARS));
        assert!(pieces[0].ends_with(','));
    }

    #[test]
    fn empty_text_reads_nothing() {
        assert!(read("  \n\n").is_empty());
    }
}
