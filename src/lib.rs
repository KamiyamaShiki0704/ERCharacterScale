use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

mod body_scale_port;
mod cloth_collider_rotation;
mod cloth_collider_rotation_hook;
mod cloth_diagnostic;
mod cloth_instance_aabb_scale;
mod cloth_local_scale;
mod cloth_mesh_scale;
mod cloth_owner_scope;
mod cloth_render_scale;
mod havok_probe;
mod havok_shape_scale;
mod hkx_collidable_probe;
mod log;
mod memory_query;
mod model_matrix_scale;
mod nr_probe;
mod ragdoll_motion_scale;
mod ragdoll_shape_scale;

use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrIns, PlayerIns, WorldChrMan},
    fd4::FD4TaskData,
};
use fromsoftware_shared::{FromStatic, SharedTaskImpExt};

const DLL_PROCESS_DETACH: u32 = 0;
const DLL_PROCESS_ATTACH: u32 = 1;

const BUILD_MODE: &str = "er-2.53-rigid-collider-velocity";
const ENABLE_SYNC_DIAGNOSTIC: bool = true;
const SCALE_MIN: f32 = 0.50;
const SCALE_MAX: f32 = 3.0;
const SCALE_COLLISION: bool = true;
const SCALE_WEIGHT: bool = false;
const SCALE_HKNP_CAPSULE_SHAPE: bool = false;
const SCALE_AABB_IDENTITY_MATRIX_BUFFER: bool = false;
const SCALE_RAGDOLL_BODY_CINFO_CAPSULES: bool = false;
const SCALE_RAGDOLL_LIVE_MOTION_COMS: bool = false;
const HAVOK_PROBE_ENABLED: bool = false;
const HAVOK_PROBE_INTERVAL_FRAMES: u32 = 900;
const HKX_COLLIDABLE_PROBE_ENABLED: bool = false;
const HKX_COLLIDABLE_PROBE_INTERVAL_FRAMES: u32 = 600;
const RAGDOLL_SINGLE_PROBE_ENABLED: bool = false;
const RAGDOLL_SINGLE_PROBE_READY_DELAY_FRAMES: u32 = 180;
const RAGDOLL_SINGLE_PROBE_STATUS_INTERVAL_FRAMES: u32 = 900;
const RAGDOLL_LIVE_PROBE_ENABLED: bool = false;
const RAGDOLL_LIVE_PROBE_READY_DELAY_FRAMES: u32 = 180;
const RAGDOLL_LIVE_PROBE_STATUS_INTERVAL_FRAMES: u32 = 900;
const CHR_INS_MODEL_INS_OFFSET: usize = 0x50;
const CLOTH_LOCAL_SCALE_AUDIT_INTERVAL_FRAMES: u32 = 600;
const BODY_SCALE_BINDING_AUDIT_INTERVAL_FRAMES: u32 = 60;
const CLOTH_TRANSITION_QUEUE_AUDIT_INTERVAL_FRAMES: u32 = 60;
const PERF_SLOW_FRAME_THRESHOLD_MICROS: u128 = 8_000;
const ENABLE_DETAILED_CLOTH_CHILD_LOGGING: bool = false;

static STARTED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct ScaleEffect {
    sp_effect: i32,
    scale: f32,
}

const SCALE_EFFECTS: &[ScaleEffect] = &[
    ScaleEffect {
        sp_effect: 21202000,
        scale: 1.05,
    },
    ScaleEffect {
        sp_effect: 21202001,
        scale: 1.10,
    },
    ScaleEffect {
        sp_effect: 21202002,
        scale: 1.15,
    },
    ScaleEffect {
        sp_effect: 21202003,
        scale: 1.20,
    },
    ScaleEffect {
        sp_effect: 21202004,
        scale: 1.25,
    },
    ScaleEffect {
        sp_effect: 21202005,
        scale: 1.30,
    },
    ScaleEffect {
        sp_effect: 21202050,
        scale: 0.95,
    },
    ScaleEffect {
        sp_effect: 21202051,
        scale: 0.90,
    },
    ScaleEffect {
        sp_effect: 21202052,
        scale: 0.85,
    },
    ScaleEffect {
        sp_effect: 21202053,
        scale: 0.80,
    },
    ScaleEffect {
        sp_effect: 21202054,
        scale: 0.75,
    },
    ScaleEffect {
        sp_effect: 21202055,
        scale: 0.70,
    },
    ScaleEffect {
        sp_effect: 8020400,
        scale: 0.50,
    },
    ScaleEffect {
        sp_effect: 8020401,
        scale: 3.00,
    },
];

#[derive(Default)]
struct ScaleState {
    task_frames: u64,
    last_runtime_stage: &'static str,
    player_addr: Option<usize>,
    baseline: Option<PhysicsBaseline>,
    hknp_capsule_baseline: Option<havok_shape_scale::HknpCapsuleBaseline>,
    aabb_identity_matrix_state: model_matrix_scale::IdentityMatrixScaleState,
    ragdoll_motion_scale_state: ragdoll_motion_scale::RagdollMotionScaleState,
    ragdoll_body_cinfo_shape_state: ragdoll_shape_scale::RagdollBodyCinfoShapeScaleState,
    cloth_local_scale_state: cloth_local_scale::ClothLocalScaleState,
    cloth_instance_aabb_scale_state: cloth_instance_aabb_scale::ClothInstanceAabbScaleState,
    probe_frame: u32,
    last_probe_scale: f32,
    last_probe_model_addr: usize,
    hkx_probe_frame: u32,
    last_hkx_probe_scale: f32,
    last_hkx_probe_model_addr: usize,
    last_hkx_probe_chr_collision_addr: usize,
    last_hkx_probe_ragdoll_addr: usize,
    ragdoll_single_ready_frames: u32,
    ragdoll_single_done_player_addr: usize,
    ragdoll_single_done_ragdoll_addr: usize,
    ragdoll_single_status_frames: u32,
    ragdoll_single_last_status_player_addr: usize,
    ragdoll_single_last_status_model_addr: usize,
    ragdoll_single_last_status_ragdoll_addr: usize,
    ragdoll_single_last_status_scale: f32,
    ragdoll_live_ready_frames: u32,
    ragdoll_live_done_player_addr: usize,
    ragdoll_live_done_ragdoll_addr: usize,
    ragdoll_live_status_frames: u32,
    ragdoll_live_last_status_player_addr: usize,
    ragdoll_live_last_status_model_addr: usize,
    ragdoll_live_last_status_ragdoll_addr: usize,
    ragdoll_live_last_status_scale: f32,
    last_port_ready: bool,
    last_port_pose_importer: usize,
    last_port_cloth_pose_importer: usize,
    last_port_cloth_pose_source: body_scale_port::ClothPoseSource,
    last_port_anim_skeleton: usize,
    last_port_requested_scale: f32,
    last_port_reason: &'static str,
    port_status_frames: u32,
    cached_binding: Option<body_scale_port::TargetBinding>,
    binding_audit_frames: u32,
    cloth_queue_audit_frames: u32,
    cloth_queue_last_target: usize,
    cloth_queue_last_scale_bits: u32,
    cloth_queue_last_generation: u64,
    cloth_probe_unit_tail_frames: u32,
    cloth_local_last_target: usize,
    cloth_local_last_scale_bits: u32,
    cloth_local_last_generation: u64,
    cloth_local_refresh_frames: u32,
    cloth_local_refresh_calls: u64,
    cloth_local_skipped_frames: u64,
    cloth_local_work_pending: bool,
    cloth_aabb_last_target: usize,
    cloth_aabb_last_scale_bits: u32,
    cloth_aabb_last_generation: u64,
    cloth_aabb_refresh_frames: u32,
    cloth_aabb_refresh_calls: u64,
    cloth_aabb_skipped_frames: u64,
    cloth_aabb_work_pending: bool,
    matrix_candidate_hits: [u64; 16],
}

#[derive(Clone, Copy)]
struct PhysicsBaseline {
    chr_hit_height: f32,
    chr_hit_radius: f32,
    hit_height: f32,
    hit_radius: f32,
    weight: f32,
}

impl ScaleState {
    fn reset(&mut self) {
        cloth_local_scale::restore_cloth_local_dimensions(&mut self.cloth_local_scale_state);
        cloth_instance_aabb_scale::clear_cloth_instance_aabb_state(
            &mut self.cloth_instance_aabb_scale_state,
        );
        body_scale_port::clear_target();
        self.player_addr = None;
        self.baseline = None;
        self.hknp_capsule_baseline = None;
        self.aabb_identity_matrix_state = model_matrix_scale::IdentityMatrixScaleState::default();
        self.ragdoll_motion_scale_state = ragdoll_motion_scale::RagdollMotionScaleState::default();
        self.ragdoll_body_cinfo_shape_state =
            ragdoll_shape_scale::RagdollBodyCinfoShapeScaleState::default();
        self.probe_frame = 0;
        self.last_probe_scale = 1.0;
        self.last_probe_model_addr = 0;
        self.hkx_probe_frame = 0;
        self.last_hkx_probe_scale = 1.0;
        self.last_hkx_probe_model_addr = 0;
        self.last_hkx_probe_chr_collision_addr = 0;
        self.last_hkx_probe_ragdoll_addr = 0;
        self.ragdoll_single_ready_frames = 0;
        self.ragdoll_single_done_player_addr = 0;
        self.ragdoll_single_done_ragdoll_addr = 0;
        self.ragdoll_single_status_frames = 0;
        self.ragdoll_single_last_status_player_addr = 0;
        self.ragdoll_single_last_status_model_addr = 0;
        self.ragdoll_single_last_status_ragdoll_addr = 0;
        self.ragdoll_single_last_status_scale = 1.0;
        self.ragdoll_live_ready_frames = 0;
        self.ragdoll_live_done_player_addr = 0;
        self.ragdoll_live_done_ragdoll_addr = 0;
        self.ragdoll_live_status_frames = 0;
        self.ragdoll_live_last_status_player_addr = 0;
        self.ragdoll_live_last_status_model_addr = 0;
        self.ragdoll_live_last_status_ragdoll_addr = 0;
        self.ragdoll_live_last_status_scale = 1.0;
        self.last_port_ready = false;
        self.last_port_pose_importer = 0;
        self.last_port_cloth_pose_importer = 0;
        self.last_port_cloth_pose_source = body_scale_port::ClothPoseSource::Unavailable;
        self.last_port_anim_skeleton = 0;
        self.last_port_requested_scale = 1.0;
        self.last_port_reason = "reset";
        self.port_status_frames = 0;
        self.cached_binding = None;
        self.binding_audit_frames = 0;
        self.cloth_queue_audit_frames = 0;
        self.cloth_queue_last_target = 0;
        self.cloth_queue_last_scale_bits = 1.0f32.to_bits();
        self.cloth_queue_last_generation = 0;
        self.cloth_probe_unit_tail_frames = 0;
        self.cloth_local_last_target = 0;
        self.cloth_local_last_scale_bits = 1.0f32.to_bits();
        self.cloth_local_last_generation = 0;
        self.cloth_local_refresh_frames = 0;
        self.cloth_local_refresh_calls = 0;
        self.cloth_local_skipped_frames = 0;
        self.cloth_local_work_pending = false;
        self.cloth_aabb_last_target = 0;
        self.cloth_aabb_last_scale_bits = 1.0f32.to_bits();
        self.cloth_aabb_last_generation = 0;
        self.cloth_aabb_refresh_frames = 0;
        self.cloth_aabb_refresh_calls = 0;
        self.cloth_aabb_skipped_frames = 0;
        self.cloth_aabb_work_pending = false;
        self.matrix_candidate_hits = [0; 16];
    }
}

