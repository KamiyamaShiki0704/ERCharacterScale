use eldenring::cs::PlayerIns;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery,
};

use crate::log;

const CHR_INS_MODEL_INS_OFFSET: usize = 0x50;
const PHYSICS_HK_COLLISION_SHAPE_OFFSET: usize = 0xb0;
const CSMODELINS_MODEL_ITEM_OFFSET: usize = 0x10;
const CSMODELINS_MODEL_DISP_ENTITY_OFFSET: usize = 0x18;
const CSMODELINS_LOCATION_ENTITY_OFFSET: usize = 0x20;
const MODEL_ITEM_FLVER_MODEL_DATA_OFFSET: usize = 0x68;
const MODEL_ITEM_MTX43_ARRAY_ENTITY_OFFSET: usize = 0x640;
const MODEL_ITEM_DEFAULT_DMY_LOCATION_MODIFIER_OFFSET: usize = 0x650;
const MODEL_ITEM_LOCATION_AABB_EXPORTER_OFFSET: usize = 0x658;

const ROOT_SCAN_BYTES: usize = 0x180;
const FOCUSED_ROOT_SLOT_BYTES: usize = 0x100;
const FOCUSED_ROOT_DUMP_BYTES: usize = 0x80;
const MODEL_ITEM_TAIL_SCAN_OFFSET: usize = 0x630;
const MODEL_ITEM_TAIL_SCAN_BYTES: usize = 0x80;
const MAX_ARRAY_PREVIEW: usize = 6;
const RAGDOLL_NESTED_OFFSETS: &[usize] = &[0x68, 0x70, 0x78, 0x90, 0x98, 0xa0, 0xb0, 0xc0, 0xe0];
const SINGLE_RAGDOLL_SYSTEM_SLOT_OFFSET: usize = 0x78;
const SINGLE_RAGDOLL_SYSTEM_DATA_OFFSET: usize = 0x18;
const HKNP_PHYSICS_SYSTEM_MATERIALS_OFFSET: usize = 0x18;
const HKNP_PHYSICS_SYSTEM_MOTION_PROPERTIES_OFFSET: usize = 0x28;
const HKNP_PHYSICS_SYSTEM_BODY_CINFOS_OFFSET: usize = 0x38;
const HKNP_PHYSICS_SYSTEM_CONSTRAINT_CINFOS_OFFSET: usize = 0x48;
const HKNP_BODY_CINFO_WITH_ATTACHMENT_SIZE: usize = 0xc0;
const SINGLE_RAGDOLL_BODY_PREVIEW: usize = 4;
const LIVE_RAGDOLL_SLOT_OFFSETS: &[usize] = &[0x68, 0x70, 0x78, 0x90, 0x98, 0xa0, 0xb0, 0xc0, 0xe0];
const LIVE_RUNTIME_SCAN_BYTES: usize = 0x100;
const LIVE_RUNTIME_ROW_BYTES: usize = 0x80;
const HKNP_BODY_SIZE: usize = 0xa0;

pub fn probe_player_hkx_collidables(player: &mut PlayerIns, scale: f32) {
    let player_addr = player as *mut _ as usize;
    let chr_ins = &mut player.chr_ins;
    let chr_ins_addr = chr_ins as *mut _ as usize;
    let chr_ctrl = chr_ins.chr_ctrl.as_mut();
    let physics = chr_ins.modules.as_mut().physics.as_mut();
    let physics_addr = physics as *mut _ as usize;
    let hk_collision_shape =
        read_usize(physics_addr + PHYSICS_HK_COLLISION_SHAPE_OFFSET).unwrap_or(0);
    let chr_model_ins = read_usize(chr_ins_addr + CHR_INS_MODEL_INS_OFFSET).unwrap_or(0);

    log::line(format_args!(
        "[player-scale-no-bone] hkx probe begin scale={scale:.3} player=0x{player_addr:x} chr_ins=0x{chr_ins_addr:x} physics=0x{physics_addr:x}"
    ));

    inspect_runtime_collision_root("ChrCtrl::chr_collision", chr_ctrl.chr_collision);
    inspect_runtime_collision_root("ChrCtrl::ragdoll_ins", chr_ctrl.ragdoll_ins);
    inspect_root(
        "CSChrPhysicsModule+0xb0 hknp/main shape",
        hk_collision_shape,
    );
    inspect_hknp_capsule_shape(
        "CSChrPhysicsModule+0xb0 hknp/main shape",
        hk_collision_shape,
    );
    inspect_root("ChrIns::chr_model_ins", chr_model_ins);
    inspect_chr_model_roots(chr_model_ins);
}

