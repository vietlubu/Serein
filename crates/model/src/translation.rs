//! Device-local message translation settings; independent of Discord accounts.

/// Service that receives a message's text after an explicit Globe click.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TranslationEngine {
	#[default]
	Google,
	OpenAiCompatible,
}
impl TranslationEngine {
	pub const ALL: [Self; 2] = [Self::Google, Self::OpenAiCompatible];
	/// Stable storage key; unknown keys from a newer build fall back to [`Self::Google`].
	fn key(self) -> &'static str {
		match self {
			Self::Google => "google",
			Self::OpenAiCompatible => "openai-compatible",
		}
	}
}
impl serde::Serialize for TranslationEngine {
	fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.serialize_str(self.key())
	}
}
impl<'de> serde::Deserialize<'de> for TranslationEngine {
	fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let value = <String as serde::Deserialize>::deserialize(deserializer)?;
		Ok(Self::ALL
			.into_iter()
			.find(|candidate| candidate.key() == value)
			.unwrap_or_default())
	}
}

/// Target languages as (Google code, autonym); `iw` and `jw` are Google's legacy codes.
pub const LANGUAGES: &[(&str, &str)] = &[
	("af", "Afrikaans"),
	("am", "አማርኛ"),
	("ar", "العربية"),
	("az", "Azərbaycan"),
	("be", "Беларуская"),
	("bg", "Български"),
	("bn", "বাংলা"),
	("bs", "Bosanski"),
	("ca", "Català"),
	("ceb", "Cebuano"),
	("co", "Corsu"),
	("cs", "Čeština"),
	("cy", "Cymraeg"),
	("da", "Dansk"),
	("de", "Deutsch"),
	("el", "Ελληνικά"),
	("en", "English"),
	("eo", "Esperanto"),
	("es", "Español"),
	("et", "Eesti"),
	("eu", "Euskara"),
	("fa", "فارسی"),
	("fi", "Suomi"),
	("fr", "Français"),
	("fy", "Frysk"),
	("ga", "Gaeilge"),
	("gd", "Gàidhlig"),
	("gl", "Galego"),
	("gu", "ગુજરાતી"),
	("ha", "Hausa"),
	("haw", "ʻŌlelo Hawaiʻi"),
	("hi", "हिन्दी"),
	("hmn", "Hmoob"),
	("hr", "Hrvatski"),
	("ht", "Kreyòl ayisyen"),
	("hu", "Magyar"),
	("hy", "Հայերեն"),
	("id", "Bahasa Indonesia"),
	("ig", "Igbo"),
	("is", "Íslenska"),
	("it", "Italiano"),
	("iw", "עברית"),
	("ja", "日本語"),
	("jw", "Basa Jawa"),
	("ka", "ქართული"),
	("kk", "Қазақ"),
	("km", "ខ្មែរ"),
	("kn", "ಕನ್ನಡ"),
	("ko", "한국어"),
	("ku", "Kurdî"),
	("ky", "Кыргызча"),
	("la", "Latina"),
	("lb", "Lëtzebuergesch"),
	("lo", "ລາວ"),
	("lt", "Lietuvių"),
	("lv", "Latviešu"),
	("mg", "Malagasy"),
	("mi", "Māori"),
	("mk", "Македонски"),
	("ml", "മലയാളം"),
	("mn", "Монгол"),
	("mr", "मराठी"),
	("ms", "Bahasa Melayu"),
	("mt", "Malti"),
	("my", "မြန်မာ"),
	("ne", "नेपाली"),
	("nl", "Nederlands"),
	("no", "Norsk"),
	("ny", "Chichewa"),
	("or", "ଓଡ଼ିଆ"),
	("pa", "ਪੰਜਾਬੀ"),
	("pl", "Polski"),
	("ps", "پښتو"),
	("pt", "Português"),
	("ro", "Română"),
	("ru", "Русский"),
	("rw", "Kinyarwanda"),
	("sd", "سنڌي"),
	("si", "සිංහල"),
	("sk", "Slovenčina"),
	("sl", "Slovenščina"),
	("sm", "Gagana Samoa"),
	("sn", "chiShona"),
	("so", "Soomaali"),
	("sq", "Shqip"),
	("sr", "Српски"),
	("st", "Sesotho"),
	("su", "Basa Sunda"),
	("sv", "Svenska"),
	("sw", "Kiswahili"),
	("ta", "தமிழ்"),
	("te", "తెలుగు"),
	("tg", "Тоҷикӣ"),
	("th", "ไทย"),
	("tk", "Türkmen"),
	("tl", "Tagalog"),
	("tr", "Türkçe"),
	("tt", "Татар"),
	("ug", "ئۇيغۇرچە"),
	("uk", "Українська"),
	("ur", "اردو"),
	("uz", "Oʻzbek"),
	("vi", "Tiếng Việt"),
	("xh", "isiXhosa"),
	("yi", "ייִדיש"),
	("yo", "Yorùbá"),
	("zh-CN", "简体中文"),
	("zh-TW", "繁體中文"),
	("zu", "isiZulu"),
];

/// Exact lookup of a target code in [`LANGUAGES`].
pub fn language(code: &str) -> Option<(&'static str, &'static str)> {
	LANGUAGES
		.iter()
		.copied()
		.find(|(candidate, _)| *candidate == code)
}

/// Target code for an interface locale tag: exact match, then its primary subtag, else English.
pub fn target_for_locale(tag: &str) -> &'static str {
	language(tag)
		.or_else(|| language(tag.split('-').next().unwrap_or_default()))
		.map_or("en", |(code, _)| code)
}
