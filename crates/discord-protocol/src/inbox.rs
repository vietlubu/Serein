//! Inbox lists: recent mentions, saved messages and unread previews. Unofficial user routes,
//! unverified in Serein.
use crate::{
	Timestamp,
	search::{Hit, list},
};
use model::{
	Id, SEARCH_PAGE_SIZE, SearchPage,
	inbox::{INBOX_PAGE_SIZE, InboxItem, InboxPage, MAX_REMINDERS},
};
use serde::Deserialize;

/// Response cap for every inbox read.
pub const MAX_WIRE: usize = 2 * 1024 * 1024;

#[derive(Deserialize)]
struct SaveData {
	channel_id: Id,
	message_id: Id,
	#[serde(default)]
	guild_id: Option<Id>,
	saved_at: Timestamp,
	#[serde(default)]
	due_at: Option<Timestamp>,
}
#[derive(Deserialize)]
struct Saved {
	#[serde(default)]
	message: Option<Hit>,
	save_data: SaveData,
}
fn item(saved: Saved) -> Result<Option<InboxItem>, &'static str> {
	// shortcut: a deleted message's save is hidden, so it cannot be removed here; add a placeholder row when users need it.
	let Some(message) = saved.message else {
		return Ok(None);
	};
	let hit = message.into_hit();
	let data = saved.save_data;
	if hit.id != data.message_id || hit.channel != data.channel_id {
		return Err("Saved message does not match its save data");
	}
	Ok(Some(InboxItem {
		hit,
		guild: data.guild_id,
		saved_at: Some(data.saved_at.0),
		due_at: data.due_at.map(|t| t.0),
	}))
}

#[derive(Deserialize)]
pub struct Mentions(#[serde(deserialize_with = "list::<_,_,25>")] Vec<Hit>);
impl Mentions {
	pub fn into_page(self, before: Option<Id>) -> Result<InboxPage, &'static str> {
		let items: Vec<_> = self
			.0
			.into_iter()
			.map(|m| InboxItem {
				hit: m.into_hit(),
				guild: None,
				saved_at: None,
				due_at: None,
			})
			.collect();
		if items.windows(2).any(|w| w[0].hit.id <= w[1].hit.id)
			|| before.is_some_and(|b| items.iter().any(|i| i.hit.id >= b))
		{
			return Err("Mention page did not advance");
		}
		let page = InboxPage {
			more: items.len() == INBOX_PAGE_SIZE,
			items,
		};
		page.valid(INBOX_PAGE_SIZE)
			.then_some(page)
			.ok_or("Invalid mentions")
	}
}

#[derive(Deserialize)]
pub struct Bookmarks {
	#[serde(deserialize_with = "list::<_,_,25>")]
	results: Vec<Saved>,
	#[serde(default)]
	has_more: bool,
}
impl Bookmarks {
	pub fn into_page(self, before: Option<i128>) -> Result<InboxPage, &'static str> {
		if self.has_more && self.results.is_empty()
			|| before.is_some_and(|b| self.results.iter().any(|s| s.save_data.saved_at.0 >= b))
		{
			return Err("Bookmark page did not advance");
		}
		let mut items = Vec::with_capacity(self.results.len());
		for saved in self.results {
			items.extend(item(saved)?);
		}
		items.sort_unstable_by_key(|i| std::cmp::Reverse(i.saved_at));
		let page = InboxPage {
			more: self.has_more && !items.is_empty(),
			items,
		};
		page.valid(INBOX_PAGE_SIZE)
			.then_some(page)
			.ok_or("Invalid bookmarks")
	}
}

#[derive(Deserialize)]
pub struct Reminders {
	#[serde(default, deserialize_with = "list::<_,_,100>")]
	reminders: Vec<Saved>,
}
impl Reminders {
	pub fn into_page(self) -> Result<InboxPage, &'static str> {
		let mut items = Vec::with_capacity(self.reminders.len());
		for saved in self.reminders {
			items.extend(item(saved)?.filter(|i| i.due_at.is_some()));
		}
		items.sort_unstable_by_key(|i| (i.due_at, i.hit.id));
		let page = InboxPage { items, more: false };
		page.valid(MAX_REMINDERS)
			.then_some(page)
			.ok_or("Invalid reminders")
	}
}