pub fn probe_ragdoll_single_physics_system(player: &mut PlayerIns, scale: f32) {
    let player_addr = player as *mut _ as usize;
    let chr_ctrl = player.chr_ins.chr_ctrl.as_mut();
    let ragdoll = chr_ctrl.ragdoll_ins;
    let system = read_usize(ragdoll + SINGLE_RAGDOLL_SYSTEM_SLOT_OFFSET).unwrap_or(0);
    let system_data = read_usize(system + SINGLE_RAGDOLL_SYSTEM_DATA_OFFSET).unwrap_or(0);

    log::line(format_args!(
        "[player-scale-no-bone] ragdoll single probe begin scale={scale:.3} player=0x{player_addr:x} ragdoll=0x{ragdoll:x} system_slot=+0x{SINGLE_RAGDOLL_SYSTEM_SLOT_OFFSET:x} system=0x{system:x} data_offset=+0x{SINGLE_RAGDOLL_SYSTEM_DATA_OFFSET:x} data=0x{system_data:x}"
    ));

    if !is_readable(system, 0x20) {
        log::line(format_args!(
            "[player-scale-no-bone] ragdoll single probe stop: system pointer is not readable"
        ));
        return;
    }

    if !is_readable(system_data, 0x68) {
        log::line(format_args!(
            "[player-scale-no-bone] ragdoll single probe stop: system data pointer is not readable"
        ));
        return;
    }

    let signatures = object_signatures(system_data);
    let materials = hk_array_summary(system_data + HKNP_PHYSICS_SYSTEM_MATERIALS_OFFSET);
    let motion_properties =
        hk_array_summary(system_data + HKNP_PHYSICS_SYSTEM_MOTION_PROPERTIES_OFFSET);
    let body_cinfos = hk_array_summary(system_data + HKNP_PHYSICS_SYSTEM_BODY_CINFOS_OFFSET);
    let constraint_cinfos =
        hk_array_summary(system_data + HKNP_PHYSICS_SYSTEM_CONSTRAINT_CINFOS_OFFSET);

    log::line(format_args!(
        "[player-scale-no-bone] ragdoll single hknpPhysicsSystemData addr=0x{system_data:x} signatures={signatures}"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] ragdoll single arrays materials={} motion_properties={} body_cinfos={} constraint_cinfos={}",
        format_hk_array(materials),
        format_hk_array(motion_properties),
        format_hk_array(body_cinfos),
        format_hk_array(constraint_cinfos)
    ));

    let Some(body_cinfos) = body_cinfos else {
        return;
    };
    if body_cinfos.size <= 0 || body_cinfos.size > 128 || body_cinfos.data <= 0x10000 {
        return;
    }

    let preview_count = (body_cinfos.size as usize).min(SINGLE_RAGDOLL_BODY_PREVIEW);
    for index in 0..preview_count {
        let entry = body_cinfos.data + index * HKNP_BODY_CINFO_WITH_ATTACHMENT_SIZE;
        inspect_body_cinfo_with_attachment(index, entry);
    }
}

pub fn probe_ragdoll_live_runtime_candidates(player: &mut PlayerIns, scale: f32) {
    let player_addr = player as *mut _ as usize;
    let chr_ctrl = player.chr_ins.chr_ctrl.as_mut();
    let ragdoll = chr_ctrl.ragdoll_ins;
    let system = read_usize(ragdoll + SINGLE_RAGDOLL_SYSTEM_SLOT_OFFSET).unwrap_or(0);
    let system_data = read_usize(system + SINGLE_RAGDOLL_SYSTEM_DATA_OFFSET).unwrap_or(0);
    let body_cinfos = hk_array_summary(system_data + HKNP_PHYSICS_SYSTEM_BODY_CINFOS_OFFSET);
    let constraint_cinfos =
        hk_array_summary(system_data + HKNP_PHYSICS_SYSTEM_CONSTRAINT_CINFOS_OFFSET);
    let body_count = body_cinfos.map(|summary| summary.size).unwrap_or(18);
    let constraint_count = constraint_cinfos.map(|summary| summary.size).unwrap_or(17);

    log::line(format_args!(
        "[player-scale-no-bone] ragdoll live probe begin scale={scale:.3} player=0x{player_addr:x} ragdoll=0x{ragdoll:x} system=0x{system:x} system_data=0x{system_data:x} body_count={body_count} constraint_count={constraint_count}"
    ));

    inspect_live_runtime_object(
        "ragdoll+0x78 hknp system",
        system,
        body_count,
        constraint_count,
    );

    if !is_readable(ragdoll, 0x100) {
        log::line(format_args!(
            "[player-scale-no-bone] ragdoll live probe stop: ragdoll root is not readable"
        ));
        return;
    }

    for &offset in LIVE_RAGDOLL_SLOT_OFFSETS {
        let ptr = read_usize(ragdoll + offset).unwrap_or(0);
        let readable = is_readable(ptr, 0x20);
        let signatures = if readable {
            object_signatures(ptr)
        } else {
            String::new()
        };
        log::line(format_args!(
            "[player-scale-no-bone] ragdoll live slot +0x{offset:x}: ptr=0x{ptr:x} readable={readable} signatures={signatures}"
        ));

        if readable {
            inspect_live_runtime_object(
                &format!("ragdoll+0x{offset:x} target"),
                ptr,
                body_count,
                constraint_count,
            );
        }
    }
}

fn inspect_live_runtime_object(name: &str, addr: usize, body_count: i32, constraint_count: i32) {
    if addr <= 0x10000 || !is_readable(addr, 0x20) {
        return;
    }

    log::line(format_args!(
        "[player-scale-no-bone] ragdoll live object {name}: addr=0x{addr:x} signatures={}",
        object_signatures(addr)
    ));
    dump_compact_rows(name, addr, LIVE_RUNTIME_ROW_BYTES);
    scan_live_inline_arrays(
        name,
        addr,
        LIVE_RUNTIME_SCAN_BYTES,
        body_count,
        constraint_count,
    );
}

fn scan_live_inline_arrays(
    name: &str,
    addr: usize,
    bytes: usize,
    body_count: i32,
    constraint_count: i32,
) {
    if !is_readable(addr, bytes) {
        return;
    }

    for offset in (0..bytes).step_by(size_of::<usize>()) {
        let Some(array) = hk_array_summary(addr + offset) else {
            continue;
        };

        if !is_live_count_match(array.size, body_count, constraint_count) {
            continue;
        }

        log::line(format_args!(
            "[player-scale-no-bone] ragdoll live array {name}+0x{offset:x}: data=0x{:x} size={} cap_flags=0x{:x}",
            array.data, array.size, array.capacity_and_flags
        ));
        preview_live_array(&format!("{name}+0x{offset:x}"), array);
    }
}

