use std::path::Path;

use warp_core::ui::appearance::Appearance;
use warpui::Element;
use warpui::assets::asset_cache::AssetSource;
use warpui::elements::{CacheOption, Icon, Image};

/// Returns a special icon for the given file path, if any.
pub fn icon_from_file_path(path: &str, appearance: &Appearance) -> Option<Box<dyn Element>> {
    let path = Path::new(path);
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    let themed = match extension.as_deref() {
        Some(
            "png" | "jpg" | "jpeg" | "jpe" | "jfif" | "gif" | "webp" | "svg" | "bmp" | "ico"
            | "tiff" | "tif" | "avif" | "heic" | "heif",
        ) => Some("bundled/svg/image-01.svg"),
        Some("toml" | "yaml" | "yml" | "ini" | "conf" | "cfg" | "properties" | "env" | "lock") => {
            Some("bundled/svg/settings.svg")
        }
        Some("sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd") => {
            Some("bundled/svg/terminal.svg")
        }
        Some("md" | "markdown") => Some("bundled/svg/file_type/markdown.svg"),
        Some("txt" | "log") => Some("bundled/svg/file-06.svg"),
        Some("csv" | "tsv") => Some("bundled/svg/bar-chart-04.svg"),
        Some("zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "zst") => {
            Some("bundled/svg/inbox-01.svg")
        }
        Some("pdf" | "epub") => Some("bundled/svg/book-open.svg"),
        Some("html" | "htm" | "css" | "scss" | "xml") => Some("bundled/svg/file-code-02.svg"),
        _ => None,
    };
    if let Some(path) = themed {
        let theme = appearance.theme();
        return Some(
            Icon::new(path, theme.main_text_color(theme.background()).into_solid()).finish(),
        );
    }

    let path = match extension.as_deref() {
        Some("rs") => "bundled/svg/file_type/rust.svg",
        Some("json" | "jsonl" | "jsonc" | "ndjson") => "bundled/svg/file_type/json.svg",
        Some("ts" | "tsx" | "mts" | "cts") => "bundled/svg/file_type/typescript.svg",
        Some("js" | "jsx" | "mjs" | "cjs") => "bundled/svg/file_type/javascript.svg",
        Some("py" | "pyi") => "bundled/svg/file_type/python.svg",
        Some("cpp" | "hpp" | "cc" | "cxx" | "hxx") => "bundled/svg/file_type/cpp.svg",
        Some("go") => "bundled/svg/file_type/go.svg",
        Some("kt" | "kts") => "bundled/svg/file_type/kotlin.svg",
        Some("php") => "bundled/svg/file_type/php.svg",
        Some("pl" | "pm") => "bundled/svg/file_type/perl.svg",
        Some("c" | "h") => "bundled/svg/file_type/c.svg",
        Some("pyx" | "pxd") => "bundled/svg/file_type/cython.svg",
        Some("swf") => "bundled/svg/file_type/flash.svg",
        Some("wasm") => "bundled/svg/file_type/wasm.svg",
        Some("zig") => "bundled/svg/file_type/zig.svg",
        Some("sql") => "bundled/svg/file_type/sql.svg",
        Some("ng" | "ngml") => "bundled/svg/file_type/angular.svg",
        Some("tf" | "hcl" | "tfvars") => "bundled/svg/file_type/terraform.svg",
        _ => return None,
    };
    Some(Image::new(AssetSource::Bundled { path }, CacheOption::BySize).finish())
}
