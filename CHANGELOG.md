# Changelog

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
