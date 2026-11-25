//! Inbox routes observed in Discord's web client: unofficial user routes, unverified in Serein.
use crate::{DiscordApi, Failure};
use client_core::inbox::{Outcome, Read, Write};
use discord_protocol::inbox::{Bookmarks, MAX_WIRE, Mentions, Messages, Reminders};
use wreq::Method;

impl DiscordApi {
	pub(super) async fn inbox(&self, read: Read) -> Result<Outcome, Failure> {
		let path = match read {
			Read::Mentions { before } => {
				let mut path = "/users/@me/mentions?limit=25&roles=true&everyone=true".to_owned();
				if let Some(before) = before {
					if before.0 == 0 {
						return Err(Failure::Protocol);
					}
					path.push_str(&format!("&before={before}"));
				}
				path
			}
			Read::Bookmarks { before } => {
				let mut path = "/users/@me/saved-messages/bookmarks?limit=25".to_owned();
				if let Some(before) = before {
					let cursor = discord_protocol::pins::format_cursor(before)
						.map_err(|_| Failure::Protocol)?;
					let encoded: String =
						cursor.bytes().map(|byte| format!("%{byte:02X}")).collect();
					path.push_str(&format!("&before={encoded}"));
				}
				path
			}
			Read::Reminders => "/users/@me/saved-messages?bookmark_ids=true".to_owned(),
			Read::Unread { channel, around } => {
				if channel.0 == 0 || around.is_some_and(|m| m.0 == 0) {
					return Err(Failure::Protocol);
				}
				match around {
					Some(around) => {
						format!("/channels/{channel}/messages?limit=30&around={around}")
					}
					None => format!("/channels/{channel}/messages?limit=30"),
				}
			}
		};
		let bytes = self
			.request_limited(Method::GET, &path, None, MAX_WIRE)
			.await?;
		let decoded = match read {
			Read::Mentions { before } => discord_protocol::decode::<Mentions>(&bytes)
				.map_err(|_| Failure::Protocol)?
				.into_page(before)
				.map(Outcome::Page),
			Read::Bookmarks { before } => discord_protocol::decode::<Bookmarks>(&bytes)
				.map_err(|_| Failure::Protocol)?
				.into_page(before)
				.map(Outcome::Page),
			Read::Reminders => discord_protocol::decode::<Reminders>(&bytes)
				.map_err(|_| Failure::Protocol)?
				.into_page()
				.map(Outcome::Page),
			Read::Unread { channel, around } => discord_protocol::decode::<Messages>(&bytes)
				.map_err(|_| Failure::Protocol)?
				.into_unread(channel, around)
				.map(Outcome::Unread),
		};
		decoded.map_err(|_| Failure::Protocol)
	}
	pub(super) async fn inbox_write(&self, write: Write) -> Result<(), Failure> {
		let (method, path, body) = match write {
			Write::Save {
				channel,
				message,
				due_at,
				source,
			} => {
				let body = match due_at {
					// Millisecond precision, like the web client.
					Some(due) => serde_json::json!({
						"due_at": discord_protocol::pins::format_cursor(due.div_euclid(1_000_000) * 1_000_000)
							.map_err(|_| Failure::Protocol)?,
						"source": source.wire(),
					}),
					None => serde_json::json!({"source": source.wire()}),
				};
				(Method::PUT, saved_path(channel, message)?, Some(body))
			}
			Write::Unsave { channel, message } => {
				(Method::DELETE, saved_path(channel, message)?, None)
			}
			Write::AckMention { message } => {
				if message.0 == 0 {
					return Err(Failure::Protocol);
				}
				(
					Method::DELETE,
					format!("/users/@me/mentions/{message}"),
					None,
				)
			}
		};
		self.request(method, &path, body).await.map(|_| ())
	}
}
fn saved_path(channel: model::Id, message: model::Id) -> Result<String, Failure> {
	if channel.0 == 0 || message.0 == 0 {
		return Err(Failure::Protocol);
	}
	Ok(format!("/users/@me/saved-messages/{channel}/{message}"))
}

