# ERCharacterScale

[简体中文](README.md) | **English**

Configure the overall size of the Elden Ring player and other characters through TOML rules. Apply a scale while a SpEffect is active, or use a constant scale without any effect requirement.

[Download v2.54.0-rc.5](https://github.com/KamiyamaShiki0704/ERCharacterScale/releases/tag/v2.54.0-rc.5) · [Configuration guide (Chinese)](docs/CONFIGURATION.md) · [Changelog](CHANGELOG.md)

## Features

- Configure the player and non-player characters separately, with scales relative to each character's original size.
- Support the local player, local c0000 NPCs, and non-c0000 character models.
- Assign different scales by `cxxxx` model ID, NPC parameter ID, or event entity ID.
- Check each character's own SpEffects, or apply a scale unconditionally with `constant` mode.
- Keep cloth simulation active.

## Installation

For **Windows x64, offline Elden Ring WW2.7.1.0 / game1.17.1**. Other executable versions require separate validation.

1. Download `ERCharacterScale-v2.54.0-rc.5-windows-x64.zip` from the [Release page](https://github.com/KamiyamaShiki0704/ERCharacterScale/releases/tag/v2.54.0-rc.5).
2. Place `ERCharacterScale.dll` and `ERCharacterScale.toml` in the same directory, and load the DLL with your existing DLL loader.
3. Edit the TOML configuration as needed, then start the game. Restart the game after later configuration-file changes.

When upgrading, exit the game before replacing the DLL and keep your existing `ERCharacterScale.toml`. The bundled TOML is the default configuration: player effect rules are enabled, and **non-player scaling is disabled by default**.

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

## Building from source

Requires Rust nightly and the Windows x64 MSVC toolchain (Visual Studio C++ Build Tools / Windows SDK).

```powershell
git clone https://github.com/KamiyamaShiki0704/ERCharacterScale.git
cd ERCharacterScale
cargo build --release --locked
```

The output is `target/release/ERCharacterScale.dll`. Cargo fetches `fromsoftware-rs` from Git at the pinned commit `02fa5681e27fd2ecd9da79d34aef9fae96805539`. This repository contains neither its source tree nor a Git submodule. `Cargo.lock` pins dependency resolution.

See [Validation](docs/VALIDATION.md) for development checks and test commands.

## License

Released under the [MIT License](LICENSE). See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for dependency and reference-code licenses and attribution.
