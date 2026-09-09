use eldenring::cs::PlayerIns;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
};

use crate::log;

const CHR_INS_MODEL_INS_OFFSET: usize = 0x50;
const CSMODELINS_MODEL_ITEM_OFFSET: usize = 0x10;
const MODEL_ITEM_LOCATION_AABB_EXPORTER_OFFSET: usize = 0x658;
const LOCATION_AABB_EXPORTER_CHILD_OFFSET: usize = 0x70;
const AABB_CHILD_MATRIX_COUNT_OFFSET: usize = 0x50;
const AABB_CHILD_IDENTITY_MATRIX_BUFFER_OFFSET: usize = 0x58;
const MTX43_STRIDE: usize = 0x30;
const MTX43_FLOAT_COUNT: usize = 12;
const MAX_MATRIX_COUNT: usize = 0x370;
const IDENTITY_EPSILON: f32 = 0.002;

#[derive(Default)]
pub struct IdentityMatrixScaleState {
    baseline: Option<IdentityMatrixBufferBaseline>,
    last_logged_scale: f32,
}

pub struct IdentityMatrixBufferBaseline {
    buffer_addr: usize,
    count: usize,
    matrices: Vec<[f32; MTX43_FLOAT_COUNT]>,
    identity_indices: Vec<usize>,
}

pub fn scale_player_aabb_identity_matrices(
    player: &PlayerIns,
    state: &mut IdentityMatrixScaleState,
    scale: f32,
) {
    let Some(candidate) = find_candidate_buffer(player) else {
        state.baseline = None;
        state.last_logged_scale = 0.0;
        return;
    };

    let should_capture = state
        .baseline
        .as_ref()
        .map(|baseline| {
            baseline.buffer_addr != candidate.buffer_addr || baseline.count != candidate.count
        })
        .unwrap_or(true);

    if should_capture {
        state.baseline = capture_baseline(candidate);
        state.last_logged_scale = 0.0;
    }

    let Some(baseline) = state.baseline.as_ref() else {
        return;
    };

    let bytes = baseline.count.saturating_mul(MTX43_STRIDE);
    if !is_writable(baseline.buffer_addr, bytes) {
        log::line(format_args!(
            "[player-scale-no-bone] aabb identity matrix scale skipped: buffer=0x{:x} bytes=0x{bytes:x} not writable",
            baseline.buffer_addr
        ));
        return;
    }

    for &index in &baseline.identity_indices {
        let matrix = baseline.matrices[index];
        let scaled = scale_matrix_basis(matrix, scale);
        write_mtx43_unchecked(baseline.buffer_addr + index * MTX43_STRIDE, scaled);
    }

    if (state.last_logged_scale - scale).abs() > f32::EPSILON {
        log::line(format_args!(
            "[player-scale-no-bone] aabb identity matrix scale write: buffer=0x{:x} scale={scale:.3} written={written}/{}",
            baseline.buffer_addr,
            baseline.count,
            written = baseline.identity_indices.len(),
        ));
        state.last_logged_scale = scale;
    }
}

fn find_candidate_buffer(player: &PlayerIns) -> Option<CandidateBuffer> {
    let chr_ins_addr = &player.chr_ins as *const _ as usize;
    let chr_model_ins = read_usize(chr_ins_addr + CHR_INS_MODEL_INS_OFFSET)?;
    let model_item = read_usize(chr_model_ins + CSMODELINS_MODEL_ITEM_OFFSET)?;
    let location_aabb_exporter = read_usize(model_item + MODEL_ITEM_LOCATION_AABB_EXPORTER_OFFSET)?;
    let aabb_child = read_usize(location_aabb_exporter + LOCATION_AABB_EXPORTER_CHILD_OFFSET)?;
    let buffer_addr = read_usize(aabb_child + AABB_CHILD_IDENTITY_MATRIX_BUFFER_OFFSET)?;
    let count = read_u32(aabb_child + AABB_CHILD_MATRIX_COUNT_OFFSET)? as usize;

    if !(1..=MAX_MATRIX_COUNT).contains(&count) {
        log::line(format_args!(
            "[player-scale-no-bone] aabb identity matrix candidate rejected: aabb_child=0x{aabb_child:x} count=0x{count:x}"
        ));
        return None;
    }

    Some(CandidateBuffer { buffer_addr, count })
}

