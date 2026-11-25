//! Inbox popout (Unread, Mentions, Bookmarks, Reminders) and the reminder time dialog. It
//! reuses the search pane's result cards, caches and attachment viewer.
use super::{MediaUi, SearchUi, result_placeholders};
use crate::{
	design,
	i18n::{translate, translate_args},
	icons::{self, Icon},
};
use client_core::{
	Command, State,
	inbox::{InboxView, Source, Tab, Write},
};
use egui::RichText;
use model::Id;

const TABS: [Tab; 4] = [Tab::Unread, Tab::Mentions, Tab::Bookmarks, Tab::Reminders];

/// One user action (or lazy preview load) per frame, applied once the view borrow ends.
enum Click {
	Jump(Option<Id>, Id, Option<Id>),
	Write(Write),
	MarkRead(Id),
	Load(Id),
	Edit(Id, Id, i128),
	Older,
	Retry,
}

/// Reminder presets shared by the message menu and the reminder dialog.
#[derive(Clone, Copy)]
pub(crate) enum RemindAt {
	Minutes20,
	Hour,
	Hours3,
	Tomorrow,
	NextWeek,
}
impl RemindAt {
	pub(crate) const ALL: [Self; 5] = [
		Self::Minutes20,
		Self::Hour,
		Self::Hours3,
		Self::Tomorrow,
		Self::NextWeek,
	];
	pub(crate) fn label(self) -> &'static str {
		match self {
			Self::Minutes20 => "inbox-remind-20-minutes",
			Self::Hour => "inbox-remind-1-hour",
			Self::Hours3 => "inbox-remind-3-hours",
			Self::Tomorrow => "inbox-remind-tomorrow",
			Self::NextWeek => "inbox-remind-next-week",
		}
	}
	/// Due time in Unix nanoseconds.
	pub(crate) fn due(self) -> i128 {
		use time::Duration;
		let now = crate::local_time::now();
		let nine = time::Time::from_hms(9, 0, 0).unwrap_or(time::Time::MIDNIGHT);
		match self {
			Self::Minutes20 => now + Duration::minutes(20),
			Self::Hour => now + Duration::HOUR,
			Self::Hours3 => now + Duration::hours(3),
			Self::Tomorrow => (now + Duration::DAY).replace_time(nine),
			Self::NextWeek => (now + Duration::days(7)).replace_time(nine),
		}
		.unix_timestamp_nanos()
	}
}

pub(crate) struct ReminderDraft {
	channel: Id,
	message: Id,
	source: Source,
	text: String,
	error: Option<&'static str>,
}

