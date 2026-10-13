// Viewmodel shading: Source VertexLitGeneric's ambient-cube lighting (the
// way Portal 2 lights models without local lights) on top of Bevy's
// StandardMaterial inputs (base texture, normal map).
#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::main_pass_post_lighting_processing,
}

struct AmbientCube {
    // Source axis order: +X, -X, +Y, -Y, +Z, -Z (Z up), linear light.
    sides: array<vec4<f32>, 6>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> cube: AmbientCube;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    // World (Y up) normal -> Source (Z up).
    let w = normalize(pbr_input.N);
    let n = vec3(w.x, -w.z, w.y);
    let sq = n * n;
    let x = select(cube.sides[1].rgb, cube.sides[0].rgb, n.x >= 0.0);
    let y = select(cube.sides[3].rgb, cube.sides[2].rgb, n.y >= 0.0);
    let z = select(cube.sides[5].rgb, cube.sides[4].rgb, n.z >= 0.0);
    let light = x * sq.x + y * sq.y + z * sq.z;
    var out: FragmentOutput;
    let base = pbr_input.material.base_color;
    out.color = vec4(base.rgb * light, base.a);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