impl PhysicsBaseline {
    fn capture(physics: &eldenring::cs::CSChrPhysicsModule) -> Self {
        Self {
            chr_hit_height: physics.chr_hit_height,
            chr_hit_radius: physics.chr_hit_radius,
            hit_height: physics.hit_height,
            hit_radius: physics.hit_radius,
            weight: physics.weight,
        }
    }
}

/// # Safety
/// Exposed for Windows LoadLibrary. Do not call directly.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DllMain(hmodule: usize, reason: u32) -> bool {
    match reason {
        DLL_PROCESS_ATTACH => {
            SHUTDOWN.store(false, Ordering::Release);
            if STARTED.swap(true, Ordering::AcqRel) {
                return true;
            }

            std::thread::spawn(move || run_task_thread(hmodule));
            true
        }
        DLL_PROCESS_DETACH => {
            SHUTDOWN.store(true, Ordering::Release);
            log::line(format_args!("dll detach requested"));
            true
        }
        _ => true,
    }
}

fn run_task_thread(hmodule: usize) {
    let thread_started = Instant::now();
    log::initialize(hmodule);
    let logger_micros = thread_started.elapsed().as_micros();
    let exe_path = std::env::current_exe()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "<unknown>".to_string());
    let exe_path_lower = exe_path.to_ascii_lowercase();

    if exe_path_lower.contains("nightreign.exe") {
        run_nightreign_readonly_probe(&exe_path);
        return;
    }

    if !exe_path_lower.contains("eldenring.exe") {
        log::line(format_args!(
            "[player-scale-no-bone] unsupported host process exe_path={exe_path}; no task registered"
        ));
        return;
    }

    let hook_started = Instant::now();
    let body_scale_hooks_ready = body_scale_port::install();
    let hook_install_micros = hook_started.elapsed().as_micros();
    log::line(format_args!(
        "[player-scale-no-bone] ER body-scale port install_ready={body_scale_hooks_ready}"
    ));
    // Never reach fsrs lazy version detection or task registration after a
    // rejected executable/hook layout (including unsupported future updates).
    if !body_scale_hooks_ready {
        log::line(format_args!(
            "[player-scale-no-bone] runtime rejected; no upstream task access"
        ));
        return;
    }

    log::line(format_args!(
        "[player-scale-no-bone] waiting for upstream ER 2.7 task system"
    ));
    let task_wait_started = Instant::now();
    let cs_task = match CSTaskImp::wait_for_instance(Duration::MAX) {
        Ok(cs_task) => cs_task,
        Err(reason) => {
            log::line(format_args!(
                "[player-scale-no-bone] failed to find upstream ER 2.7 task system: {reason}"
            ));
            STARTED.store(false, Ordering::Release);
            return;
        }
    };
    let task_wait_micros = task_wait_started.elapsed().as_micros();

    log::line(format_args!(
        "[player-scale-no-bone] upstream ER 2.7 task system ready; registering scale task"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] build_mode={BUILD_MODE} task_group=ChrIns_PrePhysics body_scale_hooks_ready={body_scale_hooks_ready} scale_collision={SCALE_COLLISION} scale_weight={SCALE_WEIGHT} scale_hknp_capsule={SCALE_HKNP_CAPSULE_SHAPE} scale_aabb_identity_matrix={SCALE_AABB_IDENTITY_MATRIX_BUFFER} scale_ragdoll_body_cinfo_capsules={SCALE_RAGDOLL_BODY_CINFO_CAPSULES} scale_ragdoll_live_motion_coms={SCALE_RAGDOLL_LIVE_MOTION_COMS} heavy_havok_probe={HAVOK_PROBE_ENABLED} hkx_collidable_probe={HKX_COLLIDABLE_PROBE_ENABLED} ragdoll_single_probe={RAGDOLL_SINGLE_PROBE_ENABLED} ragdoll_live_probe={RAGDOLL_LIVE_PROBE_ENABLED}"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] cloth_collision_map_offsets=observed-unscaled-falsified cloth_collision_readback=disabled-falsified cloth_self_collision_dimensions=scaled-baseline cloth_virtual_collision_safe_radius=observed-unscaled-falsified-current-equipment cloth_collision_aabb_scale=current-root-attached-native-cache-only"
    ));
    log::line(format_args!(
        "[PERF-ERPS-INIT] logger_us={logger_micros} hook_install_us={hook_install_micros} task_wait_us={task_wait_micros} persistent_log_handle=true debugger_mirror=false"
    ));

    let mut state = ScaleState::default();
    cs_task.run_recurring(
        move |_: &FD4TaskData| {
            state.task_frames = state.task_frames.wrapping_add(1);
            if state.task_frames == 1 {
                log::line(format_args!(
                    "[player-scale-no-bone] ER 2.7 scale task callback active"
                ));
            }
            if SHUTDOWN.load(Ordering::Acquire) {
                maybe_log_runtime_stage(&mut state, "shutdown");
                state.reset();
                return;
            }

            let Ok(world_chr_man) = (unsafe { WorldChrMan::instance_mut() }) else {
                maybe_log_runtime_stage(&mut state, "world-chr-man-unavailable");
                state.reset();
                return;
            };

            let Some(player_ptr) = world_chr_man.main_player.as_mut() else {
                maybe_log_runtime_stage(&mut state, "main-player-unavailable");
                state.reset();
                return;
            };

            maybe_log_runtime_stage(&mut state, "local-player-ready");
            apply_player_scale(player_ptr.as_mut(), &mut state);
            if ENABLE_SYNC_DIAGNOSTIC {
                cloth_diagnostic::tick(state.task_frames);
            }
        },
        CSTaskGroupIndex::ChrIns_PrePhysics,
    );
}

fn maybe_log_runtime_stage(state: &mut ScaleState, stage: &'static str) {
    if stage == state.last_runtime_stage {
        return;
    }
    log::line(format_args!(
        "[player-scale-no-bone] ER 2.7 runtime stage={stage}"
    ));
    state.last_runtime_stage = stage;
}

fn run_nightreign_readonly_probe(exe_path: &str) {
    log::line(format_args!(
        "[player-scale-no-bone] build_mode={BUILD_MODE} host=nightreign exe_path={exe_path}"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] NR readonly probe: no ER singleton/task access, no Havok writes"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] NR static anchor: CHARACTER_INIT_PARAM.character_scale offset=0x10C size=0x140"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] NR candidate rvas from movss +0x10C scan: read 0xAC4C96/write 0xAC4CB6 checks byte +0x109; read 0x7DD67E checks byte +0x100; read 0x66CF66 and 0x6702CE copy +0x10C to object +0x6B8/+0x6A4"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] NR installing read-only runtime hooks for Havok/cloth/physics root discovery"
    ));
    nr_probe::install_havok_runtime_root_probe();
}