fn is_live_count_match(size: i32, body_count: i32, constraint_count: i32) -> bool {
    size > 0 && (size == body_count || size == constraint_count)
}

fn preview_live_array(name: &str, array: HkArraySummary) {
    if array.data <= 0x10000 || !is_readable(array.data, 0x20) {
        return;
    }

    let signatures = object_signatures(array.data);
    let q0 = read_usize(array.data).unwrap_or(0);
    let q8 = read_usize(array.data + 0x8).unwrap_or(0);
    let f0 = read_f32(array.data).unwrap_or(f32::NAN);
    let f4 = read_f32(array.data + 0x4).unwrap_or(f32::NAN);
    let f8 = read_f32(array.data + 0x8).unwrap_or(f32::NAN);
    let fc = read_f32(array.data + 0xc).unwrap_or(f32::NAN);

    log::line(format_args!(
        "[player-scale-no-bone] ragdoll live array preview {name}[0] addr=0x{:x}: q=[0x{q0:x},0x{q8:x}] f=[{f0:.5},{f4:.5},{f8:.5},{fc:.5}] signatures={signatures}",
        array.data
    ));

    if let Some(summary) = hknp_body_summary(array.data) {
        log::line(format_args!(
            "[player-scale-no-bone] ragdoll live array {name} as hknpBody[0]: {summary}"
        ));
        preview_hknp_body_shape(&format!("{name}.hknpBody[0]"), array.data);
        if array.size > 1 {
            let second = array.data + HKNP_BODY_SIZE;
            if let Some(summary) = hknp_body_summary(second) {
                log::line(format_args!(
                    "[player-scale-no-bone] ragdoll live array {name} as hknpBody[1]: {summary}"
                ));
                preview_hknp_body_shape(&format!("{name}.hknpBody[1]"), second);
            }
        }
    }

    if let Some(summary) = hknp_motion_summary(array.data) {
        log::line(format_args!(
            "[player-scale-no-bone] ragdoll live array {name} as hknpMotion[0]: {summary}"
        ));
    }

    if let Some(summary) = hknp_body_data_summary(array.data) {
        log::line(format_args!(
            "[player-scale-no-bone] ragdoll live array {name} as hknpBodyData[0]: {summary}"
        ));
    }
}

fn preview_hknp_body_shape(name: &str, body_addr: usize) {
    let shape = read_usize(body_addr + 0x60).unwrap_or(0);
    if shape <= 0x10000 || !is_readable(shape, 0x20) {
        return;
    }

    log::line(format_args!(
        "[player-scale-no-bone] ragdoll live {name}.shape: addr=0x{shape:x} signatures={}",
        object_signatures(shape)
    ));
    inspect_hknp_capsule_shape(&format!("{name}.shape"), shape);
}

fn inspect_chr_model_roots(chr_model_ins: usize) {
    if !is_readable(chr_model_ins, 0x30) {
        return;
    }

    let model_item = read_usize(chr_model_ins + CSMODELINS_MODEL_ITEM_OFFSET).unwrap_or(0);
    let model_disp_entity =
        read_usize(chr_model_ins + CSMODELINS_MODEL_DISP_ENTITY_OFFSET).unwrap_or(0);
    let location_entity =
        read_usize(chr_model_ins + CSMODELINS_LOCATION_ENTITY_OFFSET).unwrap_or(0);
    let flver_model_data = read_usize(model_item + MODEL_ITEM_FLVER_MODEL_DATA_OFFSET).unwrap_or(0);
    let mtx43_array_entity =
        read_usize(model_item + MODEL_ITEM_MTX43_ARRAY_ENTITY_OFFSET).unwrap_or(0);
    let default_dmypoly_location_modifier =
        read_usize(model_item + MODEL_ITEM_DEFAULT_DMY_LOCATION_MODIFIER_OFFSET).unwrap_or(0);
    let location_aabb_exporter =
        read_usize(model_item + MODEL_ITEM_LOCATION_AABB_EXPORTER_OFFSET).unwrap_or(0);

    log::line(format_args!(
        "[player-scale-no-bone] hkx probe model roots: model_item=0x{model_item:x} model_disp_entity=0x{model_disp_entity:x} location_entity=0x{location_entity:x}"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] hkx probe model item fields: flver=0x{flver_model_data:x} mtx43=0x{mtx43_array_entity:x} dmy_modifier=0x{default_dmypoly_location_modifier:x} aabb_exporter=0x{location_aabb_exporter:x}"
    ));

    inspect_root("CSChrModelIns", chr_model_ins);
    inspect_root("CSFD4ModelItem", model_item);
    inspect_root("model_disp_entity", model_disp_entity);
    inspect_root("location_entity", location_entity);
    inspect_root("flver_model_data", flver_model_data);
    inspect_root("mtx43_array_entity", mtx43_array_entity);
    inspect_root(
        "default_dmypoly_location_modifier",
        default_dmypoly_location_modifier,
    );
    inspect_root("location_aabb_exporter", location_aabb_exporter);

    scan_pointer_candidates_range(
        "CSFD4ModelItem tail",
        model_item,
        MODEL_ITEM_TAIL_SCAN_OFFSET,
        MODEL_ITEM_TAIL_SCAN_BYTES,
    );
}

