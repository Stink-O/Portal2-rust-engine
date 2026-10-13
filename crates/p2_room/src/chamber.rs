//! The generated test chamber: geometry built in code, Portal 2's own chamber
//! materials read from the player's install (vmt + vtf). Nothing from the game
//! is shipped; without it the surfaces are untextured.
//!
//! Layout is written in Source units (`u`), Y up, floor top at y = 0, feet
//! positions in [`RoomSpots`]. See `README.md` for the coordinates.
use avian3d::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::math::Affine2;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use p2_core::{u, Portalable, U};
use source_assets::GameFs;

/// Portal 2 chamber textures use texture scale 0.25: 4 texels per Source unit
/// (checked against sp_a2_laser_intro and sp_a2_pit_flings texinfo). A 1024 px
/// tile therefore spans 256 u.
pub const TEXELS_PER_UNIT: f32 = 4.0;

/// Spawns the test chamber at `Startup`.
pub struct RoomPlugin;

impl Plugin for RoomPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(RoomSpots::default())
            .add_systems(Startup, setup_scene)
            .add_systems(Update, chamber_ambient_on_cameras);
    }
}

/// A place in the room: feet position (world metres), facing yaw and pitch
/// (radians; yaw 0 looks down -Z, positive yaw turns left, positive pitch looks up).
#[derive(Clone, Copy, Debug)]
pub struct RoomSpot {
    pub feet: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

/// Named places in the room for examples and scripts. All positions are in
/// Source units converted with `u()`. Feet are on the floor (y = 0) unless
/// stated. The coordinates in each field's doc comment are in Source units,
/// `(x, y, z)` with y up.
#[derive(Resource, Clone, Debug)]
pub struct RoomSpots {
    /// Player start (feet). (400, 0, 400), facing -Z.
    pub spawn: Vec3,
    /// Yaw at spawn (radians, 0 = looking down -Z).
    pub spawn_yaw: f32,
    /// Between two big white walls facing each other (x = -512 and x = +512,
    /// 1024 u apart; the east one is white at z = -64). Feet (-256, 0, -64),
    /// facing +X.
    pub recursion: RoomSpot,
    /// Rim of the deep pit with a portalable floor (pit x [-448, -160], z
    /// [160, 448], floor at y = -512, 512 u down). Feet (-304, 0, 168) on the top of
    /// the 16 u black pit wall at the lip, facing +Z, looking 60 degrees down; the
    /// pit floor centre is ~74 degrees down from the eye.
    pub pit_rim: RoomSpot,
    /// Raised white ledge (x [224, 448], z [-448, -256], 128 u high). Feet at
    /// its base (120, 0, -352), facing +X at the ledge's west face (x = 224).
    pub ledge: RoomSpot,
    /// 45 degree portalable panel rising north from (0, 0, -79) to (0, 226, -305).
    /// Feet (0, 0, 40), facing -Z, looking 20 degrees up at the slope.
    pub angled_panel: RoomSpot,
    /// Portalable ceiling patch (x [-192, 192], z [-64, 192], y = 448).
    /// Feet (0, 0, 320), facing -Z, looking 60 degrees up at the patch.
    pub ceiling: RoomSpot,
    /// White panel too small for a portal: 48 x 96 u (x [-96, -48], y [112, 208])
    /// set into the south wall at z = 512. A 64 x 112 u portal does not fit.
    /// Feet (-72, 0, 360), facing +Z, looking 10 degrees up at the panel.
    pub small_panel: RoomSpot,
    /// White east wall (x = 512) next to a black strip (z [0, 128]), so the
    /// white edge at z = 0 is a bump test. Feet (360, 0, -40), facing +X.
    pub edge_bump: RoomSpot,
    /// Black non-portalable north wall (z = -512). Feet (-200, 0, -300), facing -Z.
    pub black_wall: RoomSpot,
    /// Looking up a 1-block (128 u wide) corridor: x [128, 256], z [256, 544],
    /// walls x [96, 128] and [256, 288]. Feet (192, 0, 440), facing -Z.
    pub corridor: RoomSpot,
    /// Inside the clean room (x [-1600, -576], z [-512, 512], clean white
    /// panels; doorway from the chamber's west wall at z [288, 416]).
    /// Feet (-1088, 0, 0), facing -X at its west wall.
    pub clean_room: RoomSpot,
}

impl Default for RoomSpots {
    fn default() -> Self {
        let spot = |x: f32, y: f32, z: f32, yaw: f32, pitch: f32| RoomSpot {
            feet: uv3([x, y, z]),
            yaw,
            pitch,
        };
        let face_east = -std::f32::consts::FRAC_PI_2;
        Self {
            spawn: uv3([400.0, 0.0, 400.0]),
            spawn_yaw: 0.0,
            recursion: spot(-256.0, 0.0, -64.0, face_east, 0.0),
            pit_rim: spot(-304.0, 0.0, 168.0, std::f32::consts::PI, -60f32.to_radians()),
            ledge: spot(120.0, 0.0, -352.0, face_east, 0.0),
            angled_panel: spot(0.0, 0.0, 40.0, 0.0, 20f32.to_radians()),
            ceiling: spot(0.0, 0.0, 320.0, 0.0, 60f32.to_radians()),
            small_panel: spot(-72.0, 0.0, 360.0, std::f32::consts::PI, 10f32.to_radians()),
            edge_bump: spot(360.0, 0.0, -40.0, face_east, 0.0),
            black_wall: spot(-200.0, 0.0, -300.0, 0.0, 0.0),
            corridor: spot(192.0, 0.0, 440.0, 0.0, 0.0),
            clean_room: spot(-1088.0, 0.0, 0.0, std::f32::consts::FRAC_PI_2, 0.0),
        }
    }
}

/// Source-unit point to world metres.
fn uv3(p: [f32; 3]) -> Vec3 {
    Vec3::new(u(p[0]), u(p[1]), u(p[2]))
}

/// Source-unit box `(min, max)` to world metres.
fn ub(min: [f32; 3], max: [f32; 3]) -> (Vec3, Vec3) {
    (uv3(min), uv3(max))
}

/// Mesh for a cuboid of half extents `half` (metres, box-local axes), with
/// UVs taken from the world position so that textures line up across boxes.
/// `rot` is the box's world rotation (it sets the world normals and UVs; the
/// entity carries the same rotation).
fn cuboid_mesh(half: Vec3, centre: Vec3, rot: Quat) -> Mesh {
    let (mut pos, mut nor, mut uv, mut idx) = (vec![], vec![], vec![], vec![]);
    for i in 0..3 {
        let (j, k) = ((i + 1) % 3, (i + 2) % 3);
        for s in [-1.0f32, 1.0] {
            let mut n_local = Vec3::ZERO;
            n_local[i] = s;
            let mut corners = [Vec3::ZERO; 4];
            for (c, (a, b)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].into_iter().enumerate() {
                let mut v = Vec3::ZERO;
                v[i] = s * half[i];
                v[j] = a * half[j];
                v[k] = b * half[k];
                corners[c] = v;
            }
            let n_world = rot * n_local;
            let base = pos.len() as u32;
            for c in corners {
                pos.push(c.to_array());
                nor.push(n_local.to_array());
                uv.push(world_uv(centre + rot * c, n_world));
            }
            // Counter-clockwise seen from outside (the side the normal faces).
            let flip = (corners[1] - corners[0]).cross(corners[2] - corners[0]).dot(n_local) < 0.0;
            if flip {
                idx.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
            } else {
                idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, nor)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
        .with_inserted_indices(Indices::U32(idx))
}

/// World-aligned texture coordinates in metres. Walls get (horizontal, -y) so
/// the image stands upright; floors and ceilings get (x, z). The material's
/// `uv_transform` scales metres to texture tiles.
fn world_uv(p: Vec3, n: Vec3) -> [f32; 2] {
    let a = n.abs();
    if a.y >= a.x && a.y >= a.z {
        [p.x, p.z]
    } else if a.x >= a.z {
        [p.z, -p.y]
    } else {
        [p.x, -p.y]
    }
}

/// Spawn a static box (metres). `portalable` marks it with [`Portalable`].
pub fn spawn_box(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mat: &Handle<StandardMaterial>,
    min: Vec3,
    max: Vec3,
    portalable: bool,
    name: &str,
) -> Entity {
    spawn_oriented(commands, meshes, mat, (min + max) / 2.0, Quat::IDENTITY, (max - min) / 2.0, portalable, name)
}

/// Spawn a static box rotated by `rotation` about its `centre` (metres).
/// `half` is the half extent along the box's own axes. Used for the 45 degree
/// panel. `portalable` marks it with [`Portalable`].
pub fn spawn_slab(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mat: &Handle<StandardMaterial>,
    centre: Vec3,
    rotation: Quat,
    half: Vec3,
    portalable: bool,
    name: &str,
) -> Entity {
    spawn_oriented(commands, meshes, mat, centre, rotation, half, portalable, name)
}

fn spawn_oriented(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mat: &Handle<StandardMaterial>,
    centre: Vec3,
    rotation: Quat,
    half: Vec3,
    portalable: bool,
    name: &str,
) -> Entity {
    let size = half * 2.0;
    let mut e = commands.spawn((
        Name::new(name.to_string()),
        RigidBody::Static,
        Collider::cuboid(size.x, size.y, size.z),
        Mesh3d(meshes.add(cuboid_mesh(half, centre, rotation))),
        MeshMaterial3d(mat.clone()),
        Transform::from_translation(centre).with_rotation(rotation),
    ));
    if portalable {
        e.insert(Portalable);
    }
    e.id()
}

/// Which surface a box is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    WhiteWall,
    WhiteFloor,
    WhiteCeiling,
    CleanWall,
    CleanFloor,
    CleanCeiling,
    BlackWall,
    BlackFloor,
    BlackCeiling,
    MarkerRed,
    MarkerGreen,
}

impl Kind {
    fn portalable(self) -> bool {
        matches!(
            self,
            Kind::WhiteWall | Kind::WhiteFloor | Kind::WhiteCeiling | Kind::CleanWall | Kind::CleanFloor | Kind::CleanCeiling
        )
    }
}

struct Surfaces {
    white_wall: Handle<StandardMaterial>,
    white_floor: Handle<StandardMaterial>,
    white_ceiling: Handle<StandardMaterial>,
    clean_wall: Handle<StandardMaterial>,
    clean_floor: Handle<StandardMaterial>,
    clean_ceiling: Handle<StandardMaterial>,
    black_wall: Handle<StandardMaterial>,
    black_floor: Handle<StandardMaterial>,
    black_ceiling: Handle<StandardMaterial>,
    red: Handle<StandardMaterial>,
    green: Handle<StandardMaterial>,
}

impl Surfaces {
    fn get(&self, k: Kind) -> &Handle<StandardMaterial> {
        match k {
            Kind::WhiteWall => &self.white_wall,
            Kind::WhiteFloor => &self.white_floor,
            Kind::WhiteCeiling => &self.white_ceiling,
            Kind::CleanWall => &self.clean_wall,
            Kind::CleanFloor => &self.clean_floor,
            Kind::CleanCeiling => &self.clean_ceiling,
            Kind::BlackWall => &self.black_wall,
            Kind::BlackFloor => &self.black_floor,
            Kind::BlackCeiling => &self.black_ceiling,
            Kind::MarkerRed => &self.red,
            Kind::MarkerGreen => &self.green,
        }
    }
}

/// Repeat sampling with anisotropy, as the game's textures are tiled.
fn tiling_sampler() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        anisotropy_clamp: 16,
        ..ImageSamplerDescriptor::linear()
    })
}

