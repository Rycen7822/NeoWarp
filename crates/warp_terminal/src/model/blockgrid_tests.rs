use pathfinder_geometry::vector::Vector2F;
use warp_core::features::FeatureFlag;

use crate::SizeInfo;
use crate::event_listener::ChannelEventListener;
use crate::model::ansi::Handler;
use crate::model::blockgrid::BlockGrid;
use crate::model::grid::Dimensions;
use crate::model::grid::grid_handler::PerformResetGridChecks;
use crate::model::image_map::StoredImageMetadata;
use crate::model::index::Point;
use crate::model::iterm_image::{ITermImage, ITermImageDimensionUnit, ITermImageMetadata};
use crate::model::kitty::{CursorMovementPolicy, KittyAction};
use crate::model::secrets::ObfuscateSecrets;
use crate::test_util::{
    mock_blockgrid, test_kitty_image_metadata_map, test_kitty_store_and_display_action,
};

#[test]
pub fn test_finish_truncates_grid_basic() {
    let size = SizeInfo::new_without_font_metrics(10, 7);
    let mut block_grid = BlockGrid::new(
        size,
        1000, /* max_scroll_limit */
        ChannelEventListener::new_for_test(),
        ObfuscateSecrets::No,
        PerformResetGridChecks::default(),
    );

    for c in "hello".chars() {
        block_grid.input(c);
    }
    block_grid.linefeed();
    block_grid.finish();

    assert_eq!(block_grid.len(), 2);
    assert_eq!(block_grid.grid_handler().total_rows(), 2);
    assert_eq!(block_grid.grid_handler().columns(), size.columns);
    assert_eq!(block_grid.grid_storage().estimated_heap_usage_bytes(), 0);
}

#[test]
pub fn test_finish_truncates_grid_cursor_at_bottom() {
    let size = SizeInfo::new_without_font_metrics(10, 7);
    let mut block_grid = BlockGrid::new(
        size,
        1000, /* max_scroll_limit */
        ChannelEventListener::new_for_test(),
        ObfuscateSecrets::No,
        PerformResetGridChecks::default(),
    );

    for _ in 0..300 {
        block_grid.input('a');
        block_grid.linefeed();
        block_grid.carriage_return();
    }

    block_grid.finish();

    assert_eq!(block_grid.len(), 300);
    assert_eq!(block_grid.grid_handler().total_rows(), 301);
    assert_eq!(block_grid.grid_handler().columns(), size.columns);
}

#[test]
pub fn test_resize_finished_block() {
    let size = SizeInfo::new_without_font_metrics(10, 7);
    let mut block_grid = BlockGrid::new(
        size,
        1000, /* max_scroll_limit */
        ChannelEventListener::new_for_test(),
        ObfuscateSecrets::No,
        PerformResetGridChecks::default(),
    );

    for _ in 0..5 {
        for c in "hello".chars() {
            block_grid.input(c);
        }

        block_grid.linefeed();
        block_grid.carriage_return();
    }

    block_grid.finish();

    assert_eq!(block_grid.len(), 5);
    assert_eq!(block_grid.grid_handler().total_rows(), 6);

    block_grid.resize(SizeInfo::new_without_font_metrics(10, 10));

    // The len and total rows of the grid should be unchanged even after resize.
    assert_eq!(block_grid.len(), 5);
    assert_eq!(block_grid.grid_handler().total_rows(), 6);

    // Resize so that the contents of can no longer fit on one line. The length of the grid
    // should increase because each row is now soft-wrapped.
    block_grid.resize(SizeInfo::new_without_font_metrics(10, 3));
    assert_eq!(block_grid.len(), 10);
    // Each of the 5 "hello" rows should be split into "hel" and "lo" (for a total
    // of 10 rows), plus one final empty row at the end.
    assert_eq!(block_grid.grid_handler().total_rows(), 11);

    // Resizing the height in either direction should have no effect on the total grid height.
    block_grid.resize(SizeInfo::new_without_font_metrics(3, 7));
    assert_eq!(block_grid.len(), 5);
    assert_eq!(block_grid.grid_handler().total_rows(), 6);

    block_grid.resize(SizeInfo::new_without_font_metrics(300, 7));
    assert_eq!(block_grid.len(), 5);
    assert_eq!(block_grid.grid_handler().total_rows(), 6);
}

#[test]
pub fn test_resize_finished_softwrapped_block() {
    let size = SizeInfo::new_without_font_metrics(10, 3);
    let mut block_grid = BlockGrid::new(
        size,
        1000, /* max_scroll_limit */
        ChannelEventListener::new_for_test(),
        ObfuscateSecrets::No,
        PerformResetGridChecks::default(),
    );

    for _ in 0..5 {
        for c in "hello".chars() {
            block_grid.input(c);
        }

        block_grid.linefeed();
        block_grid.carriage_return();
    }

    block_grid.finish();

    assert_eq!(block_grid.len(), 10);
    assert_eq!(block_grid.grid_handler().total_rows(), 11);

    // Resize so each item can only fit on a given line.
    block_grid.resize(SizeInfo::new_without_font_metrics(10, 10));

    assert_eq!(block_grid.len(), 5);
    assert_eq!(block_grid.grid_handler().total_rows(), 6);
}

