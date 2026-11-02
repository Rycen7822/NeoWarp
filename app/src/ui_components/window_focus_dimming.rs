use warp_core::ui::color::coloru_with_opacity;
use warpui::elements::{Container, Element, Fill, Hoverable, MouseStateHandle};
use warpui::windowing::WindowManager;
use warpui::{AppContext, SingletonEntity, WindowId};

use crate::window_settings::WindowSettings;
use crate::workspace::panel_header_corner_radius;

/// Opacity level for dimming the header of unfocused windows.
/// 0 means no dimming, 100 means 100% cover the top bar.
const UNFOCUSED_WINDOW_DIMMING_OPACITY: crate::util::color::Opacity = 45;

/// Utility functions for applying consistent window focus dimming across all UI components.
pub struct WindowFocusDimming;

impl WindowFocusDimming {
    /// Returns true if the specified window is currently focused.
    ///
    pub fn is_window_focused(window_id: WindowId, ctx: &AppContext) -> bool {
        let window_manager = WindowManager::as_ref(ctx);
        if !window_manager.app_is_active() {
            return false;
        }

        window_manager.active_window() == Some(window_id)
    }

    /// Applies dimming overlay for headers and top bar areas.
    pub fn apply_panel_header_dimming(
        element: Box<dyn Element>,
        mouse_state: MouseStateHandle,
        background_color: warpui::color::ColorU,
        window_id: WindowId,
        ctx: &AppContext,
    ) -> Box<dyn Element> {
        if !Self::is_window_focused(window_id, ctx) {
            let background_opacity = WindowSettings::as_ref(ctx)
                .background_opacity
                .effective_opacity(window_id, ctx);
            let scaled_opacity =
                (UNFOCUSED_WINDOW_DIMMING_OPACITY as f32 * background_opacity as f32 / 100.) as u8;
            // Dimming is decoration, not an input-blocking layer over the controls.
            let dimmed = Container::new(element)
                .with_foreground_overlay(Fill::Solid(coloru_with_opacity(
                    background_color,
                    scaled_opacity,
                )))
                .with_corner_radius(panel_header_corner_radius())
                .finish();
            // Wrap the dimmed content in a hoverable that can clear dimming
            // if the window becomes active during hover (failsafe mechanism)
            Hoverable::new(mouse_state, |_| dimmed)
                .on_hover(move |is_hovered, ctx, app, _position| {
                    if is_hovered {
                        // Double-check if window became active during hover
                        // If so, trigger a re-render to clear the dimming
                        if Self::is_window_focused(window_id, app) {
                            ctx.notify();
                        }
                    }
                })
                .finish()
        } else {
            element
        }
    }
}
