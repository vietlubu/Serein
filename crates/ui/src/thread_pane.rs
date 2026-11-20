//! Discord-style thread peek beside the parent channel: message rows with the main chat's actions and a composer with replies, edits, `@` suggestions, a multi-line input and the emoji/GIF picker.
use crate::{design, icons, shortcuts::ShortcutView, timeline::TimelineView};
use client_core::{Command, State};
use egui::RichText;
use model::{Id, Message};

#[derive(Default)]
pub(super) struct ThreadPaneUi {
	thread: Option<Id>,
	/// The row the peek was opened from; a message-started thread shares its id.
	origin: Option<Id>,
	draft: String,
	composing: bool,
	formats: crate::markdown::FormatCache,
	mentions: crate::mentions::Menu,
	lookup: crate::member_search::Search,
	layout: crate::composer_text::Layout,
	emoji: crate::emoji_picker::Picker,
	/// Returns focus to the input after a pick closes the picker.
	refocus: bool,
	/// The message the composer edits: channel and id.
	editing: Option<(Id, Id)>,
	/// The draft set aside while editing.
	saved_draft: Option<String>,
	reply: Option<client_core::Reply>,
	viewing: Option<(Id, Id)>,
	/// A reply target to scroll to.
	jump: Option<Id>,
	/// The row whose hover toolbar was painted last frame, and the toolbar.
	toolbar: Option<(Id, egui::Rect)>,
	edit_request: Option<(Id, Id, String)>,
	reply_request: Option<Id>,
	pub opening: Option<String>,
	pub channel_reference: Option<Id>,
	/// The thread whose name or "Open in Full View" was chosen.
	pub full_view: Option<Id>,
	/// A delete for the host's confirmation dialog: channel and message.
	pub deleting: Option<(Id, Id)>,
}

impl ThreadPaneUi {
	pub fn open(&mut self, thread: Id, origin: Id) {
		if self.thread != Some(thread) {
			self.draft.clear();
			self.formats = Default::default();
			self.mentions = Default::default();
			self.layout = Default::default();
			self.editing = None;
			self.saved_draft = None;
			self.reply = None;
			self.viewing = None;
			self.jump = None;
			self.toolbar = None;
			self.deleting = None;
		}
		self.thread = Some(thread);
		self.origin = Some(origin);
	}

	fn cancel_edit(&mut self) {
		self.editing = None;
		self.draft = self.saved_draft.take().unwrap_or_default();
	}

