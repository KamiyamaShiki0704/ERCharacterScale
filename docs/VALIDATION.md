# Development checks

Run the following with the repository's Rust toolchain and Windows x64 MSVC build tools:

```powershell
cargo fmt -- --check
cargo clippy --release --locked --all-targets -- -D warnings
cargo test --release --locked -- --test-threads=1
cargo build --release --locked
```

The default build keeps optional runtime diagnostics disabled. The `diagnostics` Cargo feature is intended for development.

Some ignored tests require separately supplied game assets or captured inputs. Their ignore annotations describe the required environment variables. Only use assets you are entitled to access, and keep them outside the public repository. Ordinary test results do not establish game behavior for every model or executable version.

Before distributing a build, check that its dependency lockfile, Windows target and static runtime settings match the source. Inspect normal and delayed DLL imports to confirm that no additional C/C++ runtime installation is required.

Game checks should cover scale changes in both directions, returning to the original scale, equipping and removing armor, mounted scaling, and armor proportion retargeting combined with overall scaling. Check ordinary cloth movement, weapons, items, attachment points and camera follow. Longer play sessions help reveal intermittent physics failures.

Supported executable versions and configuration requirements are listed in the README. Other versions and different model or physics assets require separate checks.
