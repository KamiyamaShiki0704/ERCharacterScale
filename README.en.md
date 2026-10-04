# ERCharacterScale

[简体中文](README.md) | **English**

Configure the overall size of the Elden Ring player and other characters through TOML rules. Apply a scale while a SpEffect is active, or use a constant scale without any effect requirement.

[Download v2.55.1](https://github.com/KamiyamaShiki0704/ERCharacterScale/releases/tag/v2.55.1) · [Configuration guide (Chinese)](docs/CONFIGURATION.md) · [Changelog](CHANGELOG.md)

## Features

- Configure the player and non-player characters separately, with scales relative to each character's original size.
- Support the local player, local c0000 NPCs, and non-c0000 character models.
- Assign different scales by `cxxxx` model ID, NPC parameter ID, or event entity ID.
- Check each character's own SpEffects, or apply a scale unconditionally with `constant` mode.
- Keep cloth simulation active.
- Automatically detect different player-equipment proportions, animate at authored bone lengths, and adapt physics, camera follow, motion, limb IK, weapons and body attachment points.
- Retarget other characters' animations onto the player by matching bone names, preserving target proportions and adapting root motion and weapon-bone motion.

## Latest changes

Improve seated poses and leg animation adaptation, align weapon and item attachment points, and correct arm lifting with independent-hand weapon movesets.

## Installation

For **Windows x64, offline Elden Ring WW2.7.1.0 / game1.17.1**. Other executable versions require separate validation.

1. Download `ERCharacterScale-v2.55.1-windows-x64.zip` from the [Release page](https://github.com/KamiyamaShiki0704/ERCharacterScale/releases/tag/v2.55.1).
2. Place `ERCharacterScale.dll` and `ERCharacterScale.toml` in the same directory, and load the DLL with your existing DLL loader.
3. Edit the TOML configuration as needed, then start the game. Restart the game after later configuration-file changes.


The released DLL includes its C/C++ runtime. No separate Visual C++ Redistributable or Rust installation is required. Windows system components are supplied by the operating system.

## Configuration

The configuration file is [ERCharacterScale.toml](ERCharacterScale.toml), stored beside the DLL and encoded as UTF-8.

- `[player]` and `[enemies]` enable player and non-player scaling separately.
- `target = "enemy"` covers supported local non-player characters, including allies and summons. Add `hostile_only = true` only when the rule should require hostility toward the player.
- Use `character_ids = [9520]` to match c9520. Create separate rules for different models.
- Each character uses the first fully matching rule in file order. Rules do not multiply together; no match restores the character's original size.
- SpEffect rules respond to effects being gained or lost by that character. `constant` mode needs no SpEffect.

Start with one of these complete examples, copy it beside the DLL as `ERCharacterScale.toml`, and adjust it as needed:

- [Fixed scales](examples/ERCharacterScale.fixed.toml): player at 0.75, supported non-player characters at 0.5.
- [Half-size non-player characters](examples/ERCharacterScale.enemies-half.toml): default player effect rules, with supported non-player characters fixed at 0.5.

See the [full configuration guide (Chinese)](docs/CONFIGURATION.md) for filters, rule ordering, and configuration errors. The bundled TOML files include English comments.

## Retargeting

[Equipment skeleton retargeting](docs/EQUIPMENT_RETARGET.md) detects proportion differences automatically; no equipment list is required. Preserve the model's reference skeleton and skin binding, and author hair/cloth physics for the new proportions. The optional `[retarget]` table can disable detection or define model exceptions.

[Cross-character animation retargeting](docs/ANIMATION_RETARGET.md) enables player targets by default. Arrange animation loading and invocation yourself, keeping the original source binding. The DLL resolves track indices through the source SK and maps names to the target skeleton. Source SK files can be supplied in `skeletons/` beside the DLL. Enemy-to-enemy retargeting is disabled by default; `[animation_retarget] enemy_targets = true` opts in to biped targets.

Use the [minimal retargeting configuration](examples/ERCharacterScale.retarget.toml) to enable retargeting without overall scale rules.

## Building from source

Requires Rust nightly and the Windows x64 MSVC toolchain (Visual Studio C++ Build Tools / Windows SDK).

```powershell
git clone https://github.com/KamiyamaShiki0704/ERCharacterScale.git
cd ERCharacterScale
cargo build --release --locked
```

The output is `target/x86_64-pc-windows-msvc/release/ERCharacterScale.dll`. The repository configures static C/C++ runtime linking. Cargo fetches `fromsoftware-rs` from Git at a pinned commit. This repository contains neither its source tree nor a Git submodule. `Cargo.lock` pins dependency resolution.

See [Validation](docs/VALIDATION.md) for development checks and test commands.

## License

Released under the [MIT License](LICENSE). See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for dependency and reference-code licenses and attribution.