/// Portal 2's base texture for `name` (a material path such as
/// `tile/white_wall_tile004c`), and its width in texels.
fn source_texture(fs: &GameFs, name: &str) -> Option<(Image, u32)> {
    let (_, kv) = source_assets::vmt::load_vmt(&|p| fs.read(p), name)?;
    let base = kv.get("$basetexture")?;
    let data = fs.read(&format!("materials/{}.vtf", source_assets::vmt::canonical_name(base)))?;
    let vtf = source_assets::vtf::VtfTexture::parse(&data).ok()?;
    let width = vtf.width;
    let mut img = vtf.into_image(false, true);
    img.sampler = tiling_sampler();
    Some((img, width))
}

/// A material with the game's texture. The UVs are in metres, so
/// `uv_transform` scales them to one texture tile per `tile` metres (256 u for
/// a 1024 px chamber tile). Untextured if the game does not provide it.
fn surface(
    fs: Option<&GameFs>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    source_name: &str,
    roughness: f32,
) -> Handle<StandardMaterial> {
    let mut mat = StandardMaterial { perceptual_roughness: roughness, ..default() };
    match fs.and_then(|fs| source_texture(fs, source_name)) {
        Some((img, width)) => {
            debug!("room material {source_name}: {width} px");
            mat.base_color_texture = Some(images.add(img));
            mat.uv_transform = Affine2::from_scale(Vec2::splat(1.0 / (width as f32 / TEXELS_PER_UNIT * U)));
        }
        None if fs.is_some() => error!("room material {source_name} not found in the Portal 2 install"),
        None => {}
    }
    materials.add(mat)
}