fn inspect_root(name: &str, root: usize) {
    let readable = is_readable(root, 0x10);
    let first_qword = read_usize(root).unwrap_or(0);
    log::line(format_args!(
        "[player-scale-no-bone] hkx root {name}: addr=0x{root:x} readable={readable} first_qword=0x{first_qword:x}"
    ));

    scan_pointer_candidates(name, root, ROOT_SCAN_BYTES);
    inspect_candidate_object(name, root);
}

fn inspect_runtime_collision_root(name: &str, root: usize) {
    inspect_root(name, root);

    if !is_readable(root, 0x10) {
        return;
    }

    log::line(format_args!(
        "[player-scale-no-bone] hkx focused slots {name}: root=0x{root:x}"
    ));

    scan_pointer_slots_verbose(name, root, FOCUSED_ROOT_SLOT_BYTES);
    dump_compact_rows(name, root, FOCUSED_ROOT_DUMP_BYTES);

    if name.contains("ragdoll_ins") {
        inspect_ragdoll_nested_slots(name, root);
    }
}

fn scan_pointer_candidates(name: &str, root: usize, bytes: usize) {
    scan_pointer_candidates_range(name, root, 0, bytes);
}

fn scan_pointer_candidates_range(name: &str, root: usize, start_offset: usize, bytes: usize) {
    let start = root.saturating_add(start_offset);
    if !is_readable(start, bytes) {
        return;
    }

    for rel_offset in (0..bytes).step_by(size_of::<usize>()) {
        let offset = start_offset + rel_offset;
        let ptr = read_usize(root + offset).unwrap_or(0);
        if ptr <= 0x10000 || !is_readable(ptr, 0x20) {
            continue;
        }

        let signatures = object_signatures(ptr);
        if !signatures.is_empty() {
            log::line(format_args!(
                "[player-scale-no-bone] hkx candidate {name}+0x{offset:x} -> 0x{ptr:x}: {signatures}"
            ));
            inspect_candidate_details(&format!("{name}+0x{offset:x}"), ptr);
        }
    }
}

fn scan_pointer_slots_verbose(name: &str, root: usize, bytes: usize) {
    if !is_readable(root, bytes) {
        return;
    }

    for offset in (0..bytes).step_by(size_of::<usize>()) {
        let ptr = read_usize(root + offset).unwrap_or(0);
        if ptr <= 0x10000 {
            continue;
        }

        let readable = is_readable(ptr, 0x20);
        let first_qword = read_usize(ptr).unwrap_or(0);
        let signatures = if readable {
            object_signatures(ptr)
        } else {
            String::new()
        };
        log::line(format_args!(
            "[player-scale-no-bone] hkx slot {name}+0x{offset:x}: ptr=0x{ptr:x} readable={readable} first_qword=0x{first_qword:x} {signatures}"
        ));

        if !signatures.is_empty() {
            inspect_candidate_details(&format!("{name}+0x{offset:x}"), ptr);
        }
    }
}

fn inspect_ragdoll_nested_slots(name: &str, root: usize) {
    for &offset in RAGDOLL_NESTED_OFFSETS {
        let ptr = read_usize(root + offset).unwrap_or(0);
        if ptr <= 0x10000 || !is_readable(ptr, 0x20) {
            continue;
        }

        let nested_name = format!("{name}+0x{offset:x} target");
        log::line(format_args!(
            "[player-scale-no-bone] hkx ragdoll nested {nested_name}: addr=0x{ptr:x} signatures={}",
            object_signatures(ptr)
        ));
        scan_pointer_candidates(&nested_name, ptr, 0x100);
        dump_compact_rows(&nested_name, ptr, 0x80);
    }
}

fn dump_compact_rows(name: &str, root: usize, bytes: usize) {
    if !is_readable(root, bytes) {
        return;
    }

    for offset in (0..bytes).step_by(0x10) {
        let q0 = read_usize(root + offset).unwrap_or(0);
        let q8 = read_usize(root + offset + 0x8).unwrap_or(0);
        let f0 = read_f32(root + offset).unwrap_or(f32::NAN);
        let f4 = read_f32(root + offset + 0x4).unwrap_or(f32::NAN);
        let f8 = read_f32(root + offset + 0x8).unwrap_or(f32::NAN);
        let fc = read_f32(root + offset + 0xc).unwrap_or(f32::NAN);

        log::line(format_args!(
            "[player-scale-no-bone] hkx row {name}+0x{offset:x}: q=[0x{q0:x},0x{q8:x}] f=[{f0:.5},{f4:.5},{f8:.5},{fc:.5}]"
        ));
    }
}

fn inspect_candidate_object(name: &str, addr: usize) {
    if addr <= 0x10000 || !is_readable(addr, 0x20) {
        return;
    }

    let signatures = object_signatures(addr);
    if !signatures.is_empty() {
        log::line(format_args!(
            "[player-scale-no-bone] hkx candidate {name} @0x{addr:x}: {signatures}"
        ));
    }

    inspect_sim_cloth_data(name, addr);
    inspect_collidable(name, addr);
}

fn inspect_candidate_details(name: &str, addr: usize) {
    inspect_sim_cloth_data(name, addr);
    inspect_collidable(name, addr);
    inspect_hknp_capsule_shape(name, addr);
}

