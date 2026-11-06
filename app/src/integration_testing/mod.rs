use std::borrow::Cow;

use warpui::AssetProvider;

pub mod assertions;
pub mod block;
pub mod block_filtering;
pub mod clipboard;
pub mod command_palette;
pub mod command_search;
pub mod context_chips;
pub mod find;
pub mod goto_line;
pub mod input;
pub mod keybindings;
pub mod launch_configs;
pub mod navigation_palette;
pub mod pane_group;
pub mod persistence;
pub mod remote_server;
pub mod secret_redaction;
pub mod settings;
pub mod step;
pub mod subshell;
pub mod tab;
pub mod terminal;
pub mod themes;
pub mod view_getters;
pub mod window;
pub mod workspace;

pub fn create_file_from_assets(
    assets: impl AssetProvider,
    asset_src: &str,
    dest_path: &std::path::Path,
) {
    let bytes = assets
        .get(asset_src)
        .expect("Should be able to retrieve file");
    create_file_with_contents(<Cow<'_, [u8]> as AsRef<[u8]>>::as_ref(&bytes), dest_path);
}

pub fn create_file_with_contents(contents: impl AsRef<[u8]>, file_path: &std::path::Path) {
    let mut file =
        crate::util::file::create_file(file_path).expect("Should be able to create file");
    std::io::Write::write_all(&mut file, contents.as_ref())
        .expect("Should be able to write to file");
}
