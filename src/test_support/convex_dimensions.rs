use super::*;
use crate::body_scale_port::test_characters::{BASE, Environment};

#[test]
fn convex_plane_collection_scales_distances_bounds_and_restores_on_both_profiles() {
    let _environment = Environment::new();
    for newer in [false, true] {
        let _profile = crate::compatibility::owned_image_profile(newer);
        let mut shape = [0usize; 0xe0 / 8];
        let mut planes = [
            [1.0f32, 0.0, 0.0, -2.0],
            [-1.0, 0.0, 0.0, -2.0],
            [0.0, 1.0, 0.0, -1.0],
        ];
        shape[0] = BASE + crate::compatibility::rva(ER_HCL_CONVEX_PLANES_SHAPE_VTABLE_RVA);
        shape[0x20 / 8] = planes.as_mut_ptr() as usize;
        shape[0x28 / 8] = planes.len();
        let address = shape.as_mut_ptr() as usize;
        for (i, off) in [0xb0, 0xc0, 0xd0].into_iter().enumerate() {
            for k in 0..4 {
                write_f32(address + off + k * 4, (i * 4 + k + 1) as f32);
            }
        }
        let original_shape = shape;
        let original_planes = planes;
        let mut baseline = capture_shape_baseline(address).expect("convex plane shape rejected");
        for scale in [0.3, 0.85, 2.0, 1.0] {
            baseline.apply(scale).unwrap();
            for (actual, original) in planes.iter().zip(original_planes) {
                assert_eq!(actual[..3], original[..3], "unit normal changed");
                assert_eq!(actual[3], original[3] * scale);
            }
            for (off, words) in [(0xb0, 3), (0xc0, 3), (0xd0, 3)] {
                for k in 0..words {
                    let old = read_f32(original_shape.as_ptr() as usize + off + k * 4).unwrap();
                    assert_eq!(read_f32(address + off + k * 4), Some(old * scale));
                }
                assert_eq!(
                    read_f32(address + off + 12),
                    read_f32(original_shape.as_ptr() as usize + off + 12)
                );
            }
            assert_eq!(
                &shape[..0xb0 / 8],
                &original_shape[..0xb0 / 8],
                "matrix/header changed"
            );
        }
        assert_eq!(shape, original_shape);
        assert_eq!(planes, original_planes);
    }
}

#[test]
fn convex_planes_grid_and_contact_distances_remain_coherent() {
    let _environment = Environment::new();
    let mut shape = [0usize; 0x120 / 8];
    let mut planes = [
        1.0f32, 0.0, 0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -3.0, -4.0, -5.0, -2.0,
    ];
    let grid = [0u16, 1];
    let cells = [0u8];
    shape[0] = BASE + ER_HCL_CONVEX_GEOMETRY_SHAPE_VTABLE_RVA;
    shape[0x20 / 8] = grid.as_ptr() as usize;
    shape[0x28 / 8] = grid.len();
    shape[0x30 / 8] = cells.as_ptr() as usize;
    shape[0x38 / 8] = cells.len();
    shape[0x40 / 8] = planes.as_mut_ptr() as usize;
    shape[0x48 / 8] = 1;
    let address = shape.as_ptr() as usize;
    for (offset, values) in [
        (0xD0, [-2.0, -3.0, -4.0]),
        (0xE0, [3.0, 4.0, 5.0]),
        (0xF0, [0.25, 0.5, 0.75]),
        (0x100, [0.2, 1.0 / 7.0, 1.0 / 9.0]),
    ] {
        for (i, v) in values.into_iter().enumerate() {
            write_f32(address + offset + i * 4, v);
        }
    }
    // Deliberately nonzero W/transform lanes: they must remain byte-identical.
    for off in (0x50..0xD0).step_by(4).chain([0xDC, 0xEC, 0xFC, 0x10C]) {
        write_f32(address + off, 0.375);
    }
    let original_shape = shape;
    let original_planes = planes;
    let mut baseline = capture_shape_baseline(address).unwrap();
    for scale in [0.15, 0.85, 2.0, 0.15, 1.0] {
        baseline.apply(scale).unwrap();
        assert_eq!(
            &planes[..12],
            &original_planes[..12],
            "normals must stay dimensionless"
        );
        for off in (0..0xD0)
            .step_by(4)
            .chain([0xDC, 0xEC, 0xFC, 0x10C, 0x110, 0x114, 0x118, 0x11C])
        {
            assert_eq!(
                read_f32(address + off).unwrap().to_bits(),
                read_f32(original_shape.as_ptr() as usize + off)
                    .unwrap()
                    .to_bits(),
                "non-dimension byte changed at {off:x}"
            );
        }
        for point in [
            [0.1, 0.2, 0.3],
            [2.99, 3.99, 4.99],
            [3.01, 4.01, 5.01],
            [-2.1, 0.0, 0.0],
        ] {
            for (axis, coordinate) in point.iter().enumerate() {
                let original_min =
                    read_f32(original_shape.as_ptr() as usize + 0xD0 + axis * 4).unwrap();
                let original_inv =
                    read_f32(original_shape.as_ptr() as usize + 0x100 + axis * 4).unwrap();
                let expected = (coordinate - original_min) * original_inv;
                let actual = (coordinate * scale - read_f32(address + 0xD0 + axis * 4).unwrap())
                    * read_f32(address + 0x100 + axis * 4).unwrap();
                assert!((actual - expected).abs() < 1e-5, "grid lookup changed");
            }
            // Original ER 15CE950 / 15CED00: four independent plane distances.
            for lane in 0..4 {
                let distance = original_planes[lane] * point[0]
                    + original_planes[4 + lane] * point[1]
                    + original_planes[8 + lane] * point[2]
                    + original_planes[12 + lane];
                let actual = planes[lane] * point[0] * scale
                    + planes[4 + lane] * point[1] * scale
                    + planes[8 + lane] * point[2] * scale
                    + planes[12 + lane];
                assert!(
                    (actual - distance * scale).abs() < 1e-5,
                    "contact lane {lane}"
                );
                assert_eq!(actual <= 0.0, distance <= 0.0, "inside/outside changed");
            }
        }
    }
    assert_eq!(shape, original_shape);
    assert_eq!(planes, original_planes);
    assert_eq!(grid, [0, 1]);
    assert_eq!(cells, [0]);
    // A changed equation allocation invalidates even restoration before writes.
    shape[0x40 / 8] = 0;
    let stale = shape;
    assert!(baseline.apply(0.15).is_none());
    assert_eq!(shape, stale);
    shape[0x40 / 8] = planes.as_mut_ptr() as usize;
    planes[15] = f32::NAN;
    assert!(
        capture_shape_baseline(address).is_none(),
        "fourth distance must be checked"
    );
    assert!(planes[15].is_nan());
    assert_eq!(shape, original_shape);
    planes[15] = original_planes[15];
    shape[0x48 / 8] = 65_536;
    assert!(
        capture_shape_baseline(address).is_none(),
        "unbounded array must be rejected"
    );
    assert_eq!(shape[0x48 / 8], 65_536);
    assert_eq!(planes, original_planes);
}
