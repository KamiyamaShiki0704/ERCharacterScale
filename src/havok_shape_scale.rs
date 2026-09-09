use eldenring::cs::PlayerIns;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
};

use crate::log;

const PHYSICS_HK_COLLISION_SHAPE_OFFSET: usize = 0xb0;
const HKNP_SHAPE_CONVEX_RADIUS_OFFSET: usize = 0x20;
const HKNP_CAPSULE_A_OFFSET: usize = 0x60;
const HKNP_CAPSULE_B_OFFSET: usize = 0x70;
const HKNP_CAPSULE_TOTAL_SIZE: usize = 0x80;

#[derive(Clone, Copy)]
pub struct HknpCapsuleBaseline {
    shape_addr: usize,
    convex_radius: f32,
    a: [f32; 4],
    b: [f32; 4],
}

pub fn scale_player_hknp_capsule_shape(
    player: &mut PlayerIns,
    baseline: &mut Option<HknpCapsuleBaseline>,
    scale: f32,
) {
    let physics = player.chr_ins.modules.as_mut().physics.as_mut();
    let physics_addr = physics as *mut _ as usize;
    let shape_addr = read_usize(physics_addr + PHYSICS_HK_COLLISION_SHAPE_OFFSET).unwrap_or(0);

    if shape_addr == 0 {
        *baseline = None;
        return;
    }

    let should_capture = baseline
        .as_ref()
        .map(|existing| existing.shape_addr != shape_addr)
        .unwrap_or(true);

    if should_capture {
        *baseline = capture_hknp_capsule_baseline(shape_addr);
    }

    let Some(capsule) = baseline.as_ref().copied() else {
        return;
    };

    if !is_writable(shape_addr, HKNP_CAPSULE_TOTAL_SIZE) {
        log::line(format_args!(
            "[player-scale-no-bone] hknp capsule scale skipped: shape=0x{shape_addr:x} not writable"
        ));
        return;
    }

    let mut scaled_a = scale_vec3(capsule.a, scale);
    let mut scaled_b = scale_vec3(capsule.b, scale);
    scaled_a[3] = scale_radius_like(capsule.a[3], scale);
    scaled_b[3] = scale_radius_like(capsule.b[3], scale);

    write_f32(
        shape_addr + HKNP_SHAPE_CONVEX_RADIUS_OFFSET,
        capsule.convex_radius * scale,
    );
    write_vec4(shape_addr + HKNP_CAPSULE_A_OFFSET, scaled_a);
    write_vec4(shape_addr + HKNP_CAPSULE_B_OFFSET, scaled_b);
}

fn capture_hknp_capsule_baseline(shape_addr: usize) -> Option<HknpCapsuleBaseline> {
    if !is_readable(shape_addr, HKNP_CAPSULE_TOTAL_SIZE) {
        log::line(format_args!(
            "[player-scale-no-bone] hknp capsule baseline failed: shape=0x{shape_addr:x} not readable"
        ));
        return None;
    }

    let convex_radius = read_f32(shape_addr + HKNP_SHAPE_CONVEX_RADIUS_OFFSET)?;
    let a = read_vec4(shape_addr + HKNP_CAPSULE_A_OFFSET)?;
    let b = read_vec4(shape_addr + HKNP_CAPSULE_B_OFFSET)?;

    if !looks_like_capsule(convex_radius, a, b) {
        log::line(format_args!(
            "[player-scale-no-bone] hknp capsule baseline rejected: shape=0x{shape_addr:x} radius={convex_radius:.6} a={a:?} b={b:?}"
        ));
        return None;
    }

    log::line(format_args!(
        "[player-scale-no-bone] hknp capsule baseline captured: shape=0x{shape_addr:x} radius={convex_radius:.6} a={a:?} b={b:?}"
    ));

    Some(HknpCapsuleBaseline {
        shape_addr,
        convex_radius,
        a,
        b,
    })
}