fn apply_player_scale(player: &mut PlayerIns, state: &mut ScaleState) {
    let frame_started = Instant::now();
    let player_addr = player as *mut PlayerIns as usize;
    if state.player_addr != Some(player_addr) {
        cloth_local_scale::restore_cloth_local_dimensions(&mut state.cloth_local_scale_state);
        cloth_instance_aabb_scale::clear_cloth_instance_aabb_state(
            &mut state.cloth_instance_aabb_scale_state,
        );
        state.player_addr = Some(player_addr);
        state.baseline = None;
        state.hknp_capsule_baseline = None;
        state.aabb_identity_matrix_state = model_matrix_scale::IdentityMatrixScaleState::default();
        state.ragdoll_motion_scale_state = ragdoll_motion_scale::RagdollMotionScaleState::default();
        state.ragdoll_body_cinfo_shape_state =
            ragdoll_shape_scale::RagdollBodyCinfoShapeScaleState::default();
        state.last_probe_model_addr = 0;
        state.last_hkx_probe_model_addr = 0;
        state.last_hkx_probe_chr_collision_addr = 0;
        state.last_hkx_probe_ragdoll_addr = 0;
        state.ragdoll_single_ready_frames = 0;
        state.ragdoll_single_done_player_addr = 0;
        state.ragdoll_single_done_ragdoll_addr = 0;
        state.ragdoll_single_status_frames = 0;
        state.ragdoll_single_last_status_player_addr = 0;
        state.ragdoll_single_last_status_model_addr = 0;
        state.ragdoll_single_last_status_ragdoll_addr = 0;
        state.ragdoll_single_last_status_scale = 1.0;
        state.ragdoll_live_ready_frames = 0;
        state.ragdoll_live_done_player_addr = 0;
        state.ragdoll_live_done_ragdoll_addr = 0;
        state.ragdoll_live_status_frames = 0;
        state.ragdoll_live_last_status_player_addr = 0;
        state.ragdoll_live_last_status_model_addr = 0;
        state.ragdoll_live_last_status_ragdoll_addr = 0;
        state.ragdoll_live_last_status_scale = 1.0;
        state.last_port_ready = false;
        state.last_port_pose_importer = 0;
        state.last_port_cloth_pose_importer = 0;
        state.last_port_cloth_pose_source = body_scale_port::ClothPoseSource::Unavailable;
        state.last_port_anim_skeleton = 0;
        state.last_port_requested_scale = 1.0;
        state.last_port_reason = "player-changed";
        state.port_status_frames = 0;
        state.cached_binding = None;
        state.binding_audit_frames = 0;
        state.cloth_queue_audit_frames = 0;
        state.cloth_queue_last_target = 0;
        state.cloth_queue_last_scale_bits = 1.0f32.to_bits();
        state.cloth_queue_last_generation = 0;
        state.cloth_probe_unit_tail_frames = 0;
        state.cloth_local_last_target = 0;
        state.cloth_local_last_scale_bits = 1.0f32.to_bits();
        state.cloth_local_last_generation = 0;
        state.cloth_local_refresh_frames = 0;
        state.cloth_local_refresh_calls = 0;
        state.cloth_local_skipped_frames = 0;
        state.cloth_local_work_pending = false;
        state.cloth_aabb_last_target = 0;
        state.cloth_aabb_last_scale_bits = 1.0f32.to_bits();
        state.cloth_aabb_last_generation = 0;
        state.cloth_aabb_refresh_frames = 0;
        state.cloth_aabb_refresh_calls = 0;
        state.cloth_aabb_skipped_frames = 0;
        state.cloth_aabb_work_pending = false;
        state.matrix_candidate_hits = [0; 16];
    }

    let requested_scale = active_player_scale(&player.chr_ins);
    let chr_ins_addr = &player.chr_ins as *const ChrIns as usize;
    let bind_started = Instant::now();
    let bind_queries_before = memory_query::query_count();
    state.binding_audit_frames = state.binding_audit_frames.saturating_add(1);
    let binding_audit = should_audit_body_scale_binding(
        state.cached_binding.is_some_and(|binding| binding.ready),
        state.binding_audit_frames,
    );
    let binding = if binding_audit {
        let binding = body_scale_port::bind_local_player(chr_ins_addr, requested_scale);
        state.cached_binding = Some(binding);
        state.binding_audit_frames = 0;
        binding
    } else if body_scale_port::publish_target_scale(requested_scale) {
        state
            .cached_binding
            .expect("ready binding cache disappeared")
    } else {
        let binding = body_scale_port::bind_local_player(chr_ins_addr, requested_scale);
        state.cached_binding = Some(binding);
        state.binding_audit_frames = 0;
        binding
    };
    if binding.ready {
        body_scale_port::refresh_owned_cloth_inputs(chr_ins_addr, binding.cloth_pose_importer);
    }
    let bind_micros = bind_started.elapsed().as_micros();
    let bind_queries = memory_query::query_count() - bind_queries_before;
    let applied_scale = if binding.ready { requested_scale } else { 1.0 };
    let topology_generation = body_scale_port::cloth_topology_generation();
    state.cloth_local_refresh_frames = state.cloth_local_refresh_frames.saturating_add(1);
    let local_refresh = binding.ready
        && should_refresh_cloth_local_scale(
            (
                state.cloth_local_last_target,
                state.cloth_local_last_scale_bits,
                state.cloth_local_last_generation,
            ),
            state.cloth_local_refresh_frames,
            state.cloth_local_work_pending,
            (
                binding.cloth_pose_importer,
                applied_scale.to_bits(),
                topology_generation,
            ),
        );
    let local_scale_started = Instant::now();
    let local_queries_before = memory_query::query_count();
    let mut local_scale_result = cloth_local_scale::ClothLocalScaleResult::default();
    if binding.ready {
        if local_refresh {
            state.cloth_local_refresh_calls = state.cloth_local_refresh_calls.saturating_add(1);
            local_scale_result = cloth_local_scale::scale_target_cloth_local_dimensions(
                binding.cloth_pose_importer,
                applied_scale,
                topology_generation,
                &mut state.cloth_local_scale_state,
            );
            if local_scale_result.topology_readable {
                state.cloth_local_last_target = binding.cloth_pose_importer;
                state.cloth_local_last_scale_bits = applied_scale.to_bits();
                state.cloth_local_last_generation = topology_generation;
                state.cloth_local_work_pending = local_scale_result.work_pending;
                state.cloth_local_refresh_frames = if local_scale_result.work_pending {
                    CLOTH_LOCAL_SCALE_AUDIT_INTERVAL_FRAMES
                } else {
                    0
                };
            } else {
                // Retry on the next frame while the equipment topology is
                // still materializing instead of suppressing it for 600
                // frames after an incomplete load-time sample.
                state.cloth_local_refresh_frames = CLOTH_LOCAL_SCALE_AUDIT_INTERVAL_FRAMES;
                state.cloth_local_work_pending = true;
            }
        } else {
            state.cloth_local_skipped_frames = state.cloth_local_skipped_frames.saturating_add(1);
        }
    } else {
        cloth_local_scale::restore_cloth_local_dimensions(&mut state.cloth_local_scale_state);
        state.cloth_local_last_target = 0;
        state.cloth_local_last_scale_bits = 1.0f32.to_bits();
        state.cloth_local_last_generation = topology_generation;
        state.cloth_local_refresh_frames = 0;
        state.cloth_local_work_pending = false;
    }
    let local_scale_micros = local_scale_started.elapsed().as_micros();
    let local_queries = memory_query::query_count() - local_queries_before;

    let queue_started = Instant::now();
    state.cloth_queue_audit_frames = state.cloth_queue_audit_frames.saturating_add(1);
    let queue_audit =
        state.cloth_queue_audit_frames >= CLOTH_TRANSITION_QUEUE_AUDIT_INTERVAL_FRAMES;
    if binding.ready {
        let current_queue_state = (
            binding.cloth_pose_importer,
            applied_scale.to_bits(),
            topology_generation,
        );
        let previous_queue_state = (
            state.cloth_queue_last_target,
            state.cloth_queue_last_scale_bits,
            state.cloth_queue_last_generation,
        );
        if should_queue_cloth_scale_transitions(
            previous_queue_state,
            current_queue_state,
            queue_audit,
        ) {
            body_scale_port::queue_cloth_scale_transitions(
                binding.cloth_pose_importer,
                applied_scale,
            );
            state.cloth_queue_last_target = binding.cloth_pose_importer;
            state.cloth_queue_last_scale_bits = applied_scale.to_bits();
            state.cloth_queue_last_generation = body_scale_port::cloth_topology_generation();
            state.cloth_queue_audit_frames = 0;
        }
    } else {
        state.cloth_queue_last_target = 0;
        state.cloth_queue_last_scale_bits = 1.0f32.to_bits();
        state.cloth_queue_last_generation = topology_generation;
    }
    let queue_micros = queue_started.elapsed().as_micros();
    let transition_pending = body_scale_port::has_pending_cloth_scale_transitions();

    let aabb_scale_started = Instant::now();
    let aabb_queries_before = memory_query::query_count();
    state.cloth_aabb_refresh_frames = state.cloth_aabb_refresh_frames.saturating_add(1);
    let aabb_refresh = binding.ready
        && should_refresh_cloth_instance_aabbs(
            state.cloth_local_work_pending,
            transition_pending,
            (
                state.cloth_aabb_last_target,
                state.cloth_aabb_last_scale_bits,
                state.cloth_aabb_last_generation,
            ),
            state.cloth_aabb_refresh_frames,
            state.cloth_aabb_work_pending,
            (
                binding.cloth_pose_importer,
                applied_scale.to_bits(),
                topology_generation,
            ),
        );
    let mut aabb_scale_result = cloth_instance_aabb_scale::ClothInstanceAabbScaleResult::default();
    if binding.ready {
        if aabb_refresh {
            state.cloth_aabb_refresh_calls = state.cloth_aabb_refresh_calls.saturating_add(1);
            aabb_scale_result = cloth_instance_aabb_scale::rebuild_target_cloth_instance_aabbs(
                binding.cloth_pose_importer,
                applied_scale,
                &mut state.cloth_instance_aabb_scale_state,
            );
            if aabb_scale_result.topology_readable {
                state.cloth_aabb_last_target = binding.cloth_pose_importer;
                state.cloth_aabb_last_scale_bits = applied_scale.to_bits();
                state.cloth_aabb_last_generation = topology_generation;
                state.cloth_aabb_work_pending = aabb_scale_result.work_pending;
                state.cloth_aabb_refresh_frames = if aabb_scale_result.work_pending {
                    CLOTH_LOCAL_SCALE_AUDIT_INTERVAL_FRAMES
                } else {
                    0
                };
            } else {
                state.cloth_aabb_refresh_frames = CLOTH_LOCAL_SCALE_AUDIT_INTERVAL_FRAMES;
                state.cloth_aabb_work_pending = true;
            }
        } else {
            state.cloth_aabb_skipped_frames = state.cloth_aabb_skipped_frames.saturating_add(1);
        }
    } else {
        cloth_instance_aabb_scale::clear_cloth_instance_aabb_state(
            &mut state.cloth_instance_aabb_scale_state,
        );
        state.cloth_aabb_last_target = 0;
        state.cloth_aabb_last_scale_bits = 1.0f32.to_bits();
        state.cloth_aabb_last_generation = topology_generation;
        state.cloth_aabb_refresh_frames = 0;
        state.cloth_aabb_work_pending = false;
    }
    let aabb_scale_micros = aabb_scale_started.elapsed().as_micros();
    let aabb_queries = memory_query::query_count() - aabb_queries_before;

    let status_log_started = Instant::now();
    memory_query::scoped(|| {
        maybe_log_body_scale_port_state(player_addr, requested_scale, binding, state)
    });
    let status_log_micros = status_log_started.elapsed().as_micros();
    apply_visual_scale(player, applied_scale);

    if SCALE_COLLISION {
        apply_physics_scale(player, state, applied_scale);
    }

    if SCALE_HKNP_CAPSULE_SHAPE {
        havok_shape_scale::scale_player_hknp_capsule_shape(
            player,
            &mut state.hknp_capsule_baseline,
            applied_scale,
        );
    }

    if SCALE_AABB_IDENTITY_MATRIX_BUFFER {
        model_matrix_scale::scale_player_aabb_identity_matrices(
            player,
            &mut state.aabb_identity_matrix_state,
            applied_scale,
        );
    }

    if SCALE_RAGDOLL_BODY_CINFO_CAPSULES {
        ragdoll_shape_scale::scale_player_ragdoll_body_cinfo_capsules(
            player,
            &mut state.ragdoll_body_cinfo_shape_state,
            applied_scale,
        );
    }

    if SCALE_RAGDOLL_LIVE_MOTION_COMS {
        ragdoll_motion_scale::scale_player_ragdoll_motion_coms(
            player,
            &mut state.ragdoll_motion_scale_state,
            applied_scale,
        );
    }

    maybe_probe_havok(player, state, applied_scale);
    maybe_probe_hkx_collidables(player, state, applied_scale);
    maybe_probe_ragdoll_single(player, state, applied_scale);
    maybe_probe_ragdoll_live(player, state, applied_scale);

    let total_micros = frame_started.elapsed().as_micros();
    let periodic_perf_summary = state.task_frames.is_multiple_of(600);
    if local_refresh
        || aabb_refresh
        || total_micros >= PERF_SLOW_FRAME_THRESHOLD_MICROS
        || periodic_perf_summary
    {
        log::line(format_args!(
            "[PERF-ERPS-FRAME] requested={requested_scale:.3} ready={} total_us={total_micros} bind_us={bind_micros} local_scale_us={local_scale_micros} queue_us={queue_micros} aabb_scale_us={aabb_scale_micros} status_log_us={status_log_micros} binding_audit={binding_audit} queue_audit={queue_audit} local_refresh={local_refresh} local_work_pending={} transition_pending={transition_pending} refresh_calls={} skipped_frames={} aabb_refresh={aabb_refresh} aabb_work_pending={} aabb_refresh_calls={} aabb_skipped_frames={} topology_generation={topology_generation} fields_written={} aabb_fields_written={} collision_readback=false detailed_child_logging={ENABLE_DETAILED_CLOTH_CHILD_LOGGING} bind_queries={bind_queries} local_queries={local_queries} aabb_queries={aabb_queries}",
            binding.ready,
            state.cloth_local_work_pending,
            state.cloth_local_refresh_calls,
            state.cloth_local_skipped_frames,
            state.cloth_aabb_work_pending,
            state.cloth_aabb_refresh_calls,
            state.cloth_aabb_skipped_frames,
            local_scale_result.fields_written,
            aabb_scale_result.fields_written,
        ));
    }
}

