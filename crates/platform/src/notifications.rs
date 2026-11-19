//! Device-preference-controlled OS alerts. Message previews are bounded before reaching this worker.
use std::sync::{
	Arc,
	atomic::{AtomicU64, Ordering},
	mpsc::{self, Receiver, SyncSender},
};

// Windows delivers activation through its callback; other platforms retain a cancellable future per alert.
#[cfg(target_os = "windows")]
type NotificationHandle = ();
#[cfg(not(target_os = "windows"))]
struct NotificationHandle {
	response: std::pin::Pin<Box<dyn Future<Output = ()>>>,
	close: Box<dyn FnOnce()>,
}

/// A delivered alert; `key` is `(channel, message)` for channel alerts.
struct Shown {
	key: Option<(model::Id, model::Id)>,
	handle: NotificationHandle,
}

#[derive(Clone)]
struct Activation {
	send: SyncSender<(u64, model::Id)>,
	generation: u64,
	channel: Option<model::Id>,
	wake: Arc<dyn Fn() + Send + Sync>,
	restore: Arc<dyn Fn() + Send + Sync>,
}
impl Activation {
	fn clicked(&self) {
		if let Some(channel) = self.channel
			&& self.send.try_send((self.generation, channel)).is_ok()
		{
			(self.restore)();
			(self.wake)();
		}
	}
}

// Eight bounded commands; overflow drops an alert, never message state.
const QUEUE_ITEMS: usize = 8;
// Each tracked macOS banner costs one delivered-notifications poll every 500 ms.
const MAX_SHOWN: usize = 32;
const GENERIC_BODY: &str = "You have a new message.";
const TITLE_BYTES: usize = 256;
const BODY_BYTES: usize = 512;
const IMAGE_PATH_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
	Disabled,
	Enabling,
	Ready,
	Denied,
	Unavailable,
	QueueFull,
}

impl Status {
	pub fn label(self) -> &'static str {
		match self {
			Self::Disabled => "System notifications are off in Serein settings.",
			Self::Enabling => "Checking system notification permission…",
			Self::Ready => "",
			Self::Denied => "System notifications are disabled in your OS settings.",
			Self::QueueFull => {
				"Notification queue full; an alert was skipped. Unread indicators are retained."
			}
			Self::Unavailable => {
				#[cfg(target_os = "macos")]
				{
					"System notifications unavailable. Run the packaged Serein.app and check System Settings > Notifications."
				}
				#[cfg(target_os = "windows")]
				{
					"System notifications unavailable. Register the packaged Start Menu shortcut with install-notifications.ps1 and check Windows notification settings."
				}
				#[cfg(not(any(target_os = "macos", target_os = "windows")))]
				{
					"System notifications unavailable. Check your desktop notification service and settings."
				}
			}
		}
	}
}

struct Alert {
	title: Box<str>,
	body: Box<str>,
	image_path: Option<Box<str>>,
}

struct Command {
	generation: u64,
	key: Option<(model::Id, model::Id)>,
	alert: Option<Alert>,
	close: Vec<(model::Id, model::Id)>,
}

/// Created without OS calls or a thread. The worker starts only when the device setting is enabled.
pub struct Notifications {
	activated: Receiver<(u64, model::Id)>,
	activation_send: SyncSender<(u64, model::Id)>,
	send: Option<SyncSender<Command>>,
	generation: Arc<AtomicU64>,
	status: Arc<AtomicU64>,
	wake: Arc<dyn Fn() + Send + Sync>,
	restore: Arc<dyn Fn() + Send + Sync>,
	/// Delivered channel alerts, oldest first, mirrored for read-close.
	shown: Vec<(model::Id, model::Id)>,
	/// Generation at which notifications were last enabled.
	session: Arc<AtomicU64>,
}

