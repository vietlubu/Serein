//! One bounded "start a thread" request at a time, drafted in the right-hand thread pane.
use crate::{design, i18n::translate, icons};
use client_core::{Command, State, channel_actions::Action};
use egui::RichText;
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
	/// Moves keyboard focus to the opening-message composer on the next frame.
	focus: bool,
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

/// The typed name, else one derived from the opening message the way Discord seeds it.
fn thread_name(name: &str, content: &str) -> String {
	let name = name.trim();
	if name.is_empty() {
		suggested_name(content)
	} else {
		name.to_owned()
	}
}

#[derive(Default)]
pub(crate) struct ThreadCreateUi {
	request: Option<Request>,
	generation: u64,
	/// IME composition is active in the opening-message composer.
	composing: bool,
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
			focus: true,
		});
	}
	pub fn is_open(&self) -> bool {
		self.request.is_some()
	}
	/// Drops stale drafts and, once the thread exists, sends the opening message and returns its id.
	pub fn poll(&mut self, state: &mut State, commands: &mut Vec<Command>) -> Option<Id> {
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
		// Leaving the channel abandons an unsent draft, like Discord.
		if !request.submitted && state.selected != Some(channel) {
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
		None
	}
	/// Draws the draft inside the right panel: header, bottom-aligned form and opening composer.
	pub fn show(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		commands: &mut Vec<Command>,
		keybinds: &model::Keybinds,
	) {
		let Self {
			request: slot,
			composing,
			..
		} = self;
		let Some(request) = slot.as_mut() else {
			return;
		};
		let channel = request.channel;
		let colors = design::palette(ui);
		let mut close = false;
		egui::Panel::top("thread-create-header")
			.exact_size(48.0)
			.show_separator_line(false)
			.frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 0)))
			.show(ui, |ui| {
				let rect = ui.max_rect();
				ui.painter().hline(
					rect.x_range().expand(16.0),
					rect.bottom(),
					egui::Stroke::new(1.0, colors.border),
				);
				ui.horizontal_centered(|ui| {
					ui.spacing_mut().item_spacing.x = 8.0;
					icons::inline(ui, icons::Icon::Thread, 22.0, colors.muted);
					ui.label(
						design::semibold(ui, translate("thread-create-new-thread"), 16.0)
							.color(colors.text_strong),
					);
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						close = icons::button(
							ui,
							icons::Icon::Close,
							28.0,
							&translate("thread-pane-close"),
						)
						.clicked();
					});
				});
			});
		let can_submit = !request.submitted
			&& !state.channel_action_pending()
			&& (state.demo || state.gateway_connected)
			&& client_core::channel_actions::valid_name(&thread_name(
				&request.name,
				&request.content,
			));
		let mut submit = false;
		egui::Panel::bottom("thread-create-composer")
			.show_separator_line(false)
			.frame(egui::Frame::new().inner_margin(egui::Margin {
				left: 16,
				right: 16,
				top: 2,
				bottom: 8,
			}))
			.show(ui, |ui| {
				let mut ime_frame = *composing;
				ui.input(|input| {
					for event in &input.events {
						if let egui::Event::Ime(event) = event {
							ime_frame = true;
							*composing = matches!(
								event,
								egui::ImeEvent::Preedit { text, .. } if !text.is_empty()
							);
						}
					}
				});
				let ctx = ui.ctx().clone();
				let input_id = egui::Id::unique(("thread-create-input", channel));
				if std::mem::take(&mut request.focus) {
					ctx.memory_mut(|m| m.request_focus(input_id));
				}
				// Consumed before the input is added so Enter submits instead of inserting a newline.
				let enter = !ime_frame
					&& ctx.memory(|m| m.has_focus(input_id))
					&& ctx.input_mut(|i| {
						keybinds
							.chords(model::KeybindAction::SendMessage)
							.any(|chord| crate::keybinds::pressed_exact(i, chord))
					});
				design::glass_frame(ui, colors.raised, egui::Margin::symmetric(10, 6))
					.corner_radius(8)
					.show(ui, |ui| {
						ui.horizontal(|ui| {
							ui.spacing_mut().item_spacing.x = 8.0;
							ui.with_layout(
								egui::Layout::right_to_left(egui::Align::Center),
								|ui| {
									let send = if request.submitted {
										ui.spinner();
										false
									} else {
										ui.add_enabled_ui(can_submit, |ui| {
											icons::button(
												ui,
												icons::Icon::Send,
												28.0,
												&translate("lib-ime-updates-text-send-message"),
											)
										})
										.inner
										.clicked()
									};
									submit = enter || send;
									ui.with_layout(
										egui::Layout::left_to_right(egui::Align::Center),
										|ui| {
											ui.add(
											egui::TextEdit::multiline(&mut request.content)
												.id(input_id)
												.hint_text(translate(
													"thread-create-show-enter-a-message-to-start-the-conversation",
												))
												.char_limit(
													client_core::MAX_CONTENT
														+ model::message_options::PREFIX_ALLOWANCE,
												)
												.desired_rows(1)
												.desired_width(f32::INFINITY)
												.frame(egui::Frame::NONE),
										);
											request.content.shrink_to_fit();
										},
									);
								},
							);
						});
					});
			});
		crate::thread_pane::bottom_scroll(ui, egui::Id::unique(("thread-create", channel)), |ui| {
			ui.add_space(16.0);
			egui::Frame::new()
				.inner_margin(egui::Margin::symmetric(16, 0))
				.show(ui, |ui| {
					ui.set_width(ui.available_width());
					crate::timeline::badge(ui, icons::Icon::Thread);
					ui.add_space(16.0);
					let label = ui.label(
						design::semibold(ui, translate("thread-create-show-thread-name"), 15.0)
							.color(colors.text_strong),
					);
					ui.add_space(8.0);
					design::input(
						ui,
						egui::TextEdit::singleline(&mut request.name)
							.hint_text(translate("thread-create-new-thread"))
							.align(egui::Align2::LEFT_CENTER)
							.char_limit(100),
					)
					.labelled_by(label.id);
					request.name.shrink_to_fit();
					if request.message.is_none() && state.can_create_private_thread(channel) {
						ui.add_space(16.0);
						ui.label(
							design::semibold(
								ui,
								translate("thread-create-show-private-thread"),
								15.0,
							)
							.color(colors.text_strong),
						);
						ui.checkbox(
							&mut request.private,
							translate("thread-create-show-private-hint"),
						);
					} else {
						request.private = false;
					}
					if let Some(message) = request.message.and_then(|id| state.timeline.get(id)) {
						ui.add_space(16.0);
						ui.label(
							design::semibold(ui, state.message_author_name(message), 14.0)
								.color(colors.text_strong),
						);
						ui.add(
							egui::Label::new(
								RichText::new(crate::i18n::message_text(message))
									.color(colors.text),
							)
							.wrap(),
						);
					}
					if let Some(error) = state
						.channel_action_status(channel)
						.filter(|_| !state.channel_action_succeeded(channel))
					{
						ui.add_space(12.0);
						design::notice(ui, design::Level::Error, error);
					}
					ui.add_space(12.0);
				});
		});
		if submit && can_submit {
			// `poll` finds a message-less thread by this exact name.
			request.name = thread_name(&request.name, &request.content);
			let action = Action::CreateThread {
				name: request.name.clone(),
				message: request.message,
				private: request.private && request.message.is_none(),
			};
			if let Some(command) = state.request_channel_action(channel, action) {
				commands.push(command);
				request.submitted = true;
			}
		}
		if close {
			*slot = None;
		}
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
