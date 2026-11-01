use std::ffi::CString;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{E_INVALIDARG, HWND};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::{Error, Interface, PCSTR, Result};

use crate::platform::CapturedFrame;
use crate::rendering::GPUPowerPreference;

pub(super) struct Pipeline {
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    layout: ID3D11InputLayout,
    stride: u32,
}

pub(super) struct Texture {
    pub texture: ID3D11Texture2D,
    pub view: ID3D11ShaderResourceView,
}

pub(super) struct Device {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    swap_chain: IDXGISwapChain1,
    _composition: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    render_target: Option<ID3D11RenderTargetView>,
    quad: ID3D11Buffer,
    uniforms: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    raster: ID3D11RasterizerState,
    blend: ID3D11BlendState,
    size: (u32, u32),
    pub low_power_available: bool,
}

impl Device {
    pub fn new(
        hwnd: HWND,
        width: u32,
        height: u32,
        preference: GPUPowerPreference,
    ) -> Result<Self> {
        // These COM resources and the immediate context stay on the window's render thread.
        unsafe {
            let factory: IDXGIFactory6 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            let gpu_preference = match preference {
                GPUPowerPreference::LowPower => DXGI_GPU_PREFERENCE_MINIMUM_POWER,
                GPUPowerPreference::HighPerformance => DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
            };
            let adapter: IDXGIAdapter1 = factory.EnumAdapterByGpuPreference(0, gpu_preference)?;
            let (device, context) = create_device(&adapter)?;
            let low_power_available = low_power_gpu_available(&factory, &adapter, &device)?;
            let swap_chain = factory.CreateSwapChainForComposition(
                &device,
                &DXGI_SWAP_CHAIN_DESC1 {
                    Width: width,
                    Height: height,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: 2,
                    Scaling: DXGI_SCALING_STRETCH,
                    SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                    AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                    ..Default::default()
                },
                None,
            )?;
            let dxgi: IDXGIDevice = device.cast()?;
            dxgi.cast::<IDXGIDevice1>()?.SetMaximumFrameLatency(2)?;
            let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let target = composition.CreateTargetForHwnd(hwnd, false)?;
            let visual = composition.CreateVisual()?;
            visual.SetContent(&swap_chain)?;
            target.SetRoot(&visual)?;
            composition.Commit()?;
            let quad = create_buffer(
                &device,
                bytemuck::cast_slice(&[0f32, 0., 0., 1., 1., 0., 1., 0., 0., 1., 1., 1.]),
                D3D11_BIND_VERTEX_BUFFER,
            )?;
            let uniforms = create_buffer(
                &device,
                bytemuck::cast_slice(&[width as f32, height as f32, 0., 0.]),
                D3D11_BIND_CONSTANT_BUFFER,
            )?;
            let (mut sampler, mut raster, mut blend) = (None, None, None);
            device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    ComparisonFunc: D3D11_COMPARISON_NEVER,
                    MaxLOD: f32::MAX,
                    ..Default::default()
                },
                Some(&mut sampler),
            )?;
            device.CreateRasterizerState(
                &D3D11_RASTERIZER_DESC {
                    FillMode: D3D11_FILL_SOLID,
                    CullMode: D3D11_CULL_NONE,
                    DepthClipEnable: true.into(),
                    ScissorEnable: true.into(),
                    ..Default::default()
                },
                Some(&mut raster),
            )?;
            let mut blend_descriptor = D3D11_BLEND_DESC::default();
            blend_descriptor.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_SRC_ALPHA,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: 15,
            };
            device.CreateBlendState(&blend_descriptor, Some(&mut blend))?;
            Ok(Self {
                device,
                context,
                swap_chain,
                _composition: composition,
                _target: target,
                _visual: visual,
                render_target: None,
                quad,
                uniforms,
                sampler: sampler.expect("Successful creation returns sampler"),
                raster: raster.expect("Successful creation returns rasterizer"),
                blend: blend.expect("Successful creation returns blend state"),
                size: (width, height),
                low_power_available,
            })
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        if self.size == (width, height) {
            return Ok(());
        }
        unsafe {
            self.context.ClearState();
            self.render_target = None;
            self.swap_chain.ResizeBuffers(
                0,
                width,
                height,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }
        self.uniforms = create_buffer(
            &self.device,
            bytemuck::cast_slice(&[width as f32, height as f32, 0., 0.]),
            D3D11_BIND_CONSTANT_BUFFER,
        )?;
        self.size = (width, height);
        Ok(())
    }

    pub fn begin_frame(&mut self) -> Result<()> {
        unsafe {
            self.device.GetDeviceRemovedReason()?;
            if self.render_target.is_none() {
                let buffer: ID3D11Texture2D = self.swap_chain.GetBuffer(0)?;
                self.device
                    .CreateRenderTargetView(&buffer, None, Some(&mut self.render_target))?;
            }
            self.context
                .OMSetRenderTargets(Some(&[self.render_target.clone()]), None);
            self.context.ClearRenderTargetView(
                self.render_target
                    .as_ref()
                    .expect("Render target created above"),
                &[0.; 4],
            );
            self.context.OMSetBlendState(&self.blend, None, u32::MAX);
            self.context.RSSetState(&self.raster);
            self.context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: self.size.0 as f32,
                Height: self.size.1 as f32,
                MaxDepth: 1.,
                ..Default::default()
            }]));
            self.context
                .VSSetConstantBuffers(0, Some(&[Some(self.uniforms.clone())]));
            self.context
                .PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            self.context
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.context.IASetVertexBuffers(
                0,
                1,
                Some([Some(self.quad.clone())].as_ptr()),
                Some([8].as_ptr()),
                Some([0].as_ptr()),
            );
        }
        Ok(())
    }

    pub fn draw(
        &self,
        pipeline: &Pipeline,
        buffer: &ID3D11Buffer,
        start: usize,
        count: usize,
        texture: Option<&Texture>,
    ) {
        unsafe {
            self.context.IASetInputLayout(&pipeline.layout);
            self.context.VSSetShader(&pipeline.vertex, None);
            self.context.PSSetShader(&pipeline.pixel, None);
            self.context.IASetVertexBuffers(
                1,
                1,
                Some([Some(buffer.clone())].as_ptr()),
                Some([pipeline.stride].as_ptr()),
                Some([0].as_ptr()),
            );
            self.context
                .PSSetShaderResources(0, Some(&[texture.map(|texture| texture.view.clone())]));
            self.context.DrawInstanced(6, count as u32, 0, start as u32);
        }
    }

    pub fn present(&self) -> Result<()> {
        unsafe { self.swap_chain.Present(0, DXGI_PRESENT(0)).ok() }
    }

    pub fn capture(&self) -> Result<CapturedFrame> {
        unsafe {
            let buffer: ID3D11Texture2D = self.swap_chain.GetBuffer(0)?;
            let mut description = D3D11_TEXTURE2D_DESC::default();
            buffer.GetDesc(&mut description);
            description.Usage = D3D11_USAGE_STAGING;
            description.BindFlags = 0;
            description.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            self.device
                .CreateTexture2D(&description, None, Some(&mut staging))?;
            let staging = staging.expect("Successful creation returns staging texture");
            self.context.CopyResource(&staging, &buffer);
            self.context.Flush();
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match self.context.Map(
                    &staging,
                    0,
                    D3D11_MAP_READ,
                    D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
                    Some(&mut mapped),
                ) {
                    Ok(()) => break,
                    Err(error)
                        if error.code() == DXGI_ERROR_WAS_STILL_DRAWING
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => return Err(error),
                }
            }
            let mut rgba =
                Vec::with_capacity((description.Width * description.Height * 4) as usize);
            for row in 0..description.Height as usize {
                let bytes = std::slice::from_raw_parts(
                    mapped
                        .pData
                        .cast::<u8>()
                        .add(row * mapped.RowPitch as usize),
                    description.Width as usize * 4,
                );
                for pixel in bytes.chunks_exact(4) {
                    rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
            }
            self.context.Unmap(&staging, 0);
            Ok(CapturedFrame::new(
                description.Width,
                description.Height,
                rgba,
            ))
        }
    }
}

