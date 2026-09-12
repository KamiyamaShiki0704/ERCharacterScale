# Changelog

## 2.54.0 — embedded C/C++ runtime

- Statically link the required C/C++ runtime into ERCharacterScale.dll; no separate Visual C++ Redistributable installation is required. Windows system components remain OS-provided.
- Make the x64 MSVC target and static CRT flags part of the default Cargo configuration, keeping host proc-macros separate.
- Add a dependency check for normal and delayed PE imports. Update source-build output paths and runtime requirements in both READMEs.
- Retain the rc.6 scaling code and user configuration behavior. The user confirmed rc.6 works normally after correcting duplicate rule names; static runtime packaging is validated separately.

## 2.54.0-rc.6 — positive finite scales

- Remove the fixed 0.5–3.0 scale restriction from configuration, body pose transitions and cloth routes. Player and model-specific non-player rules accept any positive finite f32 scale.
- Check derived body dimensions before publishing the requested scale. Preflight cloth dimension batches using cached extrema, preserving budgeted updates and shared original baselines.
- Preserve numeric overflow/underflow rejection and use wider intermediate math where necessary; do not clamp requested values.
- Add small/large scale, cached-pose transition, shared-cloth and partial-write regression cases. Game visual acceptance for the new scale range is pending.
- Simplify both language READMEs by removing the compatibility section and the scale-range and default-log feature descriptions.

## 2.54.0-rc.5 — resident-memory permission query cost

- rc.4 failed in-game performance acceptance at a reported 6 FPS. Small-memory query-count tests missed the much higher per-call cost on large resident game regions.
- Use bounded `K32QueryWorkingSetEx` page checks for metadata and small buffers. Query every covered page; preserve read/write/guard checks, fresh ownership reads and invalidation at each native-operation boundary.
- Fall back to the original `VirtualQuery` checks for nonresident or unavailable page information and large spans. Keep full region discovery for cloth field-span grouping.
- Add large resident-heap regression through the actual pose dispatcher, plus cross-page, read-only, guard-preservation, decommit, nonresident, nested-scope, unwind and thread-isolation checks.
- Keep `ERCharacterScale.dll`, quiet default, model rules and cloth/scale algorithms unchanged. Player scaling/switching and cloth passed the reported in-game scene on 2026-09-12; c9520 was observed at 0.6. Other models and Boss phases retain their individual validation requirements.

## 2.54.0-rc.4 — runtime cost and quiet defaults

- Name the Cargo library and delivered DLL `ERCharacterScale`; build output is `ERCharacterScale.dll`.
- Disable log file creation, message formatting and one-shot diagnostics by default. The opt-in `diagnostics` Cargo feature retains developer evidence collection.
- Reuse current-operation memory permissions for full consumer identity capture and subject-owned effect traversal, retaining fresh data reads and invalidation across native calls/frames.
- Preserve the existing 60-frame binding audit cadence instead of forcing a full rebind every frame; scale changes, missing bindings and identity replacement still rebind.
- Reuse immutable prepared cloth resource spans during restoration, preserving alias protection and departed-root checks.
- Add deterministic OS-query, binding-cadence, restoration-scan and no-log regressions. rc.3 c9520 shrink/cloth behavior was accepted by the user; rc.4 subsequently failed game performance acceptance (reported 6 FPS).

## 2.54.0-rc.3 — model matching and local character coverage

- Include verified role-7 EnemyIns observed on debug-spawned c9520; enumerate debug and summon collections directly and deduplicate their aliases.
- Keep the existing `enemy` target/switch names for compatibility, but match supported local non-player characters regardless of allegiance by default. Existing model rules also apply to friendly and summoned instances.
- Add optional per-rule `hostile_only` (default false). Only true queries native hostility; false/unknown relations skip that rule and permit later rules to match. Reject true on player rules.
- Include verified EnemyIns roles 5/6/7 and local NPC PlayerIns roles 5/19/20/21. Preserve class, owner, pose and active-entry checks, and ghost/remote-player/Torrent exclusions.
- Add real-input role-7 regressions, collection-to-rule coverage, model mismatch controls, optional-hostility fallback checks and no-query assertions. Keep pinned Cargo dependencies and cloth algorithms unchanged.

## 2.54.0-rc.2 — loaded-character readiness fix

- Fix the shared readiness gate rejecting loaded players and enemies: WW2.7.1.0 uses entry state 4, not the dependency's state-2 label.
- Filter remote/ghost roles using ChrIns character type; entry byte +9 also contains 4 for ordinary local actors and cannot identify remote units.
- Validate three original executable instruction windows before registering the scale task, including player-only configurations.
- Distinguish a missing main-player pointer from failed player identity validation in the runtime log.
- Add a regression for loaded player/enemy entries [4,4], retain inactive-state rejection, and correct the synthetic character fixture.
- Preserve configuration schema, user rules, pinned dependency revision and the existing cloth/scale algorithms. In-game visual acceptance remains separate.

## 2.54.0-rc.1 — candidate, runtime acceptance pending

- Add UTF-8 TOML configuration, ordered per-unit SpEffect rules and unconditional scaling.
- Add model, NpcParam and event-entity filters, independent player/enemy switches, and fail-closed configuration validation.
- Add native hostile-character selection and direct EnemyIns / CSChrModelIns cloth ownership, including non-c0000 models.
- Retain independent character baselines and route pose, cloth and collider callbacks by verified instance ownership.
- Coordinate shared cloth originals; keep conflicting consumers at baseline without changing unrelated characters.
- Preserve the 2.53 rigid collider rotation correction and default player mappings.
- Add synthetic multi-character, lifecycle, shared-cloth and native-layout checks. In-game enemy and player visual acceptance remains pending.

## 2.53.0

- Fix sustained cloth twitching caused by scaled rotation matrices entering
  collider quaternion-velocity calculation.
- Preserve original translation, collision dimensions and native simulation.
- Apply correction only to verified current local-player cloth colliders.
- Retain complete automatic diagnostic steps; add direct before/after velocity evidence.
- Confirm normal game behavior forBD9004 at0.5 scale.
- Publish a standalone Cargo project with a commit-pinned fromsoftware-rs dependency.
