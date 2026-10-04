//! Bounded pose decoding and native format conversion. All readers are supplied
//! by the caller so the exact runtime decoder can also consume owned fixtures.
use crate::equipment_retarget::{Bone, LocalPose};
use glam::{DMat4, DQuat, DVec3};

pub(crate) const LIMIT: usize = crate::cloth_render_scale::MAX_TRANSFORMS;
pub(crate) type Reader<'a> = dyn Fn(usize, &mut [u8]) -> bool + 'a;

pub(crate) fn bytes<const N: usize>(read: &Reader<'_>, address: usize) -> Option<[u8; N]> {
    let mut value = [0; N];
    (address != 0 && address.checked_add(N).is_some() && read(address, &mut value)).then_some(value)
}

pub(crate) fn pointer(read: &Reader<'_>, address: usize) -> Option<usize> {
    Some(usize::from_le_bytes(bytes(read, address)?))
}

fn i32_at(raw: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(raw[offset..offset + 4].try_into().unwrap())
}

fn ptr_at(raw: &[u8], offset: usize) -> usize {
    usize::from_le_bytes(raw[offset..offset + 8].try_into().unwrap())
}

pub(crate) fn text(read: &Reader<'_>, address: usize, wide: bool) -> Option<String> {
    if address == 0 {
        return None;
    }
    let mut out = Vec::new();
    for index in 0..128 {
        let value = if wide {
            u16::from_le_bytes(bytes(read, address.checked_add(index * 2)?)?)
        } else {
            bytes::<1>(read, address.checked_add(index)?)?[0] as u16
        };
        if value == 0 {
            return if wide {
                String::from_utf16(&out).ok()
            } else {
                String::from_utf8(out.into_iter().map(|v| v as u8).collect()).ok()
            };
        }
        out.push(value);
    }
    None
}

pub(crate) fn qs(raw: &[u8]) -> Option<LocalPose> {
    if raw.len() != 48 {
        return None;
    }
    let values: [f64; 12] = std::array::from_fn(|i| {
        f32::from_le_bytes(raw[i * 4..i * 4 + 4].try_into().unwrap()) as f64
    });
    let q = DQuat::from_xyzw(values[4], values[5], values[6], values[7]);
    if !q.is_finite() || (q.length_squared() - 1.0).abs() > 0.005 {
        return None;
    }
    let pose = LocalPose {
        translation: DVec3::new(values[0], values[1], values[2]),
        rotation: q.normalize(),
        scale: DVec3::new(values[8], values[9], values[10]),
    };
    (pose.translation.is_finite() && pose.scale.is_finite() && pose.scale.min_element() > 1e-10)
        .then_some(pose)
}

pub(crate) fn matrix(pose: LocalPose) -> DMat4 {
    DMat4::from_scale_rotation_translation(pose.scale, pose.rotation, pose.translation)
}

pub(crate) fn local(matrix: DMat4) -> Option<LocalPose> {
    if !matrix.is_finite() || matrix.determinant().abs() < 1e-12 {
        return None;
    }
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    let pose = LocalPose {
        translation,
        rotation: rotation.normalize(),
        scale,
    };
    let reconstructed = self::matrix(pose);
    // Havok Qs cannot represent shear. Never silently discard it on a cloth input.
    if !scale.is_finite()
        || scale.min_element() <= 1e-10
        || !pose.rotation.is_finite()
        || reconstructed
            .to_cols_array()
            .iter()
            .zip(matrix.to_cols_array())
            .any(|(a, b)| (a - b).abs() > 1e-5 * (1.0 + b.abs()))
    {
        return None;
    }
    Some(pose)
}

pub(crate) fn affine(raw: &[u8]) -> Option<DMat4> {
    if raw.len() != 48 {
        return None;
    }
    let mut c = [0.0; 16];
    c[15] = 1.0;
    for row in 0..3 {
        for col in 0..4 {
            c[col * 4 + row] = f32::from_le_bytes(
                raw[(row * 4 + col) * 4..(row * 4 + col + 1) * 4]
                    .try_into()
                    .unwrap(),
            ) as f64;
        }
    }
    let m = DMat4::from_cols_array(&c);
    m.is_finite().then_some(m)
}

