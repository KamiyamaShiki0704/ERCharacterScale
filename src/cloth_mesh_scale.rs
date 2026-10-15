//! Fresh mesh-to-mesh triangle frames, after construction and before bind
//! composition. Do not apply this to render palettes or authored matrices.

pub const MAX_FRAMES: usize = 8192;

/// Mode1 P frames store a raw triangle cross product, proportional to area.
/// The feature adds an external uniform body scale, so remove one factor of
/// that scale from fresh Z only. This is not normalization of simulated bends.
pub fn scale_area_depth(frames: &mut [[f32; 16]], scale: f32) -> Option<usize> {
    if !crate::scale_math::valid(scale) || frames.is_empty() || frames.len() > MAX_FRAMES {
        return None;
    }
    // Validate the ENTIRE batch first, including overflow and actual mode.
    // A mode0 unit column must not accidentally enter the inverse-scale path.
    for m in frames.iter() {
        if !m.iter().all(|v| v.is_finite())
            || m[3] != 0.
            || m[7] != 0.
            || m[11] != 0.
            || m[15] != 1.
        {
            return None;
        }
        let cross = [
            m[1] * m[6] - m[2] * m[5],
            m[2] * m[4] - m[0] * m[6],
            m[0] * m[5] - m[1] * m[4],
        ];
        for (i, expected) in cross.iter().enumerate() {
            if !expected.is_finite()
                || !crate::scale_math::representable(m[8 + i], m[8 + i] / scale)
                || (m[8 + i] - expected).abs() > 1e-10f32.max(expected.abs() * 1e-4)
            {
                return None;
            }
        }
    }
    if scale == 1. {
        return Some(0);
    }
    for m in frames.iter_mut() {
        for value in &mut m[8..11] {
            *value /= scale;
        }
    }
    Some(frames.len())
}

pub fn restore_normal_length(
    bytes: &mut [u8],
    count: usize,
    stride: usize,
    scale: f32,
) -> Option<usize> {
    if !crate::scale_math::valid(scale)
        || count == 0
        || count > MAX_FRAMES
        || ![12, 16].contains(&stride)
        || bytes.len() != count * stride
    {
        return None;
    }
    for row in bytes.chunks_exact(stride) {
        for value in row[..12].chunks_exact(4) {
            let value = f32::from_le_bytes(value.try_into().ok()?);
            if !crate::scale_math::representable(value, value / scale) {
                return None;
            }
        }
    }
    if scale == 1. {
        return Some(0);
    }
    for row in bytes.chunks_exact_mut(stride) {
        for value in row[..12].chunks_exact_mut(4) {
            let n = f32::from_le_bytes((&*value).try_into().ok()?) / scale;
            value.copy_from_slice(&n.to_le_bytes());
        }
    }
    Some(count)
}