impl Notifications {
	/// `restore` runs on the notification thread when a click needs the window shown; a hidden
	/// window may receive no frames until then.
	pub fn new(
		wake: impl Fn() + Send + Sync + 'static,
		restore: impl Fn() + Send + Sync + 'static,
	) -> Self {
		let (activation_send, activated) = mpsc::sync_channel(QUEUE_ITEMS);
		Self {
			activated,
			activation_send,
			send: None,
			generation: Arc::new(AtomicU64::new(0)),
			status: Arc::new(AtomicU64::new(0)),
			wake: Arc::new(wake),
			restore: Arc::new(restore),
			shown: Vec::new(),
			session: Arc::new(AtomicU64::new(0)),
		}
	}

	/// Enable for this session only. macOS may ask for OS permission; never blocks rendering.
	pub fn set_enabled(&mut self, enabled: bool) {
		if (self.generation.load(Ordering::Acquire) & 1 != 0) == enabled {
			return;
		}
		let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
		if enabled {
			self.session.store(generation, Ordering::Release);
		} else {
			self.shown.clear();
		}
		self.status.store(
			encoded(
				generation,
				if enabled {
					Status::Enabling
				} else {
					Status::Disabled
				},
			),
			Ordering::Release,
		);
		if enabled && self.send.is_none() {
			let (send, receive) = mpsc::sync_channel(QUEUE_ITEMS);
			let current = Arc::clone(&self.generation);
			let status = Arc::clone(&self.status);
			let wake = Arc::clone(&self.wake);
			let restore = Arc::clone(&self.restore);
			let activated = self.activation_send.clone();
			let session = Arc::clone(&self.session);
			if std::thread::Builder::new()
				.name("serein-notifications".into())
				.spawn(move || worker(receive, current, session, status, wake, restore, activated))
				.is_err()
			{
				self.status
					.store(encoded(generation, Status::Unavailable), Ordering::Release);
				return;
			}
			self.send = Some(send);
		}
		// If full, the next queued command also observes the changed generation and clears.
		if let Some(send) = &self.send {
			let _ = send.try_send(Command {
				generation,
				key: None,
				close: Vec::new(),
				alert: None,
			});
		}
	}

	/// Cancel queued alerts and close the outstanding alert where supported. OS history may remain.
	pub fn clear(&mut self) {
		self.set_enabled(false);
	}

	/// Cancel queued, not-yet-shown alerts after mute, DND, access or connection changes.
	/// Delivered alerts stay until read, answered, or the session ends.
	/// Retains the session opt-in and does not request OS authorization again.
	pub fn dismiss(&mut self) {
		let result = match self.status() {
			Status::QueueFull => Status::Ready,
			result => result,
		};
		let generation = self.generation.fetch_add(2, Ordering::AcqRel) + 2;
		self.status
			.store(encoded(generation, result), Ordering::Release);
		if let Some(send) = &self.send {
			let _ = send.try_send(Command {
				generation,
				key: None,
				close: Vec::new(),
				alert: None,
			});
		}
	}

