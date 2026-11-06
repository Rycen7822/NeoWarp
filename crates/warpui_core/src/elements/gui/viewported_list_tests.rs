use super::*;
use crate::elements::{ConstrainedBox, Rect};
use crate::platform::WindowStyle;
use crate::units::IntoPixels;
use crate::{App, AppContext, Entity, TypedActionView, ViewContext};

struct TestView {
    list_state: ListState<()>,
    heights: Vec<f32>,
}

impl TestView {
    fn new(item_heights: Vec<f32>, ctx: &mut ViewContext<Self>) -> Self {
        let handle = ctx.handle();
        let list_state = ListState::new(move |index, _, app| {
            let height = handle
                .upgrade(app)
                .and_then(|handle| handle.as_ref(app).heights.get(index).copied())
                .unwrap_or(50.);
            ConstrainedBox::new(Rect::new().finish())
                .with_height(height)
                .with_width(100.)
                .finish()
        });

        for _ in 0..item_heights.len() {
            list_state.add_item();
        }

        Self {
            list_state,
            heights: item_heights,
        }
    }

    fn set_height(&mut self, index: usize, height: f32) {
        self.heights[index] = height;
    }
}

impl Entity for TestView {
    type Event = ();
}

impl crate::core::View for TestView {
    fn ui_name() -> &'static str {
        "viewported_list_tests::TestView"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        List::new(self.list_state.clone()).finish()
    }
}

impl TypedActionView for TestView {
    type Action = ();
}

#[test]
fn test_scroll_position_invalidation_scrolled_to() {
    // Test: Create items, scroll to a position within an item, invalidate that item, layout.
    // Scroll position should not change.
    App::test((), |mut app| async move {
        let app = &mut app;
        let item_heights = vec![100., 1200.];

        let (_window_id, view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            TestView::new(item_heights, ctx)
        });

        let scroll_index = 1;
        let scroll_offset = 50.0.into_pixels();
        view.update(app, |view, ctx| {
            view.list_state
                .scroll_to_with_offset(scroll_index, scroll_offset);
            ctx.notify();

            assert_eq!(view.list_state.get_scroll_index(), scroll_index);
            assert_eq!(view.list_state.get_scroll_offset(), scroll_offset);
        });

        // Verify scroll position is preserved after layout
        view.read(app, |view, _| {
            assert_eq!(
                view.list_state.get_scroll_index(),
                scroll_index,
                "Scroll index should be preserved after layout"
            );
            assert_eq!(
                view.list_state.get_scroll_offset(),
                scroll_offset,
                "Scroll offset should be preserved after layout"
            );
        });

        view.update(app, |view, ctx| {
            view.list_state.invalidate_height_for_index(scroll_index);
            ctx.notify();
        });

        // Verify scroll position is preserved after invalidation
        view.read(app, |view, _| {
            assert_eq!(
                view.list_state.get_scroll_index(),
                scroll_index,
                "Scroll index should be preserved after invalidation"
            );
            assert_eq!(
                view.list_state.get_scroll_offset(),
                scroll_offset,
                "Scroll offset should be preserved after invalidation"
            );
        });
    });
}

#[test]
fn test_scroll_position_invalidation_on_screen() {
    // Test: Create items, scroll to a position within an item, invalidate that item, layout.
    // Scroll position should not change.
    App::test((), |mut app| async move {
        let app = &mut app;

        let item_heights = vec![100., 1200.];

        let (_window_id, view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            TestView::new(item_heights, ctx)
        });

        // Scroll to item 0 (first item) with offset 50
        // This puts us at pixel position 50, well within max scroll range
        let scroll_index = 0;
        let scroll_offset = 50.0.into_pixels();
        view.update(app, |view, ctx| {
            view.list_state
                .scroll_to_with_offset(scroll_index, scroll_offset);
            ctx.notify();

            assert_eq!(view.list_state.get_scroll_index(), scroll_index);
            assert_eq!(view.list_state.get_scroll_offset(), scroll_offset);
        });

        // Verify scroll position is preserved after layout
        view.read(app, |view, _| {
            assert_eq!(
                view.list_state.get_scroll_index(),
                scroll_index,
                "Scroll index should be preserved after layout"
            );
            assert_eq!(
                view.list_state.get_scroll_offset(),
                scroll_offset,
                "Scroll offset should be preserved after layout"
            );
        });

        // Invalidate item 1 (index 0) - the item we're scrolled into - and re-layout
        view.update(app, |view, ctx| {
            view.list_state.invalidate_height_for_index(scroll_index);
            ctx.notify();
        });

        // Verify scroll position is preserved after invalidation
        view.read(app, |view, _| {
            assert_eq!(
                view.list_state.get_scroll_index(),
                scroll_index,
                "Scroll index should be preserved after invalidation"
            );
            assert_eq!(
                view.list_state.get_scroll_offset(),
                scroll_offset,
                "Scroll offset should be preserved after invalidation"
            );
        });
    });
}

#[test]
fn test_scroll_position_clamp() {
    // Test: Create items, scroll to a position within an item, invalidate that item, layout.
    // Scroll position should not change.
    App::test((), |mut app| async move {
        let app = &mut app;

        let item_heights = vec![100., 1200.];

        let (_window_id, view) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            TestView::new(item_heights, ctx)
        });

        let scroll_index = 1;
        let scroll_offset = 300.0.into_pixels();
        view.update(app, |view, ctx| {
            view.list_state
                .scroll_to_with_offset(scroll_index, scroll_offset);
            ctx.notify();

            assert_eq!(view.list_state.get_scroll_index(), scroll_index);
            assert_eq!(view.list_state.get_scroll_offset(), scroll_offset);
        });

        // Verify scroll position is preserved after layout
        view.read(app, |view, _| {
            assert_eq!(
                view.list_state.get_scroll_index(),
                scroll_index,
                "Scroll index should be preserved after layout"
            );
            assert_eq!(
                view.list_state.get_scroll_offset(),
                scroll_offset,
                "Scroll offset should be preserved after layout"
            );
        });

        view.update(app, |view, ctx| {
            view.set_height(scroll_index, 500.);
            view.list_state.invalidate_height_for_index(scroll_index);
            ctx.notify();
        });

        // Verify scroll position is preserved after invalidation
        view.read(app, |view, _| {
            assert_eq!(
                view.list_state.get_scroll_index(),
                0,
                "Scroll index should be preserved after invalidation"
            );
            assert_eq!(
                view.list_state.get_scroll_offset(),
                0.0.into_pixels(),
                "Scroll offset should be preserved after invalidation"
            );
        });
    });
}