#[cfg(test)]
mod tests {
	use super::*;
	use client_core::{Command, Event, auth::SessionSecret, inbox::Source};
	use model::Id;
	use std::{sync::Arc, time::Duration};
	use tokio::{
		io::{AsyncReadExt, AsyncWriteExt},
		net::TcpListener,
	};
	#[tokio::test]
	async fn inbox_routes_bodies_and_cursors() {
		tokio::time::timeout(Duration::from_secs(10), async {
			let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
			let mut api = DiscordApi::new(Arc::new(
				SessionSecret::from_owner_input("SYNTHETIC_INBOX_TOKEN".into()).unwrap(),
			))
			.unwrap();
			api.base = format!("http://{}", listener.local_addr().unwrap());
			let msg = |id: u64| {
				format!(r#"{{"id":"{id}","channel_id":"1","author":{{"id":"7","username":"Synthetic"}},"content":"synthetic"}}"#)
			};
			let saved = |due: &str| {
				format!(
					r#"{{"message":{},"save_data":{{"channel_id":"1","message_id":"9","saved_at":"2026-11-10T07:48:11Z","due_at":{due}}}}}"#,
					msg(9)
				)
			};
			let script: Vec<(&str, String, &str, String)> = vec![
				("GET", "/users/@me/mentions?limit=25&roles=true&everyone=true".into(), "200 OK", format!("[{}]", msg(9))),
				("GET", "/users/@me/mentions?limit=25&roles=true&everyone=true&before=9".into(), "200 OK", "[]".into()),
				("GET", "/users/@me/saved-messages/bookmarks?limit=25".into(), "200 OK", format!(r#"{{"results":[{}],"has_more":true}}"#, saved("null"))),
				("GET", "/users/@me/saved-messages/bookmarks?limit=25&before=%32%30%32%36%2D%31%31%2D%31%30%54%30%37%3A%34%38%3A%31%31%5A".into(), "200 OK", r#"{"results":[],"has_more":false}"#.into()),
				("GET", "/users/@me/saved-messages?bookmark_ids=true".into(), "200 OK", format!(r#"{{"reminders":[{}],"bookmark_ids":["9"]}}"#, saved(r#""2026-11-30T14:30:00+00:00""#))),
				("GET", "/channels/1/messages?limit=30&around=8".into(), "200 OK", format!("[{},{}]", msg(9), msg(8))),
				("GET", "/channels/1/messages?limit=30".into(), "403 Forbidden", r#"{"code":50013}"#.into()),
				("PUT", "/users/@me/saved-messages/1/9".into(), "200 OK", "{}".into()),
				("PUT", "/users/@me/saved-messages/1/9".into(), "200 OK", "{}".into()),
				("DELETE", "/users/@me/saved-messages/1/9".into(), "204 No Content", String::new()),
				("DELETE", "/users/@me/mentions/9".into(), "204 No Content", String::new()),
			];
			let bodies = [
				None,
				None,
				None,
				None,
				None,
				None,
				None,
				Some(r#"{"due_at":"2026-11-25T07:22:53.652Z","source":"context_menu"}"#),
				Some(r#"{"source":"context_menu"}"#),
				None,
				None,
			];
			let server = tokio::spawn(async move {
				for ((method, route, status, body), expected) in script.into_iter().zip(bodies) {
					let (mut socket, _) = listener.accept().await.unwrap();
					let mut request = Vec::new();
					loop {
						let mut buffer = [0; 1024];
						let n = socket.read(&mut buffer).await.unwrap();
						assert!(n > 0);
						request.extend_from_slice(&buffer[..n]);
						assert!(request.len() < 8192);
						if request.windows(4).any(|w| w == b"\r\n\r\n")
							&& expected.is_none_or(|e| request.ends_with(e.as_bytes()))
						{
							break;
						}
					}
					let text = std::str::from_utf8(&request).unwrap();
					assert!(text.starts_with(&format!("{method} {route} HTTP/1.1\r\n")), "{text}");
					assert!(text.contains("SYNTHETIC_INBOX_TOKEN"));
					socket
						.write_all(
							format!(
								"HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
								body.len()
							)
							.as_bytes(),
						)
						.await
						.unwrap();
				}
			});
			let Event::Inbox {
				request: 3,
				result: Ok(Outcome::Page(page)),
				..
			} = api
				.execute(Command::Inbox {
					request: 3,
					read: Read::Mentions { before: None },
				})
				.await
			else {
				panic!()
			};
			assert_eq!(page.items[0].hit.id, Id(9));
			let Outcome::Page(empty) = api
				.inbox(Read::Mentions { before: Some(Id(9)) })
				.await
				.unwrap()
			else {
				panic!()
			};
			assert!(empty.items.is_empty() && !empty.more);
			let Outcome::Page(page) = api.inbox(Read::Bookmarks { before: None }).await.unwrap() else {
				panic!()
			};
			assert!(page.more);
			let Outcome::Page(empty) = api
				.inbox(Read::Bookmarks {
					before: page.items[0].saved_at,
				})
				.await
				.unwrap()
			else {
				panic!()
			};
			assert!(empty.items.is_empty() && !empty.more);
			let Outcome::Page(reminders) = api.inbox(Read::Reminders).await.unwrap() else {
				panic!()
			};
			assert!(reminders.items[0].due_at.is_some());
			let Outcome::Unread(unread) = api
				.inbox(Read::Unread {
					channel: Id(1),
					around: Some(Id(8)),
				})
				.await
				.unwrap()
			else {
				panic!()
			};
			assert_eq!(unread.hits.iter().map(|h| h.id).collect::<Vec<_>>(), [Id(9)]);
			assert!(matches!(
				api.inbox(Read::Unread {
					channel: Id(1),
					around: None
				})
				.await,
				Err(Failure::Forbidden)
			));
			for due_at in [Some(1_795_591_373_652_123_456), None] {
				let Event::InboxWritten { result: Ok(()), .. } = api
					.execute(Command::InboxWrite {
						request: 4,
						write: Write::Save {
							channel: Id(1),
							message: Id(9),
							due_at,
							source: Source::ContextMenu,
						},
					})
					.await
				else {
					panic!()
				};
			}
			api.inbox_write(Write::Unsave {
				channel: Id(1),
				message: Id(9),
			})
			.await
			.unwrap();
			api.inbox_write(Write::AckMention { message: Id(9) })
				.await
				.unwrap();
			server.await.unwrap();
		})
		.await
		.unwrap();
	}
}