	/// Queue a privacy-preserving generic alert. False means disabled, unavailable or overloaded.
	pub fn notify(&mut self) -> bool {
		self.enqueue(Alert {
			title: "Serein".into(),
			body: GENERIC_BODY.into(),
			image_path: None,
		})
	}
	pub fn notify_message(
		&mut self,
		title: String,
		body: String,
		image_path: Option<String>,
	) -> bool {
		self.message(None, title, body, image_path)
	}
	pub fn notify_channel(
		&mut self,
		channel: model::Id,
		message: model::Id,
		title: String,
		body: String,
		image_path: Option<String>,
	) -> bool {
		self.message(Some((channel, message)), title, body, image_path)
	}
	/// Close delivered channel alerts whose message `read(channel, message)` reports read.
	pub fn close_read(&mut self, read: impl Fn(model::Id, model::Id) -> bool) {
		let Some(send) = &self.send else { return };
		let close: Vec<_> = self
			.shown
			.extract_if(.., |&mut (channel, message)| read(channel, message))
			.collect();
		if close.is_empty() {
			return;
		}
		// A full queue keeps the keys so the next read change retries.
		if let Err(mpsc::TrySendError::Full(command)) = send.try_send(Command {
			generation: self.generation.load(Ordering::Acquire),
			key: None,
			alert: None,
			close,
		}) {
			self.shown.extend(command.close);
		}
	}
	/// Clicks survive dismissals but not a disable/re-enable.
	pub fn take_activation(&self) -> Option<model::Id> {
		let current = self.generation.load(Ordering::Acquire);
		let session = self.session.load(Ordering::Acquire);
		self.activated.try_iter().find_map(|(generation, channel)| {
			(current & 1 != 0 && generation >= session).then_some(channel)
		})
	}
	fn message(
		&mut self,
		key: Option<(model::Id, model::Id)>,
		title: String,
		body: String,
		image_path: Option<String>,
	) -> bool {
		if title.len() > TITLE_BYTES
			|| body.len() > BODY_BYTES
			|| image_path
				.as_ref()
				.is_some_and(|path| path.len() > IMAGE_PATH_BYTES)
		{
			return false;
		}
		self.enqueue_channel(
			Alert {
				title: title.into_boxed_str(),
				body: body.into_boxed_str(),
				image_path: image_path.map(String::into_boxed_str),
			},
			key,
		)
	}
	fn enqueue(&mut self, alert: Alert) -> bool {
		self.enqueue_channel(alert, None)
	}
	fn enqueue_channel(&mut self, alert: Alert, key: Option<(model::Id, model::Id)>) -> bool {
		if !matches!(self.status(), Status::Ready | Status::QueueFull) {
			return false;
		}
		let generation = self.generation.load(Ordering::Acquire);
		let Some(send) = &self.send else { return false };
		match send.try_send(Command {
			generation,
			key,
			alert: Some(alert),
			close: Vec::new(),
		}) {
			Ok(()) => {
				if let Some(key) = key {
					if self.shown.len() >= MAX_SHOWN {
						self.shown.remove(0);
					}
					self.shown.push(key);
				}
				true
			}
			Err(error) => {
				let status = match error {
					mpsc::TrySendError::Full(_) => Status::QueueFull,
					mpsc::TrySendError::Disconnected(_) => Status::Unavailable,
				};
				self.status
					.store(encoded(generation, status), Ordering::Release);
				false
			}
		}
	}

	pub fn status(&self) -> Status {
		let generation = self.generation.load(Ordering::Acquire);
		let value = self.status.load(Ordering::Acquire);
		if value >> 8 != generation {
			return if generation & 1 == 0 {
				Status::Disabled
			} else {
				Status::Enabling
			};
		}
		match value & 255 {
			0 => Status::Disabled,
			1 => Status::Enabling,
			2 => Status::Ready,
			3 => Status::Denied,
			5 => Status::QueueFull,
			_ => Status::Unavailable,
		}
	}
}

impl Drop for Notifications {
	fn drop(&mut self) {
		self.clear();
		// Dropping the sole sender wakes the worker; never join an OS call on the UI thread.
	}
}

fn encoded(generation: u64, status: Status) -> u64 {
	(generation << 8) | status as u64
}

