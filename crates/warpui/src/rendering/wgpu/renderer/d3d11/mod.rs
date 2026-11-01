mod device;

use std::collections::HashMap;
use std::ops::Range;

use anyhow::Result;
use device::{Device, Pipeline, Texture, create_buffer, create_texture};
use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::Vector2F;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_VERTEX_BUFFER, D3D11_BOX, ID3D11Buffer};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
    DXGI_ERROR_DRIVER_INTERNAL_ERROR,
};

use super::glyph::shaders::GlyphInstanceData;
use super::image::shaders::{ColorModifier, ImageInstanceData};
use super::{Error, rect};
use crate::Scene;
use crate::fonts::SubpixelAlignment;
use crate::platform::CapturedFrame;
use crate::rendering::atlas::TextureId;
use crate::rendering::texture_cache::{TextureCache, TextureCacheIndex};
use crate::rendering::wgpu::shader_types::RectData;
use crate::rendering::{
    CornerRadius, GPUPowerPreference, GlyphCache, GlyphRasterBoundsFn, RasterizeGlyphFn,
};
use crate::scene::GlyphFade;

type CachedTexture = windows::core::Result<Texture>;

pub(crate) fn requested() -> bool {
    std::env::var_os("WGPU_BACKEND")
        .is_none_or(|backend| backend.to_string_lossy().eq_ignore_ascii_case("dx11"))
}

pub(crate) struct Renderer {
    device: Device,
    rect: Pipeline,
    glyph: Pipeline,
    image: Pipeline,
    glyphs: GlyphCache<CachedTexture>,
    images: TextureCache<CachedTexture>,
}

struct LayerDraw {
    clip: RECT,
    rects: Range<usize>,
    images: Vec<(TextureCacheIndex, usize)>,
    glyphs: Vec<(TextureId, Range<usize>)>,
}

impl Renderer {
    pub fn new(hwnd: HWND, size: Vector2F, preference: GPUPowerPreference) -> Result<Self> {
        let device = Device::new(
            hwnd,
            size.x().max(1.) as u32,
            size.y().max(1.) as u32,
            preference,
        )?;
        let rect = Pipeline::new(
            &device.device,
            include_str!(concat!(env!("OUT_DIR"), "/d3d11-rect.hlsl")),
            "rect_fs_main",
            RectData::desc(),
        )?;
        let glyph = Pipeline::new(
            &device.device,
            include_str!(concat!(env!("OUT_DIR"), "/d3d11-glyph.hlsl")),
            "fs_main",
            GlyphInstanceData::desc(),
        )?;
        let image = Pipeline::new(
            &device.device,
            include_str!(concat!(env!("OUT_DIR"), "/d3d11-image.hlsl")),
            "fs_main",
            ImageInstanceData::desc(),
        )?;
        Ok(Self {
            device,
            rect,
            glyph,
            image,
            glyphs: GlyphCache::new(Default::default()),
            images: TextureCache::new(),
        })
    }

    pub fn low_power_gpu_available(&self) -> bool {
        self.device.low_power_available
    }

