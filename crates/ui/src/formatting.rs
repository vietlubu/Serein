//! Composer keyboard formatting: Cmd/Ctrl shortcuts wrap or unwrap the selection with markdown.
use egui::text::{CCursor, CCursorRange};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
	Bold,
	Italic,
	Underline,
	Strikethrough,
	Code,
	CodeBlock,
	Spoiler,
}

impl Style {
	fn markers(self) -> (&'static str, &'static str) {
		match self {
			Self::Bold => ("**", "**"),
			Self::Italic => ("*", "*"),
			Self::Underline => ("__", "__"),
			Self::Strikethrough => ("~~", "~~"),
			Self::Code => ("`", "`"),
			Self::CodeBlock => ("```\n", "\n```"),
			Self::Spoiler => ("||", "||"),
		}
	}

	/// Consumes the first matching shortcut this frame. Shift combinations are checked first
	/// because a plain `Cmd+letter` match would otherwise also swallow them.
	pub fn consume(ctx: &egui::Context, bindings: &model::Keybinds) -> Option<Self> {
		const SHORTCUTS: [(model::KeybindAction, Style); 7] = [
			(model::KeybindAction::Strikethrough, Style::Strikethrough),
			(model::KeybindAction::CodeBlock, Style::CodeBlock),
			(model::KeybindAction::Spoiler, Style::Spoiler),
			(model::KeybindAction::Bold, Style::Bold),
			(model::KeybindAction::Italic, Style::Italic),
			(model::KeybindAction::Underline, Style::Underline),
			(model::KeybindAction::InlineCode, Style::Code),
		];
		ctx.input_mut(|input| {
			SHORTCUTS
				.iter()
				.find(|(action, _)| {
					bindings
						.chords(*action)
						.any(|chord| crate::keybinds::pressed(input, chord))
				})
				.map(|(_, style)| *style)
		})
	}
}

fn char_to_byte(text: &str, index: usize) -> usize {
	text.char_indices()
		.nth(index)
		.map_or(text.len(), |(byte, _)| byte)
}

/// Wraps the selection (or inserts an empty pair at the caret) and returns the new selection.
/// A selection that is already wrapped, or sits exactly inside the markers, is unwrapped instead.
pub fn apply(
	draft: &mut String,
	style: Style,
	range: Option<CCursorRange>,
	remaining: usize,
	editing: bool,
) -> Option<CCursorRange> {
	let (prefix, suffix) = style.markers();
	let count = draft.chars().count();
	let (start, end) = range.map_or((count, count), |range| {
		let range = range.as_sorted_char_range();
		(range.start.0.min(count), range.end.0.min(count))
	});
	let (prefix_len, suffix_len) = (prefix.chars().count(), suffix.chars().count());
	let selected: String = draft.chars().skip(start).take(end - start).collect();

	// Toggle off: `**text**` selected, or `text` selected with the markers just outside.
	if let Some(inner) = selected
		.strip_prefix(prefix)
		.and_then(|rest| rest.strip_suffix(suffix))
		&& !selected.is_empty()
	{
		let inner = inner.to_owned();
		let (from, to) = (char_to_byte(draft, start), char_to_byte(draft, end));
		draft.replace_range(from..to, &inner);
		return Some(CCursorRange::two(
			CCursor::new(start),
			CCursor::new(start + inner.chars().count()),
		));
	}
	if start >= prefix_len && end + suffix_len <= count {
		let before: String = draft
			.chars()
			.skip(start - prefix_len)
			.take(prefix_len)
			.collect();
		let after: String = draft.chars().skip(end).take(suffix_len).collect();
		if before == prefix && after == suffix {
			let from = char_to_byte(draft, start - prefix_len);
			let to = char_to_byte(draft, end + suffix_len);
			draft.replace_range(from..to, &selected);
			return Some(CCursorRange::two(
				CCursor::new(start - prefix_len),
				CCursor::new(end - prefix_len),
			));
		}
	}

	let wrapped = format!("{prefix}{selected}{suffix}");
	let range = Some(CCursorRange::two(CCursor::new(start), CCursor::new(end)));
	crate::emoji_picker::insert(draft, &wrapped, range, remaining, editing)?;
	let inner_start = start + prefix_len;
	Some(CCursorRange::two(
		CCursor::new(inner_start),
		CCursor::new(inner_start + (end - start)),
	))
}

