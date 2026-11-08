use settings::SupportedPlatforms;
use settings::macros::define_settings_group;

define_settings_group!(CodeSettings, settings: [
    code_as_default_editor: CodeAsDefaultEditor {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        private: false,
        toml_path: "code.editor.use_warp_as_default_editor",
        description: "Whether Warp is used as the default code editor.",
    }
    // Controls whether the Warp text editor automatically saves file changes as the
    // user types (debounced) and when the editor loses focus. Only applies to the
    // Warp text editor, not the command line.
    auto_save: AutoSave {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        private: false,
        toml_path: "code.editor.auto_save",
        description: "Whether the Warp text editor automatically saves changes as you type and when the editor loses focus.",
    },
    diff_auto_collapse_lines: DiffAutoCollapseLines {
        type: u16,
        default: 1000,
        supported_platforms: SupportedPlatforms::ALL,
        private: false,
        toml_path: "code.review.auto_collapse_lines",
        description: "Collapse file diffs above this many displayed code lines (0 always collapses, maximum 10000).",
    },
]);

impl DiffAutoCollapseLines {
    pub const MAX: u16 = 10_000;

    fn validate(&self, value: u16) -> u16 {
        value.min(Self::MAX)
    }
}
