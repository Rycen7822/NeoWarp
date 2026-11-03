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
mod tests {
    use settings_value::SettingsValue;

    use super::*;

    #[test]
    fn language_preserves_defaults_identifiers_and_untranslated_text() {
        assert_eq!(Language::default(), Language::En);
        assert_eq!(Language::En.translate("Appearance"), "Appearance");
        assert_eq!(Language::ZhCn.translate("Appearance"), "外观");
        assert_eq!(Language::ZhCn.translate("my-project.rs"), "my-project.rs");
        assert_eq!(Language::ZhCn.to_file_value(), serde_json::json!("zh_cn"));
        assert_eq!(
            Language::from_file_value(&serde_json::json!("zh_cn")).unwrap(),
            Language::ZhCn
        );
        assert!(Language::from_file_value(&serde_json::json!("unsupported")).is_none());
    }
}