fn worker(
	receive: Receiver<Command>,
	current: Arc<AtomicU64>,
	session: Arc<AtomicU64>,
	status: Arc<AtomicU64>,
	wake: Arc<dyn Fn() + Send + Sync>,
	restore: Arc<dyn Fn() + Send + Sync>,
	activated: SyncSender<(u64, model::Id)>,
) {
	let mut generation = 0;
	let mut started = 0;
	let mut outcome = Status::Disabled;
	let mut shown: Vec<Shown> = Vec::new();
	loop {
		// A resolved response means the OS alert was answered or dismissed; it is already gone.
		#[cfg(not(target_os = "windows"))]
		let waiting = {
			let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
			shown.retain_mut(|entry| entry.handle.response.as_mut().poll(&mut cx).is_pending());
			!shown.is_empty()
		};
		#[cfg(target_os = "windows")]
		let waiting = false;
		let command = if waiting {
			match receive.recv_timeout(std::time::Duration::from_millis(100)) {
				Ok(command) => command,
				Err(mpsc::RecvTimeoutError::Timeout) => continue,
				Err(mpsc::RecvTimeoutError::Disconnected) => break,
			}
		} else {
			let Ok(command) = receive.recv() else { break };
			command
		};
		let active = current.load(Ordering::Acquire);
		if active != generation {
			let session_start = session.load(Ordering::Acquire);
			// Dismissals only cancel queued alerts; delivered ones stay until read, answered or the session ends.
			if active & 1 == 0 || session_start != started {
				close_all(&mut shown);
				started = session_start;
			}
			let was_enabled = generation & 1 != 0;
			generation = active;
			outcome = if generation & 1 == 0 {
				Status::Disabled
			} else if !was_enabled
				|| status.load(Ordering::Acquire) == encoded(generation, Status::Enabling)
			{
				authorize()
			} else {
				outcome
			};
			publish(&status, generation, outcome);
			wake();
		}
		// Reads close alerts across dismissals, so this ignores the command generation.
		for entry in shown.extract_if(.., |entry| {
			entry.key.is_some_and(|key| command.close.contains(&key))
		}) {
			close(entry);
		}
		if outcome != Status::Ready
			|| command.alert.is_none()
			|| command.generation != generation
			|| current.load(Ordering::Acquire) != generation
		{
			continue;
		}
		let activation = Activation {
			send: activated.clone(),
			generation,
			channel: command.key.map(|(channel, _)| channel),
			wake: Arc::clone(&wake),
			restore: Arc::clone(&restore),
		};
		outcome = match show(command.alert.as_ref().expect("checked above"), activation) {
			Ok(handle) => {
				if shown.len() >= MAX_SHOWN {
					// shortcut: the oldest alert stays visible but loses click routing and read-close; raise MAX_SHOWN if users keep more alerts.
					shown.remove(0);
				}
				shown.push(Shown {
					key: command.key,
					handle,
				});
				Status::Ready
			}
			Err(()) => Status::Unavailable,
		};
		if current.load(Ordering::Acquire) & 1 == 0 || session.load(Ordering::Acquire) != started {
			close_all(&mut shown);
		}
		publish(&status, generation, outcome);
		wake();
	}
	close_all(&mut shown);
}

fn publish(status: &AtomicU64, generation: u64, outcome: Status) {
	// An OS result must not overwrite the next session/dismissal's state.
	let _ = status.try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
		(current >> 8 == generation).then_some(encoded(generation, outcome))
	});
}

fn authorize() -> Status {
	#[cfg(target_os = "macos")]
	{
		if notify_rust::check_bundle().is_err() {
			return Status::Unavailable;
		}
		request_badge_authorization();
		match notify_rust::request_auth_blocking() {
			Ok(true) => Status::Ready,
			Ok(false) => Status::Denied,
			Err(_) => Status::Unavailable,
		}
	}
	#[cfg(target_os = "windows")]
	{
		use windows::{UI::Notifications::ToastNotificationManager, core::HSTRING};
		let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
			"cz.viceverse.serein",
		));
		match notifier {
			Ok(notifier) => windows_setting_status(notifier.Setting(), windows_shortcut_exists()),
			Err(_) => Status::Unavailable,
		}
	}
	#[cfg(not(any(target_os = "macos", target_os = "windows")))]
	{
		Status::Ready
	}
}