fn should_refresh_cloth_local_scale(
    previous: (usize, u32, u64),
    frames_since_refresh: u32,
    work_pending: bool,
    current: (usize, u32, u64),
) -> bool {
    work_pending
        || current != previous
        || frames_since_refresh >= CLOTH_LOCAL_SCALE_AUDIT_INTERVAL_FRAMES
}

fn should_refresh_cloth_instance_aabbs(
    local_work_pending: bool,
    transition_pending: bool,
    previous: (usize, u32, u64),
    frames_since_refresh: u32,
    work_pending: bool,
    current: (usize, u32, u64),
) -> bool {
    !local_work_pending
        && !transition_pending
        && should_refresh_cloth_local_scale(previous, frames_since_refresh, work_pending, current)
}

fn should_queue_cloth_scale_transitions(
    previous: (usize, u32, u64),
    current: (usize, u32, u64),
    periodic_audit: bool,
) -> bool {
    current.0 != 0 && (periodic_audit || previous != current)
}

fn should_audit_body_scale_binding(cached_ready: bool, frames_since_audit: u32) -> bool {
    !cached_ready || frames_since_audit >= BODY_SCALE_BINDING_AUDIT_INTERVAL_FRAMES
}

fn active_player_scale(chr: &ChrIns) -> f32 {
    let scale = first_matching_scale(|sp_effect| {
        chr.special_effect
            .entries()
            .any(|entry| entry.param_id == sp_effect)
    });

    clamp_scale(scale)
}

fn first_matching_scale(mut is_active: impl FnMut(i32) -> bool) -> f32 {
    SCALE_EFFECTS
        .iter()
        .find(|effect| is_active(effect.sp_effect))
        .map(|effect| effect.scale)
        .unwrap_or(1.0)
}

fn maybe_log_body_scale_port_state(
    player_addr: usize,
    requested_scale: f32,
    binding: body_scale_port::TargetBinding,
    state: &mut ScaleState,
) {
    let counters = body_scale_port::counters();
    state.port_status_frames = state.port_status_frames.wrapping_add(1);
    let changed = binding.ready != state.last_port_ready
        || binding.pose_importer != state.last_port_pose_importer
        || binding.cloth_pose_importer != state.last_port_cloth_pose_importer
        || binding.cloth_pose_source != state.last_port_cloth_pose_source
        || binding.anim_skeleton_modifier != state.last_port_anim_skeleton
        || (requested_scale - state.last_port_requested_scale).abs() > f32::EPSILON
        || binding.reason != state.last_port_reason;
    let requested_is_unit = (requested_scale - 1.0).abs() <= f32::EPSILON;
    let previous_was_non_unit = (state.last_port_requested_scale - 1.0).abs() > f32::EPSILON;
    if changed && requested_is_unit && previous_was_non_unit {
        state.cloth_probe_unit_tail_frames = 600;
    } else if !requested_is_unit {
        state.cloth_probe_unit_tail_frames = 0;
    }
    let periodic_non_unit_status =
        !requested_is_unit && state.port_status_frames.is_multiple_of(120);
    let periodic_unit_tail_status = requested_is_unit
        && state.cloth_probe_unit_tail_frames > 0
        && state.port_status_frames.is_multiple_of(120);
    if state.cloth_probe_unit_tail_frames > 0 {
        state.cloth_probe_unit_tail_frames -= 1;
    }
    let periodic_status = periodic_non_unit_status || periodic_unit_tail_status;
    if changed || periodic_status {
        maybe_log_matrix_candidates(binding.anim_skeleton_modifier, state);
    }
    if !changed && !periodic_status {
        return;
    }

    log::line(format_args!(
        "[player-scale-no-bone] ER body-scale binding player=0x{player_addr:X} ready={} reason={} requested={requested_scale:.3} pose_importer=0x{:X} cloth_pose_importer=0x{:X} cloth_pose_source={} cloth_pose_distinct={} anim_skeleton=0x{:X} counters=pose_calls:{}/pose_writes:{} cloth_pose_calls:{}/cloth_pose_writes:{} affine_single_calls:{}/affine_single_writes:{} affine_range_calls:{}/affine_range_writes:{} single_calls:{}/single_writes:{} range_calls:{}/range_writes:{} rejected:{}",
        binding.ready,
        binding.reason,
        binding.pose_importer,
        binding.cloth_pose_importer,
        binding.cloth_pose_source.name(),
        binding.cloth_pose_importer != binding.pose_importer,
        binding.anim_skeleton_modifier,
        counters.pose_target_calls,
        counters.pose_transforms_written,
        counters.cloth_pose_target_calls,
        counters.cloth_pose_transforms_written,
        counters.affine_single_target_calls,
        counters.affine_single_matrices_written,
        counters.affine_range_target_calls,
        counters.affine_range_matrices_written,
        counters.single_target_calls,
        counters.single_matrices_written,
        counters.range_target_calls,
        counters.range_matrices_written,
        counters.rejected_outputs,
    ));
    log::line(format_args!(
        "[DEBUG-ERPS-POSE-GATE] player=0x{player_addr:X} requested={requested_scale:.3} materialized=00:{}/01:{}/10:{}/11:{}/other:{} cached_applied={:.3} cloth_cached_applied={:.3}",
        counters.pose_gate_00,
        counters.pose_gate_01,
        counters.pose_gate_10,
        counters.pose_gate_11,
        counters.pose_gate_other,
        f32::from_bits(counters.pose_applied_scale_bits),
        f32::from_bits(counters.cloth_pose_applied_scale_bits),
    ));
    log::line(format_args!(
        "[DEBUG-ERPS-AFFINE-RANGE] player=0x{player_addr:X} requested={requested_scale:.3} fallback_items={} provider_successes={} provider_identity_outputs={} provider_failures={} matrices_written={} rejected_outputs={}",
        counters.affine_range_target_calls,
        counters.affine_range_provider_successes,
        counters.affine_range_provider_identities,
        counters.affine_range_provider_failures,
        counters.affine_range_matrices_written,
        counters.rejected_outputs,
    ));
    let cloth_counters = body_scale_port::cloth_instance_probe_counters();
    let mesh_frames = body_scale_port::cloth_mesh_frame_counters();
    let skin_normals = body_scale_port::cloth_skin_normal_counters();
    log::line(format_args!(
        "[ERPS-CLOTH-SKIN-NORMAL] requested={requested_scale:.3} corrected={} normal_rows={} own_max_us={}",
        skin_normals[0], skin_normals[1], skin_normals[2]
    ));
    log::line(format_args!(
        "[ERPS-CLOTH-MESH-FRAME] requested={requested_scale:.3} seen={} corrected={} frames={} own_max_us={} own_max_queries={} normal_rows={} pn_own_max_us={} area_calls={} area_frames={}",
        mesh_frames[0],
        mesh_frames[1],
        mesh_frames[2],
        mesh_frames[3],
        mesh_frames[4],
        mesh_frames[5],
        mesh_frames[6],
        mesh_frames[7],
        mesh_frames[8],
    ));
    log::line(format_args!(
        "[ERPS-CLOTH-RENDER] requested={requested_scale:.3} calls={} rows={} rejected={} own_max_us={} own_max_queries={}",
        counters.render_calls,
        counters.render_rows,
        counters.render_rejected,
        counters.render_max_us,
        counters.render_max_queries,
    ));
    log::line(format_args!(
        "[DEBUG-ERPS-CLOTH-TRANSITION] target_input=0x{:X} requested={requested_scale:.3} pending={} queued={} deferred={} rejected={} commits={} wrapper_deferred={} wrapper_rejected={} secondary_adjusted={} secondary_passthrough={} secondary_dirty_marked={} secondary_dirty_rejected={}",
        binding.cloth_pose_importer,
        body_scale_port::has_pending_cloth_scale_transitions(),
        cloth_counters.scale_transitions_queued,
        cloth_counters.scale_transitions_deferred,
        cloth_counters.scale_transitions_rejected,
        cloth_counters.scale_reference_commits,
        cloth_counters.scale_wrapper_deferred,
        cloth_counters.scale_wrapper_rejected,
        cloth_counters.secondary_reference_adjusted,
        cloth_counters.secondary_reference_passthrough,
        cloth_counters.secondary_dirty_marked,
        cloth_counters.secondary_dirty_rejected,
    ));
    // The bounded child/AABB walk is diagnostic-only and was the largest
    // contributor to the transition-frame status spike. Keep it available on
    // a periodic audit, but do not put it back on the scale-change hot path.
    if periodic_status {
        let profile_collision_aabbs = state.port_status_frames.is_multiple_of(360);
        maybe_log_cloth_instances(
            binding.cloth_pose_importer,
            requested_scale,
            profile_collision_aabbs,
        );
    }
    state.last_port_ready = binding.ready;
    state.last_port_pose_importer = binding.pose_importer;
    state.last_port_cloth_pose_importer = binding.cloth_pose_importer;
    state.last_port_cloth_pose_source = binding.cloth_pose_source;
    state.last_port_anim_skeleton = binding.anim_skeleton_modifier;
    state.last_port_requested_scale = requested_scale;
    state.last_port_reason = binding.reason;
    if changed {
        state.port_status_frames = 0;
    }
}

