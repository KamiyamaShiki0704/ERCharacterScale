# Changelog

## 2.53.0

- Fix sustained cloth twitching caused by scaled rotation matrices entering
  collider quaternion-velocity calculation.
- Preserve original translation, collision dimensions and native simulation.
- Apply correction only to verified current local-player cloth colliders.
- Retain complete automatic diagnostic steps; add direct before/after velocity evidence.
- Confirm normal game behavior forBD9004 at0.5 scale.
- Publish a standalone Cargo project with a commit-pinned fromsoftware-rs dependency.
