// Frozen 2.41 nested guard oracle, TEST ONLY.
fn packed_normal_span_241(op: usize, output: usize, offset: usize) -> Option<MeshNormalSpan> {
    let deformer = op.checked_add(offset)?;
    if read_u8(deformer.checked_add(0x94)?)? != 0 {
        return None;
    }
    let start = usize::from(read_u16(deformer.checked_add(0x90)?)?);
    let end = usize::from(read_u16(deformer.checked_add(0x92)?)?);
    let count = end.checked_sub(start)?.checked_add(1)?;
    if count > crate::cloth_mesh_scale::MAX_FRAMES {
        return None;
    }
    let stride = usize::from(read_u8(output.checked_add(0x4C)?)?);
    let position_stride = usize::from(read_u8(output.checked_add(0x24)?)?);
    let position_flags = read_u8(output.checked_add(0x28)?)? & 1;
    let normal_flags = read_u8(output.checked_add(0x50)?)? & 1;
    // Bit 0 selects aligned float4 stores in 15A5330, not a compressed normal.
    // Only the two packed PN wrappers installed above may mark this pair.
    if ![12, 16].contains(&stride)
        || ![12, 16].contains(&position_stride)
        || position_flags != normal_flags
        || (normal_flags == 1 && (stride != 16 || position_stride != 16))
    {
        return None;
    }
    let data = read_usize(output.checked_add(0x40)?)?.checked_add(start * stride)?;
    let positions = read_usize(output.checked_add(0x18)?)?;
    if data == 0
        || positions == 0
        || (normal_flags == 1 && (data % 16 != 0 || positions % 16 != 0))
        || !is_memory_accessible(data, count * stride, true)
    {
        return None;
    }
    // Do not divide un-written normals in a sparse/partial deformer. These
    // four layouts are verified against the native packed PN kernel; higher
    // blend formats and unknown control bytes fail closed for the whole pair.
    let mut seen = [0u64; 128];
    let mut blocks = [0usize; 4];
    for index in 0..8 {
        let at = deformer.checked_add(index * 16)?;
        let n = bounded_i32_count(at + 8, 512)?;
        if index < 4 {
            if n != 0 {
                return None;
            }
            continue;
        }
        blocks[index - 4] = n;
        let begin = read_usize(at)?;
        let size = [224usize, 128 + 48, 128, 64][index - 4];
        if n > 0 && !is_memory_accessible(begin, n * size, false) {
            return None;
        }
        for b in 0..n {
            for v in 0..16 {
                let vertex = usize::from(read_u16(begin + b * size + v * 2)?);
                if vertex < start || vertex > end {
                    return None;
                }
                let at = vertex - start;
                seen[at / 64] |= 1u64 << (at % 64);
            }
        }
    }
    let control_count = bounded_i32_count(deformer.checked_add(0x88)?, 512)?;
    let controls = read_usize(deformer.checked_add(0x80)?)?;
    let mut actual = [0usize; 4];
    if control_count == 0 || !is_memory_accessible(controls, control_count, false) {
        return None;
    }
    for i in 0..control_count {
        let c = usize::from(read_u8(controls + i)?);
        if c >= 4 {
            return None;
        }
        actual[c] += 1;
    }
    if actual != blocks || (0..count).any(|i| seen[i / 64] & (1u64 << (i % 64)) == 0) {
        return None;
    }
    let position_end = positions.checked_add((end + 1) * position_stride)?;
    if !(data + count * stride <= positions || position_end <= data) {
        return None;
    }
    Some(MeshNormalSpan {
        data,
        count,
        stride,
    })
}
