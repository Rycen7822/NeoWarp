//! Grid tooltips for the terminal view

use pathfinder_geometry::vector::vec2f;
use warpui::elements::{
    ChildAnchor, Dismiss, MouseStateHandle, OffsetPositioning, PositionedElementAnchor,
    PositionedElementOffsetBounds, Stack,
};
use warpui::event::ModifiersState;
use warpui::{AppContext, Element, EventContext};

use super::{GridHighlightedLink, TerminalAction, TerminalView};
use crate::appearance::Appearance;
use crate::localization::tr;
use crate::terminal::TerminalModel;
use crate::terminal::links::directly_open_link_keybinding_string;
use crate::terminal::model::{ObfuscateSecrets, Secret};
use crate::terminal::safe_mode_settings::get_secret_obfuscation_mode;
use crate::util::tooltips::TooltipLink;

struct GridTooltipLink {
    text: String,
    action: TerminalAction,
    /// Optional detail text to show after the link.
    detail: Option<String>,
    mouse_state: MouseStateHandle,
}

/// Returns a GridTooltipLink for revealing the file in the platform's file explorer
/// (Finder on macOS, file manager on Linux/Windows).
#[cfg(feature = "local_fs")]
fn show_in_file_explorer_tooltip(
    path: std::path::PathBuf,
    mouse_state: MouseStateHandle,
) -> GridTooltipLink {
    let text = if cfg!(target_os = "macos") {
        "Show in Finder"
    } else {
        "Show containing folder"
    };
    GridTooltipLink {
        text: tr(text).to_owned(),
        action: TerminalAction::ShowInFileExplorer(path),
        mouse_state,
        detail: None,
    }
}

impl TerminalView {
    /// Renders the link and/or secrets tooltips on top of the grid
    /// Expects at least one of the two tooltips to be visible.
    // Unused variables allowed when no local filesystem as the `app` argument
    // is unused.
    #[cfg_attr(not(feature = "local_fs"), allow(unused_variables))]
    pub(super) fn render_grid_tooltip(
        &self,
        stack: &mut Stack,
        model: &TerminalModel,
        appearance: &Appearance,
        app: &AppContext,
    ) {
        let mut element_id = "terminal_view:first_cell_in_link".to_string();
        let mut links = vec![];

        if let Some(tooltip) = &self.open_secret_tool_tip {
            let handle = *tooltip;

            // Position the tooltip above the first cell in the secret.
            element_id = format!(
                "terminal_view:first_cell_in_secret_{}",
                handle.get_inner().id()
            );

            if matches!(get_secret_obfuscation_mode(app), ObfuscateSecrets::Yes) {
                let is_redacted = model
                    .secret_from_handle(tooltip)
                    .is_some_and(Secret::is_obfuscated);

                if is_redacted {
                    links.push(GridTooltipLink {
                        text: "Reveal secret".to_string(),
                        action: TerminalAction::ToggleGridSecret {
                            handle,
                            show_secret: true,
                        },
                        mouse_state: self.mouse_states.toggle_secrets_tooltip.clone(),
                        detail: None,
                    });
                } else {
                    links.push(GridTooltipLink {
                        text: "Hide secret".to_string(),
                        action: TerminalAction::ToggleGridSecret {
                            handle,
                            show_secret: false,
                        },
                        mouse_state: self.mouse_states.toggle_secrets_tooltip.clone(),
                        detail: None,
                    })
                }
            }

            links.push(GridTooltipLink {
                text: "Copy secret".to_string(),
                action: TerminalAction::CopyGridSecret(handle),
                mouse_state: self.mouse_states.copy_secrets_tooltip.clone(),
                detail: None,
            });
        }

        #[cfg_attr(not(feature = "local_fs"), allow(unused_mut))]
        if let Some(link) = &self.open_grid_link_tool_tip {
            let mut show_in_file_explorer = None;
            let modifier = directly_open_link_keybinding_string();
            let detail = if link.is_file(model) {
                Some(format!(
                    "[{}; {modifier} {}]",
                    tr("Click: open in right pane"),
                    tr("Click: open with system default app"),
                ))
            } else {
                Some(format!("[{modifier} {}]", tr("Click")))
            };
            #[cfg(feature = "local_fs")]
            {
                if let GridHighlightedLink::File(file_link) = link
                    && let Some(path) = file_link.get_inner().absolute_path()
                {
                    show_in_file_explorer = Some(show_in_file_explorer_tooltip(
                        path,
                        self.mouse_states.show_in_file_explorer_tooltip.clone(),
                    ));
                }
            }

            links.push(GridTooltipLink {
                // OSC 8 links hide the destination behind arbitrary visible
                // text, so show the URI itself in the tooltip; auto-detected
                // URLs already show their destination as the visible text.
                text: match link {
                    GridHighlightedLink::Hyperlink { uri, .. } => uri.clone(),
                    _ => tr(link.tooltip_text()).to_owned(),
                },
                action: TerminalAction::OpenGridLink {
                    link: link.clone(),
                    modifiers: ModifiersState::default(),
                },
                mouse_state: self.mouse_states.grid_link_tooltip.clone(),
                detail,
            });

            links.extend(show_in_file_explorer);
        }

        stack.add_positioned_overlay_child(
            render_tooltip(links, appearance),
            OffsetPositioning::offset_from_save_position_element(
                element_id,
                // Add a small buffer between the tooltip and the top of the cell.
                vec2f(0., -2.),
                PositionedElementOffsetBounds::ParentByPosition,
                PositionedElementAnchor::TopLeft,
                ChildAnchor::BottomLeft,
            ),
        );
    }
}

fn render_tooltip(
    tooltip_links: impl IntoIterator<Item = GridTooltipLink>,
    appearance: &Appearance,
) -> Box<dyn Element> {
    // Convert GridTooltipLink to shared TooltipLink
    let shared_links = tooltip_links.into_iter().map(|link| {
        let action = link.action;
        TooltipLink {
            text: link.text,
            on_click: move |ctx: &mut EventContext, held_modifiers| {
                let mut action = action.clone();
                if let TerminalAction::OpenGridLink { modifiers, .. } = &mut action {
                    *modifiers = held_modifiers;
                }
                ctx.dispatch_typed_action(action);
            },
            detail: link.detail,
            mouse_state: link.mouse_state,
        }
    });

    let tooltip_content = crate::util::tooltips::render_tooltip(shared_links, appearance);

    Dismiss::new(tooltip_content)
        .on_dismiss(|ctx, _app| {
            ctx.dispatch_typed_action(TerminalAction::MaybeDismissToolTip {
                from_keybinding: false,
            })
        })
        .finish()
}