fn object_signatures(addr: usize) -> String {
    let mut signatures = Vec::new();

    if let Some(summary) = hcl_collidable_summary(addr) {
        signatures.push(summary);
    }
    if let Some(summary) = hcl_capsule_shape_summary(addr) {
        signatures.push(summary);
    }
    if let Some(summary) = hknp_shape_instance_summary(addr) {
        signatures.push(summary);
    }
    if let Some(summary) = hcl_sim_cloth_data_summary(addr) {
        signatures.push(summary);
    }
    if let Some(summary) = hknp_body_summary(addr) {
        signatures.push(summary);
    }
    if let Some(summary) = hknp_motion_summary(addr) {
        signatures.push(summary);
    }
    if let Some(summary) = hknp_physics_system_data_summary(addr) {
        signatures.push(summary);
    }
    if let Some(summary) = hknp_body_data_summary(addr) {
        signatures.push(summary);
    }

    signatures.join(" | ")
}

fn inspect_collidable(name: &str, addr: usize) {
    if hcl_collidable_summary(addr).is_none() {
        return;
    }

    let shape = read_usize(addr + 0x88).unwrap_or(0);
    let transform = read_matrix4(addr + 0x20).unwrap_or([f32::NAN; 16]);
    let user_data = read_usize(addr + 0x80).unwrap_or(0);
    let name_ptr = read_usize(addr + 0x90).unwrap_or(0);
    let pinch_radius = read_f32(addr + 0x98).unwrap_or(f32::NAN);
    let raw_flags = [
        read_u8(addr + 0x98).unwrap_or(0),
        read_u8(addr + 0x99).unwrap_or(0),
        read_u8(addr + 0x9a).unwrap_or(0),
        read_u8(addr + 0x9b).unwrap_or(0),
        read_u8(addr + 0x9c).unwrap_or(0),
        read_u8(addr + 0x9d).unwrap_or(0),
        read_u8(addr + 0x9e).unwrap_or(0),
        read_u8(addr + 0x9f).unwrap_or(0),
    ];
    let col_translation = [transform[3], transform[7], transform[11], transform[15]];
    let row_translation = [transform[12], transform[13], transform[14], transform[15]];

    log::line(format_args!(
        "[player-scale-no-bone] hkx hclCollidable detail {name}: addr=0x{addr:x} shape=0x{shape:x} user_data=0x{user_data:x} name=0x{name_ptr:x} pinch_radius={pinch_radius:.5} raw_98_9f={raw_flags:02x?} col_t=[{:.3},{:.3},{:.3},{:.3}] row_t=[{:.3},{:.3},{:.3},{:.3}]",
        col_translation[0],
        col_translation[1],
        col_translation[2],
        col_translation[3],
        row_translation[0],
        row_translation[1],
        row_translation[2],
        row_translation[3]
    ));

    inspect_shape(&format!("{name}.hclCollidable.shape"), shape);
}

fn inspect_shape(name: &str, shape: usize) {
    if shape == 0 || !is_readable(shape, 0x60) {
        return;
    }

    let type_id = read_u32(shape + 0x18).unwrap_or(u32::MAX);
    log::line(format_args!(
        "[player-scale-no-bone] hkx shape {name}: addr=0x{shape:x} type=0x{type_id:x} {}",
        object_signatures(shape)
    ));
    inspect_hknp_capsule_shape(name, shape);
    inspect_hcl_tapered_capsule_shape(name, shape);
}

fn inspect_sim_cloth_data(name: &str, addr: usize) {
    if hcl_sim_cloth_data_summary(addr).is_none() {
        return;
    }

    inspect_hk_array_of_ptrs(&format!("{name}.perInstanceCollidables"), addr + 0xd0);
    inspect_hk_array_of_ptrs(&format!("{name}.staticConstraintSets"), addr + 0x88);
    inspect_hk_array_of_ptrs(&format!("{name}.actions"), addr + 0xf8);
}

fn inspect_hk_array_of_ptrs(name: &str, array_addr: usize) {
    let data = read_usize(array_addr).unwrap_or(0);
    let size = read_i32(array_addr + 0x8).unwrap_or(-1);
    let capacity_and_flags = read_i32(array_addr + 0xc).unwrap_or(-1);
    if data <= 0x10000 || !(0..=128).contains(&size) || !is_readable(data, size as usize * 8) {
        return;
    }

    log::line(format_args!(
        "[player-scale-no-bone] hkx array {name}: data=0x{data:x} size={size} cap_flags=0x{capacity_and_flags:x}"
    ));

    for index in 0..(size as usize).min(MAX_ARRAY_PREVIEW) {
        let ptr = read_usize(data + index * 8).unwrap_or(0);
        let signatures = object_signatures(ptr);
        log::line(format_args!(
            "[player-scale-no-bone] hkx array {name}[{index}]=0x{ptr:x}: {signatures}"
        ));
    }
}

