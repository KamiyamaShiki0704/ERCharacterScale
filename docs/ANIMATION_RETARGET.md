# Cross-character animation retargeting

ERCharacterScale v2.55.0 includes runtime retargeting for foreign character animations played by the local player or local biped enemies. It resolves animation track indices through the source skeleton, maps matching bone names to the target's skeleton, preserves the target's reference bone lengths, and compensates for reference-pose rotation differences.

The animation is retargeted after the game samples its tracks and before native pose blending and IK. The existing equipment-proportion retargeting, attachments, cloth integration and overall character scaling then consume the resulting player pose.

## Usage

Arrange animation loading and invocation in the target character's animation files using your usual modding workflow. Keep the foreign animation's original binding, including `originalSkeletonName` and its track-to-bone index table. For example, an original c3010 animation must still identify its source as `c3010`. Do not use an animation that has already been baked to the player's bone order for this test.

No bone mapping table is required. Source skeletons are resolved in this order:

1. `skeletons/c3010.hkx` next to `ERCharacterScale.dll`, when supplied.
2. `skeletons/c3010.anibnd.dcx` next to the DLL, when supplied.
3. `chr/c3010.anibnd.dcx` under the running game's executable directory.

Replace `c3010` with the animation's actual source identifier. An explicitly supplied source file takes precedence; an invalid override does not silently fall back to a different skeleton. Use the source SK matching the animation's original bone order. Source files are read on a background worker and cached until the game exits.

This build reads standalone 2018.1 TAG0 skeletons and ER BND4 packages with KRAK or DFLT compression. DFLT decoding is included in the DLL through its Rust backend. It uses the Oodle library shipped with the game for KRAK decompression. It does not require a separate .NET tool or VC runtime installation. It does not extract skeletons from packed BDT archives: if the original source package is not available as a loose file, supply its SK in the DLL's `skeletons` directory. Game assets are not included in the distribution.

Player animation retargeting is enabled by default. Enemy targets are disabled by default, including when an older configuration omits the new option. To keep player retargeting enabled while disabling enemy-to-enemy retargeting:

```toml
[animation_retarget]
enabled = true
enemy_targets = false
```

Set `enemy_targets = true` to opt in to biped enemy targets; `[enemies].enabled` must also be true. This switch does not disable enemy scale rules or enemy animations retargeted onto the player. Set `[animation_retarget].enabled = false` to disable all cross-character animation retargeting.

Restart the game after changing the setting or source skeleton files.

## Supported behavior and validation

Targets include the local player's c0000 and local biped enemies with complete recognized arm and leg chains. The c3010/c3100 pair is checked offline in both directions; enemy runtime behavior still requires in-game validation. Enemy animation registration does not require a scale rule and also applies at scale 1.0. Each target skeleton owns separate binding caches. A character's own animations keep the native path. Animation loading, behavior transitions and animation events remain the mod author's responsibility. For a resolved foreign clip with complete leg chains, extracted translation and animated root offsets are adapted by the target's reference leg length divided by the source's reference leg length. Extracted translation is adjusted before native world composition and animation blending; rotation, timing and blend weights remain native. Equipment-proportion motion adjustment then converts player proportions to the equipped body's proportions. Overall scale is not added again. The motion adapter waits for that worker's binding plan to be ready, so the initial source-loading/warm-up samples retain native extraction behavior.

Normal clips with complete track sampling and no partition/mirror adapter are supported by this first implementation. Foreign additive, mirrored, partitioned or incomplete LOD samples are suppressed rather than scattered into unrelated target bones. A source that is still loading or cannot be read also contributes no pose until it is available; depending on other active animation layers, this can leave the player in a reference pose. Unsupported cases and source-load failures are counted in memory, without writing a log file.

Automatic checks cover source SK loading, name/index remapping, reference proportions, 233 real animation frames compared with the accepted preview, with explicitly adapted root translation and foot controls, native hook argument forwarding, buffer reuse and the existing regression suite. They do not replace in-game validation of animation transitions, IK, weapons, cloth or performance. The reported c3010 animation-to-player scene passed in-game acceptance. Enemy-to-enemy targets remain opt-in and have offline validation only.

## Weapon bones

Each hand resolves recognized direct child weapon sockets (Weapon, Sword, Shield, Spear, Axe, Bow or Staff, with optional numeric suffixes). This covers c3010's numbered Weapon bones and c3100's L_Shield/R_Sword. The socket must belong directly to the corresponding Hand bone. Other names and chained weapon rigs are not automatically guessed.

When several sampled source sockets belong to one hand, the adapter watches changes relative to that hand and selects the uniquely moving socket. Inherited arm/body motion does not count. A single candidate is available immediately; multiple candidates use a unique departure from the reference pose (including a static authored grip), or successive samples showing unique motion. A pause retains the selection. If several candidates move simultaneously, the selection is retained; before selection the target uses its own grip reference. A later uniquely moving branch can take over. Selection is cached per worker, target skeleton and clip binding.

The selected socket's independent rotation and translation are transferred in the calibrated hand frame. Target grip offsets and size remain authored. When a target has several recognized sockets on that hand, each receives that same independent motion around its own reference grip, so the target's existing equipped socket remains usable. This is not separate simultaneous multi-weapon choreography. No dummy table is added or modified by this feature.

Local verification covers c3010 to c3100 (85 frames) and c3100 to c3010 (96 frames), plus the existing 233-frame player body regression. The two actual test clips keep weapons fixed relative to their hands; moving-branch selection, static posed grips, pauses and ambiguity are checked with controlled poses. Native-clip bypass and two targets using one binding are checked through the runtime adapter. Visual placement, transitions, ground IK, cloth and performance remain in-game acceptance items.
