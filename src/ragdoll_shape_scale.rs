use eldenring::cs::PlayerIns;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
};

use crate::log;

const RAGDOLL_SYSTEM_SLOT_OFFSET: usize = 0x78;
const RAGDOLL_SYSTEM_DATA_OFFSET: usize = 0x18;
const HKNP_PHYSICS_SYSTEM_BODY_CINFOS_OFFSET: usize = 0x38;
const HKNP_BODY_CINFO_WITH_ATTACHMENT_SIZE: usize = 0xc0;
const HKNP_BODY_CINFO_SHAPE_OFFSET: usize = 0x00;
const HKNP_SHAPE_CONVEX_RADIUS_OFFSET: usize = 0x20;
const HKNP_CAPSULE_A_OFFSET: usize = 0x60;
const HKNP_CAPSULE_B_OFFSET: usize = 0x70;
const HKNP_CAPSULE_TOTAL_SIZE: usize = 0x80;
const MAX_BODY_CINFOS: i32 = 64;

#[derive(Default)]
pub struct RagdollBodyCinfoShapeScaleState {
    baseline: Option<RagdollBodyCinfoShapeBaseline>,
    last_logged_scale: f32,
}

struct RagdollBodyCinfoShapeBaseline {
    ragdoll_addr: usize,
    system_data_addr: usize,
    body_cinfos_data: usize,
    body_cinfos_size: i32,
    capsules: Vec<CapsuleShapeBaseline>,
}

#[derive(Clone, Copy)]
struct CapsuleShapeBaseline {
    shape_addr: usize,
    convex_radius: f32,
    a: [f32; 4],
    b: [f32; 4],
}

pub fn scale_player_ragdoll_body_cinfo_capsules(
    player: &mut PlayerIns,
    state: &mut RagdollBodyCinfoShapeScaleState,
    scale: f32,
) {
    let Some(candidate) = find_candidate(player) else {
        state.baseline = None;
        state.last_logged_scale = 0.0;
        return;
    };

    let should_capture = state
        .baseline
        .as_ref()
        .map(|baseline| {
            baseline.ragdoll_addr != candidate.ragdoll_addr
                || baseline.system_data_addr != candidate.system_data_addr
                || baseline.body_cinfos_data != candidate.body_cinfos_data
                || baseline.body_cinfos_size != candidate.body_cinfos_size
        })
        .unwrap_or(true);

    if should_capture {
        state.baseline = capture_baseline(candidate);
        state.last_logged_scale = 0.0;
    }

    let Some(baseline) = state.baseline.as_ref() else {
        return;
    };

    let mut written = 0usize;
    for capsule in &baseline.capsules {
        if !is_writable(capsule.shape_addr, HKNP_CAPSULE_TOTAL_SIZE) {
            continue;
        }

        write_capsule_unchecked(*capsule, scale);
        written += 1;
    }

    if (state.last_logged_scale - scale).abs() > f32::EPSILON {
        log::line(format_args!(
            "[player-scale-no-bone] ragdoll body_cinfo capsule write: system_data=0x{:x} scale={scale:.3} written={written}/{}",
            baseline.system_data_addr,
            baseline.capsules.len()
        ));
        state.last_logged_scale = scale;
    }
}

fn find_candidate(player: &mut PlayerIns) -> Option<Candidate> {
    let ragdoll_addr = player.chr_ins.chr_ctrl.as_mut().ragdoll_ins;
    let system_addr = read_usize(ragdoll_addr + RAGDOLL_SYSTEM_SLOT_OFFSET)?;
    let system_data_addr = read_usize(system_addr + RAGDOLL_SYSTEM_DATA_OFFSET)?;
    let body_cinfos = read_hk_array(system_data_addr + HKNP_PHYSICS_SYSTEM_BODY_CINFOS_OFFSET)?;

    if body_cinfos.size <= 0 || body_cinfos.size > MAX_BODY_CINFOS {
        return None;
    }

    let bytes = body_cinfos.size.max(0) as usize * HKNP_BODY_CINFO_WITH_ATTACHMENT_SIZE;
    if !is_readable(body_cinfos.data, bytes) {
        return None;
    }

    Some(Candidate {
        ragdoll_addr,
        system_data_addr,
        body_cinfos_data: body_cinfos.data,
        body_cinfos_size: body_cinfos.size,
    })
}

fn capture_baseline(candidate: Candidate) -> Option<RagdollBodyCinfoShapeBaseline> {
    let mut capsules = Vec::new();
    for index in 0..candidate.body_cinfos_size as usize {
        let entry = candidate.body_cinfos_data + index * HKNP_BODY_CINFO_WITH_ATTACHMENT_SIZE;
        let shape_addr = read_usize(entry + HKNP_BODY_CINFO_SHAPE_OFFSET).unwrap_or(0);
        let Some(capsule) = capture_capsule_baseline(shape_addr) else {
            continue;
        };
        capsules.push(capsule);
    }

    if capsules.is_empty() {
        log::line(format_args!(
            "[player-scale-no-bone] ragdoll body_cinfo capsule baseline rejected: system_data=0x{:x} body_cinfos={} no writable capsule shapes",
            candidate.system_data_addr, candidate.body_cinfos_size
        ));
        return None;
    }

    log::line(format_args!(
        "[player-scale-no-bone] ragdoll body_cinfo capsule baseline captured: system_data=0x{:x} body_cinfos={} capsules={}",
        candidate.system_data_addr,
        candidate.body_cinfos_size,
        capsules.len()
    ));

    Some(RagdollBodyCinfoShapeBaseline {
        ragdoll_addr: candidate.ragdoll_addr,
        system_data_addr: candidate.system_data_addr,
        body_cinfos_data: candidate.body_cinfos_data,
        body_cinfos_size: candidate.body_cinfos_size,
        capsules,
    })
}

