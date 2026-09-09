// Frozen 2.41 decision oracle, TEST ONLY. Keep rejection behavior unchanged in 2.42.
fn skin_normal_call_241(op: usize, context: usize) -> Option<SkinNormalCall> {
    let base = MODULE_BASE.load(Ordering::Acquire);
    let scale = current_scale();
    if !HOOKS_READY.load(Ordering::Acquire)
        || !valid_active_scale(scale)
        || read_usize(op)? != base.checked_add(ER_CLOTH_SKIN_PN_VTABLE_RVA)?
    {
        return None;
    }
    let root = read_usize(context.checked_add(0x10)?)?;
    let generation = cloth_topology_generation();
    let scope = *TARGET_CLOTH_SCOPE.try_read().ok()?;
    if scope.anchor == 0 || scope.anchor != TARGET_CLOTH_POSE_IMPORTER.load(Ordering::Acquire) {
        return None;
    }
    let mut selected = None;
    for route in scope.routes.iter().copied().filter(|r| r.owner != 0) {
        let Some(slot) = (0..CLOTH_INSTANCE_SLOTS).find(|&s| {
            CLOTH_INSTANCE_OWNERS[s].load(Ordering::Acquire) == route.owner
                && CLOTH_INSTANCE_INPUTS[s].load(Ordering::Acquire) == route.input
                && CLOTH_INSTANCE_CORES[s].load(Ordering::Acquire) == route.core
        }) else {
            continue;
        };
        if CLOTH_INSTANCE_APPLIED_SCALE_BITS[slot].load(Ordering::Acquire) != scale.to_bits()
            || CLOTH_INSTANCE_PENDING_SCALE_BITS[slot].load(Ordering::Acquire)
                != NO_PENDING_CLOTH_SCALE_BITS
            || !scope.route_is_current(route, base, read_usize)
        {
            continue;
        }
        let (groups, count) = bounded_vector_span(route.core, 0x28, 0x30, 8, MAX_CLOTH_GROUPS)?;
        for i in 0..count {
            if read_usize(read_usize(groups.checked_add(i * 8)?)?)? == root {
                if selected.is_some() {
                    return None;
                }
                selected = Some((slot, route));
            }
        }
    }
    let (slot, route) = selected?;
    let buffers = read_usize(root.checked_add(0x20)?)?;
    let count = bounded_i32_count(root.checked_add(0x28)?, 128)?;
    let first = bounded_i32_count(op.checked_add(0x68)?, 127)?;
    if first >= count {
        return None;
    }
    let selector = read_usize(buffers.checked_add(first * 8)?)?;
    let index = bounded_i32_count(selector.checked_add(0x110)?, 127)?;
    if index >= count {
        return None;
    }
    let output = read_usize(buffers.checked_add(index * 8)?)?;
    let span = packed_normal_span_241(op, output, 0x70)?;
    let vertices = bounded_i32_count(
        output.checked_add(0x20)?,
        crate::cloth_mesh_scale::MAX_FRAMES,
    )?;
    if usize::from(read_u16(op.checked_add(0x102)?)?) >= vertices
        || read_u8(output.checked_add(0x28)?)? & 1 != 1
        || read_u8(output.checked_add(0x50)?)? & 1 != 1
        || !cloth_matrix_array_basis_matches_scale(&read_matrix(output.checked_add(0x90)?)?, 1.)
        || !cloth_matrix_array_basis_matches_scale(&read_matrix(output.checked_add(0xD0)?)?, 1.)
    {
        return None;
    }
    // Only the packed PN branch demonstrated by 15A9966 -> 15A3CE0.
    let packed = read_usize(op.checked_add(0x108)?)?;
    let packed_count = bounded_i32_count(op.checked_add(0x110)?, 512)?;
    if packed_count == 0
        || packed_count != bounded_i32_count(op.checked_add(0xF8)?, 512)?
        || !is_memory_accessible(packed, packed_count.checked_mul(256)?, false)
    {
        return None;
    }
    let sets = read_usize(root.checked_add(0x30)?)?;
    let set_count = bounded_i32_count(root.checked_add(0x38)?, 128)?;
    let set_index = bounded_i32_count(op.checked_add(0x6C)?, 127)?;
    if set_index >= set_count {
        return None;
    }
    let transform_set = read_usize(sets.checked_add(set_index * 8)?)?;
    let matrices = read_usize(transform_set.checked_add(0x18)?)?;
    let matrix_count = bounded_i32_count(transform_set.checked_add(0x20)?, 2048)?;
    let subset = read_usize(op.checked_add(0x58)?)?;
    let subset_count = bounded_i32_count(op.checked_add(0x60)?, 256)?;
    let binds = read_usize(op.checked_add(0x48)?)?;
    if subset_count == 0 || bounded_i32_count(op.checked_add(0x50)?, 256)? != subset_count {
        return None;
    }
    for i in 0..subset_count {
        let bone = usize::from(read_u16(subset.checked_add(i * 2)?)?);
        if bone >= matrix_count
            || !cloth_matrix_array_basis_matches_scale(
                &read_matrix(matrices.checked_add(bone * 64)?)?,
                scale,
            )
            || !cloth_matrix_array_basis_matches_scale(
                &read_matrix(binds.checked_add(i * 64)?)?,
                1.,
            )
        {
            return None;
        }
    }
    if current_scale().to_bits() != scale.to_bits() || generation != cloth_topology_generation() {
        return None;
    }
    Some(SkinNormalCall {
        root,
        output,
        transform_set,
        span,
        slot,
        owner: route.owner,
        core: route.core,
        anchor: scope.anchor,
        generation,
        scale_bits: scale.to_bits(),
    })
}