#[test]
pub fn test_content_summary() {
    let blockgrid = mock_blockgrid("1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10\r\n");

    // Test a summary that omits the middle of the grid.
    let summary = blockgrid.content_summary(1, 2, false);
    assert_eq!(summary, "1\n...(truncated)...\n9\n10\n");

    // Test a summary that perfectly covers the entire grid.
    let summary = blockgrid.content_summary(7, 3, false);
    assert_eq!(summary, "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");

    // Test a summary where the total range exceeds the grid size.
    let summary = blockgrid.content_summary(7, 6, false);
    assert_eq!(summary, "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
}

#[test]
pub fn test_non_moving_kitty_image_keeps_finished_grid_visible() {
    let _kitty_images = FeatureFlag::KittyImages.override_enabled(true);
    let size = SizeInfo::new_without_font_metrics(10, 10);
    let mut block_grid = BlockGrid::new(
        size,
        1000,
        ChannelEventListener::new_for_test(),
        ObfuscateSecrets::No,
        PerformResetGridChecks::default(),
    );
    let mut metadata = test_kitty_image_metadata_map(1);
    let mut action = test_kitty_store_and_display_action(1, 1);
    let KittyAction::StoreAndDisplay(store_and_display) = &mut action else {
        panic!("expected StoreAndDisplay action");
    };
    store_and_display.placement_data.cursor_movement_policy = CursorMovementPolicy::DoNotMoveCursor;

    block_grid
        .handle_completed_kitty_action(action, &mut metadata)
        .expect("kitty action should be handled")
        .expect("kitty image should render");

    assert_eq!(block_grid.grid_handler().cursor_point(), Point::new(0, 0));
    assert!(block_grid.grid_handler().has_visible_images());
    assert!(!block_grid.should_show_as_empty_when_finished());
}

#[test]
pub fn test_kitty_subpixel_height_uses_one_cell() {
    let _kitty_images = FeatureFlag::KittyImages.override_enabled(true);
    let mut block_grid = BlockGrid::new(
        SizeInfo::new_without_font_metrics(10, 10),
        1000,
        ChannelEventListener::new_for_test(),
        ObfuscateSecrets::No,
        PerformResetGridChecks::default(),
    );
    block_grid.start();

    let mut metadata = test_kitty_image_metadata_map(1);
    let Some(StoredImageMetadata::Kitty(kitty_metadata)) = metadata.get_mut(&1) else {
        panic!("expected Kitty metadata");
    };
    kitty_metadata.image_size = Vector2F::new(100.0, 1.0);

    let mut action = test_kitty_store_and_display_action(1, 1);
    let KittyAction::StoreAndDisplay(store_and_display) = &mut action else {
        panic!("expected StoreAndDisplay action");
    };
    store_and_display.image.metadata.image_size = Vector2F::new(100.0, 1.0);
    store_and_display.placement_data.rows = None;

    block_grid
        .handle_completed_kitty_action(action, &mut metadata)
        .expect("kitty action should be handled")
        .expect("kitty image should render");

    let placement = block_grid
        .grid_handler()
        .get_image_placement_data(1, 1)
        .expect("image placement should exist");
    assert_eq!(placement.height_cells, 1);
    assert_eq!(block_grid.grid_handler().cursor_point(), Point::new(0, 1));
}

#[test]
pub fn test_iterm_subpixel_height_uses_one_cell() {
    let _iterm_images = FeatureFlag::ITermImages.override_enabled(true);
    let mut block_grid = BlockGrid::new(
        SizeInfo::new_without_font_metrics(5, 8),
        1000,
        ChannelEventListener::new_for_test(),
        ObfuscateSecrets::No,
        PerformResetGridChecks::default(),
    );
    block_grid.start();

    block_grid.handle_completed_iterm_image(ITermImage {
        metadata: ITermImageMetadata {
            id: 1,
            desired_width: Some((50, ITermImageDimensionUnit::Percent)),
            desired_height: Some((1, ITermImageDimensionUnit::Percent)),
            preserve_aspect_ratio: true,
            name: "x".to_string(),
            inline: true,
            image_size: Vector2F::new(100.0, 100.0),
        },
        data: vec![0],
    });

    let placements = block_grid.grid_handler().get_image_ids_in_range(0, 1);
    assert_eq!(placements.len(), 1);
    let placement = block_grid
        .grid_handler()
        .get_image_placement_data(1, placements[0].placement_id)
        .expect("image placement should exist");
    assert_eq!(placement.height_cells, 1);
    assert_eq!(block_grid.grid_handler().cursor_point(), Point::new(0, 4));
}

#[test]
pub fn test_cursor_display_point_preserves_marked_text_across_rows() {
    let size = SizeInfo::new_without_font_metrics(10, 10);
    let mut block_grid = BlockGrid::new(
        size,
        1000,
        ChannelEventListener::new_for_test(),
        ObfuscateSecrets::No,
        PerformResetGridChecks::default(),
    );

    block_grid.start();
    block_grid.input('a');
    block_grid.input('b');
    block_grid.input('c');
    block_grid
        .grid_handler_mut()
        .set_marked_text("12345678", &(0..0));

    assert_eq!(block_grid.len_displayed(), 1);
    assert_eq!(block_grid.cursor_display_point(), Point::new(1, 1));
}
