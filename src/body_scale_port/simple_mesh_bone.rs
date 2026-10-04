//! SimpleMeshBoneDeform constructs a raw area normal internally. Give one
//! synchronous invocation private bind coefficients for the current body scale.
//! Authored/shared operators and simulated particle positions stay untouched.
use super::*;

pub(super) const ENTRY: usize = 0x15E3500;
const VTABLE: usize = 0x2D7DE30;
const ENTRY_BYTES: &[u8] = &[
    0x48, 0x8B, 0x42, 0x10, 0x4C, 0x63, 0x51, 0x4C, 0x4C, 0x8B, 0x58, 0x20, 0x4C, 0x8B, 0x40, 0x30,
    0x48, 0x63, 0x41, 0x48, 0x4F, 0x8B, 0x04, 0xD0, 0x4D, 0x8B, 0x0C, 0xC3, 0x49, 0x63, 0x91, 0x10,
    0x01, 0, 0, 0x49, 0x8B, 0x14, 0xD3, 0xE9, 4, 0, 0, 0,
];

pub(super) fn validate(base: usize) -> bool {
    bytes_equal(base + ENTRY, ENTRY_BYTES)
        && read_usize(base + VTABLE + 0x20) == Some(base + ENTRY)
        // Local bind array, triangle cross, translation composition and store.
        && bytes_equal(base + 0x15E3579, &[0x4C, 0x8B, 0x4F, 0x50, 0x4C, 0x8B, 0x47, 0x60])
        && bytes_equal(base + 0x15E3637, &[0x0F, 0xC6, 0xE4, 0xC9, 0x0F, 0x54, 0xE0])
        && bytes_equal(base + 0x15E36C5, &[0x0F, 0xC6, 0xCA, 0xAA, 0x0F, 0x59, 0xCC])
        && bytes_equal(base + 0x15E370C, &[0x44, 0x0F, 0x29, 0x40, 0x30])
}

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct Matrix([f32; 16]);

#[repr(C, align(16))]
#[derive(Default)]
struct Operator([u64; 14]);

thread_local! {
    static SCRATCH: RefCell<Vec<Matrix>> = const { RefCell::new(Vec::new()) };
}

