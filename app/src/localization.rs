//! Software UI text only. User content, protocol values and configuration identifiers stay unchanged.

use std::borrow::Cow;
use std::sync::OnceLock;

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

static LANGUAGE: OnceLock<Language> = OnceLock::new();

/// Called after settings load, before constructing UI. Changes apply on the next launch.
pub fn initialize(language: Language) {
    LANGUAGE.get_or_init(|| language);
}

pub fn tr(english: &str) -> &str {
    LANGUAGE
        .get()
        .copied()
        .unwrap_or_default()
        .translate(english)
}

/// Retains ownership on an untranslated dynamic label, without allocating on each render.
pub fn localize(english: impl Into<Cow<'static, str>>) -> Cow<'static, str> {
    let english = english.into();
    if LANGUAGE.get() == Some(&Language::ZhCn)
        && let Some(chinese) = zh_cn::lookup(&english)
    {
        return Cow::Borrowed(chinese);
    }
    english
}

#[cfg(test)]
#[path = "localization_tests.rs"]
mod tests;
