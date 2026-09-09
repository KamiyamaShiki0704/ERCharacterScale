use eldenring::cs::PlayerIns;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery,
};

use crate::log;

const CHR_INS_CLOTH_STATE_OFFSET: usize = 0x3b8;
const CHR_INS_CLOTH_FLAG_OFFSET: usize = 0x548;
const CHR_INS_MODEL_INS_OFFSET: usize = 0x50;
const PHYSICS_HK_COLLISION_SHAPE_OFFSET: usize = 0xb0;
const POINTER_SCAN_BYTES: usize = 0x80;
const FOCUSED_SCAN_BYTES: usize = 0x100;
const WORD_DUMP_BYTES: usize = 0xc0;
const CSMODELINS_MODEL_ITEM_OFFSET: usize = 0x10;
const CSMODELINS_MODEL_DISP_ENTITY_OFFSET: usize = 0x18;
const CSMODELINS_LOCATION_ENTITY_OFFSET: usize = 0x20;
const MODEL_ITEM_FLVER_MODEL_DATA_OFFSET: usize = 0x68;
const MODEL_ITEM_TAIL_OFFSET: usize = 0x630;
const MODEL_ITEM_TAIL_BYTES: usize = 0x80;
const MODEL_ITEM_MTX43_ARRAY_ENTITY_OFFSET: usize = 0x640;
const MODEL_ITEM_DEFAULT_DMY_LOCATION_MODIFIER_OFFSET: usize = 0x650;
const MODEL_ITEM_LOCATION_AABB_EXPORTER_OFFSET: usize = 0x658;
const MODEL_ENTITY_DUMP_BYTES: usize = 0x80;
const MODEL_CHILD_DUMP_BYTES: usize = 0x100;

const CHR_INS_UPDATE_TASKS: &[(&str, usize)] = &[
    ("update_data_module_task", 0x410),
    ("update_chr_ctrl_task", 0x440),
    ("update_chr_model_task", 0x470),
    ("update_havok_task", 0x4a0),
    ("update_replay_recorder_task", 0x4d0),
    ("update_behavior_task", 0x500),
];

pub fn probe_player_havok_roots(player: &mut PlayerIns, scale: f32) {
    let player_addr = player as *mut _ as usize;
    let chr_ins = &mut player.chr_ins;
    let chr_ins_addr = chr_ins as *mut _ as usize;
    let chr_ctrl = chr_ins.chr_ctrl.as_mut();
    let chr_ctrl_addr = chr_ctrl as *mut _ as usize;
    let physics = chr_ins.modules.as_mut().physics.as_mut();
    let physics_addr = physics as *mut _ as usize;
    let hk_collision_shape =
        read_usize(physics_addr + PHYSICS_HK_COLLISION_SHAPE_OFFSET).unwrap_or(0);
    let cloth_state = chr_ins_addr + CHR_INS_CLOTH_STATE_OFFSET;
    let chr_model_ins = read_usize(chr_ins_addr + CHR_INS_MODEL_INS_OFFSET).unwrap_or(0);

    log_line(format_args!(
        "[player-scale-no-bone] havok probe scale={:.3} player=0x{:x} chr_ins=0x{:x} chr_ctrl=0x{:x} physics=0x{:x}",
        scale, player_addr, chr_ins_addr, chr_ctrl_addr, physics_addr
    ));

    log_root("ChrCtrl::chr_collision", chr_ctrl.chr_collision);
    log_root("ChrCtrl::ragdoll_ins", chr_ctrl.ragdoll_ins);
    log_root(
        "CSChrPhysicsModule+0xb0 hk_collision_shape",
        hk_collision_shape,
    );
    log_root("ChrIns+0x3b8 cloth_state", cloth_state);
    log_root(
        "ChrIns+0x548 cloth flag/state",
        chr_ins_addr + CHR_INS_CLOTH_FLAG_OFFSET,
    );
    log_root("ChrIns::chr_model_ins", chr_model_ins);

    scan_pointer_fields("chr_collision", chr_ctrl.chr_collision);
    scan_pointer_fields("ragdoll_ins", chr_ctrl.ragdoll_ins);
    scan_pointer_fields_focused("hk_collision_shape", hk_collision_shape);
    scan_pointer_fields_focused("cloth_state", cloth_state);
    dump_words("hk_collision_shape", hk_collision_shape);
    dump_words("cloth_state", cloth_state);
    probe_chr_update_tasks(chr_ins_addr);
    probe_chr_model_ins(chr_model_ins);
}