fn inspect_body_cinfo_with_attachment(index: usize, entry: usize) {
    if !is_readable(entry, HKNP_BODY_CINFO_WITH_ATTACHMENT_SIZE) {
        log::line(format_args!(
            "[player-scale-no-bone] ragdoll body_cinfo[{index}] entry=0x{entry:x} readable=false"
        ));
        return;
    }

    let shape = read_usize(entry).unwrap_or(0);
    let flags = read_u32(entry + 0x8).unwrap_or(0);
    let collision_control = read_u32(entry + 0xc).unwrap_or(0);
    let collision_filter = read_u32(entry + 0x10).unwrap_or(0);
    let material_id = read_u16(entry + 0x14).unwrap_or(u16::MAX);
    let quality_id = read_u16(entry + 0x16).unwrap_or(u16::MAX);
    let name = read_usize(entry + 0x18).unwrap_or(0);
    let user_data = read_usize(entry + 0x20).unwrap_or(0);
    let motion_type = read_u8(entry + 0x28).unwrap_or(u8::MAX);
    let position = read_vec4(entry + 0x30).unwrap_or([f32::NAN; 4]);
    let orientation = read_vec4(entry + 0x40).unwrap_or([f32::NAN; 4]);
    let linear_velocity = read_vec4(entry + 0x50).unwrap_or([f32::NAN; 4]);
    let angular_velocity = read_vec4(entry + 0x60).unwrap_or([f32::NAN; 4]);
    let mass = read_f32(entry + 0x70).unwrap_or(f32::NAN);
    let motion_properties_id = read_u16(entry + 0x88).unwrap_or(u16::MAX);
    let desired_body_id = read_u32(entry + 0x8c).unwrap_or(u32::MAX);
    let motion_id = read_u32(entry + 0x90).unwrap_or(u32::MAX);
    let collision_look_ahead = read_f32(entry + 0x94).unwrap_or(f32::NAN);
    let activation_priority = read_u8(entry + 0xa0).unwrap_or(u8::MAX);
    let attached_body = read_usize(entry + 0xb0).unwrap_or(0);
    let shape_signatures = if shape > 0x10000 && is_readable(shape, 0x20) {
        object_signatures(shape)
    } else {
        String::new()
    };

    log::line(format_args!(
        "[player-scale-no-bone] ragdoll body_cinfo[{index}] entry=0x{entry:x} shape=0x{shape:x} flags=0x{flags:x} collision_control=0x{collision_control:x} filter=0x{collision_filter:x} material={material_id} quality={quality_id} motion_type={motion_type} motion_props={motion_properties_id} desired_body=0x{desired_body_id:x} motion=0x{motion_id:x} activation={activation_priority} attached=0x{attached_body:x} name=0x{name:x} user=0x{user_data:x}"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] ragdoll body_cinfo[{index}] pos=[{:.3},{:.3},{:.3},{:.3}] ori=[{:.3},{:.3},{:.3},{:.3}] lin_vel=[{:.3},{:.3},{:.3}] ang_vel=[{:.3},{:.3},{:.3}] mass={mass:.5} look_ahead={collision_look_ahead:.5} shape_signatures={shape_signatures}",
        position[0],
        position[1],
        position[2],
        position[3],
        orientation[0],
        orientation[1],
        orientation[2],
        orientation[3],
        linear_velocity[0],
        linear_velocity[1],
        linear_velocity[2],
        angular_velocity[0],
        angular_velocity[1],
        angular_velocity[2]
    ));

    inspect_hknp_capsule_shape(&format!("ragdoll body_cinfo[{index}].shape"), shape);
}

fn hcl_collidable_summary(addr: usize) -> Option<String> {
    if !is_readable(addr, 0xa0) {
        return None;
    }

    let transform = read_matrix4(addr + 0x20)?;
    if !looks_like_transform(&transform) {
        return None;
    }

    let shape = read_usize(addr + 0x88)?;
    if shape <= 0x10000 || !is_readable(shape, 0x20) {
        return None;
    }

    let pinch_radius = read_f32(addr + 0x98)?;
    let enabled = read_u8(addr + 0x9f)?;
    if !pinch_radius.is_finite() || !(-1.0..=10.0).contains(&pinch_radius) || enabled > 1 {
        return None;
    }

    let translation = [transform[3], transform[7], transform[11], transform[15]];
    let shape_type = read_u32(shape + 0x18).unwrap_or(u32::MAX);
    Some(format!(
        "hclCollidable? shape=0x{shape:x} shape_type=0x{shape_type:x} t=[{:.3},{:.3},{:.3},{:.3}] enabled={enabled}",
        translation[0], translation[1], translation[2], translation[3]
    ))
}

fn hcl_capsule_shape_summary(addr: usize) -> Option<String> {
    if !is_readable(addr, 0x60) {
        return None;
    }

    let start = read_vec4(addr + 0x20)?;
    let end = read_vec4(addr + 0x30)?;
    let dir = read_vec4(addr + 0x40)?;
    let radius = read_f32(addr + 0x50)?;
    let cap_len_sqrd_inv = read_f32(addr + 0x54)?;
    if !looks_like_hcl_capsule(start, end, dir, radius, cap_len_sqrd_inv) {
        return None;
    }

    Some(format!(
        "hclCapsuleShape? start=[{:.3},{:.3},{:.3}] end=[{:.3},{:.3},{:.3}] radius={radius:.4}",
        start[0], start[1], start[2], end[0], end[1], end[2]
    ))
}

fn inspect_hcl_tapered_capsule_shape(name: &str, addr: usize) {
    if !is_readable(addr, 0xb0) {
        return;
    }

    let small = read_vec4(addr + 0x20).unwrap_or([f32::NAN; 4]);
    let big = read_vec4(addr + 0x30).unwrap_or([f32::NAN; 4]);
    let small_radius = read_f32(addr + 0x90).unwrap_or(f32::NAN);
    let big_radius = read_f32(addr + 0x94).unwrap_or(f32::NAN);
    if small.into_iter().chain(big).all(f32::is_finite)
        && small_radius.is_finite()
        && big_radius.is_finite()
        && (0.001..=5.0).contains(&small_radius)
        && (0.001..=5.0).contains(&big_radius)
        && (0.01..=20.0).contains(&distance3(small, big))
    {
        log::line(format_args!(
            "[player-scale-no-bone] hkx hclTaperedCapsule landmark {name}: addr=0x{addr:x} small=[{:.3},{:.3},{:.3}] big=[{:.3},{:.3},{:.3}] small_radius={small_radius:.4} big_radius={big_radius:.4}",
            small[0], small[1], small[2], big[0], big[1], big[2]
        ));
    }
}

