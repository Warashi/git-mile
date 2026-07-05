pub(super) mod detail_pane;
pub(super) mod filter_bar;
pub(super) mod popups;
pub(super) mod task_list;
pub(super) mod util;

#[cfg(test)]
pub(super) fn truncate_with_ellipsis(input: &str, max_graphemes: usize) -> std::borrow::Cow<'_, str> {
    util::truncate_with_ellipsis(input, max_graphemes)
}

#[cfg(test)]
pub(super) fn markdown_text(source: &str) -> ratatui::text::Text<'_> {
    util::markdown_text(source)
}