/// Builds boxes from Source-unit coordinates.
struct Chamber<'a, 'w, 's> {
    commands: &'a mut Commands<'w, 's>,
    meshes: &'a mut Assets<Mesh>,
    mats: &'a Surfaces,
}

impl Chamber<'_, '_, '_> {
    fn cube(&mut self, min: [f32; 3], max: [f32; 3], kind: Kind, name: &str) -> Entity {
        let (a, b) = ub(min, max);
        spawn_box(self.commands, self.meshes, self.mats.get(kind), a, b, kind.portalable(), name)
    }

    /// A wall, floor or ceiling panel `min..max` (thin along `thick`) with a
    /// rectangular hole `hole_min..hole_max` filled with `hole_kind`.
    fn panel_with_hole(
        &mut self,
        min: [f32; 3],
        max: [f32; 3],
        kind: Kind,
        hole_min: [f32; 3],
        hole_max: [f32; 3],
        hole_kind: Kind,
        name: &str,
    ) {
        // Thin axis: the one with the least extent. In-plane axes a, b.
        let thick = (0..3).min_by(|&i, &j| (max[i] - min[i]).total_cmp(&(max[j] - min[j]))).unwrap();
        let (a, b) = ((thick + 1) % 3, (thick + 2) % 3);
        let piece = |a_lo: f32, a_hi: f32, b_lo: f32, b_hi: f32| {
            let (mut lo, mut hi) = (min, max);
            lo[a] = a_lo;
            hi[a] = a_hi;
            lo[b] = b_lo;
            hi[b] = b_hi;
            (lo, hi)
        };
        let (ha0, ha1, hb0, hb1) = (hole_min[a], hole_max[a], hole_min[b], hole_max[b]);
        let pieces = [
            piece(min[a], ha0, min[b], max[b]),
            piece(ha1, max[a], min[b], max[b]),
            piece(ha0, ha1, min[b], hb0),
            piece(ha0, ha1, hb1, max[b]),
        ];
        for (i, (lo, hi)) in pieces.into_iter().enumerate() {
            self.cube(lo, hi, kind, &format!("{name} piece {i}"));
        }
        self.cube(hole_min, hole_max, hole_kind, &format!("{name} insert"));
    }
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let fs = p2_assets::find_portal2().ok();
    if fs.is_none() {
        error!("Portal 2 not found: the chamber is untextured");
    }
    let fs = fs.as_ref();
    let mats = Surfaces {
        white_wall: surface(fs, &mut images, &mut materials, "tile/white_wall_tile004c", 0.6),
        white_floor: surface(fs, &mut images, &mut materials, "tile/white_floor_tile004d", 0.65),
        white_ceiling: surface(fs, &mut images, &mut materials, "tile/ceiling_tile002b", 0.7),
        // The clean room: Portal 2's new-chamber white panels.
        clean_wall: surface(fs, &mut images, &mut materials, "tile/white_wall_tile003a", 0.5),
        clean_floor: surface(fs, &mut images, &mut materials, "tile/white_floor_tile002a", 0.55),
        clean_ceiling: surface(fs, &mut images, &mut materials, "tile/white_ceiling_tile002a", 0.6),
        black_wall: surface(fs, &mut images, &mut materials, "metal/black_wall_metal_002b", 0.55),
        black_floor: surface(fs, &mut images, &mut materials, "metal/black_floor_metal_004b", 0.6),
        black_ceiling: surface(fs, &mut images, &mut materials, "metal/black_ceiling_metal_001a", 0.6),
        red: materials.add(StandardMaterial { base_color: Color::srgb(0.62, 0.14, 0.11), perceptual_roughness: 0.7, ..default() }),
        green: materials.add(StandardMaterial { base_color: Color::srgb(0.22, 0.52, 0.26), perceptual_roughness: 0.7, ..default() }),
    };
    let mut c = Chamber { commands: &mut commands, meshes: &mut meshes, mats: &mats };
    build_chamber(&mut c);
    drop(c);
    if let Some(fs) = fs {
        spawn_cube(&mut commands, &mut meshes, &mut materials, &mut images, fs, uv3([250.0, 0.0, 150.0]));
    }

    // Ceiling lights, neutral white (no garish colour); the last two light the clean room.
    for p in [
        [-300.0, 400.0, -200.0],
        [300.0, 400.0, -200.0],
        [-300.0, 400.0, 300.0],
        [300.0, 400.0, 300.0],
        [-1088.0, 400.0, -256.0],
        [-1088.0, 400.0, 256.0],
    ] {
        commands.spawn((
            PointLight {
                color: Color::srgb(1.0, 0.99, 0.97),
                intensity: 1_000_000.0,
                range: 60.0,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_translation(uv3(p)),
        ));
    }
}