fn maybe_log_cloth_instances(
    target_input: usize,
    requested_scale: f32,
    profile_collision_aabbs: bool,
) {
    let counters = body_scale_port::cloth_instance_probe_counters();
    let snapshots = body_scale_port::cloth_instance_snapshots(target_input);
    let matching_instances = snapshots.iter().filter(|snapshot| snapshot.valid).count();
    log::line(format_args!(
        "[DEBUG-ERPS-CLOTH-INSTANCE-SUMMARY] target_input=0x{target_input:X} requested={requested_scale:.3} setter_calls={} slot_inserts={} slot_replacements={} occupied_slots={} matching_instances={matching_instances} scale_transitions_queued={} scale_transitions_deferred={} scale_transitions_rejected={} scale_reference_commits={} scale_wrapper_deferred={} scale_wrapper_rejected={} secondary_reference_calls={} secondary_reference_adjusted={} secondary_reference_passthrough={} secondary_reference_rejected={} secondary_probe_calls={} secondary_source_owner_e0_matches={} secondary_source_owner_e0_mismatches={} secondary_pre_core_requested={} secondary_pre_core_unit={} secondary_pre_core_other={} secondary_post_core_requested={} secondary_post_core_unit={} secondary_post_core_other={} secondary_post_copy_matches={} secondary_post_copy_mismatches={} secondary_core_changed={} secondary_dirty_marked={} secondary_dirty_rejected={} solver_source_bracket_calls={} solver_source_bracket_transforms={} solver_source_bracket_restores={} solver_source_bracket_rejected={} solver_source_direct_transforms={} solver_source_lazy_candidates={} solver_source_lazy_resolved={} solver_source_lazy_rejected={} solver_source_local_scale_calls={} solver_source_local_scale_transforms={} solver_source_local_scale_restores={} solver_source_local_scale_rejected={} solver_source_local_scale_passthrough={} immediate_probe_transitions={} immediate_children_observed={} immediate_particle_matches={} immediate_transform_matches={} immediate_both_matches={} immediate_unreadable={} transform_resync_calls={} transform_resync_children_observed={} transform_resync_children_changed={} transform_resync_entries_observed={} transform_resync_entries_changed={} transform_resync_entries_already_scaled={} transform_resync_entries_rejected={} transform_resync_topology_rejected={} attachment_position_buffers_shifted={} attachment_particles_shifted={} attachment_aabbs_shifted={} attachment_position_rejected={}",
        counters.setter_calls,
        counters.slot_inserts,
        counters.slot_replacements,
        counters.occupied_slots,
        counters.scale_transitions_queued,
        counters.scale_transitions_deferred,
        counters.scale_transitions_rejected,
        counters.scale_reference_commits,
        counters.scale_wrapper_deferred,
        counters.scale_wrapper_rejected,
        counters.secondary_reference_calls,
        counters.secondary_reference_adjusted,
        counters.secondary_reference_passthrough,
        counters.secondary_reference_rejected,
        counters.secondary_probe_calls,
        counters.secondary_source_owner_e0_matches,
        counters.secondary_source_owner_e0_mismatches,
        counters.secondary_pre_core_requested,
        counters.secondary_pre_core_unit,
        counters.secondary_pre_core_other,
        counters.secondary_post_core_requested,
        counters.secondary_post_core_unit,
        counters.secondary_post_core_other,
        counters.secondary_post_copy_matches,
        counters.secondary_post_copy_mismatches,
        counters.secondary_core_changed,
        counters.secondary_dirty_marked,
        counters.secondary_dirty_rejected,
        counters.solver_source_bracket_calls,
        counters.solver_source_bracket_transforms,
        counters.solver_source_bracket_restores,
        counters.solver_source_bracket_rejected,
        counters.solver_source_direct_transforms,
        counters.solver_source_lazy_candidates,
        counters.solver_source_lazy_resolved,
        counters.solver_source_lazy_rejected,
        counters.solver_source_local_scale_calls,
        counters.solver_source_local_scale_transforms,
        counters.solver_source_local_scale_restores,
        counters.solver_source_local_scale_rejected,
        counters.solver_source_local_scale_passthrough,
        counters.immediate_probe_transitions,
        counters.immediate_children_observed,
        counters.immediate_particle_matches,
        counters.immediate_transform_matches,
        counters.immediate_both_matches,
        counters.immediate_unreadable,
        counters.transform_resync_calls,
        counters.transform_resync_children_observed,
        counters.transform_resync_children_changed,
        counters.transform_resync_entries_observed,
        counters.transform_resync_entries_changed,
        counters.transform_resync_entries_already_scaled,
        counters.transform_resync_entries_rejected,
        counters.transform_resync_topology_rejected,
        counters.attachment_position_buffers_shifted,
        counters.attachment_particles_shifted,
        counters.attachment_aabbs_shifted,
        counters.attachment_position_rejected,
    ));

    log::line(format_args!(
        "[ERPS-CLOTH-PRIVATE-POSE] requested={requested_scale:.3} calls={} returns={} source_storage=private local_scale=passthrough",
        counters.solver_private_context_calls, counters.solver_private_context_returns,
    ));

    for snapshot in snapshots.iter().filter(|snapshot| snapshot.valid) {
        let matrix_60 = snapshot.matrix_60;
        let matrix_e0 = snapshot.matrix_e0;
        let core_matrix_90 = snapshot.core_matrix_90;
        let core_matrix_d0 = snapshot.core_matrix_d0;
        let core_matrix_50 = snapshot.core_matrix_50;
        log::line(format_args!(
            "[ERPS-CLOTH-OWNER] requested={requested_scale:.3} slot={} owner=0x{:X} input=0x{:X} equipment_owned={} source_calls={} reference_commits={} applied_scale={:.3} pending_scale={:.3}",
            snapshot.slot,
            snapshot.owner,
            snapshot.input,
            snapshot.equipment_owned,
            snapshot.source_calls,
            snapshot.reference_commits,
            f32::from_bits(snapshot.applied_scale_bits),
            f32::from_bits(snapshot.pending_scale_bits),
        ));
        let (core_groups_readable, core_group_count) = snapshot
            .core_group_count
            .map_or((false, 0), |count| (true, count));
        let (downstream_entries_readable, downstream_entry_count) = snapshot
            .downstream_entry_count
            .map_or((false, 0), |count| (true, count));
        log::line(format_args!(
            "[DEBUG-ERPS-CLOTH-INSTANCE] requested={requested_scale:.3} slot={} owner=0x{:X} input=0x{:X} setter_hits={} applied_scale={:.3} pending_scale={:.3} inner=0x{:X} inner_vtable=0x{:X} inner_vtable_rva=0x{:X} outer_flags=0x{:08X} outer_flag4a={} inner_flag49={} inner_flag61={} inner_flag62={} core=0x{:X} downstream_state=0x{:X} core_flag4c={} core_flag4d={} core_groups_readable={core_groups_readable} core_group_count={core_group_count} downstream_entries_readable={downstream_entries_readable} downstream_entry_count={downstream_entry_count} core50_readable={} core50_basis={:.4},{:.4},{:.4} core50_translation={:.4},{:.4},{:.4} core90_readable={} core90_basis={:.4},{:.4},{:.4} core90_translation={:.4},{:.4},{:.4} cored0_readable={} cored0_basis={:.4},{:.4},{:.4} cored0_translation={:.4},{:.4},{:.4} matrix60_readable={} matrix60_basis={:.4},{:.4},{:.4} matrix60_translation={:.4},{:.4},{:.4} matrixe0_readable={} matrixe0_basis={:.4},{:.4},{:.4} matrixe0_translation={:.4},{:.4},{:.4}",
            snapshot.slot,
            snapshot.owner,
            snapshot.input,
            snapshot.setter_hits,
            f32::from_bits(snapshot.applied_scale_bits),
            f32::from_bits(snapshot.pending_scale_bits),
            snapshot.inner,
            snapshot.inner_vtable,
            body_scale_port::module_rva(snapshot.inner_vtable),
            snapshot.outer_flags,
            snapshot.outer_flag_4a,
            snapshot.inner_flag_49,
            snapshot.inner_flag_61,
            snapshot.inner_flag_62,
            snapshot.core,
            snapshot.downstream_state,
            snapshot.core_flag_4c,
            snapshot.core_flag_4d,
            core_matrix_50.readable,
            core_matrix_50.basis_x,
            core_matrix_50.basis_y,
            core_matrix_50.basis_z,
            core_matrix_50.translation_x,
            core_matrix_50.translation_y,
            core_matrix_50.translation_z,
            core_matrix_90.readable,
            core_matrix_90.basis_x,
            core_matrix_90.basis_y,
            core_matrix_90.basis_z,
            core_matrix_90.translation_x,
            core_matrix_90.translation_y,
            core_matrix_90.translation_z,
            core_matrix_d0.readable,
            core_matrix_d0.basis_x,
            core_matrix_d0.basis_y,
            core_matrix_d0.basis_z,
            core_matrix_d0.translation_x,
            core_matrix_d0.translation_y,
            core_matrix_d0.translation_z,
            matrix_60.readable,
            matrix_60.basis_x,
            matrix_60.basis_y,
            matrix_60.basis_z,
            matrix_60.translation_x,
            matrix_60.translation_y,
            matrix_60.translation_z,
            matrix_e0.readable,
            matrix_e0.basis_x,
            matrix_e0.basis_y,
            matrix_e0.basis_z,
            matrix_e0.translation_x,
            matrix_e0.translation_y,
            matrix_e0.translation_z,
        ));
        let solver = body_scale_port::cloth_solver_input_snapshot(snapshot.core);
        let solver_translation = solver.translation_bounds;
        log::line(format_args!(
            "[DEBUG-ERPS-CLOTH-SOLVER-OUTPUT] requested={requested_scale:.3} slot={} owner=0x{:X} core=0x{:X} readable={} entry_count={} transform_count={} scale_min={:.4},{:.4},{:.4} scale_max={:.4},{:.4},{:.4} translation_center={:.4},{:.4},{:.4} translation_span={:.4},{:.4},{:.4}",
            snapshot.slot,
            snapshot.owner,
            snapshot.core,
            solver.readable,
            solver.entry_count,
            solver.transform_count,
            solver.scale_min_x,
            solver.scale_min_y,
            solver.scale_min_z,
            solver.scale_max_x,
            solver.scale_max_y,
            solver.scale_max_z,
            (solver_translation.min_x + solver_translation.max_x) * 0.5,
            (solver_translation.min_y + solver_translation.max_y) * 0.5,
            (solver_translation.min_z + solver_translation.max_z) * 0.5,
            solver_translation.span_x(),
            solver_translation.span_y(),
            solver_translation.span_z(),
        ));
        if profile_collision_aabbs {
            log_cloth_collision_aabbs(snapshot, requested_scale);
        }
        if ENABLE_DETAILED_CLOTH_CHILD_LOGGING {
            log_cloth_children(snapshot, requested_scale);
        }
    }
}

