//! Discord-style thread peek: the newest messages of one thread shown beside its parent.
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
	/// Whether `channel` is the selected conversation or the visible peeked thread.
	pub fn showing(&self, channel: Id) -> bool {
		self.selected == Some(channel)
			|| self
				.thread_pane()
				.is_some_and(|pane| pane.thread == channel)
	}
	/// A loaded message of the selected conversation, else of the visible peeked thread.
	pub fn message(&self, id: Id) -> Option<&Message> {
		self.timeline.get(id).or_else(|| {
			let messages = self.thread_pane()?.messages;
			messages
				.binary_search_by_key(&id, |m| m.id)
				.ok()
				.map(|i| &messages[i])
		})
	}
	/// Replaces reactions on a loaded message of either view. The peek keeps its old list when
	/// the new one exceeds the bounds, so a peek copy never invalidates the parent conversation.
	pub fn set_message_reactions(
		&mut self,
		id: Id,
		reactions: Option<Vec<model::Reaction>>,
	) -> Result<(), &'static str> {
		if self.timeline.get(id).is_some() {
			return self.timeline.set_reactions(id, reactions);
		}
		let messages = &mut self.thread_pane.messages;
		if let Ok(index) = messages.binary_search_by_key(&id, |m| m.id)
			&& reactions.as_ref().is_none_or(|r| model::valid_reactions(r))
		{
			let mut message = messages.remove(index);
			message.reactions = reactions;
			insert(messages, message);
			self.revision += 1;
		}
		Ok(())
	}
	/// Applies a patch to the peeked copy; a copy that would exceed the bounds keeps its old content.
	pub(crate) fn patch_thread_pane(
		&mut self,
		patch: &model::MessagePatch,
	) -> Result<(), &'static str> {
		let pane = &mut self.thread_pane;
		if pane.thread != Some(patch.channel) {
			return Ok(());
		}
		let Ok(index) = pane.messages.binary_search_by_key(&patch.id, |m| m.id) else {
			return Ok(());
		};
		let mut message = pane.messages[index].clone();
		session_cache::apply_patch(&mut message, patch);
		if !session_cache::Timeline::valid_message(&message)
			|| message.bytes() > crate::MAX_EVENT_BYTES
		{
			return Err("Message patch exceeds capacity");
		}
		insert(&mut pane.messages, message);
		self.revision += 1;
		Ok(())
	}
	/// A local content patch for whichever view holds `patch.channel`.
	pub(crate) fn patch_loaded(&mut self, patch: model::MessagePatch) -> Result<(), &'static str> {
		if self.selected == Some(patch.channel) {
			self.timeline.patch(patch)
		} else {
			self.patch_thread_pane(&patch)
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

	fn peek_state() -> State {
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
		state
	}
	/// A pane showing thread 30 with one reactable message 100.
	fn seeded() -> State {
		let mut state = peek_state();
		let request = requested(state.open_thread_pane(Id(30)));
		let mut m = message(100, 30);
		m.reactions = Some(vec![]);
		state.apply_thread_pane(Id(30), request, Ok(vec![m]));
		state
	}

	#[test]
	fn peek_is_bounded_ordered_and_rejects_foreign_pages() {
		let mut state = peek_state();
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

	#[test]
	fn peek_reactions_use_the_thread_and_settle_after_close() {
		use crate::reactions;
		let mut state = seeded();
		let emoji = model::ReactionEmoji {
			id: None,
			name: Some("👍".into()),
		};
		assert!(state.can_react(Id(100), Some(&emoji), true));
		let Some(Command::Reactions(reactions::Command::Set {
			channel, request, ..
		})) = state.prepare_reaction(Id(100), emoji.clone())
		else {
			panic!("a peeked message accepts a reaction");
		};
		assert_eq!(
			channel,
			Id(30),
			"the write targets the thread, not the parent"
		);

		state
			.apply_reactions(reactions::Event::Delta {
				channel: Id(30),
				message: Id(100),
				user: Id(77),
				emoji: emoji.clone(),
				add: true,
				burst: false,
			})
			.unwrap();
		let live = state
			.message(Id(100))
			.unwrap()
			.reactions
			.as_deref()
			.unwrap();
		assert!(matches!(live, [r] if r.count == 1 && !r.me));

		// A write that settles after the pane closes must not leave reactions busy forever.
		state.close_thread_pane();
		state
			.apply_reactions(reactions::Event::Written {
				channel: Id(30),
				message: Id(100),
				request,
				result: Ok(()),
			})
			.unwrap();
		state.next_reaction_read();
		assert!(!state.reactions.busy());
	}

	#[test]
	fn peek_edits_and_patches_apply_to_the_copy_without_touching_the_parent() {
		use crate::message_actions::content_patch;
		let mut state = seeded();
		state.timeline.insert(message(5, 10), false, false).unwrap();
		let content = |state: &State| state.message(Id(100)).unwrap().content.clone();
		let original = content(&state);
		let apply = |state: &mut State, event| {
			state.apply(crate::Envelope {
				generation: state.generation,
				event,
			})
		};

		let Some(Command::Edit { request, .. }) = state.prepare_edit(Id(30), Id(100), "new".into())
		else {
			panic!("an own peeked message is editable");
		};
		assert_eq!(content(&state), "new");
		apply(
			&mut state,
			crate::Event::Edited {
				channel: Id(30),
				message: Id(100),
				request,
				result: Err(Failure::Network),
			},
		);
		assert_eq!(content(&state), original, "a failed edit restores the copy");

		apply(
			&mut state,
			crate::Event::Patch(content_patch(Id(30), Id(100), "patched".into())),
		);
		assert_eq!(content(&state), "patched");
		apply(
			&mut state,
			crate::Event::Patch(content_patch(Id(30), Id(100), "x".repeat(70 * 1024))),
		);
		assert_eq!(content(&state), "patched");
		assert!(
			state.timeline.get(Id(5)).is_some(),
			"an oversized peek patch never invalidates the parent"
		);
	}
}