fn hknp_shape_instance_summary(addr: usize) -> Option<String> {
    if !is_readable(addr, 0x70) {
        return None;
    }

    let transform = read_matrix4(addr)?;
    let scale = read_vec4(addr + 0x40)?;
    let shape = read_usize(addr + 0x50)?;
    if !looks_like_transform(&transform)
        || !scale
            .into_iter()
            .all(|value| value.is_finite() && (0.001..=20.0).contains(&value.abs()))
        || shape <= 0x10000
        || !is_readable(shape, 0x20)
    {
        return None;
    }

    Some(format!(
        "hknpShapeInstance? scale=[{:.3},{:.3},{:.3},{:.3}] shape=0x{shape:x}",
        scale[0], scale[1], scale[2], scale[3]
    ))
}

fn hcl_sim_cloth_data_summary(addr: usize) -> Option<String> {
    if !is_readable(addr, 0x1d0) {
        return None;
    }

    let particle_datas = hk_array_summary(addr + 0x40)?;
    let per_instance_collidables = hk_array_summary(addr + 0xd0)?;
    let max_particle_radius = read_f32(addr + 0xe0)?;
    let total_mass = read_f32(addr + 0x108)?;

    if particle_datas.size <= 0
        || particle_datas.size > 4096
        || per_instance_collidables.size < 0
        || per_instance_collidables.size > 128
        || !max_particle_radius.is_finite()
        || !(0.0..=10.0).contains(&max_particle_radius)
        || !total_mass.is_finite()
        || !(0.0..=10000.0).contains(&total_mass)
    {
        return None;
    }

    Some(format!(
        "hclSimClothData? particles={} per_instance_collidables={} max_radius={max_particle_radius:.4} mass={total_mass:.3}",
        particle_datas.size, per_instance_collidables.size
    ))
}

fn hknp_body_summary(addr: usize) -> Option<String> {
    if !is_readable(addr, 0xa0) {
        return None;
    }

    let transform = read_matrix4(addr)?;
    let shape = read_usize(addr + 0x60)?;
    if !looks_like_transform(&transform) || shape <= 0x10000 || !is_readable(shape, 0x38) {
        return None;
    }

    let motion_id = read_u32(addr + 0x40).unwrap_or(u32::MAX);
    let flags = read_u32(addr + 0x44).unwrap_or(0);
    let collision_filter = read_u32(addr + 0x6c).unwrap_or(0);
    let body_id = read_u32(addr + 0x70).unwrap_or(u32::MAX);
    let shape_type = read_u16(shape + 0x1a).unwrap_or(u16::MAX);

    if motion_id == u32::MAX || body_id == u32::MAX {
        return None;
    }

    Some(format!(
        "hknpBody? shape=0x{shape:x} shape_type=0x{shape_type:x} motion_id=0x{motion_id:x} body_id=0x{body_id:x} flags=0x{flags:x} filter=0x{collision_filter:x}"
    ))
}

fn hknp_motion_summary(addr: usize) -> Option<String> {
    if !is_readable(addr, 0x80) {
        return None;
    }

    let center_of_mass = read_vec4(addr)?;
    let orientation = read_vec4(addr + 0x10)?;
    let linear_velocity = read_vec4(addr + 0x40)?;
    let angular_velocity = read_vec4(addr + 0x50)?;

    if !center_of_mass
        .into_iter()
        .chain(orientation)
        .chain(linear_velocity)
        .chain(angular_velocity)
        .all(|value| value.is_finite() && value.abs() <= 100000.0)
    {
        return None;
    }

    let orientation_len = length4(orientation);
    if !(0.5..=1.5).contains(&orientation_len) {
        return None;
    }

    let first_body_id = read_u32(addr + 0x28).unwrap_or(u32::MAX);
    let motion_props = read_u16(addr + 0x38).unwrap_or(u16::MAX);
    Some(format!(
        "hknpMotion? com=[{:.3},{:.3},{:.3}] first_body=0x{first_body_id:x} motion_props=0x{motion_props:x}",
        center_of_mass[0], center_of_mass[1], center_of_mass[2]
    ))
}

fn hknp_physics_system_data_summary(addr: usize) -> Option<String> {
    if !is_readable(addr, 0x68) {
        return None;
    }

    let materials = hk_array_summary(addr + 0x18)?;
    let motion_properties = hk_array_summary(addr + 0x28)?;
    let body_cinfos = hk_array_summary(addr + 0x38)?;
    let constraint_cinfos = hk_array_summary(addr + 0x48)?;
    if body_cinfos.size <= 0
        || body_cinfos.size > 512
        || materials.size < 0
        || materials.size > 512
        || motion_properties.size < 0
        || motion_properties.size > 512
        || constraint_cinfos.size < 0
        || constraint_cinfos.size > 1024
    {
        return None;
    }

    Some(format!(
        "hknpPhysicsSystemData? materials={} motions={} body_cinfos={} constraints={}",
        materials.size, motion_properties.size, body_cinfos.size, constraint_cinfos.size
    ))
}