fn log_cloth_collision_aabbs(
    instance: &body_scale_port::ClothInstanceSnapshot,
    requested_scale: f32,
) {
    let (summary, children) = body_scale_port::cloth_child_snapshots(instance.core);
    log::line(format_args!(
        "[DEBUG-ERPS-CLOTH-COLLISION-AABB-SUMMARY] requested={requested_scale:.3} slot={} owner=0x{:X} core=0x{:X} topology_readable={} child_count={} captured_children={} truncated={}",
        instance.slot,
        instance.owner,
        instance.core,
        summary.topology_readable,
        summary.child_count,
        summary.captured_children,
        summary.truncated,
    ));

    for child in children.iter().filter(|child| child.valid) {
        let profile = body_scale_port::cloth_local_simulation_profile(child.child, child.sim_data);
        let current = child.current_positions;
        let transforms = child.transform_entries;
        let particles_aabb = profile.particles_aabb;
        let collision_aabb = profile.collision_particles_aabb;
        let landscape_aabb = profile.landscape_collision_particles_aabb;
        let current_center = cloth_bounds_center(current);
        let transform_center = cloth_bounds_center(transforms.translation_bounds);
        let particles_aabb_center = cloth_bounds_center(particles_aabb);
        let collision_aabb_center = cloth_bounds_center(collision_aabb);
        let landscape_aabb_center = cloth_bounds_center(landscape_aabb);
        let root = [
            instance.matrix_60.translation_x,
            instance.matrix_60.translation_y,
            instance.matrix_60.translation_z,
        ];
        let current_root_distance = cloth_point_distance(current_center, root);
        log::line(format_args!(
            "[DEBUG-ERPS-CLOTH-COLLISION-AABB] requested={requested_scale:.3} slot={} owner=0x{:X} group_index={} child_index={} group_root=0x{:X} child=0x{:X} sim_data=0x{:X} particle_count={} instance_root={:.4},{:.4},{:.4} current_root_distance={current_root_distance:.4} current_positions_readable={} current_position_center={:.4},{:.4},{:.4} current_position_span={:.4},{:.4},{:.4} transform_readable={} transform_basis_min={:.4},{:.4},{:.4} transform_basis_max={:.4},{:.4},{:.4} transform_translation_center={:.4},{:.4},{:.4} particles_aabb_readable={} particles_aabb_center={:.4},{:.4},{:.4} particles_aabb_span={:.4},{:.4},{:.4} collision_particles_aabb_readable={} collision_particles_aabb_center={:.4},{:.4},{:.4} collision_particles_aabb_span={:.4},{:.4},{:.4} landscape_collision_particles_aabb_readable={} landscape_collision_particles_aabb_center={:.4},{:.4},{:.4} landscape_collision_particles_aabb_span={:.4},{:.4},{:.4}",
            instance.slot,
            instance.owner,
            child.group_index,
            child.child_index,
            child.group_root,
            child.child,
            child.sim_data,
            child.particle_count,
            root[0],
            root[1],
            root[2],
            current.readable,
            current_center[0],
            current_center[1],
            current_center[2],
            current.span_x(),
            current.span_y(),
            current.span_z(),
            transforms.readable,
            transforms.basis_min_x,
            transforms.basis_min_y,
            transforms.basis_min_z,
            transforms.basis_max_x,
            transforms.basis_max_y,
            transforms.basis_max_z,
            transform_center[0],
            transform_center[1],
            transform_center[2],
            particles_aabb.readable,
            particles_aabb_center[0],
            particles_aabb_center[1],
            particles_aabb_center[2],
            particles_aabb.span_x(),
            particles_aabb.span_y(),
            particles_aabb.span_z(),
            collision_aabb.readable,
            collision_aabb_center[0],
            collision_aabb_center[1],
            collision_aabb_center[2],
            collision_aabb.span_x(),
            collision_aabb.span_y(),
            collision_aabb.span_z(),
            landscape_aabb.readable,
            landscape_aabb_center[0],
            landscape_aabb_center[1],
            landscape_aabb_center[2],
            landscape_aabb.span_x(),
            landscape_aabb.span_y(),
            landscape_aabb.span_z(),
        ));
    }
}

fn cloth_bounds_center(bounds: body_scale_port::ClothPositionBounds) -> [f32; 3] {
    if !bounds.readable || bounds.count == 0 {
        return [f32::NAN; 3];
    }
    [
        (bounds.min_x + bounds.max_x) * 0.5,
        (bounds.min_y + bounds.max_y) * 0.5,
        (bounds.min_z + bounds.max_z) * 0.5,
    ]
}

fn cloth_point_distance(left: [f32; 3], right: [f32; 3]) -> f32 {
    left.into_iter()
        .zip(right)
        .map(|(left, right)| (left - right) * (left - right))
        .sum::<f32>()
        .sqrt()
}

fn log_cloth_children(instance: &body_scale_port::ClothInstanceSnapshot, requested_scale: f32) {
    let (summary, children) = body_scale_port::cloth_child_snapshots(instance.core);
    let mut profiled_sim_data = [0usize; body_scale_port::CLOTH_CHILD_SLOTS];
    let mut profiled_sim_data_count = 0usize;
    log::line(format_args!(
        "[DEBUG-ERPS-CLOTH-CHILD-SUMMARY] requested={requested_scale:.3} slot={} owner=0x{:X} core=0x{:X} topology_readable={} group_count={} child_count={} captured_children={} truncated={}",
        instance.slot,
        instance.owner,
        instance.core,
        summary.topology_readable,
        summary.group_count,
        summary.child_count,
        summary.captured_children,
        summary.truncated,
    ));

    for child in children.iter().filter(|child| child.valid) {
        let current = child.current_positions;
        let previous = child.previous_positions;
        let transforms = child.transform_entries;
        let transform_translation = transforms.translation_bounds;
        log::line(format_args!(
            "[DEBUG-ERPS-CLOTH-CHILD] requested={requested_scale:.3} slot={} owner=0x{:X} group_index={} child_index={} group_holder=0x{:X} group_root=0x{:X} child=0x{:X} child_vtable=0x{:X} child_vtable_rva=0x{:X} sim_data=0x{:X} particle_count={} current_readable={} current_count={} current_center={:.4},{:.4},{:.4} current_span={:.4},{:.4},{:.4} previous_readable={} previous_count={} previous_center={:.4},{:.4},{:.4} previous_span={:.4},{:.4},{:.4} transform_readable={} transform_count={} transform_basis_min={:.4},{:.4},{:.4} transform_basis_max={:.4},{:.4},{:.4} transform_translation_center={:.4},{:.4},{:.4} transform_translation_span={:.4},{:.4},{:.4}",
            instance.slot,
            instance.owner,
            child.group_index,
            child.child_index,
            child.group_holder,
            child.group_root,
            child.child,
            child.child_vtable,
            body_scale_port::module_rva(child.child_vtable),
            child.sim_data,
            child.particle_count,
            current.readable,
            current.count,
            (current.min_x + current.max_x) * 0.5,
            (current.min_y + current.max_y) * 0.5,
            (current.min_z + current.max_z) * 0.5,
            current.span_x(),
            current.span_y(),
            current.span_z(),
            previous.readable,
            previous.count,
            (previous.min_x + previous.max_x) * 0.5,
            (previous.min_y + previous.max_y) * 0.5,
            (previous.min_z + previous.max_z) * 0.5,
            previous.span_x(),
            previous.span_y(),
            previous.span_z(),
            transforms.readable,
            transforms.count,
            transforms.basis_min_x,
            transforms.basis_min_y,
            transforms.basis_min_z,
            transforms.basis_max_x,
            transforms.basis_max_y,
            transforms.basis_max_z,
            (transform_translation.min_x + transform_translation.max_x) * 0.5,
            (transform_translation.min_y + transform_translation.max_y) * 0.5,
            (transform_translation.min_z + transform_translation.max_z) * 0.5,
            transform_translation.span_x(),
            transform_translation.span_y(),
            transform_translation.span_z(),
        ));
        let profile_constraints =
            !profiled_sim_data[..profiled_sim_data_count].contains(&child.sim_data);
        if profile_constraints && profiled_sim_data_count < profiled_sim_data.len() {
            profiled_sim_data[profiled_sim_data_count] = child.sim_data;
            profiled_sim_data_count += 1;
        }
        log_cloth_local_simulation(instance, child, requested_scale, profile_constraints);
    }
}