fn log_root(name: &str, addr: usize) {
    let readable = is_readable(addr, 0x10);
    let first_qword = read_usize(addr).unwrap_or(0);
    log_line(format_args!(
        "[player-scale-no-bone] {name}: addr=0x{addr:x} readable={readable} first_qword=0x{first_qword:x}"
    ));
}

fn scan_pointer_fields(name: &str, root: usize) {
    scan_pointer_fields_impl(name, root, POINTER_SCAN_BYTES);
}

fn scan_pointer_fields_focused(name: &str, root: usize) {
    scan_pointer_fields_impl(name, root, FOCUSED_SCAN_BYTES);
}

fn scan_pointer_fields_impl(name: &str, root: usize, bytes: usize) {
    if !is_readable(root, bytes) {
        return;
    }

    let pointer_count = bytes / size_of::<usize>();
    for index in 0..pointer_count {
        let offset = index * size_of::<usize>();
        let Some(value) = read_usize(root + offset) else {
            continue;
        };

        if value > 0x10000 && is_readable(value, 0x10) {
            log_line(format_args!(
                "[player-scale-no-bone] {name}+0x{offset:x} -> 0x{value:x} first_qword=0x{:x}",
                read_usize(value).unwrap_or(0)
            ));
        }
    }
}

fn dump_words(name: &str, root: usize) {
    if !is_readable(root, WORD_DUMP_BYTES) {
        return;
    }

    for offset in (0..WORD_DUMP_BYTES).step_by(0x10) {
        let q0 = read_usize(root + offset).unwrap_or(0);
        let q8 = read_usize(root + offset + 0x8).unwrap_or(0);
        let f0 = read_f32(root + offset).unwrap_or(f32::NAN);
        let f4 = read_f32(root + offset + 0x4).unwrap_or(f32::NAN);
        let f8 = read_f32(root + offset + 0x8).unwrap_or(f32::NAN);
        let fc = read_f32(root + offset + 0xc).unwrap_or(f32::NAN);

        log_line(format_args!(
            "[player-scale-no-bone] {name}+0x{offset:x} qwords=[0x{q0:x}, 0x{q8:x}] f32=[{f0:.6}, {f4:.6}, {f8:.6}, {fc:.6}]"
        ));
    }
}

fn probe_chr_update_tasks(chr_ins_addr: usize) {
    for (name, offset) in CHR_INS_UPDATE_TASKS {
        let task = chr_ins_addr + offset;
        let vtable = read_usize(task).unwrap_or(0);
        let proxy = read_usize(task + 0x10).unwrap_or(0);
        let subject = read_usize(task + 0x20).unwrap_or(0);
        let executor = read_usize(task + 0x28).unwrap_or(0);
        let proxy_task = read_usize(proxy + 0x10).unwrap_or(0);

        log_line(format_args!(
            "[player-scale-no-bone] ChrIns::{name} @+0x{offset:x}: vtable=0x{vtable:x} proxy=0x{proxy:x} proxy_task=0x{proxy_task:x} subject=0x{subject:x} executor=0x{executor:x}"
        ));
    }
}

fn probe_chr_model_ins(chr_model_ins: usize) {
    if !is_readable(chr_model_ins, 0x30) {
        return;
    }

    let model_item = read_usize(chr_model_ins + CSMODELINS_MODEL_ITEM_OFFSET).unwrap_or(0);
    let model_disp_entity =
        read_usize(chr_model_ins + CSMODELINS_MODEL_DISP_ENTITY_OFFSET).unwrap_or(0);
    let location_entity =
        read_usize(chr_model_ins + CSMODELINS_LOCATION_ENTITY_OFFSET).unwrap_or(0);

    log_line(format_args!(
        "[player-scale-no-bone] CSChrModelIns fields: model_item=0x{model_item:x} model_disp_entity=0x{model_disp_entity:x} location_entity=0x{location_entity:x}"
    ));

    scan_pointer_fields_focused("chr_model_ins", chr_model_ins);
    dump_words("chr_model_ins", chr_model_ins);

    log_root("CSFD4ModelItem", model_item);
    scan_pointer_fields_focused("model_item head", model_item);
    dump_words("model_item head", model_item);
    scan_pointer_fields_range(
        "model_item tail",
        model_item,
        MODEL_ITEM_TAIL_OFFSET,
        MODEL_ITEM_TAIL_BYTES,
    );
    dump_words_range(
        "model_item tail",
        model_item,
        MODEL_ITEM_TAIL_OFFSET,
        MODEL_ITEM_TAIL_BYTES,
    );

    probe_model_named_roots(model_item, model_disp_entity, location_entity);
}

