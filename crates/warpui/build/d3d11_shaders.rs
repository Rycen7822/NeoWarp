use std::path::PathBuf;
use std::{env, fs};

use naga::back::hlsl;

/// Keep Direct3D11 shader math sourced from the same WGSL as the wgpu renderer.
pub fn generate() {
    let output_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    for name in ["rect", "glyph", "image"] {
        let source_path = format!("src/rendering/wgpu/shaders/{name}_shader.wgsl");
        println!("cargo:rerun-if-changed={source_path}");
        let source = fs::read_to_string(&source_path).expect("Read renderer WGSL");
        let module = naga::front::wgsl::parse_str(&source).expect("Parse renderer WGSL");
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("Validate renderer WGSL");
        let mut options = hlsl::Options {
            shader_model: hlsl::ShaderModel::V5_0,
            fake_missing_bindings: false,
            ..Default::default()
        };
        for (group, binding) in [(0, 0), (1, 0), (1, 1)] {
            options.binding_map.insert(
                naga::ResourceBinding { group, binding },
                hlsl::BindTarget::default(),
            );
        }
        options.sampler_buffer_binding_map.insert(
            hlsl::SamplerIndexBufferKey { group: 1 },
            hlsl::BindTarget {
                register: 1,
                ..Default::default()
            },
        );
        let mut output = String::new();
        hlsl::Writer::new(&mut output, &options, &hlsl::PipelineOptions::default())
            .write(&module, &info, None)
            .expect("Generate renderer HLSL");

        if name != "rect" {
            // These pipelines each have one sampler. Naga emits DX12 sampler-heap indirection;
            // SM5 binds that same sampler directly, leaving the generated shader math intact.
            let sampler = if name == "glyph" {
                "glyphAtlasSampler"
            } else {
                "imageSampler"
            };
            let indirect = format!(
                "static const SamplerState {sampler} = nagaSamplerHeap[nagaGroup1SamplerIndexArray[0]];"
            );
            assert_eq!(
                output.matches(&indirect).count(),
                1,
                "Sampler binding changed"
            );
            output = output
                .lines()
                .filter(|line| {
                    !line.starts_with("SamplerState nagaSamplerHeap")
                        && !line.starts_with("SamplerComparisonState nagaComparisonSamplerHeap")
                        && !line.starts_with("StructuredBuffer<uint> nagaGroup1SamplerIndexArray")
                })
                .collect::<Vec<_>>()
                .join("\n");
            output = output.replace(
                &indirect,
                &format!("SamplerState {sampler} : register(s0);"),
            );
            assert!(!output.contains("nagaSamplerHeap"), "Unbound sampler heap");
        }
        fs::write(output_dir.join(format!("d3d11-{name}.hlsl")), output)
            .expect("Write renderer HLSL");
    }
}
