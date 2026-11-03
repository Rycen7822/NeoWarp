use serde::{Deserialize, Serialize};

use crate::context_chips::{ContextChipKind, available_chips};
use crate::ui_components::icons::Icon;

#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(rename_all = "snake_case")]
pub enum CliToolbeltItem {
    ContextChip(ContextChipKind),
    FileExplorer,
    RichInput,
    FileAttach,
    Settings,
}

impl CliToolbeltItem {
    pub(crate) fn available() -> Vec<Self> {
        let mut items: Vec<_> = available_chips()
            .into_iter()
            .map(Self::ContextChip)
            .collect();
        items.extend([
            Self::FileExplorer,
            Self::RichInput,
            Self::FileAttach,
            Self::Settings,
        ]);
        items
    }

    pub(crate) fn control(&self) -> Option<(&'static str, Icon)> {
        Some(match self {
            Self::FileExplorer => ("File Explorer", Icon::FileCopy),
            Self::RichInput => ("Rich Input", Icon::TextInput),
            Self::FileAttach => ("Attach File", Icon::Plus),
            Self::Settings => ("Settings", Icon::Settings),
            Self::ContextChip(_) => return None,
        })
    }
}

#[derive(
    Clone,
    Debug,
    Default,
    Eq,
    PartialEq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(rename_all = "snake_case")]
pub enum CLIAgentToolbarChipSelection {
    #[default]
    Default,
    Custom {
        left: Vec<CliToolbeltItem>,
        right: Vec<CliToolbeltItem>,
    },
}

impl CLIAgentToolbarChipSelection {
    pub(crate) fn items(&self) -> (Vec<CliToolbeltItem>, Vec<CliToolbeltItem>) {
        let (left, right) = match self {
            Self::Default => (
                vec![
                    CliToolbeltItem::FileExplorer,
                    CliToolbeltItem::RichInput,
                    CliToolbeltItem::ContextChip(ContextChipKind::WorkingDirectory),
                ],
                vec![CliToolbeltItem::ContextChip(ContextChipKind::GitDiffStats)],
            ),
            Self::Custom { left, right } => (left.clone(), right.clone()),
        };
        let available = CliToolbeltItem::available();
        let mut used = Vec::new();
        let mut filter = |items: Vec<CliToolbeltItem>| {
            items
                .into_iter()
                .filter(|item| {
                    if !available.contains(item) || used.contains(item) {
                        return false;
                    }
                    used.push(item.clone());
                    true
                })
                .collect()
        };
        (filter(left), filter(right))
    }

    pub(crate) fn context_chips(&self) -> Vec<ContextChipKind> {
        let (left, right) = self.items();
        left.into_iter()
            .chain(right)
            .filter_map(|item| match item {
                CliToolbeltItem::ContextChip(kind) => Some(kind),
                _ => None,
            })
            .collect()
    }
}