/// Reject the entire batch before changing any coefficient. The native frame
/// has linear edge columns and a quadratic cross column, hence inverse scale
/// on row Z of the bind (including the bone's offset from the triangle).
fn compensate(binds: &mut [Matrix], scale: f32) -> Option<()> {
    if !crate::scale_math::valid(scale) || binds.is_empty() || binds.len() > 8192 {
        return None;
    }
    if binds.iter().any(|m| {
        m.0.iter().any(|v| !v.is_finite())
            || [2, 6, 10, 14]
                .into_iter()
                .any(|i| !crate::scale_math::representable(m.0[i], m.0[i] / scale))
    }) {
        return None;
    }
    for m in binds {
        for i in [2, 6, 10, 14] {
            m.0[i] /= scale;
        }
    }
    Some(())
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Marker {
    pub(super) root: usize,
    route: ClothOwnerRoute,
    slot: usize,
    generation: u64,
    pub(super) scale: f32,
}

fn scope(op: usize, context: usize) -> Option<Marker> {
    operator_scope(op, context, VTABLE)
}

pub(super) fn operator_scope(op: usize, context: usize, vtable: usize) -> Option<Marker> {
    let state = current_unit_state();
    let base = MODULE_BASE.load(Ordering::Acquire);
    let scale = current_scale();
    if !HOOKS_READY.load(Ordering::Acquire)
        || base == 0
        || !valid_active_scale(scale)
        || read_usize(op)? != base.checked_add(vtable)?
    {
        return None;
    }
    let root = read_usize(context.checked_add(0x10)?)?;
    let asset = read_usize(root.checked_add(0x18)?)?;
    if read_usize(asset)? != base.checked_add(0x2D8B738)? {
        return None;
    }
    let operators = read_usize(asset.checked_add(0x50)?)?;
    let count = bounded_i32_count(asset.checked_add(0x58)?, 256)?;
    if (0..count)
        .filter(|i| operators.checked_add(i * 8).and_then(read_usize) == Some(op))
        .count()
        != 1
    {
        return None;
    }
    let generation = cloth_topology_generation();
    let owned = *state.target_cloth_scope.try_read().ok()?;
    if owned.anchor == 0 || owned.anchor != state.target_cloth_pose_importer.load(Ordering::Acquire)
    {
        return None;
    }
    let mut selected = None;
    for route in owned.routes.iter().copied().filter(|r| r.owner != 0) {
        let Some(slot) = (0..CLOTH_INSTANCE_SLOTS).find(|&i| {
            state.cloth_instance_owners[i].load(Ordering::Acquire) == route.owner
                && state.cloth_instance_inputs[i].load(Ordering::Acquire) == route.input
                && state.cloth_instance_cores[i].load(Ordering::Acquire) == route.core
                && state.cloth_instance_applied_scale_bits[i].load(Ordering::Acquire)
                    == scale.to_bits()
                && state.cloth_instance_pending_scale_bits[i].load(Ordering::Acquire)
                    == NO_PENDING_CLOTH_SCALE_BITS
        }) else {
            continue;
        };
        if !owned.route_is_current(route, base, read_usize) {
            continue;
        }
        let (groups, count) = bounded_vector_span(route.core, 0x28, 0x30, 8, MAX_CLOTH_GROUPS)?;
        for i in 0..count {
            if read_usize(read_usize(groups.checked_add(i * 8)?)?)? == root {
                if selected.is_some() {
                    return None;
                }
                selected = Some(Marker {
                    root,
                    route,
                    slot,
                    generation,
                    scale,
                });
            }
        }
    }
    (generation == cloth_topology_generation())
        .then_some(selected)
        .flatten()
}

fn prepare(op: usize, context: usize, scratch: &mut Vec<Matrix>) -> Option<Operator> {
    let marker = scope(op, context)?;
    let count = bounded_i32_count(op.checked_add(0x68)?, 8192)?;
    if count == 0 || bounded_i32_count(op.checked_add(0x58)?, 8192)? != count {
        return None;
    }
    let binds = read_usize(op.checked_add(0x60)?)?;
    if !is_memory_accessible(op, 0x70, false) || !is_memory_accessible(binds, count * 64, false) {
        return None;
    }
    let mut private = Operator(unsafe { (op as *const [u64; 14]).read_unaligned() });
    scratch.resize(count, Matrix::default());
    unsafe {
        std::ptr::copy_nonoverlapping(binds as *const u8, scratch.as_mut_ptr().cast(), count * 64);
    }
    compensate(scratch, marker.scale)?;
    // Identity and committed scale must still describe this invocation.
    if scope(op, context) != Some(marker)
        || unsafe { (op as *const [u64; 14]).read_unaligned() } != private.0
    {
        return None;
    }
    private.0[0x60 / 8] = scratch.as_ptr() as u64;
    Some(private)
}

pub(super) fn hook(registers: *mut Registers, original: usize) -> usize {
    let r = unsafe { &*registers };
    let (op, context) = (r.rcx as usize, r.rdx as usize);
    let native: unsafe extern "C" fn(usize, usize) -> usize = unsafe { transmute(original) };
    // Take ownership before native execution: nested callbacks cannot alias
    // or reallocate this invocation's aligned array. Reuse capacity afterward.
    let mut scratch = SCRATCH.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    let private = crate::memory_query::scoped(|| prepare(op, context, &mut scratch));
    let selected = private
        .as_ref()
        .map_or(op, |p| p as *const Operator as usize);
    let result = unsafe { native(selected, context) };
    SCRATCH.with(|cell| {
        let mut saved = cell.borrow_mut();
        if scratch.capacity() >= saved.capacity() {
            *saved = scratch;
        }
    });
    result
}

#[cfg(test)]
mod tests;
