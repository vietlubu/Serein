//! `/summary`: one bounded history read handed to the desktop host, answered by a private row.
use crate::{Command, State, auth::AuthState, auth::Failure};
use model::{Id, Message};

pub const DEFAULT_COUNT: u16 = 200;
pub const MAX_COUNT: u16 = 500;
/// Fetched-page budget; stays well under `MAX_EVENT_BYTES` (4 MiB), which would stop the connection.
pub const MAX_BYTES: usize = 2 << 20;
/// Author id of local omp rows; the wire rejects id 0, so no Discord message carries it.
pub const AUTHOR: Id = Id(0);
const MAX_REPLIES: usize = 16;
const MAX_REPLY_BYTES: usize = 512 * 1024;

#[derive(Default)]
pub struct Summary {
	sequence: u64,
	/// The conversation and request still waiting for history or for the host's answer.
	pending: Option<(Id, u64)>,
	loaded: Option<Loaded>,
	/// Reader instruction for the pending request.
	instruction: String,
	/// History read in flight.
	reading: bool,
	/// omp answers kept across conversation switches, oldest first.
	replies: Vec<Message>,
}

/// History ready for the host; newest first, as Discord returns pages.
pub struct Loaded {
	pub channel: Id,
	pub request: u64,
	pub messages: Vec<Message>,
	pub instruction: String,
}

