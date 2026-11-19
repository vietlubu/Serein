//! Discord-style thread peek: the newest messages of one thread shown beside its parent.
// shortcut: edits refresh only when the pane reopens; mirror MessagePatch if peeks need live edits.
use crate::{Command, State, auth::AuthState, auth::Failure};
use model::{Id, Message};

pub const MAX_MESSAGES: usize = 50;
const MAX_BYTES: usize = 2 << 20;

/// One peeked thread and its bounded tail; reads answer only the request that asked.
#[derive(Default)]
pub struct Pane {
	thread: Option<Id>,
	/// Ascending by id.
	messages: Vec<Message>,
	pending: Option<u64>,
	failed: bool,
	sequence: u64,
}

pub struct View<'a> {
	pub thread: Id,
	pub messages: &'a [Message],
	pub loading: bool,
	pub failed: bool,
}

impl State {
	/// The peeked thread, while it is still known and visible.
	pub fn thread_pane(&self) -> Option<View<'_>> {
		let pane = &self.thread_pane;
		let thread = pane.thread?;
		(self.channel(thread).is_some() && self.can_view(thread)).then_some(View {
			thread,
			messages: &pane.messages,
			loading: pane.pending.is_some(),
			failed: pane.failed,
		})
	}
	/// Peeks a thread of the selected channel and asks for its newest messages.
	pub fn open_thread_pane(&mut self, thread: Id) -> Option<Command> {
		let channel = self.channel(thread)?;
		if !matches!(channel.kind, 10..=12)
			|| channel.guild.is_none()
			|| channel.parent_id != self.selected
			|| !self.can_read_history(thread)
			|| !(self.demo || (self.auth == AuthState::Authenticated && self.gateway_connected))
		{
			return None;
		}
		let sequence = self.thread_pane.sequence.wrapping_add(1);
		self.thread_pane = Pane {
			thread: Some(thread),
			pending: Some(sequence),
			sequence,
			..Pane::default()
		};
		self.revision += 1;
		Some(Command::ThreadPane {
			thread,
			request: sequence,
		})
	}
	pub fn close_thread_pane(&mut self) {
		self.thread_pane = Pane {
			sequence: self.thread_pane.sequence,
			..Pane::default()
		};
		self.revision += 1;
	}
	pub(crate) fn apply_thread_pane(
		&mut self,
		thread: Id,
		request: u64,
		result: Result<Vec<Message>, Failure>,
	) {
		let pane = &mut self.thread_pane;
		if pane.thread != Some(thread) || pane.pending != Some(request) {
			return;
		}
		pane.pending = None;
		match result {
			Ok(list)
				if list.len() <= MAX_MESSAGES
					&& list.iter().all(|m| m.channel == thread && m.id.0 != 0) =>
			{
				// Live messages that arrived during the read are newer than the page's copies.
				let live = std::mem::take(&mut pane.messages);
				for message in list.into_iter().chain(live) {
					if !message.ephemeral && message.bytes() <= crate::MAX_EVENT_BYTES {
						insert(&mut pane.messages, message);
					}
				}
				self.revision += 1;
			}
			Err(failure) if failure.ends_session() && failure != Failure::Capacity => {
				self.fail(failure);
			}
			_ => {
				pane.failed = true;
				pane.messages.clear();
				self.revision += 1;
			}
		}
	}
	pub(crate) fn observe_thread_pane_message(&mut self, message: &Message) {
		if self.thread_pane.thread != Some(message.channel)
			|| message.ephemeral
			|| message.bytes() > crate::MAX_EVENT_BYTES
		{
			return;
		}
		insert(&mut self.thread_pane.messages, message.clone());
		self.revision += 1;
	}
	pub(crate) fn forget_thread_pane_messages(&mut self, channel: Id, ids: &[Id]) {
		if self.thread_pane.thread == Some(channel) {
			self.thread_pane.messages.retain(|m| !ids.contains(&m.id));
			self.revision += 1;
		}
	}
}

/// Upserts by id, then drops the oldest until the item and byte bounds hold.
fn insert(messages: &mut Vec<Message>, message: Message) {
	match messages.binary_search_by_key(&message.id, |m| m.id) {
		Ok(index) => messages[index] = message,
		Err(index) => messages.insert(index, message),
	}
	let mut bytes: usize = messages.iter().map(Message::bytes).sum();
	while messages.len() > MAX_MESSAGES || (bytes > MAX_BYTES && !messages.is_empty()) {
		bytes -= messages.remove(0).bytes();
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use model::{Channel, Freshness, Guild};

	fn channel(id: u64, parent: Option<Id>, kind: u8) -> Channel {
		Channel {
			user_limit: None,
			slowmode: 0,
			id: Id(id),
			guild: Some(Id(1)),
			parent_id: parent,
			kind,
			name: "Synthetic".into(),
			position: 0,
			recipients: vec![],
			last_message: None,
			icon: None,
			member_list_id: None,
			tags: None,
			message_count: None,
		}
	}
	fn message(id: u64, channel: u64) -> Message {
		let mut message = crate::tests::message(id);
		message.channel = Id(channel);
		message
	}
	fn requested(command: Option<Command>) -> u64 {
		let Some(Command::ThreadPane { request, .. }) = command else {
			panic!("the peek asks for one bounded read");
		};
		request
	}

	#[test]
	fn peek_is_bounded_ordered_and_rejects_foreign_pages() {
		let mut state = State {
			auth: AuthState::Authenticated,
			gateway_connected: true,
			freshness: Freshness::Fresh,
			selected: Some(Id(10)),
			user: Some(crate::tests::message(1).author),
			guilds: vec![Guild {
				banner: None,
				default_message_notifications: None,
				stickers: None,
				emojis: None,
				id: Id(1),
				name: "Synthetic".into(),
				icon: None,
			}],
			channels: vec![channel(10, None, 0), channel(30, Some(Id(10)), 11)],
			..State::default()
		};
		crate::tests::grant_permissions(&mut state);
		let thread = Id(30);
		let request = requested(state.open_thread_pane(thread));

		state.apply_thread_pane(thread, request + 1, Ok(vec![]));
		assert!(
			state.thread_pane().unwrap().loading,
			"a stale answer is ignored"
		);

		let page = (100..150).rev().map(|id| message(id, 30)).collect();
		state.apply_thread_pane(thread, request, Ok(page));
		let view = state.thread_pane().unwrap();
		assert!(!view.loading && !view.failed);
		assert_eq!(view.messages.len(), MAX_MESSAGES);
		assert!(view.messages.windows(2).all(|w| w[0].id < w[1].id));

		state.observe_thread_pane_message(&message(200, 30));
		let ids = |state: &State| {
			let view = state.thread_pane().unwrap();
			(
				view.messages.len(),
				view.messages[0].id,
				view.messages.last().unwrap().id,
			)
		};
		assert_eq!(ids(&state), (MAX_MESSAGES, Id(101), Id(200)));
		state.observe_thread_pane_message(&message(300, 10));
		assert_eq!(ids(&state), (MAX_MESSAGES, Id(101), Id(200)));

		let request = requested(state.open_thread_pane(thread));
		state.apply_thread_pane(
			thread,
			request,
			Ok(vec![message(400, 30), message(401, 10)]),
		);
		let view = state.thread_pane().unwrap();
		assert!(view.failed && view.messages.is_empty());
	}
}
