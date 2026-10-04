//! Fresh BoneSpaceSkinPN normals feed LocalRange's unit-direction projection.
//! Preserve their authored magnitude; remove only the owned uniform body scale.
use super::*;
pub(super) const ENTRY: usize = 0x15666B0;
pub(super) const VTABLE: usize = 0x2D7DEC8;
pub(super) const ENTRY_BYTES: &[u8] = &[
    0x4C, 0x8B, 0x4A, 0x10, 0x48, 0x63, 0x41, 0x58, 0x4D, 0x8B, 0x51, 0x20, 0x49, 0x8B, 0x14, 0xC2,
    0x48, 0x63, 0x41, 0x5C, 0x4C, 0x63, 0x82, 0x10, 1, 0, 0, 0x49, 0x8B, 0x51, 0x30, 0x4F, 0x8B, 4,
    0xC2, 0x48, 0x8B, 0x14, 0xC2, 0xE9, 4, 0, 0, 0,
];

pub(super) fn validate(base: usize) -> bool {
    bytes_equal(base + ENTRY, ENTRY_BYTES)
        && read_usize(base + VTABLE + 0x20) == Some(base + ENTRY)
        && bytes_equal(
            base + 0x156673D,
            &[0x83, 0xBB, 0xC0, 0, 0, 0, 0, 0x7E, 0x2E],
        )
        && bytes_equal(base + 0x156675C, &[0xE8, 0xBF, 0xE5, 0xFF, 0xFF])
}

pub(super) fn hook(registers: *mut Registers, original: usize) -> usize {
    let r = unsafe { &*registers };
    let native: unsafe extern "C" fn(usize, usize) -> usize = unsafe { transmute(original) };
    let (op, context) = (r.rcx as usize, r.rdx as usize);
    let before = crate::memory_query::scoped(|| span(op, context));
    let result = unsafe { native(op, context) };
    if let Some(before) = before {
        crate::memory_query::scoped(|| {
            if span(op, context) != Some(before) {
                return;
            }
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(before.data as *mut u8, before.count * before.stride)
            };
            let _ = crate::cloth_mesh_scale::restore_normal_length(
                bytes,
                before.count,
                before.stride,
                before.owner.scale,
            );
        });
    }
    result
}

#[derive(Clone, Copy, PartialEq)]
struct Span {
    owner: simple_mesh_bone::Marker,
    buffer: usize,
    data: usize,
    count: usize,
    stride: usize,
    controls: usize,
    controls_count: usize,
    packed: usize,
    packed_count: usize,
}

fn span(op: usize, context: usize) -> Option<Span> {
    let owner = simple_mesh_bone::operator_scope(op, context, VTABLE)?;
    // Only the verified complete packed-PN producer. Partial output may retain
    // normals from earlier passes and must never be divided for a second time.
    if read_u8(op.checked_add(0xB4)?)? != 0 {
        return None;
    }
    let start = usize::from(read_u16(op.checked_add(0xB0)?)?);
    let end = usize::from(read_u16(op.checked_add(0xB2)?)?);
    if start > end || end >= crate::cloth_mesh_scale::MAX_FRAMES {
        return None;
    }
    let controls = read_usize(op.checked_add(0xA0)?)?;
    let controls_count = bounded_i32_count(op.checked_add(0xA8)?, 2048)?;
    let packed = read_usize(op.checked_add(0xB8)?)?;
    let packed_count = bounded_i32_count(op.checked_add(0xC0)?, 2048)?;
    if packed_count == 0
        || packed_count != controls_count
        || !is_memory_accessible(controls, controls_count, false)
        || !is_memory_accessible(packed, packed_count * 384, false)
    {
        return None;
    }
    let buffers = read_usize(owner.root.checked_add(0x20)?)?;
    let n = bounded_i32_count(owner.root.checked_add(0x28)?, 128)?;
    let first = bounded_i32_count(op.checked_add(0x58)?, 127)?;
    if first >= n {
        return None;
    }
    let selector = read_usize(buffers.checked_add(first * 8)?)?;
    let selected = bounded_i32_count(selector.checked_add(0x110)?, 127)?;
    if selected >= n {
        return None;
    }
    let buffer = read_usize(buffers.checked_add(selected * 8)?)?;
    let vertices = bounded_i32_count(
        buffer.checked_add(0x20)?,
        crate::cloth_mesh_scale::MAX_FRAMES,
    )?;
    let normals = bounded_i32_count(
        buffer.checked_add(0x48)?,
        crate::cloth_mesh_scale::MAX_FRAMES,
    )?;
    let stride = read_u8(buffer.checked_add(0x4C)?)? as usize;
    if end >= vertices || end >= normals || ![12, 16].contains(&stride) {
        return None;
    }
    let data = read_usize(buffer.checked_add(0x40)?)?.checked_add(start * stride)?;
    let count = end - start + 1;
    if !is_memory_accessible(data, count * stride, true) {
        return None;
    }
    Some(Span {
        owner,
        buffer,
        data,
        count,
        stride,
        controls,
        controls_count,
        packed,
        packed_count,
    })
}