fn probe_model_named_roots(model_item: usize, model_disp_entity: usize, location_entity: usize) {
    let flver_model_data = read_usize(model_item + MODEL_ITEM_FLVER_MODEL_DATA_OFFSET).unwrap_or(0);
    let mtx43_array_entity =
        read_usize(model_item + MODEL_ITEM_MTX43_ARRAY_ENTITY_OFFSET).unwrap_or(0);
    let default_dmypoly_location_modifier =
        read_usize(model_item + MODEL_ITEM_DEFAULT_DMY_LOCATION_MODIFIER_OFFSET).unwrap_or(0);
    let location_aabb_exporter =
        read_usize(model_item + MODEL_ITEM_LOCATION_AABB_EXPORTER_OFFSET).unwrap_or(0);

    log_line(format_args!(
        "[player-scale-no-bone] CSFD4ModelItem named fields: flver_model_data=0x{flver_model_data:x} mtx43_array_entity=0x{mtx43_array_entity:x} default_dmypoly_location_modifier=0x{default_dmypoly_location_modifier:x} location_aabb_exporter=0x{location_aabb_exporter:x}"
    ));

    probe_named_model_root("model_disp_entity", model_disp_entity);
    probe_named_model_root("location_entity", location_entity);
    probe_named_model_root("flver_model_data", flver_model_data);
    probe_named_model_root("mtx43_array_entity", mtx43_array_entity);
    probe_named_model_root(
        "default_dmypoly_location_modifier",
        default_dmypoly_location_modifier,
    );
    probe_named_model_root("location_aabb_exporter", location_aabb_exporter);
    probe_model_child_roots(
        location_entity,
        mtx43_array_entity,
        default_dmypoly_location_modifier,
        location_aabb_exporter,
    );
    probe_model_grandchild_roots(default_dmypoly_location_modifier, location_aabb_exporter);
}

fn probe_named_model_root(name: &str, root: usize) {
    probe_named_model_root_with_bytes(name, root, MODEL_ENTITY_DUMP_BYTES);
}

fn probe_named_model_root_with_bytes(name: &str, root: usize, bytes: usize) {
    log_root(name, root);
    scan_pointer_fields_range(name, root, 0, bytes);
    dump_words_range(name, root, 0, bytes);
}

fn probe_model_child_roots(
    location_entity: usize,
    mtx43_array_entity: usize,
    default_dmypoly_location_modifier: usize,
    location_aabb_exporter: usize,
) {
    let candidates = [
        ("location_entity", location_entity, 0x28),
        ("location_entity", location_entity, 0x30),
        ("location_entity", location_entity, 0x50),
        ("mtx43_array_entity", mtx43_array_entity, 0x50),
        ("mtx43_array_entity", mtx43_array_entity, 0x70),
        (
            "default_dmypoly_location_modifier",
            default_dmypoly_location_modifier,
            0x68,
        ),
        ("location_aabb_exporter", location_aabb_exporter, 0x28),
        ("location_aabb_exporter", location_aabb_exporter, 0x70),
        ("location_aabb_exporter", location_aabb_exporter, 0x78),
    ];

    for (parent_name, parent, offset) in candidates {
        let child_addr_field = parent.saturating_add(offset);
        let child = read_usize(child_addr_field).unwrap_or(0);

        log_line(format_args!(
            "[player-scale-no-bone] model child candidate {parent_name}+0x{offset:x}: parent=0x{parent:x} child=0x{child:x}"
        ));

        let child_name = format!("{parent_name}+0x{offset:x} child");
        probe_named_model_root_with_bytes(&child_name, child, MODEL_CHILD_DUMP_BYTES);
    }
}