pub fn scale_normal_depth(frames: &mut [[f32; 16]], scale: f32) -> Option<usize> {
    if !crate::scale_math::valid(scale) || frames.is_empty() || frames.len() > MAX_FRAMES {
        return None;
    }
    // Validate the entire fresh batch before any write. The native unit-normal
    // mode permits zero normals for degenerate triangles; rsqrt is approximate.
    for m in frames.iter() {
        let norm2 = m[8] * m[8] + m[9] * m[9] + m[10] * m[10];
        if !m.iter().all(|v| v.is_finite())
            || m[3] != 0.
            || m[7] != 0.
            || m[11] != 0.
            || m[15] != 1.
            || (norm2 != 0. && (norm2 - 1.).abs() > 0.005)
            || m[8..11]
                .iter()
                .any(|v| crate::scale_math::product(*v, scale).is_none())
        {
            return None;
        }
    }
    if scale == 1. {
        return Some(0);
    }
    for m in frames.iter_mut() {
        for v in &mut m[8..11] {
            *v *= scale;
        }
    }
    Some(frames.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> [f32; 16] {
        [
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 2., 3., 4., 1.,
        ]
    }
    #[test]
    fn area_frame_uses_inverse_scale_and_keeps_other_columns() {
        for scale in [0.25, 0.5, 1., 3., 4., 10.] {
            let mut frame = frame();
            frame[0] = 2. * scale;
            frame[5] = 3. * scale;
            frame[10] = 6. * scale * scale;
            let before = frame;
            assert_eq!(
                scale_area_depth(std::slice::from_mut(&mut frame), scale),
                Some(if scale == 1. { 0 } else { 1 })
            );
            assert_eq!(frame[10], 6. * scale);
            for i in (0..16).filter(|i| !(8..11).contains(i)) {
                assert_eq!(frame[i].to_bits(), before[i].to_bits());
            }
        }
    }
    #[test]
    fn area_frame_invalid_late_row_never_partially_writes() {
        for (index, value) in [
            (0, f32::INFINITY),
            (8, f32::NAN),
            (10, 0.5),
            (15, 0.),
            (0, f32::MAX),
        ] {
            let mut frames = [frame(), frame()];
            frames[1][index] = value;
            let before = frames.map(|m| m.map(f32::to_bits));
            assert_eq!(scale_area_depth(&mut frames, 0.5), None);
            assert_eq!(frames.map(|m| m.map(f32::to_bits)), before);
        }
        for scale in [0., -1., f32::NAN, f32::INFINITY] {
            assert_eq!(scale_area_depth(&mut [frame()], scale), None);
        }
        assert_eq!(scale_area_depth(&mut [], 0.5), None);
    }
    #[test]
    fn area_frame_degenerate_triangle_stays_zero() {
        let mut m = frame();
        m[5] = 0.;
        m[10] = 0.;
        let before = m;
        assert_eq!(scale_area_depth(std::slice::from_mut(&mut m), 0.5), Some(1));
        assert_eq!(m, before);
    }
    #[test]
    fn frame_validation_precedes_all_writes() {
        for (index, value) in [
            (15, 0.),
            (3, 1.),
            (8, f32::NAN),
            (9, f32::INFINITY),
            (10, 0.5),
        ] {
            let mut m = [frame(), frame()];
            m[1][index] = value;
            let before = m.map(|r| r.map(f32::to_bits));
            assert_eq!(scale_normal_depth(&mut m, 0.5), None);
            assert_eq!(m.map(|r| r.map(f32::to_bits)), before);
        }
    }

    #[test]
    fn normal_depth_accepts_new_scales_and_checks_late_overflow_before_writes() {
        for scale in [0.1f32, 0.25, 4.0, 10.0] {
            let mut frames = [frame(), frame()];
            assert_eq!(scale_normal_depth(&mut frames, scale), Some(2));
            assert_eq!(frames[0][10], scale);
        }
        let mut frames = [frame(), frame()];
        // Native approximate normalization allows this input, but MAX would overflow.
        frames[1][10] = 1.001;
        let before = frames;
        assert_eq!(scale_normal_depth(&mut frames, f32::MAX), None);
        assert_eq!(frames, before);
    }

    #[test]
    fn neutral_degenerate_growth_and_invalid_scales() {
        let mut m = [frame()];
        let before = m;
        assert_eq!(scale_normal_depth(&mut m, 1.), Some(0));
        assert_eq!(m, before);
        m[0][10] = 0.;
        assert_eq!(scale_normal_depth(&mut m, 3.), Some(1));
        assert_eq!(m[0][10], 0.);
        for s in [0., -1., f32::NAN, f32::INFINITY] {
            assert_eq!(scale_normal_depth(&mut m, s), None);
        }
        assert_eq!(scale_normal_depth(&mut [], 0.5), None);
    }
    #[test]
    fn normal_restore_preserves_padding_and_baseline_magnitude() {
        for stride in [12, 16] {
            for scale in [0.25, 0.5, 1., 3., 4., 10.] {
                let mut raw = vec![0xFE; 2 * stride];
                for row in raw.chunks_exact_mut(stride) {
                    for (j, n) in [0.3f32, 0.4, 0.].iter().enumerate() {
                        row[j * 4..j * 4 + 4].copy_from_slice(&(n * scale).to_le_bytes());
                    }
                }
                assert_eq!(
                    restore_normal_length(&mut raw, 2, stride, scale),
                    Some(if scale == 1. { 0 } else { 2 })
                );
                for row in raw.chunks_exact(stride) {
                    let n = f32::from_le_bytes(row[0..4].try_into().unwrap());
                    assert!((n - 0.3).abs() < 1e-6);
                    if stride == 16 {
                        assert_eq!(&row[12..16], &[0xFE; 4]);
                    }
                }
            }
        }
    }
    #[test]
    fn invalid_late_normal_rejects_batch_without_partial_writes() {
        let mut raw = vec![0u8; 32];
        raw[16..20].copy_from_slice(&f32::NAN.to_le_bytes());
        let before = raw.clone();
        assert_eq!(restore_normal_length(&mut raw, 2, 16, 0.5), None);
        assert_eq!(raw, before);
        for (count, stride, scale) in [(0, 16, 0.5), (1, 8, 0.5), (2, 16, 0.1), (3, 16, 0.5)] {
            assert_eq!(restore_normal_length(&mut raw, count, stride, scale), None);
        }
    }
}
