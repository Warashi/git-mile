use std::borrow::Cow;

use crate::config::StateKind;
use unicode_segmentation::UnicodeSegmentation;

pub(super) fn truncate_with_ellipsis(input: &str, max_graphemes: usize) -> Cow<'_, str> {
    const ELLIPSIS: &str = "...";
    const ELLIPSIS_GRAPHEMES: usize = 3;

    if max_graphemes == 0 {
        return Cow::Owned(String::new());
    }

    let grapheme_count = UnicodeSegmentation::graphemes(input, true).count();
    if grapheme_count <= max_graphemes {
        return Cow::Borrowed(input);
    }

    if max_graphemes <= ELLIPSIS_GRAPHEMES {
        let truncated: String = UnicodeSegmentation::graphemes(input, true)
            .take(max_graphemes)
            .collect();
        return Cow::Owned(truncated);
    }

    let keep = max_graphemes - ELLIPSIS_GRAPHEMES;
    let mut truncated: String = UnicodeSegmentation::graphemes(input, true).take(keep).collect();
    truncated.push_str(ELLIPSIS);
    Cow::Owned(truncated)
}

pub(super) const fn state_kind_marker(kind: Option<StateKind>) -> &'static str {
    match kind {
        Some(StateKind::Done) => " ✓",
        Some(StateKind::InProgress) => " →",
        Some(StateKind::Blocked) => " ⊗",
        Some(StateKind::Todo) => " □",
        Some(StateKind::Backlog) => " ◇",
        None => "",
    }
}
