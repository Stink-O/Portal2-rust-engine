//! Studio model skeletons: bones, local animations and sequences read
//! straight from the MDL (v44-49), and the pose of a sequence's first frame.
//!
//! Portal 2 draws `prop_dynamic` and friends in their default sequence
//! (the `DefaultAnim` keyvalue, else sequence 0), not the reference pose, and
//! many models' root bones carry a 90 degree rotation, so rendering the VVD's
//! bind-pose vertices directly points them the wrong way. `Skeleton::pose`
//! gives per-bone skinning matrices (bone-to-model x pose-to-bone) in Source
//! model space.

use bevy::math::{Affine3A, Mat3, Quat, Vec3};

#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    pub parent: i32,
    pub pos: Vec3,
    pub quat: Quat,
    /// RadianEuler (x = roll, y = pitch, z = yaw)
    pub rot: Vec3,
    pub pos_scale: Vec3,
    pub rot_scale: Vec3,
    /// Model space (bind pose) -> bone space.
    pub pose_to_bone: Affine3A,
}

#[derive(Clone, Debug)]
pub struct Sequence {
    pub label: String,
    /// Local animation index of blend 0.
    pub anim: Option<usize>,
}

#[derive(Clone, Debug)]
struct AnimDesc {
    /// Absolute offset of the first mstudioanim_t of frame 0, if in the MDL.
    data: Option<usize>,
    frames: i32,
}

#[derive(Clone, Debug)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
    pub sequences: Vec<Sequence>,
    anims: Vec<AnimDesc>,
    data: std::sync::Arc<Vec<u8>>,
}

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
fn vec3_at(d: &[u8], o: usize) -> Option<Vec3> {
    Some(Vec3::new(f32_at(d, o)?, f32_at(d, o + 4)?, f32_at(d, o + 8)?))
}
fn cstr(d: &[u8], o: usize) -> String {
    let s = d.get(o..).unwrap_or(&[]);
    let end = s.iter().position(|&c| c == 0).unwrap_or(0);
    String::from_utf8_lossy(&s[..end]).to_string()
}

