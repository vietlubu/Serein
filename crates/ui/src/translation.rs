//! In-place translation of received messages after an explicit Globe click.
use model::{Id, Message};
use std::hash::{DefaultHasher, Hash, Hasher};

/// One explicit Globe click, handed to the app; `content` fingerprints the text that was sent.
pub struct Request {
	pub message: Id,
	pub content: u64,
	pub text: String,
}
pub enum KeyRequest {
	Save(zeroize::Zeroizing<String>),
	Forget,
}
pub(crate) struct Shown {
	pub message: Id,
	pub content: u64,
	pub text: String,
}
pub(crate) const MAX_SHOWN: usize = 64;
pub(crate) fn fingerprint(content: &str) -> u64 {
	let mut hasher = DefaultHasher::new();
	content.hash(&mut hasher);
	hasher.finish()
}
/// The translation to draw instead of `message.content`; none once the message was edited.
pub(crate) fn shown<'a>(shown: &'a [Shown], message: &Message) -> Option<&'a str> {
	shown
		.iter()
		.find(|entry| entry.message == message.id && entry.content == fingerprint(&message.content))
		.map(|entry| entry.text.as_str())
}
