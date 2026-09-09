use eldenring::cs::PlayerIns;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
};

use crate::log;

const RAGDOLL_LIVE_MOTION_ROOT_OFFSET: usize = 0xe0;
const LIVE_MOTION_ARRAY_A_OFFSET: usize = 0x08;
const LIVE_MOTION_ARRAY_B_OFFSET: usize = 0x18;
const HKNP_MOTION_SIZE: usize = 0x80;
const HKNP_MOTION_CENTER_OF_MASS_OFFSET: usize = 0x00;
const EXPECTED_MOTION_COUNT: i32 = 18;
const MAX_MOTION_COUNT: i32 = 64;

#[derive(Default)]
pub struct RagdollMotionScaleState {
    baseline: Option<RagdollMotionBaseline>,
    last_logged_scale: f32,
}

struct RagdollMotionBaseline {
    ragdoll_addr: usize,
    array_a_data: usize,
    array_b_data: usize,
    count: i32,
    array_a_coms: Vec<[f32; 4]>,
    array_b_coms: Vec<[f32; 4]>,
}

pub fn scale_player_ragdoll_motion_coms(
    player: &mut PlayerIns,
    state: &mut RagdollMotionScaleState,
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
                || baseline.array_a_data != candidate.array_a.data
                || baseline.array_b_data != candidate.array_b.data
                || baseline.count != candidate.array_a.size
        })
        .unwrap_or(true);

    if should_capture {
        state.baseline = capture_baseline(candidate);
        state.last_logged_scale = 0.0;
    }

    let Some(baseline) = state.baseline.as_ref() else {
        return;
    };

    let bytes = baseline.count as usize * HKNP_MOTION_SIZE;
    if !is_writable(baseline.array_a_data, bytes) || !is_writable(baseline.array_b_data, bytes) {
        log::line(format_args!(
            "[player-scale-no-bone] ragdoll motion com scale skipped: writable=false array_a=0x{:x} array_b=0x{:x} bytes=0x{bytes:x}",
            baseline.array_a_data, baseline.array_b_data
        ));
        return;
    }

    for index in 0..baseline.count as usize {
        write_scaled_com(
            baseline.array_a_data + index * HKNP_MOTION_SIZE + HKNP_MOTION_CENTER_OF_MASS_OFFSET,
            baseline.array_a_coms[index],
            scale,
        );
        write_scaled_com(
            baseline.array_b_data + index * HKNP_MOTION_SIZE + HKNP_MOTION_CENTER_OF_MASS_OFFSET,
            baseline.array_b_coms[index],
            scale,
        );
    }

    if (state.last_logged_scale - scale).abs() > f32::EPSILON {
        log::line(format_args!(
            "[player-scale-no-bone] ragdoll motion com write: scale={scale:.3} motions={} array_a=0x{:x} array_b=0x{:x}",
            baseline.count, baseline.array_a_data, baseline.array_b_data
        ));
        state.last_logged_scale = scale;
    }
}

fn find_candidate(player: &mut PlayerIns) -> Option<Candidate> {
    let ragdoll_addr = player.chr_ins.chr_ctrl.as_mut().ragdoll_ins;
    let live_root = read_usize(ragdoll_addr + RAGDOLL_LIVE_MOTION_ROOT_OFFSET)?;
    let array_a = read_hk_array(live_root + LIVE_MOTION_ARRAY_A_OFFSET)?;
    let array_b = read_hk_array(live_root + LIVE_MOTION_ARRAY_B_OFFSET)?;

    if array_a.size != EXPECTED_MOTION_COUNT
        || array_b.size != EXPECTED_MOTION_COUNT
        || array_a.size != array_b.size
        || array_a.size <= 0
        || array_a.size > MAX_MOTION_COUNT
    {
        return None;
    }

    let bytes = array_a.size as usize * HKNP_MOTION_SIZE;
    if !is_readable(array_a.data, bytes) || !is_readable(array_b.data, bytes) {
        return None;
    }

    Some(Candidate {
        ragdoll_addr,
        array_a,
        array_b,
    })
}

fn capture_baseline(candidate: Candidate) -> Option<RagdollMotionBaseline> {
    let count = candidate.array_a.size as usize;
    let mut array_a_coms = Vec::with_capacity(count);
    let mut array_b_coms = Vec::with_capacity(count);

    for index in 0..count {
        let array_a_com = read_vec4(candidate.array_a.data + index * HKNP_MOTION_SIZE)?;
        let array_b_com = read_vec4(candidate.array_b.data + index * HKNP_MOTION_SIZE)?;
        if !looks_like_com(array_a_com) || !looks_like_com(array_b_com) {
            log::line(format_args!(
                "[player-scale-no-bone] ragdoll motion com baseline rejected: index={index} array_a={array_a_com:?} array_b={array_b_com:?}"
            ));
            return None;
        }
        array_a_coms.push(array_a_com);
        array_b_coms.push(array_b_com);
    }

    log::line(format_args!(
        "[player-scale-no-bone] ragdoll motion com baseline captured: motions={} array_a=0x{:x} array_b=0x{:x}",
        candidate.array_a.size, candidate.array_a.data, candidate.array_b.data
    ));

    Some(RagdollMotionBaseline {
        ragdoll_addr: candidate.ragdoll_addr,
        array_a_data: candidate.array_a.data,
        array_b_data: candidate.array_b.data,
        count: candidate.array_a.size,
        array_a_coms,
        array_b_coms,
    })
}

fn write_scaled_com(addr: usize, baseline: [f32; 4], scale: f32) {
    let scaled = [
        baseline[0] * scale,
        baseline[1] * scale,
        baseline[2] * scale,
        baseline[3],
    ];
    for (index, value) in scaled.into_iter().enumerate() {
        unsafe {
            ((addr + index * size_of::<f32>()) as *mut f32).write_unaligned(value);
        }
    }
}

fn looks_like_com(value: [f32; 4]) -> bool {
    value
        .into_iter()
        .all(|component| component.is_finite() && (-100.0..=100.0).contains(&component))
}

#[derive(Clone, Copy)]
struct Candidate {
    ragdoll_addr: usize,
    array_a: HkArray,
    array_b: HkArray,
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
    fn preserves_center_of_mass_w() {
        let value = [1.0, 2.0, 3.0, 0.92845];
        let scale = 0.85;
        let scaled = [
            value[0] * scale,
            value[1] * scale,
            value[2] * scale,
            value[3],
        ];
        let expected: [f32; 4] = [0.85, 1.7, 2.55, 0.92845];
        for (actual, expected) in scaled.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 0.00001);
        }
    }

    #[test]
    fn accepts_logged_com() {
        assert!(looks_like_com([0.0, 0.97752, -0.00013, 0.92845]));
    }
}