/// mac-usernotifications 0.3.1 requests only alerts and sounds, which leaves the Dock unread
/// badge without a Notifications setting. Asking for all three first keeps a single prompt;
/// the crate's request afterwards installs its delegate and reports the decided status.
#[cfg(target_os = "macos")]
fn request_badge_authorization() {
	use objc2_user_notifications::{UNAuthorizationOptions, UNUserNotificationCenter};
	let (send, decided) = std::sync::mpsc::sync_channel(1);
	UNUserNotificationCenter::currentNotificationCenter()
		.requestAuthorizationWithOptions_completionHandler(
			UNAuthorizationOptions::Alert
				| UNAuthorizationOptions::Sound
				| UNAuthorizationOptions::Badge,
			&block2::RcBlock::new(
				move |_: objc2::runtime::Bool, _: *mut objc2_foundation::NSError| {
					let _ = send.try_send(());
				},
			),
		);
	let _ = decided.recv();
}

#[cfg(target_os = "windows")]
fn windows_setting_status(
	setting: windows::core::Result<windows::UI::Notifications::NotificationSetting>,
	has_shortcut: bool,
) -> Status {
	use windows::{UI::Notifications::NotificationSetting, core::HRESULT};
	match setting {
		Ok(NotificationSetting::Enabled) => Status::Ready,
		Ok(_) => Status::Denied,
		// Windows may not have a settings entry for an unpackaged desktop app yet.
		// Its notifier can still submit a toast; show() reports delivery errors.
		Err(error) if has_shortcut && error.code() == HRESULT(0x80070490_u32 as i32) => {
			Status::Ready
		}
		Err(_) => Status::Unavailable,
	}
}

#[cfg(target_os = "windows")]
fn windows_shortcut_exists() -> bool {
	std::env::var_os("APPDATA").is_some_and(|root| {
		std::path::PathBuf::from(root)
			.join("Microsoft/Windows/Start Menu/Programs/Serein.lnk")
			.is_file()
	})
}

fn show(alert: &Alert, activation: Activation) -> Result<NotificationHandle, ()> {
	#[cfg(target_os = "macos")]
	{
		// The blocking wrapper mistakes a busy AppKit run loop for a stopped one.
		// Await the OS completion on this worker; never block the native UI thread.
		let native = mac_usernotifications::Notification::from(&notification(alert));
		let handle = futures_lite::future::block_on(native.send()).map_err(|_| ())?;
		let id = handle.notification_id().to_owned();
		Ok(NotificationHandle {
			response: Box::pin(async move {
				if handle
					.response()
					.await
					.is_ok_and(|response| response.is_default_action())
				{
					activation.clicked();
				}
			}),
			close: Box::new(move || {
				futures_lite::future::block_on(mac_usernotifications::close_delivered(&id))
			}),
		})
	}
	#[cfg(not(any(target_os = "macos", target_os = "windows")))]
	{
		let handle = Arc::new(notification(alert).show().map_err(|_| ())?);
		let response_handle = Arc::clone(&handle);
		Ok(NotificationHandle {
			response: Box::pin(async move {
				response_handle
					.wait_for_action_async(|response| {
						if response.is_default_action() {
							activation.clicked();
						}
					})
					.await;
			}),
			close: Box::new(move || {
				if let Ok(handle) = Arc::try_unwrap(handle) {
					handle.close();
				}
			}),
		})
	}
	#[cfg(target_os = "windows")]
	{
		use tauri_winrt_notification::{IconCrop, Toast};
		let mut toast = Toast::new("cz.viceverse.serein")
			.title(&alert.title)
			.text1(&alert.body)
			.on_activated(move |_| {
				activation.clicked();
				Ok(())
			})
			.sound(None);
		if let Some(path) = &alert.image_path
			&& std::path::Path::new(path.as_ref()).is_file()
		{
			toast = toast.icon(
				std::path::Path::new(path.as_ref()),
				IconCrop::Circular,
				&alert.title,
			);
		}
		toast.show().map_err(|_| ())
	}
}