fn create_device(adapter: &IDXGIAdapter1) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let (mut device, mut context) = (None, None);
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    Ok((
        device.expect("Successful D3D11CreateDevice returns a device"),
        context.expect("Requested immediate context is returned on success"),
    ))
}

fn low_power_gpu_available(
    factory: &IDXGIFactory6,
    selected: &IDXGIAdapter1,
    device: &ID3D11Device,
) -> Result<bool> {
    unsafe {
        let selected = selected.GetDesc1()?.AdapterLuid;
        for index in 0.. {
            let adapter = match factory.EnumAdapters1(index) {
                Ok(adapter) => adapter,
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(error) => return Err(error),
            };
            let description = adapter.GetDesc1()?;
            if description.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let probe = if description.AdapterLuid == selected {
                device.clone()
            } else {
                create_device(&adapter)?.0
            };
            // Reserved video memory does not reliably distinguish UMA from discrete GPUs.
            let mut architecture = D3D11_FEATURE_DATA_D3D11_OPTIONS2::default();
            probe.CheckFeatureSupport(
                D3D11_FEATURE_D3D11_OPTIONS2,
                (&mut architecture as *mut D3D11_FEATURE_DATA_D3D11_OPTIONS2).cast(),
                size_of::<D3D11_FEATURE_DATA_D3D11_OPTIONS2>() as u32,
            )?;
            if architecture.UnifiedMemoryArchitecture.as_bool() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            self.context.ClearState();
            self.context.Flush();
        }
    }
}

