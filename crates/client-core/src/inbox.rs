//! Inbox: unread conversations, recent mentions, bookmarks and reminders. Every route is an
//! unofficial user route, unverified in Serein.
use crate::{
	Command, State,
	auth::{AuthState, Failure},
};
use model::{
	Id, SearchPage,
	inbox::{INBOX_PAGE_SIZE, InboxPage, MAX_INBOX_BYTES, MAX_REMINDERS},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tab {
	#[default]
	Unread,
	Mentions,
	Bookmarks,
	Reminders,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Read {
	Mentions { before: Option<Id> },
	Bookmarks { before: Option<i128> },
	Reminders,
	Unread { channel: Id, around: Option<Id> },
}
pub enum Outcome {
	Page(InboxPage),
	Unread(SearchPage),
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
	ContextMenu,
	ForLaterList,
}
impl Source {
	pub fn wire(self) -> &'static str {
		match self {
			Self::ContextMenu => "context_menu",
			Self::ForLaterList => "for_later_list",
		}
	}
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Write {
	/// `due_at` (Unix ns) makes it a reminder; `None` bookmarks.
	Save {
		channel: Id,
		message: Id,
		due_at: Option<i128>,
		source: Source,
	},
	Unsave {
		channel: Id,
		message: Id,
	},
	AckMention {
		message: Id,
	},
}
pub const MAX_UNREAD_CARDS: usize = 50;
pub struct UnreadCard {
	pub channel: Id,
	pub marker: Option<Id>,
	pub request: u64,
	pub loading: bool,
	pub error: Option<&'static str>,
	pub page: Option<SearchPage>,
}
pub struct InboxView {
	pub tab: Tab,
	/// Last page read, for Retry; `None` on Unread.
	pub read: Option<Read>,
	pub request: u64,
	pub loading: bool,
	pub error: Option<&'static str>,
	pub page: Option<InboxPage>,
	pub cards: Vec<UnreadCard>,
}
impl InboxView {
	pub fn contains(&self, message: Id) -> bool {
		self.page
			.as_ref()
			.is_some_and(|p| p.items.iter().any(|i| i.hit.id == message))
			|| self.cards.iter().any(|c| {
				c.page
					.as_ref()
					.is_some_and(|p| p.hits.iter().any(|h| h.id == message))
			})
	}
}

impl State {
	pub fn can_inbox(&self) -> bool {
		self.auth == AuthState::Authenticated && self.gateway_connected
	}
	pub fn can_inbox_write(&self) -> bool {
		self.can_inbox() && self.inbox_pending.is_none()
	}
	/// Whether a conversation belongs on the Unread tab.
	pub fn inbox_unread(&self, channel: Id) -> bool {
		let Some(c) = self.channel(channel) else {
			return false;
		};
		if !c.supports_text() || matches!(c.kind, 15 | 16) || !self.can_view(channel) {
			return false;
		}
		if c.guild.is_some() {
			let now = std::time::SystemTime::now()
				.duration_since(std::time::UNIX_EPOCH)
				.map_or(0, |elapsed| elapsed.as_millis() as i64);
			self.lights_guild_rail(c, now) || self.mention_count(channel) > 0
		} else {
			self.dm_muted(channel) != Some(true)
				&& !self.message_request_pending(channel)
				&& !self.spam_direct(channel)
				&& (self.channel_unread(c) == Some(true) || self.unread_count(channel) > 0)
		}
	}
	pub fn open_inbox(&mut self, tab: Tab) -> Option<Command> {
		if !self.can_inbox() {
			return None;
		}
		let read = match tab {
			Tab::Unread => {
				self.inbox_request += 1;
				let mut rows: Vec<(Id, Id)> = self
					.channels
					.iter()
					.filter(|c| self.inbox_unread(c.id))
					.map(|c| (self.channel_activity(c), c.id))
					.collect();
				rows.sort_unstable_by_key(|&row| std::cmp::Reverse(row));
				rows.truncate(MAX_UNREAD_CARDS);
				let cards = rows
					.into_iter()
					.map(|(_, channel)| UnreadCard {
						channel,
						marker: self.read_marker(channel).flatten(),
						request: 0,
						loading: false,
						error: None,
						page: None,
					})
					.collect();
				self.inbox = Some(InboxView {
					tab,
					read: None,
					request: self.inbox_request,
					loading: false,
					error: None,
					page: None,
					cards,
				});
				return None;
			}
			Tab::Mentions => Read::Mentions { before: None },
			Tab::Bookmarks => Read::Bookmarks { before: None },
			Tab::Reminders => Read::Reminders,
		};
		self.request_inbox_read(tab, read)
	}
	fn request_inbox_read(&mut self, tab: Tab, read: Read) -> Option<Command> {
		self.inbox_request += 1;
		let request = self.inbox_request;
		self.inbox = Some(InboxView {
			tab,
			read: Some(read),
			request,
			loading: true,
			error: None,
			page: None,
			cards: Vec::new(),
		});
		Some(Command::Inbox { request, read })
	}
	pub fn request_older_inbox(&mut self) -> Option<Command> {
		if !self.can_inbox() {
			return None;
		}
		let view = self.inbox.as_ref().filter(|v| !v.loading)?;
		let last = view.page.as_ref().filter(|p| p.more)?.items.last()?;
		let read = match view.tab {
			Tab::Mentions => Read::Mentions {
				before: Some(last.hit.id),
			},
			Tab::Bookmarks => Read::Bookmarks {
				before: Some(last.saved_at?),
			},
			Tab::Unread | Tab::Reminders => return None,
		};
		self.request_inbox_read(view.tab, read)
	}
	pub fn retry_inbox(&mut self) -> Option<Command> {
		if !self.can_inbox() {
			return None;
		}
		let view = self.inbox.as_ref()?;
		let (tab, read) = (view.tab, view.read?);
		self.request_inbox_read(tab, read)
	}
	pub fn request_unread_card(&mut self, channel: Id) -> Option<Command> {
		if !self.can_inbox() {
			return None;
		}
		let view = self
			.inbox
			.as_mut()
			.filter(|v| v.tab == Tab::Unread && !v.cards.iter().any(|c| c.loading))?;
		let card = view
			.cards
			.iter_mut()
			.find(|c| c.channel == channel && c.page.is_none() && c.error.is_none())?;
		self.inbox_request += 1;
		card.request = self.inbox_request;
		card.loading = true;
		Some(Command::Inbox {
			request: self.inbox_request,
			read: Read::Unread {
				channel,
				around: card.marker,
			},
		})
	}
	pub fn close_inbox(&mut self) {
		self.inbox = None;
	}
	pub fn apply_inbox(&mut self, request: u64, read: Read, result: Result<Outcome, Failure>) {
		if let Err(f) = result
			&& f.ends_session()
			&& f != Failure::Capacity
		{
			self.fail(f);
			return;
		}
		let Some(view) = self.inbox.as_mut() else {
			return;
		};
		if let Read::Unread { channel, .. } = read {
			let loaded: usize = view
				.cards
				.iter()
				.filter(|c| c.channel != channel)
				.filter_map(|c| c.page.as_ref())
				.map(SearchPage::bytes)
				.sum();
			let Some(card) = view
				.cards
				.iter_mut()
				.find(|c| c.channel == channel && c.request == request && c.loading)
			else {
				return;
			};
			card.loading = false;
			match result {
				Ok(Outcome::Unread(page))
					if page.valid(Some(channel), None, true)
						&& page.hits.iter().all(|h| Some(h.id) > card.marker) =>
				{
					if loaded + page.bytes() > MAX_INBOX_BYTES {
						card.error =
							Some("Preview limit reached; open the conversation to read it");
					} else {
						card.page = Some(page);
					}
				}
				Ok(_) => card.error = Some("Unread messages were invalid or too large"),
				Err(f) => card.error = Some(f.label()),
			}
			return;
		}
		if !view.loading || view.request != request || view.read != Some(read) {
			return;
		}
		view.loading = false;
		let max = if read == Read::Reminders {
			MAX_REMINDERS
		} else {
			INBOX_PAGE_SIZE
		};
		match result {
			Ok(Outcome::Page(page)) if page.valid(max) => {
				view.page = Some(page);
				view.error = None;
			}
			Ok(_) => view.error = Some("Inbox results were invalid or too large"),
			Err(f) => view.error = Some(f.label()),
		}
	}
	pub fn open_inbox_message(
		&mut self,
		guild: Option<Id>,
		channel: Id,
		message: Option<Id>,
	) -> Option<Command> {
		if !self.can_inbox() {
			return None;
		}
		let guild = self.channel(channel).map_or(guild, |c| c.guild);
		match self.open_chat_link(guild, channel, message) {
			Ok(command) => command,
			Err(status) => {
				self.status = status;
				None
			}
		}
	}
	pub fn prepare_inbox_write(&mut self, write: Write) -> Option<Command> {
		if !self.can_inbox() {
			return None;
		}
		if self.inbox_pending.is_some() {
			self.status = "Wait for the previous inbox action to finish";
			return None;
		}
		let valid = match write {
			Write::Save {
				channel,
				message,
				due_at,
				..
			} => channel.0 > 0 && message.0 > 0 && due_at.is_none_or(|due| due > 0),
			Write::Unsave { channel, message } => channel.0 > 0 && message.0 > 0,
			Write::AckMention { message } => message.0 > 0,
		};
		if !valid {
			return None;
		}
		self.inbox_request += 1;
		self.inbox_pending = Some(self.inbox_request);
		Some(Command::InboxWrite {
			request: self.inbox_request,
			write,
		})
	}
	pub fn apply_inbox_write(&mut self, request: u64, write: Write, result: Result<(), Failure>) {
		if let Err(f) = result
			&& f.ends_session()
			&& f != Failure::Capacity
		{
			self.fail(f);
			return;
		}
		if self.inbox_pending != Some(request) {
			return;
		}
		self.inbox_pending = None;
		if result.is_err() {
			self.status = match write {
				Write::Save { due_at: None, .. } => "Bookmark failed",
				Write::Save { .. } => "Reminder failed",
				Write::Unsave { .. } => "Could not remove the saved message",
				Write::AckMention { .. } => "Could not mark the mention read",
			};
			return;
		}
		let view = self.inbox.as_mut();
		match write {
			Write::Save { due_at, .. } => {
				self.status = if due_at.is_some() {
					"Reminder set"
				} else {
					"Message bookmarked"
				};
				self.inbox_changed = true;
			}
			Write::Unsave { channel, message } => {
				if let Some(page) = view
					.filter(|v| matches!(v.tab, Tab::Bookmarks | Tab::Reminders))
					.and_then(|v| v.page.as_mut())
				{
					page.items
						.retain(|i| (i.hit.channel, i.hit.id) != (channel, message));
				}
			}
			Write::AckMention { message } => {
				if let Some(page) = view
					.filter(|v| v.tab == Tab::Mentions)
					.and_then(|v| v.page.as_mut())
				{
					page.items.retain(|i| i.hit.id != message);
				}
			}
		}
	}
}
