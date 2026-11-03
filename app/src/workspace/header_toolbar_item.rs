use serde::{Deserialize, Serialize};
use warpui::{AppContext, SingletonEntity};

use crate::features::FeatureFlag;
use crate::ui_components::icons::Icon;
use crate::workspace::tab_settings::TabSettings;

/// A configurable item in the vertical tabs header toolbar.
///
/// Buttons can be placed on either side of the toolbar. The tabs panel follows
/// its button's side; Files and Git Review retain their existing panel positions.
#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(rename_all = "snake_case")]
pub enum HeaderToolbarItemKind {
    TabsPanel,
    Files,
    CodeReview,
}

impl HeaderToolbarItemKind {
    pub fn display_label(&self) -> &'static str {
        match self {
            Self::TabsPanel => "Tabs Panel",
            Self::Files => "Files / Projects",
            Self::CodeReview => "Code Review",
        }
    }

    pub fn icon(&self) -> Icon {
        match self {
            Self::TabsPanel => Icon::Menu,
            Self::Files => Icon::Folder,
            Self::CodeReview => Icon::Diff,
        }
    }

    /// Whether this item is supported on the current platform/configuration
    /// (feature flags and compile-time features).
    /// Does not check user show/hide preferences — use `is_available` for that.
    pub fn is_supported(&self, app: &AppContext) -> bool {
        match self {
            Self::TabsPanel => {
                FeatureFlag::VerticalTabs.is_enabled()
                    && *TabSettings::as_ref(app).use_vertical_tabs
            }
            Self::Files | Self::CodeReview => cfg!(feature = "local_fs"),
        }
    }

    pub fn default_left() -> Vec<Self> {
        vec![Self::TabsPanel, Self::Files]
    }

    pub fn default_right() -> Vec<Self> {
        vec![Self::CodeReview]
    }

    /// All toolbar item variants (availability filtering is done at the call site).
    pub fn all_items() -> Vec<Self> {
        vec![Self::TabsPanel, Self::Files, Self::CodeReview]
    }
}