fn capture_capsule_baseline(shape_addr: usize) -> Option<CapsuleShapeBaseline> {
    if shape_addr <= 0x10000
        || !is_readable(shape_addr, HKNP_CAPSULE_TOTAL_SIZE)
        || !is_writable(shape_addr, HKNP_CAPSULE_TOTAL_SIZE)
    {
        return None;
    }

    let convex_radius = read_f32(shape_addr + HKNP_SHAPE_CONVEX_RADIUS_OFFSET)?;
    let a = read_vec4(shape_addr + HKNP_CAPSULE_A_OFFSET)?;
    let b = read_vec4(shape_addr + HKNP_CAPSULE_B_OFFSET)?;
    if !looks_like_capsule(convex_radius, a, b) {
        return None;
    }

    Some(CapsuleShapeBaseline {
        shape_addr,
        convex_radius,
        a,
        b,
    })
}

fn write_capsule_unchecked(capsule: CapsuleShapeBaseline, scale: f32) {
    let mut scaled_a = scale_vec3(capsule.a, scale);
    let mut scaled_b = scale_vec3(capsule.b, scale);
    scaled_a[3] = scale_radius_like(capsule.a[3], scale);
    scaled_b[3] = scale_radius_like(capsule.b[3], scale);

    unsafe {
        ((capsule.shape_addr + HKNP_SHAPE_CONVEX_RADIUS_OFFSET) as *mut f32)
            .write_unaligned(capsule.convex_radius * scale);
    }
    write_vec4_unchecked(capsule.shape_addr + HKNP_CAPSULE_A_OFFSET, scaled_a);
    write_vec4_unchecked(capsule.shape_addr + HKNP_CAPSULE_B_OFFSET, scaled_b);
}

fn write_vec4_unchecked(addr: usize, value: [f32; 4]) {
    for (index, component) in value.into_iter().enumerate() {
        unsafe {
            ((addr + index * size_of::<f32>()) as *mut f32).write_unaligned(component);
        }
    }
}

fn looks_like_capsule(convex_radius: f32, a: [f32; 4], b: [f32; 4]) -> bool {
    if !convex_radius.is_finite() || !(0.005..=2.0).contains(&convex_radius) {
        return false;
    }

    if !a
        .into_iter()
        .chain(b)
        .all(|value| value.is_finite() && (-20.0..=20.0).contains(&value))
    {
        return false;
    }

    let span = distance3(a, b);
    let radius_like = a[3] > 0.0 && a[3] <= 2.0;
    radius_like && (0.01..=10.0).contains(&span)
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

fn distance3(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

#[derive(Clone, Copy)]
struct Candidate {
    ragdoll_addr: usize,
    system_data_addr: usize,
    body_cinfos_data: usize,
    body_cinfos_size: i32,
}

#[derive(Clone, Copy)]
struct HkArray {
    data: usize,
    size: i32,
}

fn read_hk_array(addr: usize) -> Option<HkArray> {
    let data = read_usize(addr)?;
    let size = read_i32(addr + 0x8)?;
    let capacity_and_flags = read_i32(addr + 0xc)?;
    let capacity = capacity_and_flags & 0x3fff_ffff;
    if data <= 0x10000 || size < 0 || capacity < size {
        return None;
    }

    Some(HkArray { data, size })
}

fn read_vec4(addr: usize) -> Option<[f32; 4]> {
    Some([
        read_f32(addr)?,
        read_f32(addr + 0x4)?,
        read_f32(addr + 0x8)?,
        read_f32(addr + 0xc)?,
    ])
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

fn read_i32(addr: usize) -> Option<i32> {
    if !is_readable(addr, size_of::<i32>()) {
        return None;
    }

    Some(unsafe { (addr as *const i32).read_unaligned() })
}

fn read_f32(addr: usize) -> Option<f32> {
    if !is_readable(addr, size_of::<f32>()) {
        return None;
    }

    Some(unsafe { (addr as *const f32).read_unaligned() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_logged_capsule_shape() {
        assert!(looks_like_capsule(
            0.11709,
            [0.002, 0.064, 0.001, 0.118],
            [0.002, -0.063, 0.001, 0.0]
        ));
    }

    #[test]
    fn scales_radius_like_w_only_when_positive() {
        assert_eq!(scale_radius_like(0.2, 0.85), 0.17);
        assert_eq!(scale_radius_like(0.0, 0.85), 0.0);
    }
}