/// Every chamber box, in Source units. Coordinates are `(x, y, z)`, y up,
/// floor top y = 0, ceiling underside y = 448, outer walls 32 u thick.
fn build_chamber(c: &mut Chamber) {
    // Floor, around the pit (x [-448,-160], z [160,448]). The corridor floor
    // (black) takes x [128,256], z [256,544].
    c.cube([-544.0, -32.0, -544.0], [544.0, 0.0, 160.0], Kind::WhiteFloor, "floor north");
    c.cube([-544.0, -32.0, 160.0], [-448.0, 0.0, 448.0], Kind::WhiteFloor, "floor pit west");
    c.cube([-160.0, -32.0, 160.0], [544.0, 0.0, 256.0], Kind::WhiteFloor, "floor pit east north");
    c.cube([-160.0, -32.0, 256.0], [128.0, 0.0, 448.0], Kind::WhiteFloor, "floor pit east south");
    c.cube([256.0, -32.0, 256.0], [544.0, 0.0, 448.0], Kind::WhiteFloor, "floor corridor east");
    c.cube([-544.0, -32.0, 448.0], [128.0, 0.0, 544.0], Kind::WhiteFloor, "floor south west");
    c.cube([256.0, -32.0, 448.0], [544.0, 0.0, 544.0], Kind::WhiteFloor, "floor south east");
    c.cube([128.0, -32.0, 256.0], [256.0, 0.0, 544.0], Kind::BlackFloor, "floor corridor");

    // The pit: 512 u deep, portalable floor, black walls (no portals).
    c.cube([-448.0, -544.0, 160.0], [-160.0, -512.0, 448.0], Kind::WhiteFloor, "pit floor");
    c.cube([-448.0, -512.0, 160.0], [-160.0, 0.0, 176.0], Kind::BlackWall, "pit wall north");
    c.cube([-448.0, -512.0, 432.0], [-160.0, 0.0, 448.0], Kind::BlackWall, "pit wall south");
    c.cube([-448.0, -512.0, 176.0], [-432.0, 0.0, 432.0], Kind::BlackWall, "pit wall west");
    c.cube([-176.0, -512.0, 176.0], [-160.0, 0.0, 432.0], Kind::BlackWall, "pit wall east");

    // Outer shell. West wall: white, with the 128 x 192 u doorway to the clean
    // room at z [288, 416]. East wall: white, black edge strip at z [0,128], white.
    c.cube([-544.0, 0.0, -544.0], [-512.0, 448.0, 288.0], Kind::WhiteWall, "wall west north");
    c.cube([-544.0, 0.0, 416.0], [-512.0, 448.0, 544.0], Kind::WhiteWall, "wall west south");
    c.cube([-544.0, 192.0, 288.0], [-512.0, 448.0, 416.0], Kind::WhiteWall, "wall west lintel");
    c.cube([512.0, 0.0, -544.0], [544.0, 448.0, 0.0], Kind::WhiteWall, "wall east north");
    c.cube([512.0, 0.0, 0.0], [544.0, 448.0, 128.0], Kind::BlackWall, "wall east black edge");
    c.cube([512.0, 0.0, 128.0], [544.0, 448.0, 544.0], Kind::WhiteWall, "wall east south");
    c.cube([-512.0, 0.0, -544.0], [512.0, 448.0, -512.0], Kind::BlackWall, "wall north");

    // South wall: black, with a 48 x 96 u white insert too small for a portal.
    c.panel_with_hole(
        [-512.0, 0.0, 512.0],
        [512.0, 448.0, 544.0],
        Kind::BlackWall,
        [-96.0, 112.0, 512.0],
        [-48.0, 208.0, 544.0],
        Kind::WhiteWall,
        "wall south",
    );

    // Ceiling: black, with a white portalable patch.
    c.cube([-544.0, 448.0, -544.0], [544.0, 480.0, -64.0], Kind::BlackCeiling, "ceiling north");
    c.cube([-544.0, 448.0, 192.0], [544.0, 480.0, 544.0], Kind::BlackCeiling, "ceiling south");
    c.cube([-544.0, 448.0, -64.0], [-192.0, 480.0, 192.0], Kind::BlackCeiling, "ceiling west");
    c.cube([192.0, 448.0, -64.0], [544.0, 480.0, 192.0], Kind::BlackCeiling, "ceiling east");
    c.cube([-192.0, 448.0, -64.0], [192.0, 480.0, 192.0], Kind::WhiteCeiling, "ceiling patch");

    // Raised ledge, 128 u high, white.
    c.cube([224.0, 0.0, -448.0], [448.0, 128.0, -256.0], Kind::WhiteWall, "ledge");

    // 45 degree panel: rises north from (0,0,-79) to (0,226,-305). Half extents
    // (128 across, 16 thick, 160 long) rotated +45 degrees about X.
    let slope = Quat::from_rotation_x(std::f32::consts::FRAC_PI_4);
    spawn_slab(
        c.commands,
        c.meshes,
        c.mats.get(Kind::WhiteWall),
        uv3([0.0, 113.0, -192.0]),
        slope,
        uv3([128.0, 16.0, 160.0]),
        true,
        "angled panel",
    );

    // The corridor's side walls (1 block = 128 u wide between them).
    c.cube([96.0, 0.0, 256.0], [128.0, 448.0, 512.0], Kind::BlackWall, "corridor west");
    c.cube([256.0, 0.0, 256.0], [288.0, 448.0, 512.0], Kind::BlackWall, "corridor east");

    build_clean_room(c);

    // Colour markers, small so the greybox stays chamber-like.
    c.cube([-448.0, 0.0, -448.0], [-384.0, 64.0, -384.0], Kind::MarkerRed, "marker red");
    c.cube([368.0, 0.0, 224.0], [432.0, 64.0, 288.0], Kind::MarkerGreen, "marker green");
}

