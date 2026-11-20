//! Discord-style thread peek beside the parent channel: read-only rows and a reply box with `@` suggestions.
use crate::{design, icons, shortcuts::ShortcutView};
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
	pub opening: Option<String>,
	pub channel_reference: Option<Id>,
	/// The thread whose name or "Open in Full View" was chosen.
	pub full_view: Option<Id>,
}

impl ThreadPaneUi {
	pub fn open(&mut self, thread: Id, origin: Id) {
		if self.thread != Some(thread) {
			self.draft.clear();
			self.formats = Default::default();
			self.mentions = Default::default();
			self.layout = Default::default();
		}
		self.thread = Some(thread);
		self.origin = Some(origin);
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
		view: ShortcutView<'_>,
		hide_nitro_emojis: bool,
	) {
		let Some(thread) = state.thread_pane().map(|pane| pane.thread) else {
			return;
		};
		let Some(channel) = state.channel(thread).cloned() else {
			return;
		};
		let colors = design::palette(ui);
		let mut close = false;
		let mut retry = false;
		let mut submit = None;
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
		egui::Panel::bottom("thread-pane-composer")
			.show_separator_line(false)
			.frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 12)))
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
				let input_id = egui::Id::unique(("thread-pane-input", thread));
				let ctx = ui.ctx().clone();
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
				let roles = crate::mentions::known_roles(state, thread);
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
						roles,
						&state.channels,
						&state.game_mentions,
						mass,
						avatars,
						state.demo,
					);
					self.layout.select_deleted_inline(&ctx, input_id);
				}
				let frame = egui::Frame::new()
					.fill(colors.raised)
					.corner_radius(8)
					.inner_margin(egui::Margin::symmetric(12, 10))
					.show(ui, |ui| {
						ui.add_enabled_ui(state.can_send_to(thread), |ui| {
							let layout = &mut self.layout;
							let mut layouter =
								|ui: &egui::Ui, buffer: &dyn egui::TextBuffer, width: f32| {
									layout.galley_games(
										ui,
										buffer.as_str(),
										width,
										&users,
										roles,
										&state.channels,
										&state.game_mentions,
										mass,
										avatars,
										state.demo,
									)
								};
							let mut output = egui::TextEdit::singleline(&mut self.draft)
								.id(input_id)
								.layouter(&mut layouter)
								.frame(egui::Frame::NONE)
								.char_limit(state.content_limit())
								.desired_width(f32::INFINITY)
								.hint_text(crate::i18n::translate_args(
									"thread-pane-message-hint",
									&[("thread", &channel.name)],
								))
								.show(ui);
							self.layout.paint(ui, &output, avatars);
							if !ime_frame {
								self.layout.snap_cursor(&mut output, &ctx);
							}
							let input = output.response;
							if input.lost_focus()
								&& ui.input(|i| i.key_pressed(egui::Key::Enter))
								&& !ime_frame && !self.draft.trim().is_empty()
							{
								submit = Some(self.draft.trim().to_owned());
								input.request_focus();
							}
						});
					});
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
		egui::ScrollArea::vertical()
			.id_salt(("thread-pane", thread))
			.stick_to_bottom(true)
			.auto_shrink([false; 2])
			.show(ui, |ui| {
				ui.add_space(16.0);
				egui::Frame::new()
					.inner_margin(egui::Margin::symmetric(16, 0))
					.show(ui, |ui| {
						ui.set_width(ui.available_width());
						ui.label(
							design::semibold(ui, &channel.name, 22.0).color(colors.text_strong),
						);
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
							self.row(ui, state, avatars, profile, message, previous, thread);
							rendered.push(message.id);
							previous = Some(message);
						}
						for pending in state.pending.iter().filter(|p| {
							p.channel == thread && p.delivery != model::Delivery::Confirmed
						}) {
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
		if let Some(content) = submit
			&& let Some(command) = state.prepare_text_send_to(thread, &content)
		{
			commands.push(command);
			self.draft.clear();
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
		let mut edit = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
		edit.cursor
			.set_char_range(Some(egui::text::CCursorRange::one(
				egui::text::CCursor::new(cursor),
			)));
		edit.store(ctx, id);
		ctx.memory_mut(|memory| memory.request_focus(id));
	}

	/// One read-only row: avatar and header unless grouped with the previous author's row.
	#[allow(clippy::too_many_arguments)]
	fn row(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		avatars: &mut crate::avatars::Avatars,
		profile: &mut crate::profiles::ProfileSession,
		message: &Message,
		previous: Option<&Message>,
		thread: Id,
	) {
		let colors = design::palette(ui);
		let grouped = previous.is_some_and(|previous| {
			previous.author.id == message.author.id
				&& !previous.is_system()
				&& !message.is_system()
				&& (message.id.0 >> 22).saturating_sub(previous.id.0 >> 22) < 420_000
		});
		ui.push_id(message.id, |ui| {
			if grouped {
				egui::Frame::new()
					.inner_margin(egui::Margin {
						left: 52,
						..Default::default()
					})
					.show(ui, |ui| {
						self.content(ui, state, avatars, profile, message, thread)
					});
				return;
			}
			ui.add_space(8.0);
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
					self.content(ui, state, avatars, profile, message, thread);
				});
			});
		});
	}

	fn content(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		avatars: &mut crate::avatars::Avatars,
		profile: &mut crate::profiles::ProfileSession,
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
		surface.finish(ui);
		let media = message.attachments.len() + message.embeds.len();
		if media > 0 {
			ui.label(
				RichText::new(crate::i18n::translate_args(
					"thread-pane-media",
					&[("count", &media.to_string())],
				))
				.small()
				.color(colors.muted),
			);
		}
	}
}