	#[allow(clippy::too_many_arguments)]
	pub fn show(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		commands: &mut Vec<Command>,
		avatars: &mut crate::avatars::Avatars,
		profile: &mut crate::profiles::ProfileSession,
		channel_menu: &mut crate::channel_menu::ChannelMenu,
		timeline: &mut TimelineView,
		view: ShortcutView<'_>,
		hide_nitro_emojis: bool,
		keybinds: &model::Keybinds,
	) {
		let Some(thread) = state.thread_pane().map(|pane| pane.thread) else {
			return;
		};
		let Some(channel) = state.channel(thread).cloned() else {
			return;
		};
		if self
			.editing
			.is_some_and(|(c, id)| state.message(id).is_none_or(|m| m.channel != c))
		{
			self.cancel_edit();
		}
		if self
			.reply
			.is_some_and(|reply| state.message(reply.target()).is_none())
		{
			self.reply = None;
		}
		// A rejected edit returns to this composer so its text is not lost.
		if self.editing.is_none()
			&& let Some(index) = state
				.message_actions
				.failed_edits
				.iter()
				.position(|(c, _, _)| *c == thread)
		{
			let (channel, id, content) = state.message_actions.failed_edits.remove(index);
			self.saved_draft = Some(std::mem::replace(&mut self.draft, content));
			self.editing = Some((channel, id));
			self.reply = None;
		}
		let colors = design::palette(ui);
		let mut close = false;
		let mut retry = false;
		let mut submit = None;
		let mut cancel = false;
		egui::Panel::top("thread-pane-header")
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
					ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
						ui.spacing_mut().item_spacing.x = 4.0;
						close = icons::button(
							ui,
							icons::Icon::Close,
							28.0,
							&crate::i18n::translate("thread-pane-close"),
						)
						.clicked();
						channel_menu.thread_dropdown(ui, state, &channel, view, true);
						ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
							let name = ui
								.add(
									egui::Label::new(
										design::semibold(ui, &channel.name, 16.0)
											.color(colors.text_strong),
									)
									.truncate()
									.sense(egui::Sense::click()),
								)
								.on_hover_cursor(egui::CursorIcon::PointingHand)
								.on_hover_text(crate::i18n::translate(
									"post-menu-context-open-in-full-view",
								));
							if name.clicked() {
								self.full_view = Some(thread);
							}
						});
					});
				});
			});
		let input_id = egui::Id::unique(("thread-pane-input", thread));
		egui::Panel::bottom("thread-pane-composer")
			.show_separator_line(false)
			.frame(egui::Frame::new().inner_margin(egui::Margin {
				left: 16,
				right: 16,
				top: 2,
				bottom: 8,
			}))
			.show(ui, |ui| {
				let mut ime_frame = self.composing;
				ui.input(|input| {
					for event in &input.events {
						if let egui::Event::Ime(event) = event {
							ime_frame = true;
							self.composing = matches!(
								event,
								egui::ImeEvent::Preedit { text, .. } if !text.is_empty()
							);
						}
					}
				});
				let ctx = ui.ctx().clone();
				if std::mem::take(&mut self.refocus) {
					ctx.memory_mut(|m| m.request_focus(input_id));
				}
				// Suggestion rows can take focus on press; keep the input alive until release.
				let pointer = self.mentions.pointer_interacting(&ctx, thread);
				if pointer && !ime_frame {
					ctx.memory_mut(|memory| memory.request_focus(input_id));
				}
				let enabled = !ime_frame
					&& state.can_send_to(thread)
					&& ctx.memory(|m| m.has_focus(input_id));
				let users = if enabled || self.draft.contains("<@") {
					crate::mentions::known_users(state, thread)
				} else {
					Vec::new()
				};
				let cursor = egui::text_edit::TextEditState::load(&ctx, input_id)
					.and_then(|s| s.cursor.char_range())
					.filter(|r| r.is_empty())
					.map(|r| r.primary.index.0);
				self.mentions.hide_nitro_emojis = hide_nitro_emojis;
				if std::mem::take(&mut self.mentions.retry_games) {
					state.game_mentions.status = client_core::game_mentions::Status::Idle;
				}
				self.mentions.refresh(
					state,
					thread,
					&self.draft,
					cursor.filter(|_| enabled),
					&users,
				);
				if state.game_mentions.status == client_core::game_mentions::Status::Idle
					&& (self.mentions.needs_games() || self.draft.contains("<@$"))
				{
					state.game_mentions.request();
					ctx.request_repaint();
				}
				if enabled && let Some(pick) = self.mentions.keys(&ctx) {
					self.accept(&ctx, input_id, pick, state.content_limit());
				}
				if enabled && self.mentions.claims_tab() {
					// egui moves focus on unclaimed Tab before the menu can consume it.
					ctx.memory_mut(|m| {
						m.set_focus_lock_filter(
							input_id,
							egui::EventFilter {
								tab: true,
								horizontal_arrows: true,
								vertical_arrows: true,
								escape: false,
							},
						)
					});
				}
				let enter = !ime_frame
					&& ctx.memory(|m| m.has_focus(input_id))
					&& ctx.input_mut(|i| {
						keybinds
							.chords(model::KeybindAction::SendMessage)
							.any(|chord| crate::keybinds::pressed_exact(i, chord))
					});
				let mass =
					state.permission(thread, model::permissions::MENTION_EVERYONE) == Some(true);
				if !ime_frame
					&& ctx.input(|i| {
						i.key_pressed(egui::Key::Backspace) || i.key_pressed(egui::Key::Delete)
					}) {
					self.layout.galley_games(
						ui,
						&self.draft,
						ui.available_width(),
						&users,
						crate::mentions::known_roles(state, thread),
						&state.channels,
						&state.game_mentions,
						mass,
						avatars,
						state.demo,
					);
					self.layout.select_deleted_inline(&ctx, input_id);
				}
				// Reply/edit context sits flush on top of the input, like the main composer.
				let frame = ui.scope(|ui| {
					ui.spacing_mut().item_spacing.y = 0.0;
					let capped = if self.editing.is_some() {
						crate::composer_cap(ui, &colors, |ui| {
							ui.label(
								RichText::new(crate::i18n::translate(
									"lib-ime-updates-text-editing-message",
								))
								.size(13.0)
								.color(colors.muted),
							);
							ui.with_layout(
								egui::Layout::right_to_left(egui::Align::Center),
								|ui| {
									cancel |= icons::button(
										ui,
										icons::Icon::Close,
										22.0,
										&crate::i18n::translate("lib-ime-updates-text-cancel-edit"),
									)
									.clicked();
								},
							);
						});
						true
					} else if let Some(reply) = self.reply.as_mut() {
						let author = state
							.message(reply.target())
							.map(|m| state.message_author_name(m).to_owned())
							.unwrap_or_default();
						crate::composer_cap(ui, &colors, |ui| {
							ui.spacing_mut().item_spacing.x = 0.0;
							ui.label(
								RichText::new(crate::i18n::translate(
									"lib-ime-updates-text-replying-to",
								))
								.size(13.0)
								.color(colors.muted),
							);
							ui.add_space(4.0);
							ui.label(design::semibold(ui, author, 13.0).color(colors.text_strong));
							ui.with_layout(
								egui::Layout::right_to_left(egui::Align::Center),
								|ui| {
									ui.spacing_mut().item_spacing.x = 6.0;
									cancel |= icons::button(
										ui,
										icons::Icon::Close,
										22.0,
										&crate::i18n::translate(
											"lib-ime-updates-text-cancel-reply",
										),
									)
									.clicked();
									crate::mention_switch(ui, &colors, &mut reply.mention);
								},
							);
						});
						true
					} else {
						false
					};
					design::glass_frame(ui, colors.raised, egui::Margin::symmetric(10, 6))
						.corner_radius(if capped {
							egui::CornerRadius {
								nw: 0,
								ne: 0,
								sw: 8,
								se: 8,
							}
						} else {
							egui::CornerRadius::same(8)
						})
						.show(ui, |ui| {
							ui.add_enabled_ui(state.can_send_to(thread), |ui| {
								let limit = state.content_limit();
								let count = self.draft.chars().count();
								let can_send = state.can_send_to(thread)
									&& count <= limit && (self.editing.is_some()
									|| !self.draft.trim().is_empty());
								ui.horizontal(|ui| {
									ui.spacing_mut().item_spacing.x = 8.0;
									ui.with_layout(
										egui::Layout::right_to_left(egui::Align::Center),
										|ui| {
											ui.spacing_mut().item_spacing.x = 4.0;
											let send_button = ui
												.add_enabled_ui(can_send, |ui| {
													icons::button(
														ui,
														icons::Icon::Send,
														28.0,
														&crate::i18n::translate(
															if self.editing.is_some() {
																"lib-ime-updates-text-save-edit"
															} else {
																"lib-ime-updates-text-send-message"
															},
														),
													)
												})
												.inner;
											let send = enter || send_button.clicked();
											if count + 200 >= limit {
												ui.label(
													RichText::new(format!(
														"{}",
														limit.saturating_sub(count)
													))
													.size(11.0)
													.color(if count >= limit {
														colors.danger
													} else {
														colors.muted
													}),
												);
											}
											let pick = ui
												.add_enabled_ui(!ime_frame, |ui| {
													self.emoji.hide_nitro_emojis =
														hide_nitro_emojis;
													self.emoji
														.show(ui, state, thread, avatars, commands)
												})
												.inner;
											if pick.is_some() && !self.emoji.is_open() {
												self.refocus = true;
												ctx.request_repaint();
											}
											match pick {
												Some(crate::emoji_picker::Pick::Insert(text)) => {
													let range =
														egui::text_edit::TextEditState::load(
															&ctx, input_id,
														)
														.and_then(|s| s.cursor.char_range());
													match crate::emoji_picker::insert_with_limit(
														&mut self.draft,
														&text,
														range,
														limit * 4,
														limit,
													) {
														Some(cursor) => {
															place_caret(&ctx, input_id, cursor)
														}
														None => {
															state.status = "Emoji will not fit. Shorten this message or free draft space.";
														}
													}
												}
												// A chosen GIF is its own message; the typed draft stays untouched.
												Some(crate::emoji_picker::Pick::Send(url)) => {
													if self.editing.is_some() {
														state.status = "Finish or cancel the edit before sending a GIF.";
													} else if let Some(command) =
														state.prepare_text_send_to(thread, &url)
													{
														commands.push(command);
													}
												}
												// Core sends stickers only to the selected channel.
												Some(crate::emoji_picker::Pick::Sticker(_)) => {
													state.status = "Stickers can only be sent from the main composer.";
												}
												Some(
													crate::emoji_picker::Pick::React(..)
													| crate::emoji_picker::Pick::Choose(_),
												)
												| None => {}
											}
											ui.with_layout(
												egui::Layout::left_to_right(egui::Align::Center),
												|ui| {
													let mut output = egui::ScrollArea::vertical()
														.id_salt((input_id, "scroll"))
														.max_height(
															ctx.viewport_rect().height() * 0.5,
														)
														.min_scrolled_height(
															ctx.viewport_rect().height() * 0.5,
														)
														.auto_shrink([false, true])
														.show(ui, |ui| {
															let layout = &mut self.layout;
															let mut layouter =
																|ui: &egui::Ui,
																 buffer: &dyn egui::TextBuffer,
																 width: f32| {
																	layout.galley_games(
																	ui,
																	buffer.as_str(),
																	width,
																	&users,
																	crate::mentions::known_roles(
																		state, thread,
																	),
																	&state.channels,
																	&state.game_mentions,
																	mass,
																	avatars,
																	state.demo,
																)
																};
															// A pre-laid galley keeps a long thread name on one elided line.
															let hint = if self.draft.is_empty() {
																let mut job = egui::text::LayoutJob::simple_singleline(
																	crate::i18n::translate_args(
																		"thread-pane-message-hint",
																		&[("thread", &channel.name)],
																	),
																	egui::TextStyle::Body.resolve(ui.style()),
																	ui.visuals().weak_text_color(),
																);
																job.wrap = egui::text::TextWrapping::truncate_at_width(
																	ui.available_width(),
																);
																egui::WidgetText::from(
																	ui.painter().layout_job(job),
																)
															} else {
																egui::WidgetText::default()
															};
															let output = egui::TextEdit::multiline(
																&mut self.draft,
															)
															.id(input_id)
															.layouter(&mut layouter)
															.event_filter(egui::EventFilter {
																horizontal_arrows: true,
																vertical_arrows: true,
																escape: false,
																..Default::default()
															})
															.char_limit(limit)
															.desired_rows(1)
															.desired_width(f32::INFINITY)
															.min_size(egui::vec2(
																0.0,
																ui.spacing().interact_size.y,
															))
															.align(egui::Align2::LEFT_CENTER)
															.frame(egui::Frame::NONE)
															.hint_text(hint)
															.show(ui);
															self.layout.paint(ui, &output, avatars);
															output
														})
														.inner;
													if !ime_frame {
														self.layout.snap_cursor(&mut output, &ctx);
													}
													// An emptied edit submits too: it asks to delete the message.
													if send
														&& !ime_frame && (self.editing.is_some()
														|| !self.draft.trim().is_empty())
													{
														submit = Some(self.draft.trim().to_owned());
														output.response.request_focus();
													}
													// An open suggestion menu consumes Escape before the input sees it.
													if output.response.lost_focus()
														&& ui.input(|i| {
															i.key_pressed(egui::Key::Escape)
														}) && !ime_frame
													{
														cancel = true;
													}
												},
											);
										},
									);
								});
							});
						})
				});
				let frame = frame.inner;
				let cursor = egui::text_edit::TextEditState::load(&ctx, input_id)
					.and_then(|s| s.cursor.char_range())
					.filter(|r| r.is_empty())
					.map(|r| r.primary.index.0)
					.filter(|_| enabled);
				// Refresh against this frame's typing so the popup and its picks match the draft.
				self.mentions
					.refresh(state, thread, &self.draft, cursor, &users);
				if let Some(pick) = self
					.mentions
					.show(ui, frame.response.rect, avatars, state.demo)
				{
					self.accept(&ctx, input_id, pick, state.content_limit());
				}
				self.lookup.text = self.mentions.member_query().to_owned();
				self.lookup.sync(&ctx, state, thread, 0, commands);
			});
		let mut rendered = Vec::new();
		let last_toolbar = self.toolbar.take();
		// An open menu keeps its row's toolbar and highlight until it closes.
		let retained = last_toolbar
			.filter(|_| egui::Popup::is_any_open(ui.ctx()))
			.map(|(id, _)| id);
		bottom_scroll(ui, egui::Id::unique(("thread-pane", thread)), |ui| {
			ui.add_space(16.0);
			egui::Frame::new()
				.inner_margin(egui::Margin::symmetric(16, 0))
				.show(ui, |ui| {
					ui.set_width(ui.available_width());
					ui.label(design::semibold(ui, &channel.name, 22.0).color(colors.text_strong));
					let origin = self.origin.and_then(|id| state.timeline.get(id));
					if let Some(origin) = origin {
						ui.label(
							RichText::new(crate::i18n::translate_args(
								"thread-pane-started-by",
								&[("name", state.message_author_name(origin))],
							))
							.color(colors.muted),
						);
					}
					ui.separator();
					let Some(pane) = state.thread_pane() else {
						return;
					};
					// A message-started thread shares its starter's id; Discord shows it first.
					let starter = origin.filter(|m| m.id == thread);
					let mut previous: Option<&Message> = None;
					for message in starter
						.into_iter()
						.chain(pane.messages.iter().filter(|m| m.kind != 21))
					{
						self.row(
							ui,
							state,
							avatars,
							profile,
							timeline,
							message,
							previous,
							thread,
							(last_toolbar, retained),
						);
						rendered.push(message.id);
						previous = Some(message);
					}
					for pending in state
						.pending
						.iter()
						.filter(|p| p.channel == thread && p.delivery != model::Delivery::Confirmed)
					{
						ui.label(RichText::new(&pending.content).color(colors.muted));
						if matches!(
							pending.delivery,
							model::Delivery::Rejected | model::Delivery::Ambiguous
						) {
							ui.label(
								RichText::new(crate::i18n::translate("thread-pane-not-sent"))
									.color(colors.danger),
							);
						}
					}
					if pane.loading {
						ui.horizontal(|ui| {
							ui.spinner();
							ui.label(
								RichText::new(crate::i18n::translate("thread-pane-loading"))
									.color(colors.muted),
							);
						});
					} else if pane.failed {
						ui.label(
							RichText::new(crate::i18n::translate("thread-pane-failed"))
								.color(colors.muted),
						);
						retry = ui
							.button(crate::i18n::translate("post-menu-context-retry"))
							.clicked();
					} else if rendered.is_empty() {
						ui.label(
							RichText::new(crate::i18n::translate("thread-pane-empty"))
								.color(colors.muted),
						);
					}
					ui.add_space(12.0);
				});
		});
		self.formats.retain(|id| rendered.contains(&id));
		if let Some((message, attachment)) = self.viewing {
			self.viewing = state.message(message).and_then(|m| {
				crate::attachments::viewer(
					ui,
					&m.attachments,
					attachment,
					crate::attachments::Floor::Fit,
					avatars,
					&mut timeline.download,
					&mut self.opening,
					state.demo,
				)
				.map(|id| (message, id))
			});
		}
		let ctx = ui.ctx().clone();
		if let Some(id) = self.reply_request.take() {
			if self.editing.is_some() {
				self.cancel_edit();
			}
			self.reply = Some(client_core::Reply::to(id));
			place_caret(&ctx, input_id, self.draft.chars().count());
		}
		if let Some((channel, id, content)) = self.edit_request.take() {
			if self.editing.is_none() {
				self.saved_draft = Some(std::mem::replace(&mut self.draft, content));
			} else {
				self.draft = content;
			}
			self.editing = Some((channel, id));
			self.reply = None;
			place_caret(&ctx, input_id, self.draft.chars().count());
		}
		if cancel {
			if self.editing.is_some() {
				self.cancel_edit();
			} else {
				self.reply = None;
			}
		}
		if let Some(content) = submit {
			if let Some((channel, id)) = self.editing {
				if content.is_empty() {
					self.deleting = Some((channel, id));
				} else if let Some(command) = state.prepare_edit(channel, id, content) {
					commands.push(command);
					self.cancel_edit();
				}
			} else if let Some(reply) = self.reply {
				if let Some(command) =
					state.prepare_reply_send(reply.target(), &content, reply.mention)
				{
					commands.push(command);
					self.draft.clear();
					self.reply = None;
				}
			} else if let Some(command) = state.prepare_text_send_to(thread, &content) {
				commands.push(command);
				self.draft.clear();
			}
		}
		if retry && let Some(command) = state.open_thread_pane(thread) {
			commands.push(command);
		}
		if close {
			state.close_thread_pane();
		}
	}

	/// Inserts a picked suggestion and moves the caret after it.
	fn accept(
		&mut self,
		ctx: &egui::Context,
		id: egui::Id,
		pick: crate::mentions::Pick,
		limit: usize,
	) {
		// shortcut: the peek has no timestamp picker, so `@time` does nothing here; use the full composer.
		if pick.time_range().is_some() {
			return;
		}
		let Some(cursor) = crate::mentions::insert(&mut self.draft, pick, false, limit * 4) else {
			return;
		};
		place_caret(ctx, id, cursor);
	}

	/// One row: avatar and header unless grouped with the previous author's row, plus the
	/// main chat's hover toolbar and context menu.
	#[allow(clippy::too_many_arguments)]
	fn row(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		avatars: &mut crate::avatars::Avatars,
		profile: &mut crate::profiles::ProfileSession,
		timeline: &mut TimelineView,
		message: &Message,
		previous: Option<&Message>,
		thread: Id,
		hover: (Option<(Id, egui::Rect)>, Option<Id>),
	) {
		let colors = design::palette(ui);
		let grouped = message.reply_to.is_none()
			&& previous.is_some_and(|previous| {
				previous.author.id == message.author.id
					&& !previous.is_system()
					&& !message.is_system()
					&& (message.id.0 >> 22).saturating_sub(previous.id.0 >> 22) < 420_000
			});
		ui.push_id(message.id, |ui| {
			if !grouped {
				ui.add_space(8.0);
			}
			let background = ui.painter().add(egui::Shape::Noop);
			let row = egui::Frame::NONE.show(ui, |ui| {
				if grouped {
					egui::Frame::new()
						.inner_margin(egui::Margin {
							left: 52,
							..Default::default()
						})
						.show(ui, |ui| {
							self.content(ui, state, avatars, profile, timeline, message, thread)
						});
					return;
				}
				if let Some(reply) = message.reply_to.filter(|_| message.kind != 46) {
					self.reply_preview(ui, state, avatars, message, reply, thread);
				}
				ui.horizontal_top(|ui| {
					ui.spacing_mut().item_spacing.x = 12.0;
					avatars.show_plain(ui, &message.author, 40.0, state.demo);
					ui.vertical(|ui| {
						ui.set_width(ui.available_width());
						ui.horizontal(|ui| {
							ui.spacing_mut().item_spacing.x = 8.0;
							let name = state.message_author_colors(message).map_or(
								colors.text_strong,
								|role| {
									design::role_name_color(
										role.primary,
										colors.chat,
										colors.text_strong,
									)
								},
							);
							ui.label(
								design::medium(ui, state.message_author_name(message), 15.0)
									.color(name),
							);
							// Fixture IDs do not encode a real creation timestamp.
							if message.id.0 >= (1 << 22) {
								let seconds = ((message.id.0 >> 22) + 1_420_070_400_000) / 1000;
								if let Ok(utc) =
									time::OffsetDateTime::from_unix_timestamp(seconds as i64)
								{
									ui.label(
										RichText::new(crate::local_time::calendar(utc))
											.size(12.0)
											.color(colors.muted),
									);
								}
							}
						});
						self.content(ui, state, avatars, profile, timeline, message, thread);
					});
				});
			});
			// The highlight spans the pane, past the content margin.
			let rect =
				egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), row.response.rect.y_range())
					.expand2(egui::vec2(16.0, 0.0));
			if self.jump == Some(message.id) {
				ui.scroll_to_rect(rect, Some(egui::Align::Center));
				self.jump = None;
			}
			self.actions(
				ui, state, timeline, message, thread, rect, background, hover,
			);
		});
	}

	/// The reply header above a row; a loaded original can be jumped to.
	fn reply_preview(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		avatars: &mut crate::avatars::Avatars,
		message: &Message,
		reply: Id,
		thread: Id,
	) {
		let colors = design::palette(ui);
		crate::timeline::reference_row(ui, |ui| {
			if message.reply_deleted {
				ui.add(
					egui::Label::new(
						RichText::new(crate::i18n::translate(
							"timeline-show-with-scroll-message-deleted",
						))
						.size(13.0)
						.italics()
						.color(colors.muted),
					)
					.truncate(),
				);
			} else if let Some(original) = state.message(reply) {
				let avatar = avatars
					.show_plain(ui, &original.author, 16.0, state.demo)
					.interact(egui::Sense::click());
				ui.label(
					design::semibold(
						ui,
						format!("@{}", state.message_author_name(original)),
						13.0,
					)
					.color(colors.muted),
				);
				let text = if crate::embeds::has_spoilers(original) {
					RichText::new("Spoiler")
				} else if !original.attachments.is_empty() && original.content.trim().is_empty() {
					RichText::new(crate::i18n::translate(
						"timeline-show-with-scroll-click-to-see-attachment",
					))
					.italics()
				} else {
					RichText::new(crate::i18n::message_text(original).replace('\n', " "))
				};
				let label = ui
					.add(
						egui::Label::new(text.size(13.0).color(colors.muted))
							.truncate()
							.sense(egui::Sense::click()),
					)
					.on_hover_cursor(egui::CursorIcon::PointingHand);
				if label.clicked() || avatar.clicked() {
					self.jump = Some(reply);
				}
			} else if ui
				.add(
					egui::Label::new(
						RichText::new(crate::i18n::translate(
							"timeline-show-with-scroll-view-original-message",
						))
						.size(13.0)
						.color(colors.muted),
					)
					.sense(egui::Sense::click()),
				)
				.on_hover_cursor(egui::CursorIcon::PointingHand)
				.clicked()
			{
				self.full_view = Some(thread);
			}
		});
	}

	/// The main chat's normal-mode hover toolbar and message menu for one row.
	#[allow(clippy::too_many_arguments)]
	fn actions(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		timeline: &mut TimelineView,
		message: &Message,
		thread: Id,
		rect: egui::Rect,
		background: egui::layers::ShapeIdx,
		(last_toolbar, retained): (Option<(Id, egui::Rect)>, Option<Id>),
	) {
		let id = message.id;
		let allow_hover = !ui.input(|i| i.is_scrolling()) && ui.ctx().dragged_id().is_none();
		// The floating toolbar overlaps the row above; pointer inside it keeps this row active.
		let toolbar_hover = last_toolbar
			.is_some_and(|(active, toolbar)| active == id && ui.rect_contains_pointer(toolbar));
		let other_toolbar_hover = last_toolbar
			.is_some_and(|(active, toolbar)| active != id && ui.rect_contains_pointer(toolbar));
		let pointed = (ui.rect_contains_pointer(rect) || toolbar_hover)
			&& !other_toolbar_hover
			&& !egui::Popup::is_any_open(ui.ctx());
		let hovered = allow_hover && pointed && retained.is_none_or(|active| active == id);
		let pointer_menu = pointed
			&& (ui.input(|i| i.pointer.secondary_clicked()) || crate::select::open_menu(ui.ctx()));
		if !(hovered || pointer_menu || retained == Some(id)) {
			return;
		}
		let colors = design::palette(ui);
		ui.painter().set(
			background,
			egui::Shape::rect_filled(rect, 0.0, design::row_highlight(ui, colors.hover, 0.7)),
		);
		let react = state.can_react(id, None, true)
			|| message.reactions.as_ref().is_some_and(|items| {
				items
					.iter()
					.any(|r| state.can_react(id, Some(&r.emoji), true))
			});
		let own = state
			.user
			.as_ref()
			.is_some_and(|u| u.id == message.author.id);
		let can_edit = !message.unsupported && state.can_edit(message.channel, id);
		// The starter row lives in the parent channel; the pane composer only sends to the thread.
		let can_reply =
			message.channel == thread && state.can_send_to(thread) && !message.ephemeral;
		let can_delete = state.can_delete(message.channel, id);
		let can_pin = state.can_pin(message.channel, id);
		let link = state
			.channel(message.channel)
			.and_then(|channel| crate::markdown::discord_url(channel, Some(id)));
		let translate = crate::timeline::translatable(message);
		let buttons = (if react { 4 } else { 1 }) + 2 + usize::from(translate) + usize::from(own);
		let width = buttons as f32 * 30.0;
		let toolbar_rect = egui::Rect::from_min_size(
			egui::pos2(
				(rect.right() - 16.0 - width).max(rect.left()),
				rect.top() - 12.0,
			),
			egui::vec2(width, 28.0),
		);
		// A child overlay keeps hover from changing wrapping or row heights.
		let mut toolbar = ui.new_child(
			egui::UiBuilder::new()
				.id_salt("thread-pane-hover-actions")
				.max_rect(toolbar_rect)
				.layout(egui::Layout::left_to_right(egui::Align::Center)),
		);
		toolbar.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
		toolbar.spacing_mut().button_padding = egui::vec2(4.0, 2.0);
		toolbar.spacing_mut().interact_size.y = 28.0;
		toolbar
			.painter()
			.rect_filled(toolbar_rect, 6.0, colors.raised);
		toolbar.painter().rect_stroke(
			toolbar_rect,
			6.0,
			egui::Stroke::new(1.0, colors.border),
			egui::StrokeKind::Inside,
		);
		if react {
			let quick = timeline
				.quick_reactions
				.unwrap_or(crate::reactions::QUICK_DEFAULTS);
			let mut last = egui::Rect::NOTHING;
			for (index, text) in quick.into_iter().enumerate() {
				let (emoji, reacted, enabled) =
					crate::timeline::quick_reaction(state, message, text);
				let response = toolbar
					.push_id(("quick-reaction", index), |ui| {
						ui.add_enabled_ui(enabled, |ui| {
							crate::reactions::quick_button(ui, text, reacted)
						})
						.inner
					})
					.inner;
				last = response.rect;
				if response.clicked() {
					timeline.reaction = Some((id, Some(emoji)));
					if !reacted {
						timeline.quick_reaction_used = Some(text);
					}
				}
			}
			toolbar.painter().vline(
				last.right() + 1.0,
				last.y_range().shrink(6.0),
				egui::Stroke::new(1.0, colors.border),
			);
		}
		if translate
			&& crate::timeline::action_button(
				&mut toolbar,
				icons::Icon::Globe,
				"timeline-message-actions-translate",
			)
			.clicked()
			&& let Some((url, truncated)) = crate::timeline::translation_url(message)
		{
			timeline.truncated_translation = truncated.then(|| url.clone());
			timeline.browser_opening = Some(url);
		}
		if let Some((anchor, trigger)) =
			crate::reactions::add_button(&mut toolbar, react, state.reactions.busy())
		{
			timeline.reaction_picker = Some((id, anchor, trigger));
		}
		if own
			&& toolbar
				.add_enabled_ui(can_edit, |ui| {
					crate::timeline::action_button(
						ui,
						icons::Icon::Pencil,
						"timeline-message-actions-edit-message",
					)
				})
				.inner
				.clicked()
		{
			self.edit_request = Some((message.channel, id, message.content.clone()));
		}
		if toolbar
			.add_enabled_ui(can_reply, |ui| {
				crate::timeline::action_button(
					ui,
					icons::Icon::Reply,
					"timeline-message-actions-reply",
				)
			})
			.inner
			.clicked()
		{
			self.reply_request = Some(id);
		}
		let more =
			crate::timeline::action_button(&mut toolbar, icons::Icon::More, "profiles-show-more");
		more.widget_info(|| {
			egui::WidgetInfo::labeled(
				egui::Role::Button,
				toolbar.is_enabled(),
				crate::i18n::translate_args(
					"timeline-message-actions-for",
					&[("user", &message.author.name)],
				),
			)
		});
		let popup_id = egui::Id::unique(("thread-pane-message-menu", state.generation, id));
		let mut popup = egui::Popup::menu(&more).id(popup_id);
		if pointer_menu {
			popup = popup
				.open_memory(Some(egui::SetOpenCommand::Bool(true)))
				.at_pointer_fixed();
		} else if !more.clicked() && egui::Popup::position_of_id(ui.ctx(), popup_id).is_some() {
			popup = popup.at_pointer_fixed();
		}
		let mut edit = None;
		let mut edit_started = false;
		let mut no_thread = None;
		crate::timeline::message_actions(
			popup,
			(
				message,
				link.as_deref(),
				&timeline.extension_actions,
				&mut timeline.extension_request,
			),
			(own, can_reply, can_edit, can_delete),
			(None, &mut self.reply_request),
			(&mut edit, &mut edit_started),
			&mut self.deleting,
			(
				can_pin,
				state.is_pinned(message.channel, id),
				&mut timeline.pin_request,
			),
			(false, &mut no_thread),
			(state.can_forward(id), &mut timeline.forward_request),
			message
				.reactions
				.as_ref()
				.filter(|_| state.can_read_history(message.channel))
				.and_then(|items| items.first())
				.map(|reaction| (reaction.emoji.clone(), &mut timeline.reaction_users)),
		);
		if edit.is_some() {
			self.edit_request = edit;
		}
		self.toolbar = Some((id, toolbar_rect));
	}

	#[allow(clippy::too_many_arguments)]
	fn content(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		avatars: &mut crate::avatars::Avatars,
		profile: &mut crate::profiles::ProfileSession,
		timeline: &mut TimelineView,
		message: &Message,
		thread: Id,
	) {
		let colors = design::palette(ui);
		if message.is_system() {
			ui.label(RichText::new(crate::i18n::message_text(message)).color(colors.muted));
			return;
		}
		let mut surface = crate::select::Surface::new(ui, "thread-pane");
		let mut revealed = 0u32;
		self.formats
			.get(message.id, &message.content)
			.show_references(
				ui,
				&mut self.opening,
				&message.mentions,
				Some(&crate::mentions::MentionSource {
					state,
					channel: thread,
				}),
				profile,
				(
					&state.channels,
					&mut self.channel_reference,
					&state.guilds,
					crate::mentions::known_roles(state, thread),
				),
				(avatars, state.demo, &mut revealed),
				&mut surface,
				design::MessageCardSurface::Opaque,
			);
		let reveal = ui.scope_id().with(("thread-pane-media", message.id));
		let concealed = crate::embeds::has_media_spoilers(message)
			&& !ui.data(|d| d.get_temp::<bool>(reveal).unwrap_or(false));
		if concealed {
			if ui
				.button(crate::i18n::translate(
					"timeline-show-with-scroll-reveal-spoiler-media",
				))
				.clicked()
			{
				ui.data_mut(|d| d.insert_temp(reveal, true));
			}
		} else {
			if !message.attachments.is_empty() {
				crate::attachments::show(
					ui,
					message,
					avatars,
					&mut self.viewing,
					&mut self.opening,
					&mut timeline.download,
					&mut timeline.audio,
					&mut timeline.video,
					state.demo,
					&mut surface,
					design::MessageCardSurface::Opaque,
				);
			}
			if let Some(gif) = crate::embeds::show(
				ui,
				message,
				&mut self.formats,
				avatars,
				&mut self.opening,
				&mut timeline.download,
				profile,
				state,
				design::MessageCardSurface::Opaque,
				timeline.hide_media_links,
				None,
			) {
				timeline.gif_favorite = Some(gif);
			}
		}
		for sticker in &message.sticker_items {
			let response = ui
				.push_id(sticker.id, |ui| {
					crate::stickers::message(
						ui,
						sticker,
						state,
						avatars,
						&mut timeline.sticker_request,
						&mut timeline.browse_sticker,
					)
				})
				.inner;
			surface.keep(&response);
		}
		if message.edited {
			ui.label(
				RichText::new(crate::i18n::translate("timeline-show-with-scroll-edited"))
					.small()
					.color(colors.muted),
			);
		}
		if let Some(action) = crate::reactions::show(
			ui,
			state.reactions.display(message),
			state.gateway_connected
				&& state.freshness == model::Freshness::Fresh
				&& state.can_read_history(message.channel),
			state.reactions.busy(),
			state.reactions.invalidated(message.id),
			(avatars, state.demo),
			message.id,
			state.reactions.users.as_ref(),
			|emoji, add| state.can_react(message.id, Some(emoji), add),
		) {
			match action {
				crate::reactions::Action::Reload => timeline.reaction = Some((message.id, None)),
				crate::reactions::Action::Toggle(emoji) => {
					timeline.reaction = Some((message.id, Some(emoji)))
				}
				crate::reactions::Action::Inspect(emoji, open) => {
					timeline.reaction_users = Some((message.id, emoji, open))
				}
			}
		}
		surface.finish(ui);
	}
}