pub(crate) fn affine_output(matrix: DMat4) -> Option<[f32; 12]> {
    let c = matrix.to_cols_array();
    let mut out = [0.0; 12];
    for row in 0..3 {
        for col in 0..4 {
            out[row * 4 + col] = c[col * 4 + row] as f32;
        }
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

pub(crate) fn qs_output(pose: LocalPose) -> Option<[f32; 12]> {
    let mut out = [0.0; 12];
    for (i, value) in pose.translation.to_array().into_iter().enumerate() {
        out[i] = value as f32;
    }
    out[3] = 1.0;
    for (i, value) in pose.rotation.to_array().into_iter().enumerate() {
        out[4 + i] = value as f32;
    }
    for (i, value) in pose.scale.to_array().into_iter().enumerate() {
        out[8 + i] = value as f32;
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// A model-space matrix can contain shear even when every authored local Qs
/// is valid (nonuniform parent scale followed by child rotation). Preserve the
/// exact local Qs and ask hkaPose to materialize this cache entry natively.
/// This does not approximate a matrix or relax the strict `local` decoder.
pub(crate) fn solver_model_cache(
    matrix: DMat4,
    parent_dirty: bool,
    scale: f64,
) -> Option<([f32; 12], u32)> {
    if !matrix.is_finite()
        || matrix.determinant() <= 1e-12
        || !scale.is_finite()
        || scale <= 0.0
        || matrix
            .to_cols_array()
            .iter()
            .any(|v| !(*v as f32).is_finite())
        || !(matrix.w_axis.truncate() * scale).as_vec3().is_finite()
    {
        return None;
    }
    if !parent_dirty && let Some(mut pose) = local(matrix) {
        pose.translation *= scale;
        return Some((qs_output(pose)?, 0));
    }
    // Flag 2 = model dirty, local valid. The identity placeholder is never an
    // authoritative transform; the native consumer checks the flag first.
    Some((qs_output(LocalPose::IDENTITY)?, 2))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkeletonIdentity {
    pub metadata: usize,
    header: [u8; 0x50],
}

impl SkeletonIdentity {
    pub fn current(&self, read: &Reader<'_>) -> bool {
        bytes(read, self.metadata) == Some(self.header)
    }
}

pub(crate) fn skeleton(
    read: &Reader<'_>,
    metadata: usize,
) -> Option<(SkeletonIdentity, Vec<Bone>)> {
    let header: [u8; 0x50] = bytes(read, metadata)?;
    let count = usize::try_from(i32_at(&header, 0x38)).ok()?;
    if count == 0
        || count > LIMIT
        || i32_at(&header, 0x28) != count as i32
        || i32_at(&header, 0x48) != count as i32
    {
        return None;
    }
    let mut parents = vec![0u8; count * 2];
    let mut names = vec![0u8; count * 16];
    let mut references = vec![0u8; count * 48];
    if !read(ptr_at(&header, 0x20), &mut parents)
        || !read(ptr_at(&header, 0x30), &mut names)
        || !read(ptr_at(&header, 0x40), &mut references)
    {
        return None;
    }
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let parent = i16::from_le_bytes(parents[index * 2..index * 2 + 2].try_into().unwrap());
        if parent < -1 || parent >= index as i16 {
            return None;
        }
        result.push(Bone {
            name: text(read, ptr_at(&names, index * 16) & !1usize, false)?,
            parent: (parent >= 0).then_some(parent as usize),
            reference: qs(&references[index * 48..(index + 1) * 48])?,
        });
    }
    let identity = SkeletonIdentity { metadata, header };
    identity.current(read).then_some((identity, result))
}

#[derive(Default)]
pub(crate) struct PoseScratch {
    raw_local: Vec<u8>,
    raw_model: Vec<u8>,
    flags: Vec<u8>,
    pub model: Vec<DMat4>,
    pub local: Vec<LocalPose>,
}

impl PoseScratch {
    /// Equipment animation can be consumed before or after the body-scale
    /// importer materializes its uniform scale. Requested world scale alone
    /// is not proof that the source cache already contains that factor.
    /// Recognize only the two supported conventions against the authored root;
    /// never change solver-output decoding or guess from a foot's position.
    pub fn capture(
        &mut self,
        read: &Reader<'_>,
        context: usize,
        identity: &SkeletonIdentity,
        skeleton: &[Bone],
        requested_scale: f64,
    ) -> Option<()> {
        if !requested_scale.is_finite() || requested_scale <= 0.0 {
            return None;
        }
        self.capture_model(read, context, identity, skeleton, 1.0)?;
        let mut roots = skeleton
            .iter()
            .enumerate()
            .filter(|(_, b)| b.parent.is_none());
        let (root, bone) = roots.next()?;
        if roots.next().is_some() {
            return None;
        }
        let actual = local(self.model[root])?.scale;
        let reference = bone.reference.scale;
        let ratios = actual / reference;
        let matches = |factor: f64| {
            ratios
                .to_array()
                .iter()
                .all(|v| v.is_finite() && (*v / factor - 1.0).abs() <= 1e-4)
        };
        let factor = if matches(1.0) {
            1.0
        } else if matches(requested_scale) {
            requested_scale
        } else {
            return None;
        };
        let undo = DMat4::from_scale(DVec3::splat(1.0 / factor));
        for model in &mut self.model {
            *model = undo * *model;
        }
        self.local.resize(skeleton.len(), LocalPose::IDENTITY);
        for (i, bone) in skeleton.iter().enumerate() {
            self.local[i] = local(
                bone.parent
                    .map_or(self.model[i], |p| self.model[p].inverse() * self.model[i]),
            )?;
        }
        Some(())
    }

    /// Simulation output is consumed in model space. Independently scaled
    /// native Qs rows need not have a shear-free relative transform; deriving
    /// local TRS here would reject valid cloth and discard the entire render.
    pub fn capture_model(
        &mut self,
        read: &Reader<'_>,
        context: usize,
        identity: &SkeletonIdentity,
        skeleton: &[Bone],
        overall_scale: f64,
    ) -> Option<()> {
        self.local.clear();
        if !overall_scale.is_finite() || overall_scale <= 0.0 || !identity.current(read) {
            return None;
        }
        let header: [u8; 0x38] = bytes(read, context)?;
        let n = skeleton.len();
        if ptr_at(&header, 0) != identity.metadata
            || [0x10, 0x20, 0x30]
                .iter()
                .any(|&o| i32_at(&header, o) < n as i32)
        {
            return None;
        }
        self.raw_local.resize(n * 48, 0);
        self.raw_model.resize(n * 48, 0);
        self.flags.resize(n * 4, 0);
        self.model.resize(n, DMat4::IDENTITY);
        if !read(ptr_at(&header, 8), &mut self.raw_local)
            || !read(ptr_at(&header, 0x18), &mut self.raw_model)
            || !read(ptr_at(&header, 0x28), &mut self.flags)
        {
            return None;
        }
        for (i, bone) in skeleton.iter().enumerate() {
            let flags = i32_at(&self.flags, i * 4);
            if flags & 3 == 3 {
                return None;
            }
            self.model[i] = if flags & 2 == 0 {
                matrix(qs(&self.raw_model[i * 48..(i + 1) * 48])?)
            } else {
                let local = matrix(qs(&self.raw_local[i * 48..(i + 1) * 48])?);
                bone.parent.map_or(local, |p| self.model[p] * local)
            };
        }
        let undo = DMat4::from_scale(DVec3::splat(1.0 / overall_scale));
        for m in &mut self.model {
            *m = undo * *m;
            if !m.is_finite() {
                return None;
            }
        }
        // A changing array header invalidates the entire private snapshot.
        (bytes(read, context) == Some(header) && identity.current(read)).then_some(())
    }
}

#[cfg(test)]
#[path = "equipment_retarget_pose_tests.rs"]
mod tests;
