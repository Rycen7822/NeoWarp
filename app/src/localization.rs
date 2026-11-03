//! Software UI text only. User content, protocol values and configuration identifiers stay unchanged.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU8, Ordering};

mod zh_cn;

#[derive(
    Default,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Language {
    #[default]
    En,
    ZhCn,
}

impl Language {
    pub fn label(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::ZhCn => "简体中文",
        }
    }

    pub fn translate(self, english: &str) -> &str {
        match self {
            Self::En => english,
            Self::ZhCn => zh_cn::lookup(english).unwrap_or(english),
        }
    }
}

static LANGUAGE: AtomicU8 = AtomicU8::new(Language::En as u8);

/// Applies the active language without rebuilding application state.
pub fn initialize(language: Language) {
    LANGUAGE.store(language as u8, Ordering::Relaxed);
}

fn current() -> Language {
    match LANGUAGE.load(Ordering::Relaxed) {
        1 => Language::ZhCn,
        _ => Language::En,
    }
}

pub fn tr(english: &str) -> &str {
    current().translate(english)
}

/// Retains ownership on an untranslated dynamic label, without allocating on each render.
pub fn localize(english: impl Into<Cow<'static, str>>) -> Cow<'static, str> {
    let english = english.into();
    if current() == Language::ZhCn
        && let Some(chinese) = zh_cn::lookup(&english)
    {
        return Cow::Borrowed(chinese);
    }
    english
}

#[cfg(test)]
#[path = "localization_tests.rs"]
mod tests;