/// The clean room west of the chamber: interior x [-1600, -576], z [-512, 512],
/// y [0, 448], every surface Portal 2's clean white panels (all portalable).
/// Its east wall (x [-576, -544]) backs the chamber's west wall, and the
/// doorway through both is at z [288, 416], 192 u high.
fn build_clean_room(c: &mut Chamber) {
    c.cube([-1632.0, -32.0, -544.0], [-576.0, 0.0, 544.0], Kind::CleanFloor, "clean floor");
    c.cube([-576.0, -32.0, 288.0], [-544.0, 0.0, 416.0], Kind::CleanFloor, "clean threshold");
    c.cube([-1632.0, 448.0, -544.0], [-544.0, 480.0, 544.0], Kind::CleanCeiling, "clean ceiling");
    c.cube([-1632.0, 0.0, -544.0], [-1600.0, 448.0, 544.0], Kind::CleanWall, "clean wall west");
    c.cube([-1600.0, 0.0, -544.0], [-576.0, 448.0, -512.0], Kind::CleanWall, "clean wall north");
    c.cube([-1600.0, 0.0, 512.0], [-576.0, 448.0, 544.0], Kind::CleanWall, "clean wall south");
    c.cube([-576.0, 0.0, -512.0], [-544.0, 448.0, 288.0], Kind::CleanWall, "clean wall east north");
    c.cube([-576.0, 0.0, 416.0], [-544.0, 448.0, 512.0], Kind::CleanWall, "clean wall east south");
    c.cube([-576.0, 192.0, 288.0], [-544.0, 448.0, 416.0], Kind::CleanWall, "clean wall east lintel");
}

