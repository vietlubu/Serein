//! The translation API key lives only in the OS credential store; no plaintext fallback.
use crate::{ACCOUNT, CredentialError, entry, forget_entry};
use zeroize::Zeroizing;

fn name() -> String {
	format!("{ACCOUNT}.translation-api-key")
}
fn validate(key: &str) -> Result<(), CredentialError> {
	if key.is_empty() || key.len() > 4096 || key.chars().any(char::is_control) {
		return Err(CredentialError::Invalid);
	}
	Ok(())
}
pub fn load() -> Result<Option<Zeroizing<String>>, CredentialError> {
	let key = match entry(&name())?.get_password() {
		Ok(value) => Zeroizing::new(value),
		Err(keyring_core::Error::NoEntry) => return Ok(None),
		Err(_) => return Err(CredentialError::Unavailable),
	};
	validate(&key)?;
	Ok(Some(key))
}
pub fn save(key: &str) -> Result<(), CredentialError> {
	validate(key)?;
	entry(&name())?
		.set_password(key)
		.map_err(|_| CredentialError::Unavailable)
}
pub fn forget() -> Result<(), CredentialError> {
	forget_entry(&name())
}