pub(super) fn create_buffer(
    device: &ID3D11Device,
    bytes: &[u8],
    binding: D3D11_BIND_FLAG,
) -> Result<ID3D11Buffer> {
    let length = u32::try_from(bytes.len()).map_err(|_| Error::from_hresult(E_INVALIDARG))?;
    let mut buffer = None;
    unsafe {
        device.CreateBuffer(
            &D3D11_BUFFER_DESC {
                ByteWidth: length,
                Usage: D3D11_USAGE_IMMUTABLE,
                BindFlags: binding.0 as u32,
                ..Default::default()
            },
            Some(&D3D11_SUBRESOURCE_DATA {
                pSysMem: bytes.as_ptr().cast(),
                ..Default::default()
            }),
            Some(&mut buffer),
        )?;
    }
    Ok(buffer.expect("Successful CreateBuffer returns the requested buffer"))
}

pub(super) fn create_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    bytes: Option<&[u8]>,
) -> Result<Texture> {
    let mut texture = None;
    let initial = bytes.map(|bytes| D3D11_SUBRESOURCE_DATA {
        pSysMem: bytes.as_ptr().cast(),
        SysMemPitch: width * 4,
        ..Default::default()
    });
    unsafe {
        device.CreateTexture2D(
            &D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0
                    | if bytes.is_none() {
                        D3D11_BIND_RENDER_TARGET.0
                    } else {
                        0
                    }) as u32,
                ..Default::default()
            },
            initial.as_ref().map(|data| data as *const _),
            Some(&mut texture),
        )?;
        let texture = texture.expect("Successful creation returns a texture");
        if bytes.is_none() {
            // Match wgpu's zero-initialized atlas contract without a temporary 4 MiB CPU image.
            let mut clear_view = None;
            device.CreateRenderTargetView(&texture, None, Some(&mut clear_view))?;
            device.GetImmediateContext()?.ClearRenderTargetView(
                clear_view
                    .as_ref()
                    .expect("Successful creation returns clear view"),
                &[0.; 4],
            );
        }
        let mut view = None;
        device.CreateShaderResourceView(&texture, None, Some(&mut view))?;
        Ok(Texture {
            texture,
            view: view.expect("Successful creation returns a resource view"),
        })
    }
}