/// A vertical scroll area whose short content sits on its bottom edge, like Discord's thread pane.
/// Scroll contents have unbounded height, so the previous frame's measured height sizes the spacer.
pub(crate) fn bottom_scroll<R>(
	ui: &mut egui::Ui,
	id: egui::Id,
	add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
	let viewport = ui.available_height();
	let key = id.with("content-height");
	let last = ui.data(|d| d.get_temp::<f32>(key)).unwrap_or(0.0);
	egui::ScrollArea::vertical()
		.id_salt(id)
		.stick_to_bottom(true)
		.auto_shrink([false; 2])
		.show(ui, |ui| {
			// The spacer's trailing item spacing would otherwise overflow by a few pixels.
			ui.add_space((viewport - last - ui.spacing().item_spacing.y).max(0.0));
			let inner = ui.scope(add);
			let height = inner.response.rect.height();
			if (height - last).abs() > 0.5 {
				ui.data_mut(|d| d.insert_temp(key, height));
				ui.ctx().request_repaint();
			}
			inner.inner
		})
		.inner
}

/// Moves the composer caret to `cursor` (in chars) and focuses it.
fn place_caret(ctx: &egui::Context, id: egui::Id, cursor: usize) {
	let mut edit = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
	edit.cursor
		.set_char_range(Some(egui::text::CCursorRange::one(
			egui::text::CCursor::new(cursor),
		)));
	edit.store(ctx, id);
	ctx.memory_mut(|memory| memory.request_focus(id));
}