fn capture_baseline(candidate: CandidateBuffer) -> Option<IdentityMatrixBufferBaseline> {
    let bytes = candidate.count.saturating_mul(MTX43_STRIDE);
    if !is_readable(candidate.buffer_addr, bytes) {
        log::line(format_args!(
            "[player-scale-no-bone] aabb identity matrix baseline failed: buffer=0x{:x} bytes=0x{bytes:x} not readable",
            candidate.buffer_addr
        ));
        return None;
    }

    let mut matrices = Vec::with_capacity(candidate.count);
    let mut identity_indices = Vec::new();
    for index in 0..candidate.count {
        let matrix = read_mtx43(candidate.buffer_addr + index * MTX43_STRIDE)?;
        if looks_like_identity_matrix(&matrix) {
            identity_indices.push(index);
        }
        matrices.push(matrix);
    }

    if identity_indices.is_empty() {
        log::line(format_args!(
            "[player-scale-no-bone] aabb identity matrix baseline rejected: buffer=0x{:x} count={} no identity matrices",
            candidate.buffer_addr, candidate.count
        ));
        return None;
    }

    log::line(format_args!(
        "[player-scale-no-bone] aabb identity matrix baseline captured: buffer=0x{:x} count={} identity={identity_count}",
        candidate.buffer_addr,
        candidate.count,
        identity_count = identity_indices.len(),
    ));

    Some(IdentityMatrixBufferBaseline {
        buffer_addr: candidate.buffer_addr,
        count: candidate.count,
        matrices,
        identity_indices,
    })
}

fn looks_like_identity_matrix(matrix: &[f32; MTX43_FLOAT_COUNT]) -> bool {
    let expected = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    matrix.iter().zip(expected).all(|(actual, expected)| {
        actual.is_finite() && (actual - expected).abs() <= IDENTITY_EPSILON
    })
}

fn scale_matrix_basis(
    mut matrix: [f32; MTX43_FLOAT_COUNT],
    scale: f32,
) -> [f32; MTX43_FLOAT_COUNT] {
    for index in [0, 1, 2, 4, 5, 6, 8, 9, 10] {
        matrix[index] *= scale;
    }
    matrix
}

fn read_mtx43(addr: usize) -> Option<[f32; MTX43_FLOAT_COUNT]> {
    let mut matrix = [0.0; MTX43_FLOAT_COUNT];
    for (index, value) in matrix.iter_mut().enumerate() {
        *value = read_f32(addr + index * size_of::<f32>())?;
    }
    Some(matrix)
}

fn write_mtx43_unchecked(addr: usize, matrix: [f32; MTX43_FLOAT_COUNT]) {
    for (index, value) in matrix.into_iter().enumerate() {
        unsafe {
            ((addr + index * size_of::<f32>()) as *mut f32).write_unaligned(value);
        }
    }
}

#[derive(Clone, Copy)]
struct CandidateBuffer {
    buffer_addr: usize,
    count: usize,
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

fn read_u32(addr: usize) -> Option<u32> {
    if !is_readable(addr, size_of::<u32>()) {
        return None;
    }

    Some(unsafe { (addr as *const u32).read_unaligned() })
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
    fn recognizes_identity_matrix() {
        let matrix = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        assert!(looks_like_identity_matrix(&matrix));
    }

    #[test]
    fn rejects_non_identity_matrix() {
        let matrix = [
            0.85, 0.0, 0.0, 0.0, 0.0, 0.85, 0.0, 0.0, 0.0, 0.0, 0.85, 0.0,
        ];
        assert!(!looks_like_identity_matrix(&matrix));
    }

    #[test]
    fn scales_basis_but_preserves_translation() {
        let matrix = [1.0, 0.0, 0.0, 4.0, 0.0, 1.0, 0.0, 5.0, 0.0, 0.0, 1.0, 6.0];
        let scaled = scale_matrix_basis(matrix, 0.85);
        assert_eq!(
            scaled,
            [
                0.85, 0.0, 0.0, 4.0, 0.0, 0.85, 0.0, 5.0, 0.0, 0.0, 0.85, 6.0
            ]
        );
    }
}