/// Pasting a lone http(s) URL over a single-line selection writes `[selection](url)`, like Discord.
/// On success the paste event is consumed and the caret after the link is returned; otherwise
/// egui handles the paste as plain text.
pub fn paste_link(
	events: &mut Vec<egui::Event>,
	draft: &mut String,
	range: Option<CCursorRange>,
	remaining: usize,
	editing: bool,
) -> Option<usize> {
	let sorted = range?.as_sorted_char_range();
	let (start, end) = (sorted.start.0, sorted.end.0);
	let label = draft.get(char_to_byte(draft, start)..char_to_byte(draft, end))?;
	// A selected URL is simply replaced, as before.
	if label.is_empty()
		|| label.contains(['\n', '[', ']'])
		|| crate::markdown::external_url(label.trim()).is_some()
	{
		return None;
	}
	let at = events
		.iter()
		.position(|event| matches!(event, egui::Event::Paste(_)))?;
	let egui::Event::Paste(pasted) = &events[at] else {
		return None;
	};
	let url = pasted.trim();
	if url.contains(char::is_whitespace) || crate::markdown::external_url(url).is_none() {
		return None;
	}
	// Keep the pasted URL verbatim; `external_url` normalizes (e.g. adds a trailing `/`).
	let link = format!("[{label}]({url})");
	let cursor = crate::emoji_picker::insert(draft, &link, range, remaining, editing)?;
	events.remove(at);
	Some(cursor)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn selection(start: usize, end: usize) -> Option<CCursorRange> {
		Some(CCursorRange::two(CCursor::new(start), CCursor::new(end)))
	}

	#[test]
	fn wrapping_toggles_and_keeps_the_inner_selection() {
		let mut draft = "say hi there".to_owned();
		let range = apply(&mut draft, Style::Bold, selection(4, 6), 100, true).unwrap();
		assert_eq!(draft, "say **hi** there");
		assert_eq!(range.as_sorted_char_range().start.0, 6);
		assert_eq!(range.as_sorted_char_range().end.0, 8);
		let range = apply(&mut draft, Style::Bold, Some(range), 100, true).unwrap();
		assert_eq!(draft, "say hi there");
		assert_eq!(range.as_sorted_char_range().start.0, 4);
		assert_eq!(range.as_sorted_char_range().end.0, 6);
		let mut draft = "**hi**".to_owned();
		assert!(apply(&mut draft, Style::Bold, selection(0, 6), 100, true).is_some());
		assert_eq!(draft, "hi");
	}

	#[test]
	fn empty_caret_inserts_markers_and_code_blocks_span_lines() {
		let mut draft = "ab".to_owned();
		let range = apply(&mut draft, Style::Italic, selection(1, 1), 100, true).unwrap();
		assert_eq!(draft, "a**b");
		assert!(range.is_empty());
		assert_eq!(range.primary.index.0, 2);
		let mut draft = "console.log(\"hi\")".to_owned();
		let end = draft.chars().count();
		let range = apply(&mut draft, Style::CodeBlock, selection(0, end), 100, true).unwrap();
		assert_eq!(draft, "```\nconsole.log(\"hi\")\n```");
		assert_eq!(range.as_sorted_char_range().start.0, 4);
		assert_eq!(range.as_sorted_char_range().end.0, 4 + end);
	}

	#[test]
	fn quiet_formatting_fits_full_payload_but_literal_edits_keep_their_limit() {
		let mut quiet = format!("@silent {}", "x".repeat(client_core::MAX_CONTENT - 4));
		let end = quiet.chars().count();
		assert!(apply(&mut quiet, Style::Bold, selection(end - 1, end), 64, false).is_some());
		assert!(model::message_options::valid(
			&quiet,
			client_core::MAX_CONTENT,
			false
		));
		assert_eq!(quiet.chars().count(), client_core::MAX_CONTENT + 8);
		let mut literal = format!("@silent {}", "x".repeat(client_core::MAX_CONTENT - 8));
		let original = literal.clone();
		let end = literal.chars().count();
		assert!(apply(&mut literal, Style::Bold, selection(end - 1, end), 64, true).is_none());
		assert_eq!(literal, original);
	}

	#[test]
	fn budget_overflow_leaves_the_draft_unchanged() {
		let mut draft = "x".to_owned();
		draft.shrink_to_fit();
		assert!(apply(&mut draft, Style::Bold, selection(0, 1), 0, true).is_none());
		assert_eq!(draft, "x");
	}

	#[test]
	fn paste_link_masks_a_selection_only_for_a_lone_url() {
		let paste = |text: &str| vec![egui::Event::Paste(text.into())];
		let mut draft = "Đây là Google Link".to_owned();
		let mut events = paste(" https://google.com\n");
		let cursor = paste_link(&mut events, &mut draft, selection(14, 18), 100, true);
		assert_eq!(cursor, Some(40));
		assert_eq!(draft, "Đây là Google [Link](https://google.com)");
		assert!(events.is_empty());

		for (text, pasted, range) in [
			("Đây là Google Link", "hello", selection(14, 18)),
			(
				"Đây là Google Link",
				"https://a.com https://b.com",
				selection(14, 18),
			),
			(
				"Đây là Google Link",
				"https://google.com",
				selection(18, 18),
			),
			("two\nlines", "https://google.com", selection(0, 9)),
			("see https://old.com", "https://new.com", selection(4, 19)),
		] {
			let mut draft = text.to_owned();
			let mut events = paste(pasted);
			assert_eq!(paste_link(&mut events, &mut draft, range, 100, true), None);
			assert_eq!(draft, text);
			assert_eq!(events.len(), 1);
		}

		let mut draft = "Link".to_owned();
		draft.shrink_to_fit();
		let mut events = paste("https://google.com");
		assert_eq!(
			paste_link(&mut events, &mut draft, selection(0, 4), 0, true),
			None
		);
		assert_eq!(draft, "Link");
		assert_eq!(events.len(), 1);
	}
}
