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