impl Pipeline {
    pub fn new(
        device: &ID3D11Device,
        source: &str,
        fragment: &str,
        instance: wgpu::VertexBufferLayout<'_>,
    ) -> Result<Self> {
        let compile =
            |entry: &str, target: &'static [u8]| -> Result<Vec<u8>> {
                let entry = CString::new(entry).expect("Static entry point has no null byte");
                let (mut code, mut errors) = (None, None);
                unsafe {
                    if let Err(error) = D3DCompile(
                        source.as_ptr().cast(),
                        source.len(),
                        None,
                        None,
                        None,
                        PCSTR(entry.as_ptr().cast()),
                        PCSTR(target.as_ptr()),
                        D3DCOMPILE_OPTIMIZATION_LEVEL3,
                        0,
                        &mut code,
                        Some(&mut errors),
                    ) {
                        let detail = errors
                            .map(|blob| {
                                String::from_utf8_lossy(std::slice::from_raw_parts(
                                    blob.GetBufferPointer().cast::<u8>(),
                                    blob.GetBufferSize(),
                                ))
                                .into_owned()
                            })
                            .unwrap_or_else(|| error.to_string());
                        return Err(Error::new(error.code(), detail));
                    }
                    let code = code.expect("Successful compiler returns bytecode");
                    Ok(std::slice::from_raw_parts(
                        code.GetBufferPointer().cast(),
                        code.GetBufferSize(),
                    )
                    .to_vec())
                }
            };
        let vertex_code = compile("vs_main", b"vs_5_0\0")?;
        let pixel_code = compile(fragment, b"ps_5_0\0")?;
        let mut elements = vec![D3D11_INPUT_ELEMENT_DESC {
            SemanticName: PCSTR(b"LOC\0".as_ptr()),
            Format: DXGI_FORMAT_R32G32_FLOAT,
            ..Default::default()
        }];
        for attribute in instance.attributes {
            let format = match attribute.format {
                wgpu::VertexFormat::Float32x2 => DXGI_FORMAT_R32G32_FLOAT,
                wgpu::VertexFormat::Float32x3 => DXGI_FORMAT_R32G32B32_FLOAT,
                wgpu::VertexFormat::Float32x4 => DXGI_FORMAT_R32G32B32A32_FLOAT,
                wgpu::VertexFormat::Float32 => DXGI_FORMAT_R32_FLOAT,
                wgpu::VertexFormat::Sint32 => DXGI_FORMAT_R32_SINT,
                wgpu::VertexFormat::Uint32 => DXGI_FORMAT_R32_UINT,
                _ => {
                    return Err(Error::new(
                        E_INVALIDARG,
                        "Unsupported renderer instance attribute",
                    ));
                }
            };
            elements.push(D3D11_INPUT_ELEMENT_DESC {
                SemanticName: PCSTR(b"LOC\0".as_ptr()),
                SemanticIndex: attribute.shader_location,
                Format: format,
                InputSlot: 1,
                AlignedByteOffset: attribute.offset as u32,
                InputSlotClass: D3D11_INPUT_PER_INSTANCE_DATA,
                InstanceDataStepRate: 1,
            });
        }
        let (mut vertex, mut pixel, mut layout) = (None, None, None);
        unsafe {
            device.CreateVertexShader(&vertex_code, None, Some(&mut vertex))?;
            device.CreatePixelShader(&pixel_code, None, Some(&mut pixel))?;
            device.CreateInputLayout(&elements, &vertex_code, Some(&mut layout))?;
        }
        Ok(Self {
            vertex: vertex.expect("Successful creation returns vertex shader"),
            pixel: pixel.expect("Successful creation returns pixel shader"),
            layout: layout.expect("Successful creation returns input layout"),
            stride: instance.array_stride as u32,
        })
    }
}
