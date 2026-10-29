//! An answer's two parts: the short spoken part, then (after a line holding
//! only `---`) the detail that is only shown.

/// Splits a whole or partial answer into its spoken part and its detail.
/// The detail is `None` until the `---` line has arrived.
pub fn split(text: &str) -> (&str, Option<&str>) {
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let end = start + line.len();
        // Only a finished line counts: "--" may still become "---x".
        if line.ends_with('\n') && line.trim() == "---" {
            return (text[..start].trim_end(), Some(text[end..].trim_start()));
        }
        start = end;
    }
    (text.trim_end(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_at_the_marker_line() {
        let text = "Edinburgh is the capital.\n---\nIt has been since the 15th century.";
        assert_eq!(
            split(text),
            (
                "Edinburgh is the capital.",
                Some("It has been since the 15th century.")
            )
        );
    }

    #[test]
    fn no_marker_means_all_spoken() {
        assert_eq!(split("Just this.\n"), ("Just this.", None));
    }

    #[test]
    fn waits_for_the_whole_marker_line() {
        assert_eq!(split("Yes.\n---"), ("Yes.\n---", None));
        assert_eq!(split("Yes.\n---\n"), ("Yes.", Some("")));
        assert_eq!(split("Yes.\n--- \r\nMore"), ("Yes.", Some("More")));
    }

    #[test]
    fn dashes_inside_a_line_are_text() {
        assert_eq!(split("a --- b\nc"), ("a --- b\nc", None));
    }
}