impl State {
	/// Starts one summary of `channel`; a running summary blocks another.
	pub fn request_summary(
		&mut self,
		channel: Id,
		count: u16,
		instruction: &str,
	) -> Result<Command, &'static str> {
		if self.summary.pending.is_some() {
			return Err("A summary is already running.");
		}
		if self
			.channel(channel)
			.is_some_and(|c| matches!(c.kind, 15 | 16))
		{
			return Err("Open a post to summarize it.");
		}
		if !self.can_read_history(channel) {
			return Err("You can't read this conversation's history.");
		}
		if !(self.demo || (self.auth == AuthState::Authenticated && self.gateway_connected)) {
			return Err("Connect to Discord before summarizing.");
		}
		let request = self.summary.sequence.wrapping_add(1);
		self.summary.sequence = request;
		self.summary.pending = Some((channel, request));
		self.summary.loaded = None;
		self.summary.reading = true;
		self.summary.instruction = instruction.to_owned();
		self.status = "Loading messages for the summary…";
		self.revision += 1;
		Ok(Command::SummaryHistory {
			channel,
			count: count.min(MAX_COUNT),
			request,
		})
	}
	pub(crate) fn apply_summary_history(
		&mut self,
		channel: Id,
		request: u64,
		result: Result<Vec<Message>, Failure>,
	) {
		let summary = &mut self.summary;
		if summary.pending != Some((channel, request)) || !summary.reading {
			return;
		}
		summary.reading = false;
		self.status = match result {
			Ok(list) if list.is_empty() => {
				summary.pending = None;
				summary.instruction.clear();
				"No messages to summarize."
			}
			Ok(messages)
				if messages.len() <= MAX_COUNT as usize
					&& messages.iter().all(|m| m.channel == channel && m.id.0 != 0) =>
			{
				summary.loaded = Some(Loaded {
					channel,
					request,
					messages,
					instruction: std::mem::take(&mut summary.instruction),
				});
				"Summarizing with omp…"
			}
			Ok(_) => {
				summary.pending = None;
				summary.instruction.clear();
				"Discord returned an invalid message page."
			}
			Err(_) => {
				summary.pending = None;
				summary.instruction.clear();
				"Couldn't load messages for the summary."
			}
		};
		self.revision += 1;
	}
	/// Hands loaded history to the host once; the request stays pending until `finish_summary`.
	pub fn take_summary_messages(&mut self) -> Option<Loaded> {
		self.summary.loaded.take()
	}
	/// Frees a slot whose history read died with its connection; an omp run already holding
	/// the history keeps going and still answers.
	pub(crate) fn cancel_summary_read(&mut self) {
		if self.summary.reading {
			self.summary.pending = None;
			self.summary.reading = false;
			self.summary.instruction.clear();
		}
	}
	/// Keeps the host's answer as a private row of its conversation, whichever is selected.
	pub fn finish_summary(
		&mut self,
		channel: Id,
		request: u64,
		result: Result<Message, &'static str>,
	) {
		if self.summary.pending != Some((channel, request)) {
			return;
		}
		self.summary.pending = None;
		match result {
			Ok(message)
				if message.ephemeral
					&& message.channel == channel
					&& message.bytes() <= MAX_REPLY_BYTES / 2
					&& session_cache::Timeline::valid_message(&message) =>
			{
				self.status = "";
				let replies = &mut self.summary.replies;
				while !replies.is_empty()
					&& (replies.len() >= MAX_REPLIES
						|| replies.iter().map(Message::bytes).sum::<usize>() + message.bytes()
							> MAX_REPLY_BYTES)
				{
					replies.remove(0);
				}
				replies.push(message);
			}
			Ok(_) => self.status = "omp returned an invalid summary.",
			Err(error) => self.status = error,
		}
		self.revision += 1;
	}
	pub fn summary_replies(&self) -> &[Message] {
		&self.summary.replies
	}
	/// A summary of `channel` is loading history or waiting for omp.
	pub fn summary_running(&self, channel: Id) -> bool {
		self.summary.pending.is_some_and(|(c, _)| c == channel)
	}
	pub(crate) fn dismiss_summary_reply(&mut self, id: Id) {
		self.summary.replies.retain(|m| m.id != id);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use model::{Channel, Freshness, Guild};

	fn channel(id: u64, kind: u8) -> Channel {
		Channel {
			user_limit: None,
			slowmode: 0,
			id: Id(id),
			guild: Some(Id(1)),
			parent_id: None,
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
	fn requested(state: &mut State) -> u64 {
		let Ok(Command::SummaryHistory {
			count: 200,
			request,
			..
		}) = state.request_summary(Id(10), 200, "Hãy dùng tiếng Việt")
		else {
			panic!("a summary asks for one bounded read");
		};
		request
	}

	#[test]
	fn summary_answers_only_its_own_request_and_frees_the_slot() {
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
			channels: vec![channel(10, 0), channel(20, 15), channel(30, 0)],
			..State::default()
		};
		crate::tests::grant_permissions(&mut state);
		assert_eq!(
			state.request_summary(Id(20), 200, "").err(),
			Some("Open a post to summarize it.")
		);

		let first = requested(&mut state);
		assert_eq!(
			state.request_summary(Id(10), 200, "").err(),
			Some("A summary is already running.")
		);
		state.apply_summary_history(Id(10), first + 1, Ok(vec![message(100, 10)]));
		assert!(
			state.take_summary_messages().is_none(),
			"stale reads are ignored"
		);
		state.apply_summary_history(Id(10), first, Ok(vec![message(100, 99)]));
		assert!(state.take_summary_messages().is_none());
		assert_eq!(state.status, "Discord returned an invalid message page.");

		let request = requested(&mut state);
		state.apply_summary_history(
			Id(10),
			request,
			Ok(vec![message(101, 10), message(100, 10)]),
		);
		let loaded = state
			.take_summary_messages()
			.expect("history reaches the host");
		assert_eq!(
			(
				loaded.channel,
				loaded.request,
				loaded.messages.len(),
				loaded.instruction.as_str()
			),
			(Id(10), request, 2, "Hãy dùng tiếng Việt")
		);
		assert!(state.take_summary_messages().is_none(), "handed over once");

		let mut reply = message(200, 10);
		reply.ephemeral = true;
		state.finish_summary(Id(10), request + 1, Ok(reply.clone()));
		assert!(state.summary_replies().is_empty());
		state.finish_summary(Id(10), request, Ok(reply));
		assert_eq!(state.summary_replies().len(), 1);

		// Switching conversations neither cancels the run nor drops earlier answers.
		let request = requested(&mut state);
		state.select(Id(30));
		assert_eq!(state.selected, Some(Id(30)));
		assert!(state.summary_running(Id(10)));
		state.apply_summary_history(Id(10), request, Ok(vec![message(100, 10)]));
		assert!(state.take_summary_messages().is_some());
		let mut reply = message(201, 10);
		reply.ephemeral = true;
		state.finish_summary(Id(10), request, Ok(reply));
		assert!(!state.summary_running(Id(10)));
		assert_eq!(
			state
				.summary_replies()
				.iter()
				.map(|m| (m.id, m.channel))
				.collect::<Vec<_>>(),
			[(Id(200), Id(10)), (Id(201), Id(10))]
		);

		// A history read dropped with its connection frees the slot.
		requested(&mut state);
		state.apply(crate::Envelope {
			generation: state.generation,
			event: crate::Event::Disconnected,
		});
		assert!(!state.summary_running(Id(10)));
		state.gateway_connected = true;
		requested(&mut state);
	}
}
