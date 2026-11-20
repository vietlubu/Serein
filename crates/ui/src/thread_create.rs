//! One bounded "start a thread" request at a time, from a message or from the channel header.
use crate::dialog;
use client_core::{Command, State, channel_actions::Action};
use model::Id;

struct Request {
	channel: Id,
	/// The message the thread hangs off, when it was started from one.
	message: Option<Id>,
	name: String,
	/// Starts a private thread (type 12); only offered when not starting from a message.
	private: bool,
	/// Optional opening message sent into the new thread once it opens.
	content: String,
	submitted: bool,
}

/// First line of the starter message, collapsed whitespace, cut at a word boundary near 60 chars.
pub(crate) fn suggested_name(text: &str) -> String {
	let line = text
		.lines()
		.map(str::trim)
		.find(|line| !line.is_empty())
		.unwrap_or_default();
	let words: Vec<&str> = line.split_whitespace().collect();
	let mut out = String::new();
	for word in words {
		let next = out.chars().count() + word.chars().count() + usize::from(!out.is_empty());
		if next > 60 {
			break;
		}
		if !out.is_empty() {
			out.push(' ');
		}
		out.push_str(word);
	}
	if out.is_empty() {
		line.chars().take(60).collect()
	} else {
		out
	}
}

#[derive(Default)]
pub(crate) struct ThreadCreateUi {
	request: Option<Request>,
	generation: u64,
}

impl ThreadCreateUi {
	/// Opens the editor for `channel`, optionally starting from one of its messages.
	pub fn open(&mut self, channel: Id, message: Option<Id>, name: String) {
		self.request = Some(Request {
			channel,
			message,
			name: name.chars().take(100).collect(),
			private: false,
			content: String::new(),
			submitted: false,
		});
	}
	pub fn show(
		&mut self,
		ctx: &egui::Context,
		state: &mut State,
		commands: &mut Vec<Command>,
	) -> Option<Id> {
		if self.generation != state.generation {
			self.generation = state.generation;
			self.request = None;
			return None;
		}
		let request = self.request.as_mut()?;
		let channel = request.channel;
		if !state.can_create_thread(channel) && !request.submitted {
			self.request = None;
			return None;
		}
		if request.submitted && state.channel_action_succeeded(channel) {
			// The reducer admitted the new thread; the caller peeks it like a thread card.
			let name = request.name.trim().to_owned();
			let created = state
				.channels
				.iter()
				.filter(|c| {
					c.parent_id == Some(channel)
						&& matches!(c.kind, 10..=12)
						&& if let Some(m) = request.message {
							c.id == m
						} else {
							c.name.trim() == name
						}
				})
				.map(|c| c.id)
				.max();
			let content = std::mem::take(&mut request.content);
			self.request = None;
			if let Some(id) = created {
				let text = content.trim();
				if !text.is_empty() {
					match state.prepare_text_send_to(id, text) {
						Some(command) => commands.push(command),
						// Send refused (permission/slowmode/budget): keep the text in the thread composer.
						None => {
							state.drafts.insert(id, text.to_owned());
						}
					}
				}
			}
			return created;
		}
		let mut close = false;
		let result = dialog::Dialog::new(
			("create-thread", channel),
			crate::i18n::translate("thread-create-show-create-thread"),
		)
		.subtitle(crate::i18n::translate_if_key(
			if request.message.is_some() {
				"thread-create-show-the-selected-message-starts-the-thread-everyone-who-can-see"
			} else if request.private {
				"thread-create-show-private-hint"
			} else {
				"thread-create-show-everyone-who-can-see-this-channel-can-see-the-thread"
			},
		))
		.width(420.0)
		.show(ctx, |d| {
			d.content(|ui| {
				let label = dialog::label(ui, "thread-create-show-thread-name");
				dialog::input(
					ui,
					egui::TextEdit::singleline(&mut request.name)
						.align(egui::Align2::LEFT_CENTER)
						.char_limit(100),
				)
				.labelled_by(label.id);
				request.name.shrink_to_fit();
				if request.message.is_none() && state.can_create_private_thread(channel) {
					dialog::label(ui, "thread-create-show-private-thread");
					ui.checkbox(
						&mut request.private,
						crate::i18n::translate("thread-create-show-private-hint"),
					);
				} else {
					request.private = false;
				}
				let label = dialog::label(ui, "thread-create-show-message");
				dialog::input(
					ui,
					egui::TextEdit::multiline(&mut request.content)
						.hint_text(crate::i18n::translate(
							"thread-create-show-enter-a-message-to-start-the-conversation",
						))
						.char_limit(
							client_core::MAX_CONTENT + model::message_options::PREFIX_ALLOWANCE,
						)
						.desired_rows(3)
						.desired_width(f32::INFINITY),
				)
				.labelled_by(label.id);
				request.content.shrink_to_fit();
				if let Some(error) = state
					.channel_action_status(channel)
					.filter(|_| !state.channel_action_succeeded(channel))
				{
					dialog::notice(ui, dialog::Level::Error, error);
				}
			});
			d.footer(|ui| {
				ui.add_enabled_ui(
					!state.channel_action_pending()
						&& (state.demo || state.gateway_connected)
						&& client_core::channel_actions::valid_name(&request.name),
					|ui| {
						if dialog::action(ui, "thread-create-show-create", dialog::Action::Primary)
							.clicked()
						{
							let action = Action::CreateThread {
								name: request.name.trim().to_owned(),
								message: request.message,
								private: request.private && request.message.is_none(),
							};
							if let Some(command) = state.request_channel_action(channel, action) {
								commands.push(command);
								request.submitted = true;
							}
						}
					},
				);
				close = dialog::action(ui, "thread-create-show-cancel", dialog::Action::Neutral)
					.clicked();
			});
		});
		if close || result.close {
			self.request = None;
		}
		None
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn thread_create_suggested_name_bounds_length_and_cuts_words() {
		let text = "This is a very long first line of a message that will exceed sixty characters easily when words are counted";
		let suggested = suggested_name(text);
		assert!(suggested.len() <= 60);
		assert_eq!(
			suggested,
			"This is a very long first line of a message that will exceed"
		);
	}

	#[test]
	fn thread_create_suggested_name_handles_short_and_empty_text() {
		assert_eq!(
			suggested_name("   Hello world!  \nSecond line"),
			"Hello world!"
		);
		assert_eq!(suggested_name(""), "");
	}
}