#[derive(Deserialize)]
pub struct Messages(#[serde(deserialize_with = "list::<_,_,30>")] Vec<Hit>);
impl Messages {
	/// Messages after `marker`, oldest first, keeping the newest page when more arrived.
	pub fn into_unread(self, channel: Id, marker: Option<Id>) -> Result<SearchPage, &'static str> {
		let mut hits: Vec<_> = self
			.0
			.into_iter()
			.map(Hit::into_hit)
			.filter(|h| marker.is_none_or(|m| h.id > m))
			.collect();
		hits.sort_unstable_by_key(|h| h.id);
		hits.drain(..hits.len().saturating_sub(SEARCH_PAGE_SIZE));
		let page = SearchPage {
			hits,
			total: 0,
			partial: false,
			pin_cursor: None,
		};
		page.valid(Some(channel), None, true)
			.then_some(page)
			.ok_or("Invalid unread messages")
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::{Value, json};
	fn msg(id: u64, channel: u64) -> Value {
		json!({"id":id.to_string(),"channel_id":channel.to_string(),"author":{"id":"7","username":"Synthetic"},"content":"synthetic"})
	}
	fn saved(id: u64, saved_at: &str, due_at: Option<&str>) -> Value {
		json!({"message":msg(id,1),"save_data":{"channel_id":"1","message_id":id.to_string(),"guild_id":"5","saved_at":saved_at,"due_at":due_at}})
	}
	fn bytes(value: Value) -> Vec<u8> {
		serde_json::to_vec(&value).unwrap()
	}
	#[test]
	fn inbox_lists_are_bounded_ordered_and_matched() {
		let mentions = |v: Value| crate::decode::<Mentions>(&bytes(v));
		assert!(mentions(Value::Array((1..=26).rev().map(|i| msg(i, 1)).collect())).is_err());
		let page = mentions(json!([msg(9, 1), msg(8, 2)]))
			.unwrap()
			.into_page(Some(Id(10)))
			.unwrap();
		assert!(!page.more && page.items[1].hit.channel == Id(2));
		assert!(
			mentions(json!([msg(8, 1), msg(9, 1)]))
				.unwrap()
				.into_page(None)
				.is_err()
		);
		assert!(
			mentions(json!([msg(9, 1)]))
				.unwrap()
				.into_page(Some(Id(9)))
				.is_err()
		);

		let bookmarks = |v: Value| crate::decode::<Bookmarks>(&bytes(v));
		let mut wrong = saved(9, "2026-11-10T07:48:11Z", None);
		wrong["save_data"]["message_id"] = "8".into();
		assert!(
			bookmarks(json!({"results":[wrong],"has_more":false}))
				.unwrap()
				.into_page(None)
				.is_err()
		);
		assert!(
			bookmarks(json!({"results":[],"has_more":true}))
				.unwrap()
				.into_page(None)
				.is_err()
		);
		let page = bookmarks(json!({"results":[
			saved(3, "2026-11-09T00:00:00Z", None),
			{"message":null,"save_data":{"channel_id":"1","message_id":"4","saved_at":"2026-11-11T00:00:00Z"}},
			saved(2, "2026-11-10T00:00:00Z", None)
		],"has_more":true}))
		.unwrap()
		.into_page(None)
		.unwrap();
		assert_eq!(
			page.items.iter().map(|i| i.hit.id).collect::<Vec<_>>(),
			[Id(2), Id(3)]
		);
		assert!(page.more && page.items[0].guild == Some(Id(5)));
		let cursor = page.items[1].saved_at;
		assert!(
			bookmarks(json!({"results":[saved(3, "2026-11-09T00:00:00Z", None)],"has_more":false}))
				.unwrap()
				.into_page(cursor)
				.is_err()
		);

		let page = crate::decode::<Reminders>(&bytes(json!({"reminders":[
			saved(4, "2026-11-01T00:00:00Z", Some("2026-11-30T14:30:00+00:00")),
			saved(5, "2026-11-01T00:00:00Z", None),
			saved(6, "2026-11-01T00:00:00Z", Some("2026-11-29T14:30:00+00:00"))
		],"bookmark_ids":["1"]})))
		.unwrap()
		.into_page()
		.unwrap();
		assert_eq!(
			page.items.iter().map(|i| i.hit.id).collect::<Vec<_>>(),
			[Id(6), Id(4)]
		);

		let messages = |v: Value| crate::decode::<Messages>(&bytes(v));
		let page = messages(json!([msg(10, 1), msg(8, 1), msg(9, 1)]))
			.unwrap()
			.into_unread(Id(1), Some(Id(8)))
			.unwrap();
		assert_eq!(
			page.hits.iter().map(|h| h.id).collect::<Vec<_>>(),
			[Id(9), Id(10)]
		);
		assert!(
			messages(json!([msg(9, 2)]))
				.unwrap()
				.into_unread(Id(1), None)
				.is_err()
		);
		assert!(messages(Value::Array((1..=31).map(|i| msg(i, 1)).collect())).is_err());
	}
}