#[cfg(any(not(target_os = "windows"), test))]
fn notification(alert: &Alert) -> notify_rust::Notification {
	let mut notification = notify_rust::Notification::new();
	notification
		.appname("Serein")
		.summary(&alert.title)
		.body(&alert.body);
	// macOS has no native TTL: the crate's timer would remove the notification and override
	// the user's Banner/Alert style.
	#[cfg(not(target_os = "macos"))]
	notification.timeout(5_000);
	#[cfg(not(any(target_os = "macos", target_os = "windows")))]
	notification.action("default", "Open channel");
	if let Some(path) = &alert.image_path
		&& std::path::Path::new(path.as_ref()).is_file()
	{
		notification.image_path(path);
	}
	// Sound is played independently by the bounded local audio worker.
	#[cfg(target_os = "linux")]
	notification.hint(notify_rust::Hint::SuppressSound(true));
	notification
}

#[cfg(not(target_os = "windows"))]
fn close(Shown { handle, .. }: Shown) {
	// Dropping the response first releases Linux's shared handle for `close`.
	drop(handle.response);
	(handle.close)();
}

// Windows exposes no per-toast handle here, so closing one alert clears Serein's toast history.
#[cfg(target_os = "windows")]
fn close(Shown { handle: (), .. }: Shown) {
	use windows::{UI::Notifications::ToastNotificationManager, core::HSTRING};
	let _ = ToastNotificationManager::History()
		.and_then(|history| history.ClearWithId(&HSTRING::from("cz.viceverse.serein")));
}

fn close_all(shown: &mut Vec<Shown>) {
	for entry in shown.drain(..) {
		close(entry);
	}
}

/// Synthetic handoff check: never sends an OS notification or starts a worker.
#[cfg(debug_assertions)]
pub fn debug_activation_check(channel: model::Id) -> model::Id {
	let mut notifications = Notifications::new(|| {}, || {});
	notifications.generation.store(1, Ordering::Release);
	let click = Activation {
		send: notifications.activation_send.clone(),
		generation: 1,
		channel: Some(channel),
		wake: Arc::clone(&notifications.wake),
		restore: Arc::clone(&notifications.restore),
	};
	click.clicked();
	let selected = notifications
		.take_activation()
		.expect("click retains its channel");
	assert_eq!(selected, channel);
	click.clicked();
	notifications.dismiss();
	assert_eq!(notifications.take_activation(), Some(channel));
	click.clicked();
	notifications.clear();
	assert_eq!(notifications.take_activation(), None);
	selected
}

#[cfg(test)]
mod tests {
	use super::*;
	#[cfg(target_os = "windows")]
	#[test]
	fn missing_windows_settings_entry_allows_delivery_but_denials_do_not() {
		use windows::{
			UI::Notifications::NotificationSetting,
			core::{Error, HRESULT},
		};
		assert_eq!(
			windows_setting_status(
				Err(Error::from_hresult(HRESULT(0x80070490_u32 as i32))),
				true
			),
			Status::Ready
		);
		assert_eq!(
			windows_setting_status(
				Err(Error::from_hresult(HRESULT(0x80070490_u32 as i32))),
				false
			),
			Status::Unavailable
		);
		assert_eq!(
			windows_setting_status(Ok(NotificationSetting::DisabledForUser), true),
			Status::Denied
		);
		assert_eq!(
			windows_setting_status(
				Err(Error::from_hresult(HRESULT(0x80004005_u32 as i32))),
				true
			),
			Status::Unavailable
		);
	}

