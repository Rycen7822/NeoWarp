//! A read-only bitmap tab, with bounded decoding off the UI thread.
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, ensure};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use pathfinder_geometry::vector::{Vector2F, vec2f};
use warp_core::ui::appearance::Appearance;
use warpui::r#async::SpawnedFutureHandle;
use warpui::elements::new_scrollable::{
    ClippedAxisConfiguration, DualAxisConfig, ScrollableAppearance,
};
use warpui::elements::{
    Align, ClippedScrollStateHandle, ConstrainedBox, Container, Empty, Fill, Image, NewScrollable,
    Padding, Point, ScrollbarWidth, Text,
};
use warpui::event::DispatchedEvent;
use warpui::image_cache::StaticImage;
use warpui::{
    AfterLayoutContext, AppContext, Element, Entity, EventContext, LayoutContext, PaintContext,
    SingletonEntity, SizeConstraint, View, ViewContext,
};

use crate::localization::tr;
use crate::util::path::local_path_for_io;

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_DECODED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PREVIEW_DIMENSION: u32 = 2048;

fn can_decode(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
    )
}

pub(super) fn can_preview(path: &Path) -> bool {
    ImageFormat::from_path(path).is_ok_and(can_decode)
}

fn decode(path: &Path) -> Result<StaticImage> {
    let path = local_path_for_io(path);
    let metadata = std::fs::metadata(path.as_ref())?;
    ensure!(metadata.is_file(), "Not a regular image file");
    let size = metadata.len();
    ensure!(
        size <= MAX_FILE_BYTES,
        "Image exceeds the preview file size limit"
    );
    let mut bytes = Vec::with_capacity(size as usize);
    File::open(path.as_ref())?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE_BYTES,
        "Image exceeds the preview file size limit"
    );

    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    ensure!(
        reader.format().is_some_and(can_decode),
        "Unsupported image format"
    );
    let mut limits = Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    reader.limits(limits);

    let mut decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    ensure!(
        decoder
            .total_bytes()
            .max(u64::from(width) * u64::from(height) * 4)
            <= MAX_DECODED_BYTES,
        "Image exceeds the preview pixel limit"
    );
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder)?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    if image.width().max(image.height()) > MAX_PREVIEW_DIMENSION {
        image = image.thumbnail(MAX_PREVIEW_DIMENSION, MAX_PREVIEW_DIMENSION);
    }
    Ok(StaticImage::from_rgba(image.into_rgba8()))
}

enum State {
    Loading(SpawnedFutureHandle),
    Ready(Arc<StaticImage>),
    Failed,
}

pub(super) struct ImagePreview {
    pub(super) path: PathBuf,
    state: State,
    scale: f32,
    horizontal_scroll: ClippedScrollStateHandle,
    vertical_scroll: ClippedScrollStateHandle,
}

impl ImagePreview {
    pub(super) fn new(path: PathBuf, scale: f32, ctx: &mut ViewContext<Self>) -> Self {
        let input = path.clone();
        let task = ctx.spawn(
            blocking::unblock(move || decode(&input)),
            |me, result, ctx| {
                me.state = match result {
                    Ok(image) => State::Ready(Arc::new(image)),
                    Err(error) => {
                        log::debug!("Image preview failed: {error:#}");
                        State::Failed
                    }
                };
                ctx.notify();
            },
        );
        Self {
            path,
            state: State::Loading(task),
            scale,
            horizontal_scroll: Default::default(),
            vertical_scroll: Default::default(),
        }
    }

    pub(super) fn set_scale(&mut self, scale: f32, ctx: &mut ViewContext<Self>) {
        self.scale = scale;
        ctx.notify();
    }

    #[cfg(test)]
    pub(super) fn decoded(&self) -> Option<&Arc<StaticImage>> {
        match &self.state {
            State::Ready(image) => Some(image),
            _ => None,
        }
    }
}

impl Drop for ImagePreview {
    fn drop(&mut self) {
        if let State::Loading(task) = &self.state {
            task.abort();
        }
    }
}

impl Entity for ImagePreview {
    type Event = ();
}

impl View for ImagePreview {
    fn ui_name() -> &'static str {
        "ImagePreview"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let message = match &self.state {
            State::Ready(image) => {
                return Container::new(
                    ImageViewport {
                        image: image.clone(),
                        scale: self.scale,
                        horizontal_scroll: self.horizontal_scroll.clone(),
                        vertical_scroll: self.vertical_scroll.clone(),
                        child: Empty::new().finish(),
                    }
                    .finish(),
                )
                .with_padding(Padding::uniform(12.))
                .finish();
            }
            State::Loading(_) => tr("Loading image…"),
            State::Failed => tr("Unable to preview image."),
        };
        let appearance = Appearance::as_ref(app);
        Align::new(
            Text::new_inline(
                message,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .finish(),
        )
        .finish()
    }
}

/// Fits at 100% and uses the usual two-axis scrollbars for the magnified bitmap.
struct ImageViewport {
    image: Arc<StaticImage>,
    scale: f32,
    horizontal_scroll: ClippedScrollStateHandle,
    vertical_scroll: ClippedScrollStateHandle,
    child: Box<dyn Element>,
}

impl Element for ImageViewport {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let original = self.image.size().to_f32();
        let available = vec2f(
            if constraint.max.x().is_finite() {
                constraint.max.x()
            } else {
                original.x()
            },
            if constraint.max.y().is_finite() {
                constraint.max.y()
            } else {
                original.y()
            },
        );
        let fit = (available.x() / original.x()).min(available.y() / original.y());
        let size = (original * (fit * self.scale)).round().max(vec2f(1., 1.));
        let bitmap = ConstrainedBox::new(Image::from_static(self.image.clone()).finish())
            .with_width(size.x())
            .with_height(size.y())
            .finish();
        let canvas_size = size.max(available).max(constraint.min);
        let canvas = ConstrainedBox::new(Align::new(bitmap).finish())
            .with_width(canvas_size.x())
            .with_height(canvas_size.y())
            .finish();
        let theme = Appearance::as_ref(app).theme();
        let scrollbar = ScrollableAppearance::new(ScrollbarWidth::Auto, true);
        self.child = NewScrollable::horizontal_and_vertical(
            DualAxisConfig::Clipped {
                horizontal: ClippedAxisConfiguration {
                    handle: self.horizontal_scroll.clone(),
                    ..Default::default()
                },
                vertical: ClippedAxisConfiguration {
                    handle: self.vertical_scroll.clone(),
                    ..Default::default()
                },
                child: canvas,
            },
            theme.nonactive_ui_detail().into(),
            theme.active_ui_detail().into(),
            Fill::None,
        )
        .with_horizontal_scrollbar(scrollbar)
        .with_vertical_scrollbar(scrollbar)
        .finish();
        self.child.layout(constraint, ctx, app)
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.child.paint(origin, ctx, app);
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