fn hknp_body_data_summary(addr: usize) -> Option<String> {
    if !is_readable(addr, 0xb0) {
        return None;
    }

    let shape = read_usize(addr + 0x18)?;
    let initial_position = read_vec4(addr + 0x40)?;
    let initial_orientation = read_vec4(addr + 0x50)?;
    if shape <= 0x10000
        || !is_readable(shape, 0x38)
        || !initial_position
            .into_iter()
            .chain(initial_orientation)
            .all(|value| value.is_finite() && value.abs() <= 100000.0)
        || !(0.5..=1.5).contains(&length4(initial_orientation))
    {
        return None;
    }

    let shape_type = read_u16(shape + 0x1a).unwrap_or(u16::MAX);
    Some(format!(
        "hknpBodyData? shape=0x{shape:x} shape_type=0x{shape_type:x} initial_pos=[{:.3},{:.3},{:.3}]",
        initial_position[0], initial_position[1], initial_position[2]
    ))
}

fn inspect_hknp_capsule_shape(name: &str, addr: usize) {
    if !is_readable(addr, 0x80) {
        return;
    }

    let convex_radius = read_f32(addr + 0x20).unwrap_or(f32::NAN);
    let a = read_vec4(addr + 0x60).unwrap_or([f32::NAN; 4]);
    let b = read_vec4(addr + 0x70).unwrap_or([f32::NAN; 4]);
    if convex_radius.is_finite() && a.into_iter().chain(b).all(f32::is_finite) {
        log::line(format_args!(
            "[player-scale-no-bone] hkx hknp capsule landmark {name}: addr=0x{addr:x} convex_radius={convex_radius:.5} a=[{:.3},{:.3},{:.3},{:.3}] b=[{:.3},{:.3},{:.3},{:.3}]",
            a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]
        ));
    }
}

#[derive(Clone, Copy)]
struct HkArraySummary {
    data: usize,
    size: i32,
    capacity_and_flags: i32,
}

fn hk_array_summary(addr: usize) -> Option<HkArraySummary> {
    let data = read_usize(addr)?;
    let size = read_i32(addr + 0x8)?;
    let capacity_and_flags = read_i32(addr + 0xc)?;
    if data == 0 && size == 0 {
        return Some(HkArraySummary {
            data,
            size,
            capacity_and_flags,
        });
    }
    let capacity = capacity_and_flags & 0x3fff_ffff;
    if data <= 0x10000
        || size < 0
        || capacity < size
        || !is_readable(data, (size as usize).min(1) * 4)
    {
        return None;
    }

    Some(HkArraySummary {
        data,
        size,
        capacity_and_flags,
    })
}

fn format_hk_array(summary: Option<HkArraySummary>) -> String {
    match summary {
        Some(summary) => format!(
            "data=0x{:x},size={},cap_flags=0x{:x}",
            summary.data, summary.size, summary.capacity_and_flags
        ),
        None => "invalid".to_string(),
    }
}

fn looks_like_hcl_capsule(
    start: [f32; 4],
    end: [f32; 4],
    dir: [f32; 4],
    radius: f32,
    cap_len_sqrd_inv: f32,
) -> bool {
    if !start.into_iter().chain(end).chain(dir).all(f32::is_finite)
        || !radius.is_finite()
        || !cap_len_sqrd_inv.is_finite()
        || !(0.001..=5.0).contains(&radius)
        || !(0.0..=1000.0).contains(&cap_len_sqrd_inv)
    {
        return false;
    }

    let span = distance3(start, end);
    let dir_len = length3(dir);
    (0.01..=20.0).contains(&span) && (0.5..=1.5).contains(&dir_len)
}

fn looks_like_transform(matrix: &[f32; 16]) -> bool {
    if !matrix
        .iter()
        .all(|value| value.is_finite() && value.abs() <= 100000.0)
    {
        return false;
    }

    let basis_len_0 = length3([matrix[0], matrix[1], matrix[2], 0.0]);
    let basis_len_1 = length3([matrix[4], matrix[5], matrix[6], 0.0]);
    let basis_len_2 = length3([matrix[8], matrix[9], matrix[10], 0.0]);

    (0.05..=20.0).contains(&basis_len_0)
        && (0.05..=20.0).contains(&basis_len_1)
        && (0.05..=20.0).contains(&basis_len_2)
}

fn distance3(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn length3(value: [f32; 4]) -> f32 {
    (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt()
}

fn length4(value: [f32; 4]) -> f32 {
    (value[0] * value[0] + value[1] * value[1] + value[2] * value[2] + value[3] * value[3]).sqrt()
}

fn read_matrix4(addr: usize) -> Option<[f32; 16]> {
    let mut matrix = [0.0; 16];
    for (index, value) in matrix.iter_mut().enumerate() {
        *value = read_f32(addr + index * size_of::<f32>())?;
    }
    Some(matrix)
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

fn read_u16(addr: usize) -> Option<u16> {
    if !is_readable(addr, size_of::<u16>()) {
        return None;
    }

    Some(unsafe { (addr as *const u16).read_unaligned() })
}

fn read_i32(addr: usize) -> Option<i32> {
    if !is_readable(addr, size_of::<i32>()) {
        return None;
    }

    Some(unsafe { (addr as *const i32).read_unaligned() })
}

fn read_u8(addr: usize) -> Option<u8> {
    if !is_readable(addr, size_of::<u8>()) {
        return None;
    }

    Some(unsafe { (addr as *const u8).read_unaligned() })
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
    fn recognizes_plausible_hcl_capsule() {
        assert!(looks_like_hcl_capsule(
            [0.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            0.1,
            1.0
        ));
    }

    #[test]
    fn rejects_zero_span_hcl_capsule() {
        assert!(!looks_like_hcl_capsule(
            [0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            0.1,
            1.0
        ));
    }

    #[test]
    fn recognizes_plausible_transform() {
        let matrix = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 2.0, 3.0, 4.0, 1.0,
        ];
        assert!(looks_like_transform(&matrix));
    }
}
