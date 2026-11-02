pub mod clipboard;
mod network;
mod registry;
mod system_caption_buttons;
mod window_attribute;
mod window_ext;

pub use clipboard::*;
pub use network::*;
pub use registry::*;
pub use system_caption_buttons::*;
pub use window_attribute::*;
pub use window_ext::WindowExt;

/// Background automation must never activate a window on the user's desktop.
pub(super) fn background_launch() -> bool {
    static BACKGROUND: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        std::env::var_os("NEOWARP_BACKGROUND").is_some_and(|value| value == "1")
    });
    *BACKGROUND
}
