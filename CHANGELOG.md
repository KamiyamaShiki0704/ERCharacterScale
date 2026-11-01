# Changelog

## 2.55.7

- Fix ordinary equipment physics not following overall character scaling in Elden Ring 1.17.2.
- Support convex-plane collision shapes so affected equipment with custom proportions can retain the requested overall scale.
- Improve equipment physics fault isolation and recovery so invalid branches do not disable healthy cloth movement.

## 2.55.6

- Build with fromsoftware-rs while retaining multipart equipment attachment fixes.
- Improve weapon, item and body dummy alignment for matching equipment skeletons split across multiple parts.
- Apply shared attachment correction once and allow remaining matching parts to take over after removal.

## 2.55.5

- Adapt lock-on camera height and distance to equipment proportions.
- Preserve equipment proportions when physics poses become invalid and request a bounded native simulation reset.

## 2.55.4

- Fix scaling not activating with early DLL loading, including me3.
- Retry task-system lookup during startup and stop retrying on shutdown or the startup deadline.

## 2.55.3

- Fix repeated cloth resource processing on native equipment such as the Twinned Armor.
- Reduce redundant validation in equipment retargeting and body attachment queries.

## 2.55.2

- Support equipment proportion retargeting without cloth physics assets.
- Keep parts with no bones on the native rendering path without repeated binding attempts.
- Refresh bindings when optional mapper lists or mesh resources change.

## 2.55.1

- Improve seated poses, leg animation adaptation and ground contact with equipment of different proportions.
- Improve weapon, item and body attachment alignment.
- Correct arm lifting when a two-hand weapon style uses an independent-hand moveset.

## 2.55.0 — equipment and animation retargeting

- Automatically detect player equipment with different authored body proportions and preserve its bone lengths and skin binding. Integrate matching physics assets, camera height, motion, slope/foot correction, grips, weapons and body attachment points.
- Retarget foreign animations by source SK indices and matching target bone names. Adapt root motion by reference leg length and transfer recognized weapon-socket motion. Player targets are enabled by default; biped enemy targets remain opt-in through `enemy_targets = false` by default.
- Improve compatibility between equipment retargeting and overall scaling: preserve mounted scale, correct grounding at small scales, and prevent stretched or unstable cloth when enlarged.
- Add Chinese and English usage documentation, optional retargeting configuration and source-skeleton setup instructions. Retain pinned Cargo Git dependencies, static C/C++ runtime linking and the default silent build.
- Release v2.55.0 as stable. Enemy-to-enemy animation transfer remains experimental and disabled by default.


## 2.54.3 — equipment skeleton map coverage

- Fix stretched hair on enlarged equipment with large cloth skeleton maps.
- Use consistent transform bounds for skeleton poses, source mappings and render output.

## 2.54.2 — collider velocity coverage

- Fix cloth twitching on models such as c3185 by applying collider rotation correction to larger supported collider groups.

## 2.54.1 — convex geometry collision shapes

- Support convex geometry cloth collision shapes, including plane distances, bounds, centroids and grid dimensions.

## 2.54.0 — embedded runtime

- Embed the required C/C++ runtime in the DLL.
- Configure static runtime linking for Windows x64 MSVC builds.

## 2.54.0-rc.6 — positive finite scales

- Accept positive finite scales without a fixed configuration range.
- Reject derived numeric overflow or underflow before applying scale changes.

## 2.54.0-rc.5 — memory query performance

- Reduce permission-query costs for resident game memory while retaining cross-page and access checks.

## 2.54.0-rc.4 — runtime cost and quiet defaults

- Use ERCharacterScale.dll as the library name.
- Disable runtime logging and optional diagnostics by default.
- Reduce redundant permission checks, binding refreshes and resource scans.

## 2.54.0-rc.3 — local character coverage

- Apply model rules to supported local non-player characters, including friendly and summoned units.
- Add optional hostile-only filtering and cover additional local character collections.

## 2.54.0-rc.2 — loaded-character readiness

- Correct loaded-player and enemy readiness checks and exclude remote or ghost roles.

## 2.54.0-rc.1 — configurable scaling

- Add TOML configuration, ordered SpEffect rules, constant scaling and per-character filters.
- Maintain separate character baselines and protect shared cloth resources from conflicting scales.

## 2.53.0

- Correct cloth collider rotation input during scaling to prevent sustained twitching.
- Preserve native translation, collision dimensions and simulation.
- Provide a standalone Cargo project using fromsoftware-rs.
