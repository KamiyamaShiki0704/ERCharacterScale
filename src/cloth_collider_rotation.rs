//! Rigid inputs for the original collider velocity routine. Scale belongs to
//! geometry/translation, not the matrix-to-quaternion rotation calculation.
//! Owned copies only; caller supplies a separately validated ownership scope.

#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform(pub [f32; 16]);

pub fn rigid_pair(
    target: [f32; 16],
    previous: [f32; 16],
    requested: f32,
) -> Option<(Transform, Transform)> {
    if !crate::scale_math::valid(requested) || (requested - 1.).abs() <= f32::EPSILON {
        return None;
    }
    Some((rigid(target, requested)?, rigid(previous, requested)?))
}

fn rigid(mut value: [f32; 16], requested: f32) -> Option<Transform> {
    // hcl rotation stores three padded hkVector4 columns. Original169F7D0
    // writes nonzero scratch W lanes;1680980 consumes XYZ only. Preserve all
    // W/translation lanes instead of imposing a 4x4 affine matrix ABI.
    if !value.iter().all(|v| v.is_finite()) {
        return None;
    }
    let columns: [[f64; 3]; 3] =
        std::array::from_fn(|i| std::array::from_fn(|j| f64::from(value[i * 4 + j])));
    let lengths = columns.map(|c| c.iter().map(|v| v * v).sum::<f64>().sqrt());
    let unit_error = lengths.iter().map(|n| (n - 1.).abs()).fold(0., f64::max);
    let scaled_error = lengths
        .iter()
        .map(|n| (n / f64::from(requested) - 1.).abs())
        .fold(0., f64::max);
    // Near1.0 both tolerances can overlap. Select the closer authorized
    // candidate; unit history must not be divided a second time.
    let factor = if unit_error <= 0.002 && unit_error <= scaled_error {
        1.
    } else if scaled_error <= 0.002 {
        requested
    } else {
        return None;
    };
    for i in 0..3 {
        for j in i + 1..3 {
            let dot = (0..3).map(|k| columns[i][k] * columns[j][k]).sum::<f64>();
            if dot.abs() > 0.002 * lengths[i] * lengths[j] {
                return None;
            }
        }
    }
    let [a, b, c] = columns;
    let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
        + a[2] * (b[0] * c[1] - b[1] * c[0]);
    if !det.is_finite() || det <= 0. {
        return None;
    }
    // Divide by the independently authorized body factor, not an inferred
    // average length. Unit history and all translation/W lanes are retained.
    for i in 0..3 {
        for j in 0..3 {
            value[i * 4 + j] /= factor;
        }
    }
    Some(Transform(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrix(s: f32) -> [f32; 16] {
        [s, 0., 0., 0., 0., s, 0., 0., 0., 0., s, 0., 7., -3., 2., 1.]
    }

    #[test]
    fn scaled_target_and_unit_history_become_rigid_without_moving_translation() {
        for s in [
            0.1,
            0.25,
            0.5,
            0.75,
            0.999,
            1.001,
            1.5,
            3.,
            4.,
            10.,
            f32::MIN_POSITIVE,
            f32::MAX,
        ] {
            let (a, b) = rigid_pair(matrix(s), matrix(1.), s).unwrap();
            assert_eq!(a.0, matrix(1.));
            assert_eq!(b.0, matrix(1.));
            assert_eq!((a.0.as_ptr() as usize) % 16, 0);
        }
    }

    #[test]
    fn rejects_unowned_factor_shear_reflection_contraction_and_invalid_data() {
        assert!(rigid_pair(matrix(0.5), matrix(1.), 1.).is_none());
        assert!(rigid_pair(matrix(0.5), matrix(1.), 3.).is_none());
        for (index, value) in [
            (0, -0.5),
            (1, 0.1),
            (5, 0.3),
            (12, f32::NAN),
            (15, f32::INFINITY),
        ] {
            let mut bad = matrix(0.5);
            bad[index] = value;
            assert!(rigid_pair(bad, matrix(1.), 0.5).is_none());
            assert!(rigid_pair(matrix(0.5), bad, 0.5).is_none());
        }
    }

    #[test]
    fn native_rotation_padding_is_retained_and_does_not_authorize_other_basis_scale() {
        let mut target = matrix(0.5);
        let mut previous = matrix(1.);
        for (i, v) in [(3, 0.11240232), (7, 0.9933468), (11, -0.75), (15, 1.)] {
            target[i] = v;
            previous[i] = -v;
        }
        let (a, b) = rigid_pair(target, previous, 0.5).unwrap();
        for i in [3, 7, 11, 12, 13, 14, 15] {
            assert_eq!(a.0[i], target[i]);
            assert_eq!(b.0[i], previous[i]);
        }
        assert!(rigid_pair(target, previous, 3.).is_none());
    }
}
