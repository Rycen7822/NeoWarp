use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use neowarp_plugin_api as api;
use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::Vector2F;

use crate::elements::{Element, Point};
use crate::event::DispatchedEvent;
use crate::{
    Action, AfterLayoutContext, AppContext, EntityId, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};

#[derive(Clone, Copy)]
pub struct SemanticId(u64);

impl Default for SemanticId {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(2);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

pub struct Semantics<'a> {
    id: SemanticId,
    label: &'a str,
    flags: u32,
}

impl<'a> Semantics<'a> {
    pub fn new(id: SemanticId, label: &'a str) -> Self {
        Self {
            id,
            label,
            flags: 0,
        }
    }
    pub fn selected(mut self, selected: bool) -> Self {
        if selected {
            self.flags |= api::SELECTED;
        }
        self
    }
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.flags |= api::EXPANDABLE;
        if expanded {
            self.flags |= api::EXPANDED;
        }
        self
    }
    pub fn checked(mut self, checked: bool) -> Self {
        self.flags |= api::CHECKABLE;
        if checked {
            self.flags |= api::CHECKED;
        }
        self
    }
}

#[derive(Clone)]
pub struct Control {
    pub id: u64,
    pub label: String,
    pub flags: u32,
    pub bounds: RectF,
    pub(crate) view: EntityId,
    pub(crate) action: Arc<dyn Action>,
    pub(crate) hit_point: Point,
}

/// Disabled plugins do not allocate semantic wrappers, labels or action objects.
pub fn accessible<A: Action>(
    child: Box<dyn Element>,
    app: &AppContext,
    semantics: Semantics<'_>,
    action: A,
) -> Box<dyn Element> {
    if !app.accessibility_plugin_enabled() {
        return child;
    }
    Accessible {
        child,
        id: semantics.id.0,
        label: semantics.label.to_owned(),
        flags: semantics.flags,
        action: Arc::new(action),
    }
    .finish()
}

struct Accessible {
    child: Box<dyn Element>,
    id: u64,
    label: String,
    flags: u32,
    action: Arc<dyn Action>,
}

impl Element for Accessible {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        self.child.layout(constraint, ctx, app)
    }
    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }
    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let clip = ctx
            .scene
            .active_clip_bounds()
            .unwrap_or(RectF::new(Vector2F::zero(), ctx.window_size));
        self.child.paint(origin, ctx, app);
        if ctx.collect_semantics {
            let bounds = RectF::new(origin, self.child.size().unwrap_or_default());
            if let (Some(bounds), Some(view)) = (bounds.intersection(clip), ctx.current_view)
                && bounds.width() > 0.
                && bounds.height() > 0.
            {
                let scale = ctx.scene.scale_factor();
                ctx.semantic_controls.push(Control {
                    id: self.id,
                    label: self.label.clone(),
                    flags: self.flags,
                    bounds: RectF::new(bounds.origin() * scale, bounds.size() * scale),
                    view,
                    action: self.action.clone(),
                    hit_point: Point::from_vec2f(bounds.center(), ctx.scene.max_active_z_index()),
                });
            }
        }
    }
    fn size(&self) -> Option<Vector2F> {
        self.child.size()
    }
    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }
    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }
}