impl SearchUi {
	pub fn inbox_open(&self) -> bool {
		self.inbox_open
	}
	pub fn toggle_inbox(&mut self) {
		self.inbox_open = !self.inbox_open;
		if self.inbox_open {
			self.open = false;
			self.viewing = None;
		}
	}
	#[allow(clippy::too_many_arguments)]
	pub fn inbox_popout(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		anchor: egui::Rect,
		commands: &mut Vec<Command>,
		avatars: &mut crate::avatars::Avatars,
		mut media: MediaUi<'_>,
		profile: &mut crate::profiles::ProfileSession,
	) {
		if !self.inbox_open {
			return;
		}
		// Also reloads after a tab change, or once the connection returns.
		if state.inbox.as_ref().is_none_or(|v| v.tab != self.inbox_tab)
			&& let Some(command) = state.open_inbox(self.inbox_tab)
		{
			commands.push(command);
		}
		let ctx = ui.ctx().clone();
		let viewing = self.viewing.is_some();
		let colors = design::palette_for(&ctx);
		let bounds = ctx.content_rect().shrink(8.0);
		let width = 480.0_f32.min(bounds.width());
		let max_height = (bounds.height() * 0.8).clamp(320.0, 720.0);
		let x = (anchor.right() - width)
			.min(bounds.right() - width)
			.max(bounds.left());
		let y = (anchor.bottom() + 8.0).min(bounds.bottom() - 120.0);
		let area = egui::Area::new(egui::Id::unique("inbox-popout"))
			.kind(egui::UiKind::Popup)
			.order(egui::Order::Foreground)
			.fixed_pos(egui::pos2(x, y))
			.constrain_to(bounds)
			.interactable(true)
			.show(&ctx, |ui| {
				egui::Frame::new()
					.fill(colors.sidebar)
					.stroke(egui::Stroke::new(1.0, colors.border))
					.corner_radius(8)
					.shadow(egui::epaint::Shadow {
						offset: [0, 8],
						blur: 24,
						spread: 0,
						color: egui::Color32::from_black_alpha(96),
					})
					.show(ui, |ui| {
						ui.set_width(width);
						ui.set_max_height(max_height);
						ui.spacing_mut().item_spacing = egui::vec2(8.0, 0.0);
						egui::Frame::new()
							.fill(colors.base)
							.corner_radius(egui::CornerRadius {
								nw: 8,
								ne: 8,
								..Default::default()
							})
							.inner_margin(egui::Margin::symmetric(16, 0))
							.show(ui, |ui| {
								ui.set_width(ui.available_width());
								ui.set_height(48.0);
								ui.horizontal_centered(|ui| {
									ui.spacing_mut().item_spacing.x = 8.0;
									icons::inline(ui, Icon::Inbox, 20.0, colors.muted);
									ui.label(
										design::semibold(ui, translate("inbox"), 16.0)
											.color(colors.text_strong),
									);
									ui.with_layout(
										egui::Layout::right_to_left(egui::Align::Center),
										|ui| {
											if icons::button(
												ui,
												Icon::Close,
												28.0,
												&translate("inbox-close"),
											)
											.clicked()
											{
												self.inbox_open = false;
											}
											let reload = ui
												.add_enabled_ui(state.can_inbox(), |ui| {
													icons::button(
														ui,
														Icon::Reload,
														28.0,
														&translate("inbox-reload"),
													)
												})
												.inner;
											if reload.clicked()
												&& let Some(command) =
													state.open_inbox(self.inbox_tab)
											{
												commands.push(command);
											}
										},
									);
								});
							});
						egui::Frame::new()
							.inner_margin(egui::Margin::symmetric(12, 4))
							.show(ui, |ui| {
								let index = TABS.iter().position(|t| *t == self.inbox_tab);
								if let Some(index) = design::segmented(
									ui,
									&[
										"inbox-tab-unread",
										"inbox-tab-mentions",
										"inbox-tab-bookmarks",
										"inbox-tab-reminders",
									],
									index.unwrap_or(0),
								) {
									self.inbox_tab = TABS[index];
								}
							});
						ui.painter().hline(
							ui.max_rect().x_range(),
							ui.cursor().top(),
							egui::Stroke::new(1.0, colors.border),
						);
						egui::Frame::new()
							.inner_margin(egui::Margin::symmetric(12, 12))
							.show(ui, |ui| {
								ui.set_width(ui.available_width());
								self.inbox_content(
									ui, state, commands, avatars, &mut media, profile,
								);
							});
					});
			});
		let pointer = ctx.input(|i| i.pointer.interact_pos());
		let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
			&& !area
				.response
				.rect
				.contains(pointer.unwrap_or(area.response.rect.center()))
			&& !anchor.contains(pointer.unwrap_or(anchor.center()));
		// A result's menu may extend past the popout; pressing it must not close the popout.
		if clicked_outside && !viewing && self.reminder.is_none() && !egui::Popup::is_any_open(&ctx)
		{
			self.inbox_open = false;
		}
		self.viewer(ui, state, avatars, media.download);
	}
	fn inbox_content(
		&mut self,
		ui: &mut egui::Ui,
		state: &mut State,
		commands: &mut Vec<Command>,
		avatars: &mut crate::avatars::Avatars,
		media: &mut MediaUi<'_>,
		profile: &mut crate::profiles::ProfileSession,
	) {
		let colors = design::palette(ui);
		ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
		if !state.can_inbox() {
			ui.label(
				RichText::new(translate("inbox-unavailable"))
					.small()
					.color(colors.muted),
			);
			return;
		}
		let mut click = None;
		if let Some(view) = state.inbox.as_ref() {
			if view.tab == Tab::Unread {
				self.unread_cards(ui, state, view, avatars, media, profile, &mut click);
			} else {
				self.inbox_page(ui, state, view, avatars, media, profile, &mut click);
			}
		}
		let command = match click {
			Some(Click::Jump(guild, channel, message)) => {
				self.inbox_open = false;
				state.open_inbox_message(guild, channel, message)
			}
			Some(Click::Write(write)) => state.prepare_inbox_write(write),
			Some(Click::MarkRead(channel)) => state.prepare_mark_channel_read(channel),
			Some(Click::Load(channel)) => state.request_unread_card(channel),
			Some(Click::Edit(channel, message, due)) => {
				self.open_reminder(channel, message, Some(due), Source::ForLaterList);
				None
			}
			Some(Click::Older) => state.request_older_inbox(),
			Some(Click::Retry) => state.retry_inbox(),
			None => None,
		};
		commands.extend(command);
	}
	#[allow(clippy::too_many_arguments)]
	fn inbox_page(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		view: &InboxView,
		avatars: &mut crate::avatars::Avatars,
		media: &mut MediaUi<'_>,
		profile: &mut crate::profiles::ProfileSession,
		click: &mut Option<Click>,
	) {
		let colors = design::palette(ui);
		if let Some(error) = view.error {
			ui.horizontal_wrapped(|ui| {
				ui.label(RichText::new(error).color(colors.danger));
				if view.read.is_some() && ui.button(translate("inbox-retry")).clicked() {
					*click = Some(Click::Retry);
				}
			});
		}
		if view.loading {
			result_placeholders(ui, "inbox-loading", 3);
			return;
		}
		let Some(page) = &view.page else {
			return;
		};
		if page.items.is_empty() {
			let (icon, key) = match view.tab {
				Tab::Mentions => (Icon::Inbox, "mentions"),
				Tab::Bookmarks => (Icon::Star, "bookmarks"),
				Tab::Reminders | Tab::Unread => (Icon::Bell, "reminders"),
			};
			design::empty_state(
				ui,
				icon,
				&format!("inbox-empty-{key}-title"),
				&format!("inbox-empty-{key}-detail"),
			);
			return;
		}
		let can_write = state.can_inbox_write();
		let now = time::OffsetDateTime::now_utc().unix_timestamp_nanos();
		egui::ScrollArea::vertical()
			.id_salt(("inbox", view.request))
			.auto_shrink([false, true])
			.show(ui, |ui| {
				ui.spacing_mut().item_spacing.y = 8.0;
				for item in &page.items {
					let hit = &item.hit;
					ui.push_id((hit.channel, hit.id), |ui| {
						ui.allocate_ui_with_layout(
							egui::vec2(ui.available_width(), 24.0),
							egui::Layout::right_to_left(egui::Align::Center),
							|ui| {
								ui.spacing_mut().item_spacing.x = 4.0;
								ui.add_enabled_ui(can_write, |ui| {
									let button = |ui: &mut egui::Ui, icon, key| {
										icons::button(ui, icon, 24.0, &translate(key)).clicked()
									};
									let unsave = Write::Unsave {
										channel: hit.channel,
										message: hit.id,
									};
									match view.tab {
										Tab::Mentions => {
											if button(ui, Icon::Check, "inbox-mark-read") {
												*click = Some(Click::Write(Write::AckMention {
													message: hit.id,
												}));
											}
										}
										Tab::Bookmarks => {
											if button(ui, Icon::Trash, "inbox-remove-bookmark") {
												*click = Some(Click::Write(unsave));
											}
										}
										Tab::Reminders => {
											if button(ui, Icon::Check, "inbox-complete-reminder") {
												*click = Some(Click::Write(unsave));
											}
											if button(ui, Icon::Pencil, "inbox-edit-reminder")
												&& let Some(due) = item.due_at
											{
												*click =
													Some(Click::Edit(hit.channel, hit.id, due));
											}
										}
										Tab::Unread => {}
									}
								});
								ui.with_layout(
									egui::Layout::left_to_right(egui::Align::Center),
									|ui| {
										ui.spacing_mut().item_spacing.x = 8.0;
										// The due label comes first so truncation never hides it.
										if let Some(due) = item.due_at
											&& let Ok(utc) =
												time::OffsetDateTime::from_unix_timestamp_nanos(due)
										{
											let time = crate::local_time::calendar(utc);
											let overdue = due <= now;
											ui.label(
												RichText::new(translate_args(
													if overdue {
														"inbox-reminder-overdue"
													} else {
														"inbox-reminder-due"
													},
													&[("time", &time)],
												))
												.size(12.0)
												.color(if overdue {
													colors.danger
												} else {
													colors.muted
												}),
											);
										}
										ui.add(
											egui::Label::new(
												RichText::new(location(state, hit.channel))
													.size(12.0)
													.color(colors.muted),
											)
											.truncate(),
										);
									},
								);
							},
						);
						let mut target = None;
						self.result_card(ui, state, hit, "", avatars, media, profile, &mut target);
						if let Some(id) = target {
							*click = Some(Click::Jump(item.guild, hit.channel, Some(id)));
						}
					});
				}
				if page.more {
					let key = if view.tab == Tab::Mentions {
						"inbox-older-mentions"
					} else {
						"inbox-older-bookmarks"
					};
					if ui
						.add_enabled(!view.loading, egui::Button::new(translate(key)))
						.clicked()
					{
						*click = Some(Click::Older);
					}
				}
			});
	}
	#[allow(clippy::too_many_arguments)]
	fn unread_cards(
		&mut self,
		ui: &mut egui::Ui,
		state: &State,
		view: &InboxView,
		avatars: &mut crate::avatars::Avatars,
		media: &mut MediaUi<'_>,
		profile: &mut crate::profiles::ProfileSession,
		click: &mut Option<Click>,
	) {
		let colors = design::palette(ui);
		// Mark as read removes a card by making it no longer unread.
		let cards: Vec<_> = view
			.cards
			.iter()
			.filter_map(|card| {
				state
					.inbox_unread(card.channel)
					.then(|| state.channel(card.channel).map(|c| (card, c)))
					.flatten()
			})
			.collect();
		if cards.is_empty() {
			design::empty_state(
				ui,
				Icon::Check,
				"inbox-empty-unread-title",
				"inbox-empty-unread-detail",
			);
			return;
		}
		egui::ScrollArea::vertical()
			.id_salt(("inbox", view.request))
			.auto_shrink([false, true])
			.show(ui, |ui| {
				ui.spacing_mut().item_spacing.y = 8.0;
				for (card, channel) in cards {
					ui.push_id(card.channel, |ui| {
						egui::Frame::new()
							.fill(colors.base)
							.stroke(egui::Stroke::new(1.0, colors.border))
							.corner_radius(8)
							.inner_margin(egui::Margin::same(12))
							.show(ui, |ui| {
								ui.set_width(ui.available_width());
								ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
								ui.horizontal(|ui| {
									match (channel.guild, channel.kind) {
										(None, 3) => {
											avatars.show_group(ui, channel, 32.0, state.demo);
										}
										(None, _) => {
											if let Some(user) = channel.recipients.first() {
												avatars.show_plain(ui, user, 32.0, state.demo);
											}
										}
										(Some(_), kind) => {
											icons::inline(
												ui,
												icons::channel(kind),
												20.0,
												colors.muted,
											);
										}
									}
									ui.with_layout(
										egui::Layout::right_to_left(egui::Align::Center),
										|ui| {
											let read = ui
												.add_enabled_ui(
													state.can_mark_channel_read(channel.id),
													|ui| {
														icons::button(
															ui,
															Icon::Check,
															24.0,
															&translate("inbox-mark-read"),
														)
													},
												)
												.inner;
											if read.clicked() {
												*click = Some(Click::MarkRead(channel.id));
											}
											let mentions = state.mention_count(channel.id);
											if mentions > 0 {
												let (rect, _) = ui.allocate_exact_size(
													egui::vec2(24.0, 18.0),
													egui::Sense::hover(),
												);
												crate::notifications::badge(
													ui,
													rect.center(),
													mentions,
													colors.base,
												);
											}
											ui.with_layout(
												egui::Layout::left_to_right(egui::Align::Center),
												|ui| {
													let name = ui
														.add(
															egui::Label::new(
																design::semibold(
																	ui,
																	state
																		.conversation_name(channel),
																	15.0,
																)
																.color(colors.text_strong),
															)
															.truncate()
															.sense(egui::Sense::click()),
														)
														.on_hover_cursor(
															egui::CursorIcon::PointingHand,
														);
													if name.clicked() {
														*click = Some(Click::Jump(
															channel.guild,
															channel.id,
															None,
														));
													}
												},
											);
										},
									);
								});
								if let Some(guild) = channel
									.guild
									.and_then(|g| state.guilds.iter().find(|x| x.id == g))
								{
									let parent = channel
										.parent_id
										.and_then(|p| state.channel(p))
										.map(|p| format!(" › {}", p.name))
										.unwrap_or_default();
									ui.add(
										egui::Label::new(
											RichText::new(format!("{}{parent}", guild.name))
												.size(12.0)
												.color(colors.muted),
										)
										.truncate(),
									);
								}
								match (&card.page, card.error) {
									(Some(page), _) if !page.hits.is_empty() => {
										for hit in &page.hits {
											ui.push_id(hit.id, |ui| {
												let mut target = None;
												self.result_card(
													ui,
													state,
													hit,
													"",
													avatars,
													media,
													profile,
													&mut target,
												);
												if let Some(id) = target {
													*click = Some(Click::Jump(
														None,
														card.channel,
														Some(id),
													));
												}
											});
										}
									}
									(Some(_), _) => {
										ui.label(
											RichText::new(translate("inbox-unread-none-loaded"))
												.small()
												.color(colors.muted),
										);
									}
									(None, Some(error)) => {
										ui.label(RichText::new(error).color(colors.danger));
									}
									(None, None) => {
										result_placeholders(ui, "inbox-unread-loading", 1);
										// Previews load lazily, one at a time, as cards scroll in.
										if !card.loading
											&& click.is_none() && ui.is_rect_visible(ui.min_rect())
										{
											*click = Some(Click::Load(card.channel));
										}
									}
								}
							});
					});
				}
			});
	}
	pub(crate) fn open_reminder(
		&mut self,
		channel: Id,
		message: Id,
		due: Option<i128>,
		source: Source,
	) {
		let at = due
			.and_then(|due| time::OffsetDateTime::from_unix_timestamp_nanos(due).ok())
			.unwrap_or_else(|| time::OffsetDateTime::now_utc() + time::Duration::HOUR);
		let at = crate::local_time::local(at);
		self.reminder = Some(ReminderDraft {
			channel,
			message,
			source,
			text: format!(
				"{:04}-{:02}-{:02} {:02}:{:02}",
				at.year(),
				u8::from(at.month()),
				at.day(),
				at.hour(),
				at.minute()
			),
			error: None,
		});
	}
	pub(crate) fn reminder_dialog(
		&mut self,
		ctx: &egui::Context,
		state: &mut State,
		commands: &mut Vec<Command>,
	) {
		let Some(draft) = self.reminder.as_mut() else {
			return;
		};
		let mut due = None;
		let mut cancel = false;
		let response =
			crate::dialog::Dialog::new("inbox-reminder", translate("inbox-reminder-title"))
				.icon(Icon::Bell)
				.width(380.0)
				.show(ctx, |d| {
					d.content(|ui| {
						let colors = design::palette(ui);
						ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
						ui.horizontal_wrapped(|ui| {
							for at in RemindAt::ALL {
								if ui.button(translate(at.label())).clicked() {
									due = Some(Ok(at.due()));
								}
							}
						});
						ui.label(translate("inbox-reminder-time"));
						ui.add(
							egui::TextEdit::singleline(&mut draft.text)
								.char_limit(16)
								.desired_width(f32::INFINITY),
						);
						if let Some(error) = draft.error {
							ui.label(RichText::new(translate(error)).color(colors.danger));
						}
					});
					d.footer(|ui| {
						if crate::dialog::action(
							ui,
							"inbox-reminder-save",
							crate::dialog::Action::Primary,
						)
						.clicked()
						{
							due = Some(parse_due(&draft.text));
						}
						if crate::dialog::action(
							ui,
							"dialog-module-cancel",
							crate::dialog::Action::Neutral,
						)
						.clicked()
						{
							cancel = true;
						}
					});
				});
		match due {
			Some(Ok(due)) => {
				let write = Write::Save {
					channel: draft.channel,
					message: draft.message,
					due_at: Some(due),
					source: draft.source,
				};
				// The dialog stays open while an earlier inbox write is still pending.
				if let Some(command) = state.prepare_inbox_write(write) {
					commands.push(command);
					cancel = true;
				}
			}
			Some(Err(key)) => draft.error = Some(key),
			None => {}
		}
		if cancel || response.close {
			self.reminder = None;
		}
	}
}