    pub fn resize(&mut self, size: Vector2F) -> Result<(), Error> {
        if size.x() < 1. || size.y() < 1. {
            return Ok(());
        }
        self.device
            .resize(size.x() as u32, size.y() as u32)
            .map_err(|error| native_error(error.into()))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render<'a>(
        &mut self,
        scene: &Scene,
        rasterize: &RasterizeGlyphFn,
        bounds: &GlyphRasterBoundsFn,
        size: Vector2F,
        pre_present: Option<Box<dyn FnOnce() + 'a>>,
        capture: Option<Box<dyn FnOnce(CapturedFrame) + Send + 'static>>,
    ) -> Result<(), Error> {
        if size.x() < 1. || size.y() < 1. {
            return Ok(());
        }
        let result = self.render_frame(scene, rasterize, bounds, size, pre_present, capture);
        // Release bindings on errors as well as successful presents, before retiring textures.
        unsafe {
            self.device.context.ClearState();
        }
        self.images.end_frame();
        result.map_err(native_error)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_frame<'a>(
        &mut self,
        scene: &Scene,
        rasterize: &RasterizeGlyphFn,
        bounds: &GlyphRasterBoundsFn,
        size: Vector2F,
        pre_present: Option<Box<dyn FnOnce() + 'a>>,
        capture: Option<Box<dyn FnOnce(CapturedFrame) + Send + 'static>>,
    ) -> Result<()> {
        self.device.resize(size.x() as u32, size.y() as u32)?;
        self.glyphs.update_config(&scene.rendering_config().glyphs);
        let scale = scene.scale_factor();
        let device_bounds = RectF::new(Vector2F::zero(), size);
        let (mut rects, mut images, mut glyphs, mut layers) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for layer in scene.layers() {
            let clip = match layer.clip_bounds {
                Some(clip) => (clip * scale).intersection(device_bounds),
                None => Some(device_bounds),
            };
            let Some(clip) = clip else {
                continue;
            };
            let clip = RECT {
                left: clip.origin_x().round() as i32,
                top: clip.origin_y().round() as i32,
                right: clip.max_x().round() as i32,
                bottom: clip.max_y().round() as i32,
            };
            if clip.left >= clip.right || clip.top >= clip.bottom {
                continue;
            }
            let rect_start = rects.len();
            rects.extend(rect::Pipeline::instance_data(layer, scene));
            let mut draw = LayerDraw {
                clip,
                rects: rect_start..rects.len(),
                images: Vec::new(),
                glyphs: Vec::new(),
            };
            for image in &layer.images {
                let scaled = image.bounds * scale;
                let radius = CornerRadius::from_ui_corner_radius(
                    image.corner_radius,
                    scale,
                    scaled.width().min(scaled.height()),
                );
                let (texture, result) = self.images.get_or_insert_by_asset(&image.asset, |asset| {
                    create_texture(
                        &self.device.device,
                        asset.width(),
                        asset.height(),
                        Some(asset.rgba_bytes()),
                    )
                });
                result.as_ref().map_err(Clone::clone)?;
                draw.images.push((texture, images.len()));
                images.push(ImageInstanceData::new(
                    scaled,
                    ColorModifier::Image {
                        opacity: (image.opacity * 255.) as u8,
                    },
                    radius,
                ));
            }
            for icon in &layer.icons {
                let (texture, result) = self.images.get_or_insert_by_asset(&icon.asset, |asset| {
                    create_texture(
                        &self.device.device,
                        asset.width(),
                        asset.height(),
                        Some(asset.rgba_bytes()),
                    )
                });
                result.as_ref().map_err(Clone::clone)?;
                draw.images.push((texture, images.len()));
                images.push(ImageInstanceData::new(
                    icon.bounds * scale,
                    ColorModifier::Icon { color: icon.color },
                    CornerRadius::default(),
                ));
            }
            let mut groups: HashMap<TextureId, Vec<GlyphInstanceData>> = HashMap::new();
            for glyph in &layer.glyphs {
                let position = glyph.position * scale;
                let alignment = SubpixelAlignment::new(position);
                let device = &self.device.device;
                let context = &self.device.context;
                let offset = self.glyphs.get(
                    glyph.glyph_key,
                    scale,
                    alignment,
                    &|size| create_texture(device, size as u32, size as u32, None),
                    &|region, glyph, texture| {
                        if let Ok(texture) = texture {
                            let pixels = region.pixel_region;
                            let region = D3D11_BOX {
                                left: pixels.origin_x() as u32,
                                top: pixels.origin_y() as u32,
                                right: pixels.max_x() as u32,
                                bottom: pixels.max_y() as u32,
                                front: 0,
                                back: 1,
                            };
                            // GlyphCache guarantees this region lies inside this RGBA atlas.
                            unsafe {
                                context.UpdateSubresource(
                                    &texture.texture,
                                    0,
                                    Some(&region),
                                    glyph.canvas.pixels.as_ptr().cast(),
                                    glyph.canvas.size.x() as u32 * 4,
                                    0,
                                );
                            }
                        }
                    },
                    bounds,
                    rasterize,
                );
                let offset = match offset {
                    Ok(offset) => offset,
                    Err(error) => {
                        log::warn!("Unable to get glyph out of glyph cache: {error:?}, {glyph:?}");
                        groups.clear();
                        break;
                    }
                };
                let Some(offset) = offset else {
                    continue;
                };
                let (fade_start, fade_end) = match glyph.fade {
                    Some(GlyphFade::Horizontal { start, end }) => (start, end),
                    None => (0., -1.),
                };
                groups
                    .entry(offset.texture_id)
                    .or_default()
                    .push(GlyphInstanceData::new(
                        RectF::new(
                            position - alignment.to_offset() + offset.raster_bounds.origin(),
                            offset.allocated_region.pixel_region.size().to_f32(),
                        ),
                        offset.allocated_region.uv_region,
                        fade_start * scale,
                        fade_end * scale,
                        glyph.color,
                        offset.is_emoji,
                    ));
            }
            for (texture, instances) in groups {
                let start = glyphs.len();
                glyphs.extend(instances);
                draw.glyphs.push((texture, start..glyphs.len()));
            }
            layers.push(draw);
        }
        let buffer = |bytes: &[u8]| -> Result<Option<ID3D11Buffer>> {
            if bytes.is_empty() {
                Ok(None)
            } else {
                Ok(Some(create_buffer(
                    &self.device.device,
                    bytes,
                    D3D11_BIND_VERTEX_BUFFER,
                )?))
            }
        };
        let rect_buffer = buffer(bytemuck::cast_slice(&rects))?;
        let image_buffer = buffer(bytemuck::cast_slice(&images))?;
        let glyph_buffer = buffer(bytemuck::cast_slice(&glyphs))?;
        self.device.begin_frame()?;
        for layer in layers {
            unsafe {
                self.device.context.RSSetScissorRects(Some(&[layer.clip]));
            }
            if let Some(buffer) = rect_buffer.as_ref() {
                self.device.draw(
                    &self.rect,
                    buffer,
                    layer.rects.start,
                    layer.rects.len(),
                    None,
                );
            }
            if let Some(buffer) = image_buffer.as_ref() {
                for (texture, offset) in layer.images {
                    let texture = self
                        .images
                        .get(texture)
                        .expect("Texture belongs to this frame")
                        .as_ref()
                        .map_err(Clone::clone)?;
                    self.device
                        .draw(&self.image, buffer, offset, 1, Some(texture));
                }
            }
            if let Some(buffer) = glyph_buffer.as_ref() {
                for (texture, range) in layer.glyphs {
                    let texture = self
                        .glyphs
                        .texture(&texture)
                        .expect("Glyph atlas belongs to this cache")
                        .as_ref()
                        .map_err(Clone::clone)?;
                    self.device
                        .draw(&self.glyph, buffer, range.start, range.len(), Some(texture));
                }
            }
        }
        if let Some(capture) = capture {
            match self.device.capture() {
                Ok(frame) => capture(frame),
                Err(error) => log::warn!("Frame capture failed: {error}"),
            }
        }
        if let Some(pre_present) = pre_present {
            pre_present();
        }
        self.device.present()?;
        Ok(())
    }
}

fn native_error(error: anyhow::Error) -> Error {
    if error
        .downcast_ref::<windows::core::Error>()
        .is_some_and(|error| {
            matches!(
                error.code(),
                DXGI_ERROR_DEVICE_REMOVED
                    | DXGI_ERROR_DEVICE_RESET
                    | DXGI_ERROR_DEVICE_HUNG
                    | DXGI_ERROR_DRIVER_INTERNAL_ERROR
            )
        })
    {
        Error::DeviceLost
    } else {
        Error::Native(error)
    }
}