fn log_cloth_local_simulation(
    instance: &body_scale_port::ClothInstanceSnapshot,
    child: &body_scale_port::ClothChildSnapshot,
    requested_scale: f32,
    profile_constraints: bool,
) {
    let profile = body_scale_port::cloth_local_simulation_profile(child.child, child.sim_data);
    let radii = profile.particle_radii;
    let pose = profile.first_pose_positions;
    let particles_aabb = profile.particles_aabb;
    let collision_particles_aabb = profile.collision_particles_aabb;
    let landscape_collision_particles_aabb = profile.landscape_collision_particles_aabb;
    log::line(format_args!(
        "[DEBUG-ERPS-CLOTH-LOCAL] requested={requested_scale:.3} slot={} owner=0x{:X} child=0x{:X} sim_data=0x{:X} profile_readable={} particle_radii_readable={} particle_radius_count={} particle_radius_min={:.6} particle_radius_max={:.6} max_particle_radius_readable={} max_particle_radius={:.6} sim_pose_count_readable={} sim_pose_count={} first_pose_readable={} first_pose_count={} first_pose_span={:.4},{:.4},{:.4} static_constraint_count_readable={} static_constraint_count={} anti_pinch_constraint_count_readable={} anti_pinch_constraint_count={} per_instance_collidable_count_readable={} per_instance_collidable_count={} particles_aabb_readable={} particles_aabb_span={:.4},{:.4},{:.4} collision_particles_aabb_readable={} collision_particles_aabb_span={:.4},{:.4},{:.4} landscape_collision_particles_aabb_readable={} landscape_collision_particles_aabb_span={:.4},{:.4},{:.4}",
        instance.slot,
        instance.owner,
        child.child,
        child.sim_data,
        profile.readable,
        radii.readable,
        radii.count,
        radii.minimum,
        radii.maximum,
        profile.max_particle_radius_readable,
        profile.max_particle_radius,
        profile.sim_pose_count_readable,
        profile.sim_pose_count,
        pose.readable,
        pose.count,
        pose.span_x(),
        pose.span_y(),
        pose.span_z(),
        profile.static_constraint_count_readable,
        profile.static_constraint_count,
        profile.anti_pinch_constraint_count_readable,
        profile.anti_pinch_constraint_count,
        profile.per_instance_collidable_count_readable,
        profile.per_instance_collidable_count,
        particles_aabb.readable,
        particles_aabb.span_x(),
        particles_aabb.span_y(),
        particles_aabb.span_z(),
        collision_particles_aabb.readable,
        collision_particles_aabb.span_x(),
        collision_particles_aabb.span_y(),
        collision_particles_aabb.span_z(),
        landscape_collision_particles_aabb.readable,
        landscape_collision_particles_aabb.span_x(),
        landscape_collision_particles_aabb.span_y(),
        landscape_collision_particles_aabb.span_z(),
    ));

    if !profile_constraints {
        let captured_count = profile
            .static_constraint_count
            .saturating_add(profile.anti_pinch_constraint_count);
        log::line(format_args!(
            "[DEBUG-ERPS-CLOTH-CONSTRAINT-SUMMARY] requested={requested_scale:.3} slot={} owner=0x{:X} child=0x{:X} sim_data=0x{:X} topology_readable={} static_count={} anti_pinch_count={} captured_count={} truncated=false deduplicated=true",
            instance.slot,
            instance.owner,
            child.child,
            child.sim_data,
            profile.static_constraint_count_readable
                && profile.anti_pinch_constraint_count_readable,
            profile.static_constraint_count,
            profile.anti_pinch_constraint_count,
            captured_count,
        ));
        return;
    }

    let (summary, constraints) = body_scale_port::cloth_constraint_set_snapshots(child.sim_data);
    log::line(format_args!(
        "[DEBUG-ERPS-CLOTH-CONSTRAINT-SUMMARY] requested={requested_scale:.3} slot={} owner=0x{:X} child=0x{:X} sim_data=0x{:X} topology_readable={} static_count={} anti_pinch_count={} captured_count={} truncated={} deduplicated=false",
        instance.slot,
        instance.owner,
        child.child,
        child.sim_data,
        summary.topology_readable,
        summary.static_count,
        summary.anti_pinch_count,
        summary.captured_count,
        summary.truncated,
    ));
    for constraint in constraints.iter().filter(|constraint| constraint.valid) {
        let primary = constraint.primary_dimensions;
        let secondary = constraint.secondary_dimensions;
        let tertiary = constraint.tertiary_dimensions;
        log::line(format_args!(
            "[DEBUG-ERPS-CLOTH-CONSTRAINT] requested={requested_scale:.3} slot={} owner=0x{:X} child=0x{:X} sim_data=0x{:X} array={} array_index={} set=0x{:X} vtable=0x{:X} vtable_rva=0x{:X} kind={} constraint_id={} constraint_type={} elements_readable={} element_count={} primary_readable={} primary_count={} primary_min={:.6} primary_max={:.6} secondary_readable={} secondary_count={} secondary_min={:.6} secondary_max={:.6} tertiary_readable={} tertiary_count={} tertiary_min={:.6} tertiary_max={:.6} set_dimension_readable={} set_dimension={:.6}",
            instance.slot,
            instance.owner,
            child.child,
            child.sim_data,
            if constraint.anti_pinch_array {
                "anti-pinch"
            } else {
                "static"
            },
            constraint.array_index,
            constraint.set,
            constraint.vtable,
            body_scale_port::module_rva(constraint.vtable),
            constraint.kind.name(),
            constraint.constraint_id,
            constraint.constraint_type,
            constraint.elements_readable,
            constraint.element_count,
            primary.readable,
            primary.count,
            primary.minimum,
            primary.maximum,
            secondary.readable,
            secondary.count,
            secondary.minimum,
            secondary.maximum,
            tertiary.readable,
            tertiary.count,
            tertiary.minimum,
            tertiary.maximum,
            constraint.set_dimension_readable,
            constraint.set_dimension,
        ));
    }
}

fn maybe_log_matrix_candidates(target_anim_skeleton: usize, state: &mut ScaleState) {
    for candidate in body_scale_port::matrix_candidates() {
        if candidate.hits == 0 || candidate.hits == state.matrix_candidate_hits[candidate.slot] {
            continue;
        }
        log::line(format_args!(
            "[DEBUG-ERPS-MATRIX-CANDIDATE] slot={} kind={} caller_rva=0x{:X} target=0x{target_anim_skeleton:X} first_this=0x{:X} last_this=0x{:X} output=0x{:X} arg8={} arg9={} q48=0x{:X} q68=0x{:X} q88=0x{:X} hits={}",
            candidate.slot,
            body_scale_port::matrix_candidate_kind_name(candidate.kind),
            candidate.caller_rva,
            candidate.first_this,
            candidate.last_this,
            candidate.output,
            candidate.arg8,
            candidate.arg9,
            candidate.qword_48,
            candidate.qword_68,
            candidate.qword_88,
            candidate.hits,
        ));
        state.matrix_candidate_hits[candidate.slot] = candidate.hits;
    }
}

fn clamp_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(SCALE_MIN, SCALE_MAX)
    } else {
        1.0
    }
}

fn apply_visual_scale(player: &mut PlayerIns, scale: f32) {
    let chr_ctrl = player.chr_ins.chr_ctrl.as_mut();
    chr_ctrl.scale_size_x = scale;
    chr_ctrl.scale_size_y = scale;
    chr_ctrl.scale_size_z = scale;
}

fn apply_physics_scale(player: &mut PlayerIns, state: &mut ScaleState, scale: f32) {
    let physics = player.chr_ins.modules.as_mut().physics.as_mut();
    let baseline = *state
        .baseline
        .get_or_insert_with(|| PhysicsBaseline::capture(physics));

    physics.chr_hit_height = baseline.chr_hit_height * scale;
    physics.chr_hit_radius = baseline.chr_hit_radius * scale;
    physics.hit_height = baseline.hit_height * scale;
    physics.hit_radius = baseline.hit_radius * scale;

    physics.weight = if SCALE_WEIGHT {
        (baseline.weight * scale).max(1.0)
    } else {
        baseline.weight
    };
}

fn maybe_probe_havok(player: &mut PlayerIns, state: &mut ScaleState, scale: f32) {
    if !HAVOK_PROBE_ENABLED || (scale - 1.0).abs() < f32::EPSILON {
        return;
    }

    state.probe_frame = state.probe_frame.wrapping_add(1);
    let scale_changed = (scale - state.last_probe_scale).abs() > f32::EPSILON;
    let chr_model_addr = read_chr_model_addr(player);
    let model_ready_changed = chr_model_addr != 0 && chr_model_addr != state.last_probe_model_addr;
    let should_probe = scale_changed
        || state.probe_frame == 1
        || model_ready_changed
        || state
            .probe_frame
            .is_multiple_of(HAVOK_PROBE_INTERVAL_FRAMES);

    if !should_probe {
        return;
    }

    state.last_probe_scale = scale;
    state.last_probe_model_addr = chr_model_addr;
    havok_probe::probe_player_havok_roots(player, scale);
}

fn maybe_probe_hkx_collidables(player: &mut PlayerIns, state: &mut ScaleState, scale: f32) {
    if !HKX_COLLIDABLE_PROBE_ENABLED || (scale - 1.0).abs() < f32::EPSILON {
        return;
    }

    state.hkx_probe_frame = state.hkx_probe_frame.wrapping_add(1);
    let scale_changed = (scale - state.last_hkx_probe_scale).abs() > f32::EPSILON;
    let chr_model_addr = read_chr_model_addr(player);
    let (chr_collision_addr, ragdoll_addr) = read_chr_ctrl_hkx_roots(player);
    let model_ready_changed =
        chr_model_addr != 0 && chr_model_addr != state.last_hkx_probe_model_addr;
    let chr_collision_changed =
        chr_collision_addr != 0 && chr_collision_addr != state.last_hkx_probe_chr_collision_addr;
    let ragdoll_changed = ragdoll_addr != 0 && ragdoll_addr != state.last_hkx_probe_ragdoll_addr;
    let should_probe = scale_changed
        || state.hkx_probe_frame == 1
        || model_ready_changed
        || chr_collision_changed
        || ragdoll_changed
        || state
            .hkx_probe_frame
            .is_multiple_of(HKX_COLLIDABLE_PROBE_INTERVAL_FRAMES);

    if !should_probe {
        return;
    }

    state.last_hkx_probe_scale = scale;
    state.last_hkx_probe_model_addr = chr_model_addr;
    state.last_hkx_probe_chr_collision_addr = chr_collision_addr;
    state.last_hkx_probe_ragdoll_addr = ragdoll_addr;
    hkx_collidable_probe::probe_player_hkx_collidables(player, scale);
}

fn maybe_probe_ragdoll_single(player: &mut PlayerIns, state: &mut ScaleState, scale: f32) {
    if !RAGDOLL_SINGLE_PROBE_ENABLED {
        return;
    }

    let player_addr = player as *mut PlayerIns as usize;
    let chr_model_addr = read_chr_model_addr(player);
    let (_, ragdoll_addr) = read_chr_ctrl_hkx_roots(player);
    state.ragdoll_single_status_frames = state.ragdoll_single_status_frames.wrapping_add(1);

    maybe_log_ragdoll_single_status(player_addr, chr_model_addr, ragdoll_addr, scale, state);

    if (scale - 1.0).abs() < f32::EPSILON {
        state.ragdoll_single_ready_frames = 0;
        return;
    }

    if chr_model_addr == 0 || ragdoll_addr == 0 {
        state.ragdoll_single_ready_frames = 0;
        return;
    }

    if state.ragdoll_single_done_player_addr == player_addr
        && state.ragdoll_single_done_ragdoll_addr == ragdoll_addr
    {
        return;
    }

    state.ragdoll_single_ready_frames = state.ragdoll_single_ready_frames.wrapping_add(1);
    if state.ragdoll_single_ready_frames < RAGDOLL_SINGLE_PROBE_READY_DELAY_FRAMES {
        return;
    }

    state.ragdoll_single_done_player_addr = player_addr;
    state.ragdoll_single_done_ragdoll_addr = ragdoll_addr;
    hkx_collidable_probe::probe_ragdoll_single_physics_system(player, scale);
}

