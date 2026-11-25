use crate::{Id, SearchHit};

pub const INBOX_PAGE_SIZE: usize = 25;
/// Discord returns every reminder at once; more is refused as unsafe.
pub const MAX_REMINDERS: usize = 100;
/// Bound for one inbox page and for all unread previews held together.
pub const MAX_INBOX_BYTES: usize = 2 * 1024 * 1024;

pub struct InboxItem {
	pub hit: SearchHit,
	/// From save data; `None` for mentions and DMs (derive from the loaded channel).
	pub guild: Option<Id>,
	/// Unix nanoseconds; bookmarks and reminders only.
	pub saved_at: Option<i128>,
	/// Reminder due time, Unix nanoseconds.
	pub due_at: Option<i128>,
}
pub struct InboxPage {
	pub items: Vec<InboxItem>,
	/// Discord reported more items.
	pub more: bool,
}
impl InboxPage {
	pub fn bytes(&self) -> usize {
		self.items.capacity() * size_of::<InboxItem>()
			+ self
				.items
				.iter()
				.map(|i| crate::search::hit_bytes(&i.hit))
				.sum::<usize>()
	}
	pub fn valid(&self, max: usize) -> bool {
		self.items.len() <= max
			&& self.bytes() <= MAX_INBOX_BYTES
			&& self.items.iter().all(|i| crate::search::valid_hit(&i.hit))
			&& self.items.iter().enumerate().all(|(n, i)| {
				self.items[..n]
					.iter()
					.all(|o| (o.hit.channel, o.hit.id) != (i.hit.channel, i.hit.id))
			})
	}
}