/// Source `AngleQuaternion(RadianEuler)`: yaw about Z, pitch about Y, roll about X.
pub fn radian_euler_to_quat(r: Vec3) -> Quat {
    Quat::from_rotation_z(r.z) * Quat::from_rotation_y(r.y) * Quat::from_rotation_x(r.x)
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

/// Frame-0 value of an `mstudioanimvalue_t` run starting at `o`.
fn anim_value_frame0(d: &[u8], o: usize) -> Option<f32> {
    let valid = *d.get(o)?;
    let total = *d.get(o + 1)?;
    if total == 0 {
        return Some(0.0);
    }
    let idx = if valid > 0 { 1 } else { valid as usize };
    Some(i16_at(d, o + idx * 2)? as f32)
}

impl Skeleton {
    pub fn parse(data: std::sync::Arc<Vec<u8>>) -> Option<Self> {
        let d = &data[..];
        if d.get(0..4)? != b"IDST" {
            return None;
        }
        let num_bones = i32_at(d, 156)? as usize;
        let bone_index = i32_at(d, 160)? as usize;
        let num_anim = i32_at(d, 180)? as usize;
        let anim_index = i32_at(d, 184)? as usize;
        let num_seq = i32_at(d, 188)? as usize;
        let seq_index = i32_at(d, 192)? as usize;
        if num_bones > 256 || num_anim > 4096 || num_seq > 4096 {
            return None;
        }
        let mut bones = Vec::with_capacity(num_bones);
        for i in 0..num_bones {
            let b = bone_index + i * 216;
            let m = |r: usize, c: usize| f32_at(d, b + 96 + (r * 4 + c) * 4).unwrap_or(0.0);
            let pose_to_bone = Affine3A::from_mat3_translation(
                Mat3::from_cols(
                    Vec3::new(m(0, 0), m(1, 0), m(2, 0)),
                    Vec3::new(m(0, 1), m(1, 1), m(2, 1)),
                    Vec3::new(m(0, 2), m(1, 2), m(2, 2)),
                ),
                Vec3::new(m(0, 3), m(1, 3), m(2, 3)),
            );
            let q = vec3_at(d, b + 44)?;
            bones.push(Bone {
                name: cstr(d, b + i32_at(d, b)? as usize),
                parent: i32_at(d, b + 4)?,
                pos: vec3_at(d, b + 32)?,
                quat: Quat::from_xyzw(q.x, q.y, q.z, f32_at(d, b + 56)?),
                rot: vec3_at(d, b + 60)?,
                pos_scale: vec3_at(d, b + 72)?,
                rot_scale: vec3_at(d, b + 84)?,
                pose_to_bone,
            });
        }
        let mut anims = Vec::with_capacity(num_anim);
        for i in 0..num_anim {
            let a = anim_index + i * 100;
            let frames = i32_at(d, a + 16)?;
            let block = i32_at(d, a + 52)?;
            let mut index = i32_at(d, a + 56)?;
            let section_index = i32_at(d, a + 80)?;
            let section_frames = i32_at(d, a + 84)?;
            if section_frames != 0 && section_index != 0 {
                // frame 0 lives in section 0: {animblock, animindex}
                let s = a as i64 + section_index as i64;
                if s >= 0 {
                    let s = s as usize;
                    if i32_at(d, s)? == 0 {
                        index = i32_at(d, s + 4)?;
                    }
                }
            }
            let data = (block == 0 && index != 0).then(|| (a as i64 + index as i64) as usize);
            anims.push(AnimDesc { data, frames });
        }
        let mut sequences = Vec::with_capacity(num_seq);
        for i in 0..num_seq {
            let s = seq_index + i * 212;
            let label = cstr(d, s + i32_at(d, s + 4)? as usize);
            let animindexindex = i32_at(d, s + 60)?;
            let anim = i16_at(d, s + animindexindex as usize)
                .filter(|&a| a >= 0 && (a as usize) < num_anim)
                .map(|a| a as usize);
            sequences.push(Sequence { label, anim });
        }
        Some(Skeleton {
            bones,
            sequences,
            anims,
            data,
        })
    }

    pub fn sequence_index(&self, name: &str) -> Option<usize> {
        self.sequences
            .iter()
            .position(|s| s.label.eq_ignore_ascii_case(name))
    }

    /// Local (parent-space) bone transforms at frame 0 of a local animation.
    fn local_frame0(&self, anim: Option<usize>) -> Vec<(Vec3, Quat)> {
        let mut local: Vec<(Vec3, Quat)> = self.bones.iter().map(|b| (b.pos, b.quat)).collect();
        let Some(desc) = anim.and_then(|a| self.anims.get(a)) else {
            return local;
        };
        let _ = desc.frames;
        let Some(mut p) = desc.data else { return local };
        let d = &self.data[..];
        for _ in 0..self.bones.len() * 2 + 8 {
            let (Some(&bone), Some(&flags), Some(next)) = (d.get(p), d.get(p + 1), i16_at(d, p + 2)) else {
                break;
            };
            let bone = bone as usize;
            if bone >= self.bones.len() {
                break;
            }
            let b = &self.bones[bone];
            let delta = flags & 0x10 != 0;
            let mut o = p + 4;
            // rotation
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
                        e[k] = anim_value_frame0(d, o + off).unwrap_or(0.0) * b.rot_scale[k];
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
            // position
            if flags & 0x01 != 0 {
                if let (Some(x), Some(y), Some(z)) = (u16_at(d, o), u16_at(d, o + 2), u16_at(d, o + 4)) {
                    local[bone].0 = Vec3::new(half(x), half(y), half(z));
                }
            } else if flags & 0x04 != 0 {
                let mut v = Vec3::ZERO;
                for k in 0..3 {
                    let off = u16_at(d, o + k * 2).unwrap_or(0) as usize;
                    if off != 0 {
                        v[k] = anim_value_frame0(d, o + off).unwrap_or(0.0) * b.pos_scale[k];
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

    /// Bone-to-model transforms at frame 0 of `sequence` (default: 0).
    pub fn bone_to_model(&self, sequence: Option<usize>) -> Vec<Affine3A> {
        let anim = self
            .sequences
            .get(sequence.unwrap_or(0))
            .and_then(|s| s.anim);
        let local = self.local_frame0(anim);
        let mut out: Vec<Affine3A> = Vec::with_capacity(self.bones.len());
        for (i, b) in self.bones.iter().enumerate() {
            let m = Affine3A::from_rotation_translation(local[i].1.normalize(), local[i].0);
            let m = if b.parent >= 0 && (b.parent as usize) < i {
                out[b.parent as usize] * m
            } else {
                m
            };
            out.push(m);
        }
        out
    }

    /// Skinning matrices (bind-pose model space -> posed model space).
    pub fn pose(&self, sequence: Option<usize>) -> Vec<Affine3A> {
        self.bone_to_model(sequence)
            .into_iter()
            .zip(&self.bones)
            .map(|(m, b)| m * b.pose_to_bone)
            .collect()
    }

    /// Skinning matrices of the reference pose (should be ~identity).
    pub fn reference_pose(&self) -> Vec<Affine3A> {
        let mut out: Vec<Affine3A> = Vec::with_capacity(self.bones.len());
        for (i, b) in self.bones.iter().enumerate() {
            let m = Affine3A::from_rotation_translation(b.quat.normalize(), b.pos);
            let m = if b.parent >= 0 && (b.parent as usize) < i { out[b.parent as usize] * m } else { m };
            out.push(m);
        }
        out.into_iter().zip(&self.bones).map(|(m, b)| m * b.pose_to_bone).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats() {
        assert_eq!(half(0x3c00), 1.0);
        assert_eq!(half(0xc000), -2.0);
        assert_eq!(half(0), 0.0);
    }

    #[test]
    fn reference_pose_is_identity_for_real_models() {
        let cfg = crate::config::GameConfig::from_env();
        if !cfg.root.join("portal2").is_dir() {
            return;
        }
        let fs = crate::fs::GameFs::mount(cfg).unwrap();
        for m in ["models/props_backstage/item_dropper_wrecked.mdl", "models/props/portal_door_combined.mdl"] {
            let sk = Skeleton::parse(std::sync::Arc::new(fs.read(m).unwrap())).unwrap();
            assert!(!sk.bones.is_empty());
            for p in sk.reference_pose() {
                let v = p.transform_point3(Vec3::new(10.0, -20.0, 30.0));
                assert!((v - Vec3::new(10.0, -20.0, 30.0)).length() < 0.1, "{m}: {v}");
            }
        }
    }
}