	#[test]
	fn disabled_is_lazy_and_fixed_queue_is_bounded_and_invalidated() {
		let alert = notification(&Alert {
			title: "Serein".into(),
			body: GENERIC_BODY.into(),
			image_path: None,
		});
		assert_eq!(alert.summary, "Serein");
		assert_eq!(alert.body, "You have a new message.");
		let mut notifications = Notifications::new(|| {}, || {});
		assert_eq!(notifications.status(), Status::Disabled);
		assert!(notifications.send.is_none());
		assert!(!notifications.notify());
		// Substitute only the queue; tests never authorize or contact an OS notification service.
		let (send, receive) = mpsc::sync_channel(QUEUE_ITEMS);
		notifications.send = Some(send);
		notifications.generation.store(1, Ordering::Release);
		notifications
			.status
			.store(encoded(1, Status::Ready), Ordering::Release);
		for _ in 0..QUEUE_ITEMS {
			assert!(notifications.notify());
		}
		assert!(!notifications.notify());
		assert_eq!(notifications.status(), Status::QueueFull);
		notifications.dismiss();
		assert_eq!(notifications.status(), Status::Ready);
		for command in receive.try_iter().filter(|command| command.alert.is_some()) {
			assert_ne!(
				command.generation,
				notifications.generation.load(Ordering::Acquire)
			);
		}
		assert!(notifications.notify());
		notifications.clear();
		assert_eq!(notifications.status(), Status::Disabled);
		assert!(!notifications.notify());
		for command in receive.try_iter().filter(|command| command.alert.is_some()) {
			assert_ne!(
				command.generation,
				notifications.generation.load(Ordering::Acquire)
			);
		}
		assert!(
			(std::mem::size_of::<Command>()
				+ TITLE_BYTES
				+ BODY_BYTES + IMAGE_PATH_BYTES
				+ MAX_SHOWN * std::mem::size_of::<(model::Id, model::Id)>())
				* QUEUE_ITEMS
				<= 16 * 1024
		);
		let active_status = notifications.status.load(Ordering::Acquire);
		publish(&notifications.status, 1, Status::Unavailable);
		assert_eq!(notifications.status.load(Ordering::Acquire), active_status);
		assert_eq!(notifications.status(), Status::Disabled);

		{
			let mut notifications = Notifications::new(|| {}, || {});
			let (send, receive) = mpsc::sync_channel(QUEUE_ITEMS);
			notifications.send = Some(send);
			notifications.generation.store(1, Ordering::Release);
			notifications
				.status
				.store(encoded(1, Status::Ready), Ordering::Release);
			assert!(notifications.notify_message("A sender".into(), "Hello there".into(), None));
			let alert = receive.try_recv().unwrap().alert.unwrap();
			assert_eq!((&*alert.title, &*alert.body), ("A sender", "Hello there"));
			assert!(!notifications.notify_message(
				"x".repeat(TITLE_BYTES + 1),
				"Message".into(),
				None
			));
			assert!(receive.try_recv().is_err());

			// Dismissals keep delivered alerts; only their read message closes them, once.
			let id = model::Id;
			assert!(notifications.notify_channel(id(7), id(70), "A".into(), "B".into(), None));
			assert!(receive.try_recv().unwrap().alert.is_some());
			notifications.dismiss();
			assert!(receive.try_recv().unwrap().alert.is_none());
			notifications.close_read(|_, message| message > id(70));
			assert!(receive.try_recv().is_err());
			notifications.close_read(|channel, message| channel == id(7) && message <= id(70));
			let command = receive.try_recv().unwrap();
			assert_eq!(command.close, vec![(id(7), id(70))]);
			assert!(command.alert.is_none());
			notifications.close_read(|_, _| true);
			assert!(receive.try_recv().is_err());

			for message in 0..=MAX_SHOWN as u64 {
				assert!(notifications.notify_channel(
					id(8),
					id(message),
					"A".into(),
					"B".into(),
					None
				));
				receive.try_recv().unwrap();
			}
			assert_eq!(notifications.shown.len(), MAX_SHOWN);

			// Clicks survive dismissals but not a disable/re-enable.
			let generation = notifications.generation.load(Ordering::Acquire);
			notifications
				.activation_send
				.try_send((generation, id(7)))
				.unwrap();
			notifications.dismiss();
			assert_eq!(notifications.take_activation(), Some(id(7)));
			notifications.clear();
			assert!(notifications.shown.is_empty());
			notifications.set_enabled(true);
			notifications
				.activation_send
				.try_send((generation, id(7)))
				.unwrap();
			assert_eq!(notifications.take_activation(), None);
		}
	}
}