fn maybe_probe_ragdoll_live(player: &mut PlayerIns, state: &mut ScaleState, scale: f32) {
    if !RAGDOLL_LIVE_PROBE_ENABLED {
        return;
    }

    let player_addr = player as *mut PlayerIns as usize;
    let chr_model_addr = read_chr_model_addr(player);
    let (_, ragdoll_addr) = read_chr_ctrl_hkx_roots(player);
    state.ragdoll_live_status_frames = state.ragdoll_live_status_frames.wrapping_add(1);

    maybe_log_ragdoll_live_status(player_addr, chr_model_addr, ragdoll_addr, scale, state);

    if (scale - 1.0).abs() < f32::EPSILON {
        state.ragdoll_live_ready_frames = 0;
        return;
    }

    if chr_model_addr == 0 || ragdoll_addr == 0 {
        state.ragdoll_live_ready_frames = 0;
        return;
    }

    if state.ragdoll_live_done_player_addr == player_addr
        && state.ragdoll_live_done_ragdoll_addr == ragdoll_addr
    {
        return;
    }

    state.ragdoll_live_ready_frames = state.ragdoll_live_ready_frames.wrapping_add(1);
    if state.ragdoll_live_ready_frames < RAGDOLL_LIVE_PROBE_READY_DELAY_FRAMES {
        return;
    }

    state.ragdoll_live_done_player_addr = player_addr;
    state.ragdoll_live_done_ragdoll_addr = ragdoll_addr;
    hkx_collidable_probe::probe_ragdoll_live_runtime_candidates(player, scale);
}

fn maybe_log_ragdoll_live_status(
    player_addr: usize,
    chr_model_addr: usize,
    ragdoll_addr: usize,
    scale: f32,
    state: &mut ScaleState,
) {
    let player_changed = player_addr != state.ragdoll_live_last_status_player_addr;
    let model_changed = chr_model_addr != state.ragdoll_live_last_status_model_addr;
    let ragdoll_changed = ragdoll_addr != state.ragdoll_live_last_status_ragdoll_addr;
    let scale_changed = (scale - state.ragdoll_live_last_status_scale).abs() > f32::EPSILON;
    let periodic = state
        .ragdoll_live_status_frames
        .is_multiple_of(RAGDOLL_LIVE_PROBE_STATUS_INTERVAL_FRAMES);

    if !(player_changed || model_changed || ragdoll_changed || scale_changed || periodic) {
        return;
    }

    state.ragdoll_live_last_status_player_addr = player_addr;
    state.ragdoll_live_last_status_model_addr = chr_model_addr;
    state.ragdoll_live_last_status_ragdoll_addr = ragdoll_addr;
    state.ragdoll_live_last_status_scale = scale;

    let scale_active = (scale - 1.0).abs() >= f32::EPSILON;
    let model_ready = chr_model_addr != 0;
    let ragdoll_ready = ragdoll_addr != 0;
    log::line(format_args!(
        "[player-scale-no-bone] ragdoll live status frame={} scale={scale:.3} scale_active={scale_active} player=0x{player_addr:x} chr_model=0x{chr_model_addr:x} model_ready={model_ready} ragdoll=0x{ragdoll_addr:x} ragdoll_ready={ragdoll_ready} ready_frames={} delay={RAGDOLL_LIVE_PROBE_READY_DELAY_FRAMES}",
        state.ragdoll_live_status_frames, state.ragdoll_live_ready_frames
    ));
}

fn maybe_log_ragdoll_single_status(
    player_addr: usize,
    chr_model_addr: usize,
    ragdoll_addr: usize,
    scale: f32,
    state: &mut ScaleState,
) {
    let player_changed = player_addr != state.ragdoll_single_last_status_player_addr;
    let model_changed = chr_model_addr != state.ragdoll_single_last_status_model_addr;
    let ragdoll_changed = ragdoll_addr != state.ragdoll_single_last_status_ragdoll_addr;
    let scale_changed = (scale - state.ragdoll_single_last_status_scale).abs() > f32::EPSILON;
    let periodic = state
        .ragdoll_single_status_frames
        .is_multiple_of(RAGDOLL_SINGLE_PROBE_STATUS_INTERVAL_FRAMES);

    if !(player_changed || model_changed || ragdoll_changed || scale_changed || periodic) {
        return;
    }

    state.ragdoll_single_last_status_player_addr = player_addr;
    state.ragdoll_single_last_status_model_addr = chr_model_addr;
    state.ragdoll_single_last_status_ragdoll_addr = ragdoll_addr;
    state.ragdoll_single_last_status_scale = scale;

    let scale_active = (scale - 1.0).abs() >= f32::EPSILON;
    let model_ready = chr_model_addr != 0;
    let ragdoll_ready = ragdoll_addr != 0;
    log::line(format_args!(
        "[player-scale-no-bone] ragdoll single status frame={} scale={scale:.3} scale_active={scale_active} player=0x{player_addr:x} chr_model=0x{chr_model_addr:x} model_ready={model_ready} ragdoll=0x{ragdoll_addr:x} ragdoll_ready={ragdoll_ready} ready_frames={} delay={RAGDOLL_SINGLE_PROBE_READY_DELAY_FRAMES}",
        state.ragdoll_single_status_frames, state.ragdoll_single_ready_frames
    ));
}

fn read_chr_model_addr(player: &PlayerIns) -> usize {
    let chr_ins_addr = &player.chr_ins as *const _ as usize;
    unsafe { ((chr_ins_addr + CHR_INS_MODEL_INS_OFFSET) as *const usize).read_unaligned() }
}

fn read_chr_ctrl_hkx_roots(player: &mut PlayerIns) -> (usize, usize) {
    let chr_ctrl = player.chr_ins.chr_ctrl.as_mut();
    (chr_ctrl.chr_collision, chr_ctrl.ragdoll_ins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_bad_scale_to_default() {
        assert_eq!(clamp_scale(f32::NAN), 1.0);
        assert_eq!(clamp_scale(f32::INFINITY), 1.0);
    }

    #[test]
    fn clamps_scale_range() {
        assert_eq!(clamp_scale(0.1), SCALE_MIN);
        assert_eq!(clamp_scale(4.0), SCALE_MAX);
        assert_eq!(clamp_scale(1.2), 1.2);
    }

    #[test]
    fn scale_effects_use_first_configured_match_without_compounding() {
        let active = [8020400, 8020401];
        assert_eq!(
            first_matching_scale(|sp_effect| active.contains(&sp_effect)),
            0.5
        );
    }

    #[test]
    fn scale_effects_fall_through_after_higher_priority_effect_is_removed() {
        assert_eq!(first_matching_scale(|sp_effect| sp_effect == 8020401), 3.0);
    }

    #[test]
    fn steady_cloth_state_does_not_repeat_the_full_topology_refresh_each_frame() {
        let target_input = 0x1234usize;
        let scale_bits = 0.5f32.to_bits();
        let generation = 7u64;

        assert!(should_refresh_cloth_local_scale(
            (0, 1.0f32.to_bits(), 0),
            0,
            false,
            (target_input, scale_bits, generation),
        ));
        assert!(!should_refresh_cloth_local_scale(
            (target_input, scale_bits, generation),
            CLOTH_LOCAL_SCALE_AUDIT_INTERVAL_FRAMES - 1,
            false,
            (target_input, scale_bits, generation),
        ));
        assert!(should_refresh_cloth_local_scale(
            (target_input, scale_bits, generation),
            CLOTH_LOCAL_SCALE_AUDIT_INTERVAL_FRAMES,
            false,
            (target_input, scale_bits, generation),
        ));
        assert!(should_refresh_cloth_local_scale(
            (target_input, scale_bits, generation),
            1,
            false,
            (target_input, 1.0f32.to_bits(), generation),
        ));
        assert!(should_refresh_cloth_local_scale(
            (target_input, scale_bits, generation),
            1,
            false,
            (target_input, scale_bits, generation + 1),
        ));
    }

    #[test]
    fn ready_binding_uses_a_bounded_periodic_audit_instead_of_full_validation_each_frame() {
        assert!(should_audit_body_scale_binding(false, 0));
        assert!(!should_audit_body_scale_binding(true, 1));
        assert!(!should_audit_body_scale_binding(
            true,
            BODY_SCALE_BINDING_AUDIT_INTERVAL_FRAMES - 1,
        ));
        assert!(should_audit_body_scale_binding(
            true,
            BODY_SCALE_BINDING_AUDIT_INTERVAL_FRAMES,
        ));
    }

    #[test]
    fn pending_local_scale_work_forces_the_next_budgeted_refresh() {
        assert!(should_refresh_cloth_local_scale(
            (0x1234, 0.5f32.to_bits(), 7),
            1,
            true,
            (0x1234, 0.5f32.to_bits(), 7),
        ));
    }

    #[test]
    fn aabb_refresh_waits_for_local_dimensions_and_native_transition() {
        let previous = (0x1234usize, 1.0f32.to_bits(), 7u64);
        let current = (0x1234usize, 0.5f32.to_bits(), 7u64);

        assert!(!should_refresh_cloth_instance_aabbs(
            true, false, previous, 600, false, current,
        ));
        assert!(!should_refresh_cloth_instance_aabbs(
            false, true, previous, 600, false, current,
        ));
        assert!(should_refresh_cloth_instance_aabbs(
            false, false, previous, 600, false, current,
        ));
    }

    #[test]
    fn cloth_transition_queue_runs_on_identity_changes_or_bounded_audits() {
        let steady = (0x1234usize, 0.5f32.to_bits(), 7u64);
        assert!(!should_queue_cloth_scale_transitions(steady, steady, false));
        assert!(should_queue_cloth_scale_transitions(steady, steady, true));
        assert!(should_queue_cloth_scale_transitions(
            steady,
            (steady.0, 1.0f32.to_bits(), steady.2),
            false,
        ));
        assert!(should_queue_cloth_scale_transitions(
            steady,
            (steady.0, steady.1, steady.2 + 1),
            false,
        ));
        assert!(!should_queue_cloth_scale_transitions(
            steady,
            (0, steady.1, steady.2 + 1),
            true,
        ));
    }
}

#[cfg(test)]
mod test_fixtures;
