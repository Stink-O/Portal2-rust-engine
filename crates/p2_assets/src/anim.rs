//! Studio model (MDL v44-49) animation sampling at any time, on top of
//! `source_assets::skeleton::Skeleton` (which only decodes frame 0).
//!
//! TODO(assets): this belongs in `crates/source_assets`; it is here until
//! that crate exposes per-frame sampling.
//!
//! Both animation encodings are read: the classic run-length one (bone by
//! frame, the viewmodel) and the newer frame-major one (`STUDIO_FRAMEANIM`,
//! the Portal 2 player animations). Animations stored in the external `.ani`
//! file are read once [`Animator::set_anim_blocks`] has been given its bytes.
use std::sync::Arc;

use bevy::math::{Quat, Vec3};
use source_assets::skeleton::{radian_euler_to_quat, Skeleton};

fn i32_at(d: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}
fn i16_at(d: &[u8], o: usize) -> Option<i16> {
    Some(i16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}
fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}
fn f32_at(d: &[u8], o: usize) -> Option<f32> {
    Some(f32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}
fn cstr(d: &[u8], o: usize) -> String {
    let s = d.get(o..).unwrap_or(&[]);
    let end = s.iter().position(|&c| c == 0).unwrap_or(0);
    String::from_utf8_lossy(&s[..end]).to_string()
}

fn half(h: u16) -> f32 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f32;
    match e {
        0 => s * m * 2f32.powi(-24),
        31 => s * f32::INFINITY,
        _ => s * (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    }
}

fn quat48(d: &[u8], o: usize) -> Option<Quat> {
    let a = u16_at(d, o)? as f32;
    let b = u16_at(d, o + 2)? as f32;
    let c = u16_at(d, o + 4)?;
    let x = (a - 32768.0) / 32768.0;
    let y = (b - 32768.0) / 32768.0;
    let z = ((c & 0x7fff) as f32 - 16384.0) / 16384.0;
    let mut w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
    if c & 0x8000 != 0 {
        w = -w;
    }
    Some(Quat::from_xyzw(x, y, z, w))
}

fn quat64(d: &[u8], o: usize) -> Option<Quat> {
    let v = u64::from_le_bytes(d.get(o..o + 8)?.try_into().ok()?);
    let f = |bits: u64| (bits as f64 - 1048576.0) / 1048576.5;
    let x = f(v & 0x1f_ffff);
    let y = f((v >> 21) & 0x1f_ffff);
    let z = f((v >> 42) & 0x1f_ffff);
    let mut w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
    if v >> 63 != 0 {
        w = -w;
    }
    Some(Quat::from_xyzw(x as f32, y as f32, z as f32, w as f32))
}

/// Three half floats (Source `Vector48`).
fn vec48(d: &[u8], o: usize) -> Option<Vec3> {
    Some(Vec3::new(half(u16_at(d, o)?), half(u16_at(d, o + 2)?), half(u16_at(d, o + 4)?)))
}

/// Value of an `mstudioanimvalue_t` run at `frame`.
fn anim_value(d: &[u8], mut o: usize, frame: i32) -> Option<f32> {
    let mut k = frame.max(0);
    loop {
        let valid = *d.get(o)? as i32;
        let total = *d.get(o + 1)? as i32;
        if total == 0 {
            return Some(0.0);
        }
        if total > k {
            let idx = if valid > k { k + 1 } else { valid };
            return Some(i16_at(d, o + idx as usize * 2)? as f32);
        }
        k -= total;
        o += (valid as usize + 1) * 2;
    }
}

/// Decodes one frame of a frame-major animation into `local`. Layout: a
/// 24-byte header (constants offset, frame-data offset, frame record
/// length), one flag byte per bone, the constant values, then one record
/// per frame. Per bone, a constant rotation (Quaternion48) comes before a
/// constant position (Vector48); in a frame record an animated rotation
/// (Quaternion48) comes before an animated position (Vector48 or, with
/// the full-precision flag, three floats). Bones with no rotation or
/// position data keep the value `local` was initialised with.
fn frame_anim(d: &[u8], p: usize, frames: i32, frame: i32, local: &mut [(Vec3, Quat)]) {
    const CONST_POS: u8 = 0x01;
    const CONST_ROT: u8 = 0x02;
    const ANIM_POS: u8 = 0x04;
    const ANIM_ROT: u8 = 0x08;
    const FULL_POS: u8 = 0x10;
    let (Some(co), Some(fo), Some(len)) = (i32_at(d, p), i32_at(d, p + 4), i32_at(d, p + 8)) else { return };
    let n = local.len();
    let Some(flags) = d.get(p + 24..p + 24 + n) else { return };
    let frame = frame.clamp(0, (frames - 1).max(0)) as usize;
    let mut c = p + co.max(0) as usize;
    let mut f = p + fo.max(0) as usize + frame * len.max(0) as usize;
    for (b, &x) in flags.iter().enumerate() {
        if x & CONST_ROT != 0 {
            if let Some(q) = quat48(d, c) {
                local[b].1 = q;
            }
            c += 6;
        }
        if x & CONST_POS != 0 {
            if let Some(v) = vec48(d, c) {
                local[b].0 = v;
            }
            c += 6;
        }
        if x & ANIM_ROT != 0 {
            if let Some(q) = quat48(d, f) {
                local[b].1 = q;
            }
            f += 6;
        }
        if x & ANIM_POS != 0 {
            if let Some(v) = vec48(d, f) {
                local[b].0 = v;
            }
            f += 6;
        }
        if x & FULL_POS != 0 {
            if let (Some(vx), Some(vy), Some(vz)) = (f32_at(d, f), f32_at(d, f + 4), f32_at(d, f + 8)) {
                local[b].0 = Vec3::new(vx, vy, vz);
            }
            f += 12;
        }
    }
}


#[derive(Clone, Debug)]
struct AnimDesc {
    fps: f32,
    frames: i32,
    /// STUDIO_DELTA: values are offsets from identity / zero, not a pose.
    delta: bool,
    /// STUDIO_FRAMEANIM: frame-major data (per-bone flags, constants, then a
    /// fixed-size record per frame) instead of per-bone run-length tracks.
    frame_anim: bool,
    /// Animation block (0 = in the MDL, else a block of the `.ani` file) and
    /// the data offset (from the descriptor in the MDL, from the block start
    /// in the `.ani`).
    block: i32,
    index: i32,
    /// (section frames, offset of the section table).
    sections: Option<(i32, usize)>,
    base: usize,
}

#[derive(Clone, Debug)]
pub struct SeqInfo {
    pub label: String,
    pub activity: String,
    pub looping: bool,
    /// STUDIO_DELTA: an additive layer (applied on top of a base pose).
    pub delta: bool,
    /// STUDIO_POST: a delta applied in each bone's local space (after the base
    /// rotation) rather than before it; the aim matrices and gestures use it.
    pub post: bool,
    anim: Option<usize>,
    /// Blend grid: `groupsize[0] x groupsize[1]` animations, row-major by axis 1.
    grid: [usize; 2],
    grid_anims: Vec<Option<usize>>,
    /// Pose parameter driving each grid axis, and its range across the grid.
    params: [Option<usize>; 2],
    param_start: [f32; 2],
    param_end: [f32; 2],
    /// Sequences this one always layers on top (e.g. the aim matrix).
    pub autolayers: Vec<usize>,
    /// Per-bone weight of this sequence when used as a layer.
    weights: Vec<f32>,
}

/// A pose parameter of the model (e.g. `move_x`, `body_yaw`).
#[derive(Clone, Debug)]
pub struct PoseParam {
    pub name: String,
    pub start: f32,
    pub end: f32,
}

/// Samples bone poses of a studio model.
#[derive(Clone)]
pub struct Animator {
    pub skeleton: Skeleton,
    pub sequences: Vec<SeqInfo>,
    anims: Vec<AnimDesc>,
    data: Arc<Vec<u8>>,
    /// The model's pose parameters, by index.
    pub pose_params: Vec<PoseParam>,
    /// Path of the external animation file (`.ani`) when the model has one.
    pub anim_block_file: Option<String>,
    /// (start, end) of each animation block in the `.ani` file.
    blocks: Vec<(usize, usize)>,
    ani: Option<Arc<Vec<u8>>>,
}

impl Animator {
    pub fn parse(data: Arc<Vec<u8>>) -> Option<Self> {
        let skeleton = Skeleton::parse(data.clone())?;
        let d = &data[..];
        let num_anim = i32_at(d, 180)? as usize;
        let anim_index = i32_at(d, 184)? as usize;
        let num_seq = i32_at(d, 188)? as usize;
        let seq_index = i32_at(d, 192)? as usize;
        let mut anims = Vec::with_capacity(num_anim);
        for i in 0..num_anim {
            let a = anim_index + i * 100;
            let fps = f32_at(d, a + 8)?;
            let aflags = i32_at(d, a + 12)?;
            let frames = i32_at(d, a + 16)?;
            let block = i32_at(d, a + 52)?;
            let index = i32_at(d, a + 56)?;
            let section_index = i32_at(d, a + 80)?;
            let section_frames = i32_at(d, a + 84)?;
            let sections = (section_frames != 0 && section_index != 0).then_some((section_frames, a + section_index as usize));
            anims.push(AnimDesc {
                fps,
                frames,
                delta: aflags & 0x4 != 0,
                frame_anim: aflags & 0x40 != 0,
                block,
                index,
                sections,
                base: a,
            });
        }
        let nbones = skeleton.bones.len();
        let mut sequences = Vec::with_capacity(num_seq);
        for i in 0..num_seq {
            let s = seq_index + i * 212;
            let label = cstr(d, s + i32_at(d, s + 4)? as usize);
            let activity = cstr(d, s + i32_at(d, s + 8)? as usize);
            let flags = i32_at(d, s + 12)?;
            let aii = i32_at(d, s + 60)?;
            let g0 = i32_at(d, s + 68)?.max(1) as usize;
            let g1 = i32_at(d, s + 72)?.max(1) as usize;
            let anim_at = |k: usize| i16_at(d, s + aii as usize + 2 * k).filter(|&a| a >= 0 && (a as usize) < num_anim).map(|a| a as usize);
            let grid_anims: Vec<Option<usize>> = (0..g0 * g1).map(anim_at).collect();
            let anim = grid_anims.first().copied().flatten();
            let pidx = |o: usize| i32_at(d, s + o).filter(|&p| p >= 0).map(|p| p as usize);
            let params = [pidx(76), pidx(80)];
            let param_start = [f32_at(d, s + 84)?, f32_at(d, s + 88)?];
            let param_end = [f32_at(d, s + 92)?, f32_at(d, s + 96)?];
            let nal = i32_at(d, s + 148)?.max(0) as usize;
            let ali = i32_at(d, s + 152)? as usize;
            let autolayers = (0..nal).filter_map(|j| i16_at(d, s + ali + j * 24).filter(|&q| q >= 0).map(|q| q as usize)).collect();
            let wli = i32_at(d, s + 156)?;
            let weights = if wli > 0 {
                (0..nbones).map(|b| f32_at(d, s + wli as usize + 4 * b).unwrap_or(1.0)).collect()
            } else {
                vec![1.0; nbones]
            };
            sequences.push(SeqInfo {
                label,
                activity,
                looping: flags & 1 != 0,
                delta: flags & 0x4 != 0,
                post: flags & 0x10 != 0,
                anim,
                grid: [g0, g1],
                grid_anims,
                params,
                param_start,
                param_end,
                autolayers,
                weights,
            });
        }
        let npp = i32_at(d, 300)?.max(0) as usize;
        let ppi = i32_at(d, 304)? as usize;
        let pose_params = (0..npp)
            .map(|k| {
                let o = ppi + k * 20;
                PoseParam { name: cstr(d, o + i32_at(d, o).unwrap_or(0) as usize), start: f32_at(d, o + 8).unwrap_or(0.0), end: f32_at(d, o + 12).unwrap_or(1.0) }
            })
            .collect();
        let nblocks = i32_at(d, 352).unwrap_or(0).max(0) as usize;
        let block_index = i32_at(d, 356).unwrap_or(0).max(0) as usize;
        let blocks = (0..nblocks)
            .map(|k| {
                let o = block_index + k * 8;
                (i32_at(d, o).unwrap_or(0).max(0) as usize, i32_at(d, o + 4).unwrap_or(0).max(0) as usize)
            })
            .collect();
        let anim_block_file = (nblocks > 0)
            .then(|| i32_at(d, 348).filter(|&o| o > 0).map(|o| cstr(d, o as usize)))
            .flatten()
            .filter(|n| !n.is_empty());
        Some(Self { skeleton, sequences, anims, data, pose_params, anim_block_file, blocks, ani: None })
    }

    /// Gives the animator the bytes of its external animation file
    /// ([`Animator::anim_block_file`]). Until then, animations stored there
    /// sample as the rest pose.
    pub fn set_anim_blocks(&mut self, ani: Arc<Vec<u8>>) {
        self.ani = Some(ani);
    }

    /// The buffer and absolute offset of animation data at (`block`, `index`):
    /// block 0 is the MDL itself (offset from `base`), any other block lives in
    /// the `.ani` file (offset from the block start).
    fn locate(&self, block: i32, index: i32, base: usize) -> Option<(&[u8], usize)> {
        if block == 0 {
            return (index != 0).then_some((&self.data[..], base + index as usize));
        }
        let (start, end) = *self.blocks.get(block as usize)?;
        let ani = self.ani.as_ref()?;
        let at = start + index.max(0) as usize;
        (at < end && end <= ani.len()).then_some((&ani[..], at))
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.sequences.iter().position(|s| s.label.eq_ignore_ascii_case(name))
    }

    /// Duration of a sequence in seconds.
    pub fn duration(&self, seq: usize) -> f32 {
        let Some(a) = self.sequences.get(seq).and_then(|s| s.anim).and_then(|a| self.anims.get(a)) else {
            return 0.0;
        };
        ((a.frames - 1).max(1)) as f32 / a.fps.max(1.0)
    }

    /// Parent-space (position, rotation) of every bone at `time` seconds into
    /// `seq`, interpolating between frames.
    pub fn sample(&self, seq: usize, time: f32) -> Vec<(Vec3, Quat)> {
        let bones = &self.skeleton.bones;
        let mut out: Vec<(Vec3, Quat)> = bones.iter().map(|b| (b.pos, b.quat)).collect();
        let Some(info) = self.sequences.get(seq) else { return out };
        let Some(desc) = info.anim.and_then(|a| self.anims.get(a)) else { return out };
        let n = desc.frames.max(1);
        let mut f = time * desc.fps;
        if info.looping && n > 1 {
            f = f.rem_euclid((n - 1) as f32);
        } else {
            f = f.clamp(0.0, (n - 1) as f32);
        }
        let f0 = f.floor() as i32;
        let f1 = (f0 + 1).min(n - 1);
        let t = f - f0 as f32;
        let a = self.frame(desc, f0);
        let b = self.frame(desc, f1);
        for (i, o) in out.iter_mut().enumerate() {
            let (pa, qa) = a[i];
            let (pb, qb) = b[i];
            *o = (pa.lerp(pb, t), qa.slerp(qb, t).normalize());
        }
        out
    }

    fn frame(&self, desc: &AnimDesc, frame: i32) -> Vec<(Vec3, Quat)> {
        let bones = &self.skeleton.bones;
        // A delta animation's untouched bones add nothing (identity / zero).
        let mut local: Vec<(Vec3, Quat)> = if desc.delta {
            vec![(Vec3::ZERO, Quat::IDENTITY); bones.len()]
        } else {
            bones.iter().map(|b| (b.pos, b.quat)).collect()
        };
        let (start, frame) = match desc.sections {
            Some((sf, table)) => {
                let d = &self.data[..];
                let last_section = ((desc.frames - 1) / sf).max(0);
                let section = (frame / sf).min(last_section);
                let block = i32_at(d, table + section as usize * 8).unwrap_or(-1);
                let index = i32_at(d, table + section as usize * 8 + 4).unwrap_or(0);
                (self.locate(block, index, desc.base), frame - section * sf)
            }
            None => (self.locate(desc.block, desc.index, desc.base), frame),
        };
        let Some((d, p)) = start else { return local };
        if desc.frame_anim {
            frame_anim(d, p, desc.frames, frame, &mut local);
            return local;
        }
        let mut p = p;
        for _ in 0..bones.len() * 2 + 8 {
            let (Some(&bone), Some(&flags), Some(next)) = (d.get(p), d.get(p + 1), i16_at(d, p + 2)) else { break };
            let bone = bone as usize;
            if bone >= bones.len() {
                break;
            }
            let b = &bones[bone];
            let delta = flags & 0x10 != 0;
            let mut o = p + 4;
            if flags & 0x02 != 0 {
                if let Some(q) = quat48(d, o) {
                    local[bone].1 = q;
                }
                o += 6;
            } else if flags & 0x20 != 0 {
                if let Some(q) = quat64(d, o) {
                    local[bone].1 = q;
                }
                o += 8;
            } else if flags & 0x08 != 0 {
                let mut e = Vec3::ZERO;
                for k in 0..3 {
                    let off = u16_at(d, o + k * 2).unwrap_or(0) as usize;
                    if off != 0 {
                        e[k] = anim_value(d, o + off, frame).unwrap_or(0.0) * b.rot_scale[k];
                    }
                }
                if !delta {
                    e += b.rot;
                }
                local[bone].1 = radian_euler_to_quat(e);
                o += 6;
            } else if delta {
                local[bone].1 = Quat::IDENTITY;
            }
            if flags & 0x01 != 0 {
                if let (Some(x), Some(y), Some(z)) = (u16_at(d, o), u16_at(d, o + 2), u16_at(d, o + 4)) {
                    local[bone].0 = Vec3::new(half(x), half(y), half(z));
                }
            } else if flags & 0x04 != 0 {
                let mut v = Vec3::ZERO;
                for k in 0..3 {
                    let off = u16_at(d, o + k * 2).unwrap_or(0) as usize;
                    if off != 0 {
                        v[k] = anim_value(d, o + off, frame).unwrap_or(0.0) * b.pos_scale[k];
                    }
                }
                if !delta {
                    v += b.pos;
                }
                local[bone].0 = v;
            } else if delta {
                local[bone].0 = Vec3::ZERO;
            }
            if next <= 0 {
                break;
            }
            p += next as usize;
        }
        local
    }

    /// Fastest ground speed (model units per second) among the animations of
    /// a sequence's blend grid, from their recorded linear movement: the
    /// speed a movement blend (e.g. the run) plays back at without sliding.
    pub fn ground_speed(&self, seq: usize) -> f32 {
        let d = &self.data[..];
        let Some(info) = self.sequences.get(seq) else { return 0.0 };
        let mut best = 0.0f32;
        for desc in info.grid_anims.iter().flatten().filter_map(|&a| self.anims.get(a)) {
            let a = desc.base;
            let (Some(n), Some(mi)) = (i32_at(d, a + 20), i32_at(d, a + 24)) else { continue };
            if n <= 0 || mi <= 0 {
                continue;
            }
            // Last movement record: (end frame, flags, v0, v1, angle, vector, position).
            let o = a + mi as usize + (n as usize - 1) * 44;
            let (Some(x), Some(y), Some(z)) = (f32_at(d, o + 32), f32_at(d, o + 36), f32_at(d, o + 40)) else { continue };
            let secs = (desc.frames - 1).max(1) as f32 / desc.fps.max(1.0);
            best = best.max(Vec3::new(x, y, z).length() / secs);
        }
        best
    }

    /// Index of a pose parameter by name.
    pub fn pose_param(&self, name: &str) -> Option<usize> {
        self.pose_params.iter().position(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// Samples one animation at a cycle (0..1 of its length).
    fn sample_anim(&self, anim: usize, cycle: f32, looping: bool) -> Vec<(Vec3, Quat)> {
        let bones = &self.skeleton.bones;
        let Some(desc) = self.anims.get(anim) else {
            return bones.iter().map(|b| (b.pos, b.quat)).collect();
        };
        let n = desc.frames.max(1);
        let mut f = cycle * (n - 1).max(0) as f32;
        if looping && n > 1 {
            f = f.rem_euclid((n - 1) as f32);
        } else {
            f = f.clamp(0.0, (n - 1) as f32);
        }
        let f0 = f.floor() as i32;
        let f1 = (f0 + 1).min(n - 1);
        let t = f - f0 as f32;
        let a = self.frame(desc, f0);
        let b = self.frame(desc, f1);
        a.iter().zip(&b).map(|(&(pa, qa), &(pb, qb))| (pa.lerp(pb, t), qa.slerp(qb, t).normalize())).collect()
    }

    /// One sequence at `cycle`, with its blend grid resolved by `pose`
    /// (values in each pose parameter's own units), but no layers.
    pub fn sequence_pose(&self, seq: usize, cycle: f32, pose: &[f32]) -> Vec<(Vec3, Quat)> {
        let bones = &self.skeleton.bones;
        let Some(info) = self.sequences.get(seq) else {
            return bones.iter().map(|b| (b.pos, b.quat)).collect();
        };
        // Position in the grid along each axis: 0..groupsize-1.
        let axis = |k: usize| -> (usize, usize, f32) {
            let g = info.grid[k];
            if g <= 1 {
                return (0, 0, 0.0);
            }
            let v = info.params[k].and_then(|p| pose.get(p)).copied().unwrap_or(0.0);
            let (s0, s1) = (info.param_start[k], info.param_end[k]);
            let u = if (s1 - s0).abs() < 1e-6 { 0.0 } else { ((v - s0) / (s1 - s0)).clamp(0.0, 1.0) };
            let x = u * (g - 1) as f32;
            let i0 = (x.floor() as usize).min(g - 2);
            (i0, i0 + 1, x - i0 as f32)
        };
        let (x0, x1, fx) = axis(0);
        let (y0, y1, fy) = axis(1);
        let g0 = info.grid[0];
        let get = |x: usize, y: usize| {
            let a = info.grid_anims.get(y * g0 + x).copied().flatten().or(info.anim);
            match a {
                Some(a) => self.sample_anim(a, cycle, info.looping),
                None => bones.iter().map(|b| (b.pos, b.quat)).collect(),
            }
        };
        let blend = |a: &[(Vec3, Quat)], b: &[(Vec3, Quat)], t: f32| -> Vec<(Vec3, Quat)> {
            a.iter().zip(b).map(|(&(pa, qa), &(pb, qb))| (pa.lerp(pb, t), qa.slerp(qb, t).normalize())).collect()
        };
        let row0 = if info.grid[0] > 1 { blend(&get(x0, y0), &get(x1, y0), fx) } else { get(0, y0) };
        if info.grid[1] > 1 {
            let row1 = if info.grid[0] > 1 { blend(&get(x0, y1), &get(x1, y1), fx) } else { get(0, y1) };
            blend(&row0, &row1, fy)
        } else {
            row0
        }
    }

    /// Lays sequence `layer` (at `cycle`, weight `w`) over `base`, Source-style:
    /// a delta layer adds (rotation `base * delta^w` for STUDIO_POST layers,
    /// `delta^w * base` otherwise; position `base + delta*w`),
    /// a normal one blends toward its pose. Per-bone weights of the layer apply.
    pub fn apply_layer(&self, base: &mut [(Vec3, Quat)], layer: usize, cycle: f32, w: f32, pose: &[f32]) {
        let Some(info) = self.sequences.get(layer) else { return };
        if w <= 0.0 {
            return;
        }
        let l = self.sequence_pose(layer, cycle, pose);
        for (i, b) in base.iter_mut().enumerate() {
            let s = w * info.weights.get(i).copied().unwrap_or(1.0);
            if s <= 0.0 {
                continue;
            }
            let (lp, lq) = l[i];
            if info.delta {
                let dq = Quat::IDENTITY.slerp(lq, s);
                // Source: QuaternionMA (post) = base * delta; QuaternionSM = delta * base.
                let q = if info.post { b.1 * dq } else { dq * b.1 };
                *b = (b.0 + lp * s, q.normalize());
            } else {
                *b = (b.0.lerp(lp, s), b.1.slerp(lq, s).normalize());
            }
        }
    }

    /// Full Source evaluation of a base sequence: its blend grid by `pose`,
    /// then its auto-layers (e.g. the aim matrix), then `extra` layers
    /// (sequence, cycle, weight) such as gestures.
    pub fn evaluate(&self, seq: usize, cycle: f32, pose: &[f32], extra: &[(usize, f32, f32)]) -> Vec<(Vec3, Quat)> {
        let mut out = self.sequence_pose(seq, cycle, pose);
        if let Some(info) = self.sequences.get(seq) {
            for &al in &info.autolayers {
                self.apply_layer(&mut out, al, cycle, 1.0, pose);
            }
        }
        for &(layer, c, w) in extra {
            self.apply_layer(&mut out, layer, c, w, pose);
            if let Some(info) = self.sequences.get(layer) {
                for &al in &info.autolayers {
                    self.apply_layer(&mut out, al, c, w, pose);
                }
            }
        }
        out
    }

    /// Model-space attachment transform (position, rotation) given model-space bone transforms.
    pub fn attachment(&self, name: &str, bone_to_model: &[bevy::math::Affine3A]) -> Option<(Vec3, bevy::math::Mat3)> {
        let d = &self.data[..];
        let n = i32_at(d, 240)? as usize;
        let idx = i32_at(d, 244)? as usize;
        for i in 0..n {
            let a = idx + i * 92;
            let nm = cstr(d, a + i32_at(d, a)? as usize);
            if !nm.eq_ignore_ascii_case(name) {
                continue;
            }
            let bone = i32_at(d, a + 8)? as usize;
            let m = |r: usize, c: usize| f32_at(d, a + 12 + (r * 4 + c) * 4).unwrap_or(0.0);
            let local = bevy::math::Affine3A::from_mat3_translation(
                bevy::math::Mat3::from_cols(
                    Vec3::new(m(0, 0), m(1, 0), m(2, 0)),
                    Vec3::new(m(0, 1), m(1, 1), m(2, 1)),
                    Vec3::new(m(0, 2), m(1, 2), m(2, 2)),
                ),
                Vec3::new(m(0, 3), m(1, 3), m(2, 3)),
            );
            let w = *bone_to_model.get(bone)? * local;
            return Some((w.translation.into(), w.matrix3.into()));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anim_value_runs() {
        // One run: valid 2, total 4, values 10, 20 -> frames 0,1 = 10,20; 2,3 = 20.
        let d: Vec<u8> = [2u8, 4].iter().copied().chain(10i16.to_le_bytes()).chain(20i16.to_le_bytes()).collect();
        // Header is at 0; values follow at +2.
        let v: Vec<f32> = (0..4).map(|f| anim_value(&d, 0, f).unwrap()).collect();
        assert_eq!(v, vec![10.0, 20.0, 20.0, 20.0]);
    }

    #[test]
    fn frame_anim_layout() {
        // Two bones: bone 0 constant rotation + position, bone 1 animated
        // rotation + position; two frames. Header 24 bytes, 2 flag bytes,
        // constants at 28, frames at 40, 12 bytes each.
        let q = |x: u16| [x.to_le_bytes(), 32768u16.to_le_bytes(), 16384u16.to_le_bytes()].concat();
        let v = |h: u16| [h.to_le_bytes(), 0u16.to_le_bytes(), 0u16.to_le_bytes()].concat();
        let mut d: Vec<u8> = [28i32, 40, 12, 0, 0, 0].iter().flat_map(|i| i.to_le_bytes()).collect();
        d.extend([0x03u8, 0x0c, 0, 0]);
        d.extend(q(32768 + 16384)); // bone 0 rotation: x = 0.5
        d.extend(v(0x3c00)); // bone 0 position x = 1.0
        for f in 0..2u16 {
            d.extend(q(32768)); // bone 1 rotation: identity
            d.extend(v(0x4000 + f * 0x200)); // bone 1 position x = 2.0, 3.0
        }
        let mut local = vec![(Vec3::splat(9.0), Quat::IDENTITY); 2];
        frame_anim(&d, 0, 2, 1, &mut local);
        assert!((local[0].1.x - 0.5).abs() < 1e-4 && (local[0].0.x - 1.0).abs() < 1e-4);
        assert!((local[1].0.x - 3.0).abs() < 1e-4 && (local[1].1.w - 1.0).abs() < 1e-4);
    }
}
