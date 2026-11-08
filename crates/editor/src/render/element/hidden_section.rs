use warpui_core::{AppContext, LayoutContext};

use super::super::model::RenderState;
use super::super::model::viewport::ViewportItem;
use super::{RenderContext, RenderableBlock};

/// Keeps hidden-range geometry for the gutter arrows without painting a content bar.
pub struct RenderableHiddenSection {
    viewport_item: ViewportItem,
}

impl RenderableHiddenSection {
    pub fn new(viewport_item: ViewportItem) -> Self {
        Self { viewport_item }
    }
}

impl RenderableBlock for RenderableHiddenSection {
    fn viewport_item(&self) -> &ViewportItem {
        &self.viewport_item
    }

    fn layout(&mut self, _model: &RenderState, _ctx: &mut LayoutContext, _app: &AppContext) {}

    fn paint(&mut self, _model: &RenderState, _ctx: &mut RenderContext, _app: &AppContext) {}

    fn is_hidden_section(&self) -> bool {
        true
    }
}