/// Portal 2's Weighted Storage Cube (`models/props/metal_box.mdl`) as a
/// dynamic physics box standing on the floor at `feet`, with the game's own
/// textures. The collider is the model's bounding box.
fn spawn_cube(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    fs: &GameFs,
    feet: Vec3,
) {
    const MODEL: &str = "models/props/metal_box.mdl";
    let read = |p: &str| fs.read(p);
    let exists = |m: &str| fs.exists(&format!("materials/{m}.vmt"));
    let model = match source_assets::models::load_model(MODEL, &read, &exists) {
        Ok(m) => m,
        Err(e) => {
            error!("p2_room: {MODEL}: {e}");
            return;
        }
    };
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in model.meshes.iter().flat_map(|m| m.positions.iter()) {
        lo = lo.min(Vec3::from(*p));
        hi = hi.max(Vec3::from(*p));
    }
    debug!("p2_room: cube {MODEL}: {} meshes, bounds {lo:?}..{hi:?}", model.meshes.len());
    if lo.x > hi.x {
        return;
    }
    let centre = (lo + hi) * 0.5;
    let mut tex = |vmt: &source_assets::kv::Kv, key: &str, srgb: bool| {
        let t = vmt.get(key)?;
        let data = fs.read(&format!("materials/{}.vtf", source_assets::vmt::canonical_name(t)))?;
        Some(images.add(source_assets::vtf::VtfTexture::parse(&data).ok()?.into_image(false, srgb)))
    };
    commands
        .spawn((
            Name::new("weighted storage cube"),
            RigidBody::Dynamic,
            Collider::cuboid(hi.x - lo.x, hi.y - lo.y, hi.z - lo.z),
            Transform::from_translation(feet + Vec3::Y * (centre.y - lo.y)),
            Visibility::default(),
        ))
        .with_children(|parent| {
            for m in &model.meshes {
                let name = model.material_for(0, m.material_index).unwrap_or("");
                let (_, vmt) = source_assets::vmt::load_vmt(&read, name).unwrap_or_default();
                let material = StandardMaterial {
                    base_color_texture: tex(&vmt, "$basetexture", true),
                    normal_map_texture: tex(&vmt, "$bumpmap", false),
                    flip_normal_map_y: true,
                    perceptual_roughness: 0.45,
                    ..default()
                };
                let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
                    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.positions.clone())
                    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals.clone())
                    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, m.uvs.clone())
                    .with_inserted_indices(Indices::U32(m.indices.clone()));
                let mesh = match mesh.clone().with_generated_tangents() {
                    Ok(t) => t,
                    Err(_) => mesh,
                };
                parent.spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(materials.add(material)),
                    Transform::from_translation(-centre),
                ));
            }
        });
}

/// Portal 2's chamber light, applied to every camera as it appears (the
/// viewmodel and portal cameras get the same ambient as the main view).
fn chamber_ambient_on_cameras(mut commands: Commands, cams: Query<Entity, (Added<Camera3d>, Without<AmbientLight>)>) {
    for e in &cams {
        commands.entity(e).insert(AmbientLight {
            color: Color::srgb(1.0, 1.0, 0.98),
            brightness: 300.0,
            affects_lightmapped_meshes: true,
        });
    }
}