fn probe_model_grandchild_roots(
    default_dmypoly_location_modifier: usize,
    location_aabb_exporter: usize,
) {
    let default_dmy_child = read_usize(default_dmypoly_location_modifier + 0x68).unwrap_or(0);
    let aabb_child_70 = read_usize(location_aabb_exporter + 0x70).unwrap_or(0);
    let aabb_child_78 = read_usize(location_aabb_exporter + 0x78).unwrap_or(0);

    let candidates = [
        (
            "default_dmypoly_location_modifier+0x68 child",
            default_dmy_child,
            0x50,
        ),
        ("location_aabb_exporter+0x70 child", aabb_child_70, 0x58),
        ("location_aabb_exporter+0x70 child", aabb_child_70, 0xd8),
        ("location_aabb_exporter+0x70 child", aabb_child_70, 0xf8),
        ("location_aabb_exporter+0x78 child", aabb_child_78, 0x20),
        ("location_aabb_exporter+0x78 child", aabb_child_78, 0x28),
        ("location_aabb_exporter+0x78 child", aabb_child_78, 0x78),
        ("location_aabb_exporter+0x78 child", aabb_child_78, 0x80),
        ("location_aabb_exporter+0x78 child", aabb_child_78, 0xb0),
    ];

    for (parent_name, parent, offset) in candidates {
        let child = read_usize(parent.saturating_add(offset)).unwrap_or(0);

        log_line(format_args!(
            "[player-scale-no-bone] model grandchild candidate {parent_name}+0x{offset:x}: parent=0x{parent:x} child=0x{child:x}"
        ));

        let child_name = format!("{parent_name}+0x{offset:x} grandchild");
        probe_named_model_root_with_bytes(&child_name, child, MODEL_CHILD_DUMP_BYTES);
    }
}

fn scan_pointer_fields_range(name: &str, root: usize, start_offset: usize, bytes: usize) {
    let range_start = root.saturating_add(start_offset);
    if !is_readable(range_start, bytes) {
        return;
    }

    let pointer_count = bytes / size_of::<usize>();
    for index in 0..pointer_count {
        let offset = start_offset + index * size_of::<usize>();
        let Some(value) = read_usize(root + offset) else {
            continue;
        };

        if value > 0x10000 && is_readable(value, 0x10) {
            log_line(format_args!(
                "[player-scale-no-bone] {name}+0x{offset:x} -> 0x{value:x} first_qword=0x{:x}",
                read_usize(value).unwrap_or(0)
            ));
        }
    }
}

fn dump_words_range(name: &str, root: usize, start_offset: usize, bytes: usize) {
    let range_start = root.saturating_add(start_offset);
    if !is_readable(range_start, bytes) {
        return;
    }

    for rel_offset in (0..bytes).step_by(0x10) {
        let offset = start_offset + rel_offset;
        let q0 = read_usize(root + offset).unwrap_or(0);
        let q8 = read_usize(root + offset + 0x8).unwrap_or(0);
        let f0 = read_f32(root + offset).unwrap_or(f32::NAN);
        let f4 = read_f32(root + offset + 0x4).unwrap_or(f32::NAN);
        let f8 = read_f32(root + offset + 0x8).unwrap_or(f32::NAN);
        let fc = read_f32(root + offset + 0xc).unwrap_or(f32::NAN);

        log_line(format_args!(
            "[player-scale-no-bone] {name}+0x{offset:x} qwords=[0x{q0:x}, 0x{q8:x}] f32=[{f0:.6}, {f4:.6}, {f8:.6}, {fc:.6}]"
        ));
    }
}

fn is_readable(addr: usize, size: usize) -> bool {
    if addr == 0 || size == 0 {
        return false;
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
        return false;
    }

    if mbi.Protect.contains(PAGE_GUARD) || mbi.Protect.contains(PAGE_NOACCESS) {
        return false;
    }

    let start = addr;
    let end = addr.saturating_add(size);
    let region_start = mbi.BaseAddress as usize;
    let region_end = region_start.saturating_add(mbi.RegionSize);

    end >= start && start >= region_start && end <= region_end
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

fn log_line(args: std::fmt::Arguments<'_>) {
    log::line(args);
}