fn looks_like_capsule(convex_radius: f32, a: [f32; 4], b: [f32; 4]) -> bool {
    if !convex_radius.is_finite() || !(0.01..=2.0).contains(&convex_radius) {
        return false;
    }

    if !a.into_iter().chain(b).all(|value| value.is_finite()) {
        return false;
    }

    if !a
        .into_iter()
        .chain(b)
        .all(|value| (-10.0..=10.0).contains(&value))
    {
        return false;
    }

    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    let span = (dx * dx + dy * dy + dz * dz).sqrt();
    (0.05..=5.0).contains(&span)
}

fn scale_vec3(mut value: [f32; 4], scale: f32) -> [f32; 4] {
    value[0] *= scale;
    value[1] *= scale;
    value[2] *= scale;
    value
}

fn scale_radius_like(value: f32, scale: f32) -> f32 {
    if value > 0.0 && value <= 2.0 {
        value * scale
    } else {
        value
    }
}

fn read_vec4(addr: usize) -> Option<[f32; 4]> {
    Some([
        read_f32(addr)?,
        read_f32(addr + 0x4)?,
        read_f32(addr + 0x8)?,
        read_f32(addr + 0xc)?,
    ])
}

fn write_vec4(addr: usize, value: [f32; 4]) {
    write_f32(addr, value[0]);
    write_f32(addr + 0x4, value[1]);
    write_f32(addr + 0x8, value[2]);
    write_f32(addr + 0xc, value[3]);
}

fn is_readable(addr: usize, size: usize) -> bool {
    query_region(addr, size).is_some()
}

fn is_writable(addr: usize, size: usize) -> bool {
    let Some(mbi) = query_region(addr, size) else {
        return false;
    };

    mbi.Protect.contains(PAGE_READWRITE)
        || mbi.Protect.contains(PAGE_WRITECOPY)
        || mbi.Protect.contains(PAGE_EXECUTE_READWRITE)
        || mbi.Protect.contains(PAGE_EXECUTE_WRITECOPY)
}

fn query_region(addr: usize, size: usize) -> Option<MEMORY_BASIC_INFORMATION> {
    if addr == 0 || size == 0 {
        return None;
    }

    let mut mbi = MEMORY_BASIC_INFORMATION::default();
    let queried = unsafe {
        VirtualQuery(
            Some(addr as *const _),
            &mut mbi,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };

    if queried == 0 || mbi.State != MEM_COMMIT {
        return None;
    }

    if mbi.Protect.contains(PAGE_GUARD) || mbi.Protect.contains(PAGE_NOACCESS) {
        return None;
    }

    let start = addr;
    let end = addr.saturating_add(size);
    let region_start = mbi.BaseAddress as usize;
    let region_end = region_start.saturating_add(mbi.RegionSize);

    if end >= start && start >= region_start && end <= region_end {
        Some(mbi)
    } else {
        None
    }
}

fn read_usize(addr: usize) -> Option<usize> {
    if !is_readable(addr, size_of::<usize>()) {
        return None;
    }

    Some(unsafe { (addr as *const usize).read_unaligned() })
}

fn read_f32(addr: usize) -> Option<f32> {
    if !is_readable(addr, size_of::<f32>()) {
        return None;
    }

    Some(unsafe { (addr as *const f32).read_unaligned() })
}

fn write_f32(addr: usize, value: f32) {
    if !is_writable(addr, size_of::<f32>()) {
        return;
    }

    unsafe {
        (addr as *mut f32).write_unaligned(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_positive_radius_like_components_only() {
        assert_eq!(scale_radius_like(0.2, 0.5), 0.1);
        assert_eq!(scale_radius_like(0.0, 0.5), 0.0);
        assert_eq!(scale_radius_like(-1.0, 0.5), -1.0);
    }

    #[test]
    fn rejects_implausible_capsules() {
        assert!(!looks_like_capsule(0.0, [0.0; 4], [0.0; 4]));
        assert!(!looks_like_capsule(0.2, [0.0; 4], [0.0; 4]));
        assert!(looks_like_capsule(
            0.2,
            [0.0, 1.1, 0.0, 0.2],
            [0.0, 0.4, 0.0, 0.0]
        ));
    }
}
