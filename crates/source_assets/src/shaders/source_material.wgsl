// Source-style surface: albedo x baked light (lightmap, per-vertex, or none).
#import bevy_pbr::{
    forward_io::VertexOutput,
    pbr_types,
    pbr_functions::main_pass_post_lighting_processing,
}

struct SourceMaterialParams {
    color: vec4<f32>,
    light_scale: f32,
    alpha_cutoff: f32,
    flags: u32,
    // StandardMaterialFlags bits (fog, alpha mode) for Bevy's post-lighting step
    pbr_flags: u32,
};

const FLAG_LIGHTMAP: u32 = 1u;
const FLAG_VERTEX_LIGHT: u32 = 2u;
const FLAG_UNLIT: u32 = 4u;
const FLAG_BLEND2: u32 = 8u;
const FLAG_ALPHA_TEST: u32 = 16u;
const FLAG_IGNORE_TEX_ALPHA: u32 = 32u;
const FLAG_LIGHT_ONLY: u32 = 64u;

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: SourceMaterialParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_samp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var lm_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var lm_samp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var base2_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var base2_samp: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
#ifdef VERTEX_UVS_A
    let uv = in.uv;
#else
    let uv = vec2<f32>(0.0);
#endif
    var albedo = textureSample(base_tex, base_samp, uv);
    let albedo2 = textureSample(base2_tex, base2_samp, uv);
    let lm = textureSample(lm_tex, lm_samp,
#ifdef VERTEX_UVS_B
        in.uv_b
#else
        vec2<f32>(0.0)
#endif
    ).rgb;

#ifdef VERTEX_COLORS
    if (params.flags & FLAG_BLEND2) != 0u {
        albedo = mix(albedo, albedo2, clamp(in.color.a, 0.0, 1.0));
    }
#endif

    var light = vec3<f32>(1.0);
    if (params.flags & FLAG_LIGHTMAP) != 0u {
        light = lm * params.light_scale;
    }
#ifdef VERTEX_COLORS
    if (params.flags & FLAG_VERTEX_LIGHT) != 0u {
        light = in.color.rgb * params.light_scale;
    }
#endif
    if (params.flags & FLAG_UNLIT) != 0u {
        light = vec3<f32>(1.0);
    }

    var alpha = params.color.a;
    if (params.flags & FLAG_IGNORE_TEX_ALPHA) == 0u {
        alpha = alpha * albedo.a;
    }
    if (params.flags & FLAG_ALPHA_TEST) != 0u && alpha < params.alpha_cutoff {
        discard;
    }
    var color = vec4<f32>(albedo.rgb * params.color.rgb * light, alpha);
    if (params.flags & FLAG_LIGHT_ONLY) != 0u {
        color = vec4<f32>(light, 1.0);
    }
    // Fog, in-shader tonemapping (non-HDR cameras) and alpha premultiply,
    // exactly as StandardMaterial does.
    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.world_position = in.world_position;
    pbr_input.frag_coord = in.position;
    pbr_input.material.flags = params.pbr_flags;
    return main_pass_post_lighting_processing(pbr_input, color);
}
