//! Runs one explicit Globe translation or API-key change off the UI thread.
use discord_api::translate::Error;
use eframe::egui;
use model::translation::TranslationEngine;
use std::sync::mpsc;
use ui::translation::KeyRequest;

pub type Answer = mpsc::Receiver<Result<String, &'static str>>;
pub type KeyAnswer = mpsc::Receiver<Result<&'static str, &'static str>>;

#[allow(clippy::too_many_arguments)]
pub fn spawn(
	runtime: &tokio::runtime::Handle,
	ctx: &egui::Context,
	demo: bool,
	text: String,
	target: &'static str,
	engine: TranslationEngine,
	endpoint: String,
	model: String,
) -> Answer {
	let (send, answer) = mpsc::sync_channel(1);
	let ctx = ctx.clone();
	runtime.spawn(async move {
		let result = if demo {
			Ok(format!("[{target}] {text}"))
		} else {
			translate(&text, target, engine, &endpoint, &model).await
		};
		let _ = send.send(result);
		ctx.request_repaint();
	});
	answer
}

async fn translate(
	text: &str,
	target: &'static str,
	engine: TranslationEngine,
	endpoint: &str,
	model: &str,
) -> Result<String, &'static str> {
	let result = match engine {
		TranslationEngine::Google => discord_api::translate::google(text, target).await,
		TranslationEngine::OpenAiCompatible => {
			let key = match tokio::task::spawn_blocking(platform::translation_key::load).await {
				Ok(Ok(Some(key))) => key,
				Ok(Ok(None)) => return Err("translation-error-missing-key"),
				Ok(Err(_)) | Err(_) => return Err("translation-key-failed"),
			};
			let name = model::translation::language(target).map_or(target, |(_, name)| name);
			discord_api::translate::openai(text, target, name, endpoint, model, &key).await
		}
	};
	result.map_err(|error| match error {
		Error::TooLong => "translation-error-too-long",
		Error::InvalidEndpoint => "translation-error-endpoint",
		Error::MissingKey => "translation-error-missing-key",
		Error::Network => "translation-error-network",
		Error::RateLimited => "translation-error-rate-limited",
		Error::Unauthorized => "translation-error-unauthorized",
		Error::Status | Error::InvalidResponse | Error::ResponseLimit | Error::Empty => {
			"translation-error-response"
		}
	})
}

pub fn spawn_key(
	runtime: &tokio::runtime::Handle,
	ctx: &egui::Context,
	demo: bool,
	request: KeyRequest,
) -> KeyAnswer {
	let (send, answer) = mpsc::sync_channel(1);
	let ctx = ctx.clone();
	runtime.spawn_blocking(move || {
		let result = match request {
			KeyRequest::Save(_) if demo => Ok("translation-key-saved"),
			KeyRequest::Forget if demo => Ok("translation-key-removed"),
			KeyRequest::Save(key) => platform::translation_key::save(&key)
				.map(|()| "translation-key-saved")
				.map_err(|_| "translation-key-failed"),
			KeyRequest::Forget => platform::translation_key::forget()
				.map(|()| "translation-key-removed")
				.map_err(|_| "translation-key-failed"),
		};
		let _ = send.send(result);
		ctx.request_repaint();
	});
	answer
}