fn location(state: &State, channel: Id) -> String {
	let Some(c) = state.channel(channel) else {
		return translate("inbox-unknown-conversation");
	};
	match c.guild {
		Some(guild) => match state.guilds.iter().find(|g| g.id == guild) {
			Some(guild) => format!("#{} · {}", c.name, guild.name),
			None => format!("#{}", c.name),
		},
		None => state.conversation_name(c).to_owned(),
	}
}

/// Parses `YYYY-MM-DD HH:MM` in the local zone into a future Unix-nanosecond due time.
fn parse_due(text: &str) -> Result<i128, &'static str> {
	let format =
		time::format_description::parse_borrowed::<2>("[year]-[month]-[day] [hour]:[minute]")
			.map_err(|_| "inbox-reminder-invalid")?;
	let at = time::PrimitiveDateTime::parse(text.trim(), &format)
		.map_err(|_| "inbox-reminder-invalid")?;
	// shortcut: uses today's local offset, so a time across a DST change shifts by the DST delta; use a tz database if users report it.
	let due = at
		.assume_offset(crate::local_time::now().offset())
		.unix_timestamp_nanos();
	if due <= time::OffsetDateTime::now_utc().unix_timestamp_nanos() {
		return Err("inbox-reminder-past");
	}
	Ok(due)
}

#[cfg(test)]
mod tests {
	#[test]
	fn reminder_time_parses_local_future_only() {
		assert_eq!(
			super::parse_due("25/11/2026"),
			Err("inbox-reminder-invalid")
		);
		assert_eq!(
			super::parse_due("2000-01-01 09:00"),
			Err("inbox-reminder-past")
		);
		let expected = time::Date::from_calendar_date(2999, time::Month::January, 1)
			.unwrap()
			.with_hms(9, 0, 0)
			.unwrap()
			.assume_utc()
			.unix_timestamp_nanos();
		assert_eq!(super::parse_due(" 2999-01-01 09:00 "), Ok(expected));
	}
}
