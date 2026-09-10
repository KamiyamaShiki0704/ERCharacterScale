# Validation

## 2.54.0-rc.3 model selection regression

A live hostile c9520 had EnemyIns class, role 7 and active entry [4,4] in the debug collection (also referenced by chr_sets[133]). Both rc.2 enumeration and class selection rejected it. Separate regression runs reproduced each rejection before the fix.

The collection regression now exercises actual bounded readers over owned raw bytes: debug and summon units are selected, duplicate collection references are processed once, ghost aliases and remote players stay excluded, and the exact model-only rule reaches identity resolution without a team query. Role-6 and role-19 cases are synthetic coverage, not in-game visual acceptance.

Rules default to no allegiance restriction. Tests cover explicit hostile-only rules, false/unknown relation fallback, mismatched models and prohibition on player hostile-only rules. Game checks and human visual acceptance are recorded separately.

## 2.54.0-rc.2 readiness regression

Original executable inspection and read-only loaded-character observation identified
the rc.1 startup rejection: a reciprocal entry has state 4 at +8, and ordinary local
actors also have 4 at +9. The old fixtures incorrectly used state 2 and a zero next
byte, so their passing results did not validate the real runtime readiness gate.

The new regression passes loaded player/enemy entries through the actual identity
and set-enumeration functions, rejects inactive states, and preserves remote-role
exclusion. The original-PE test also corrupts each new readiness witness and checks
rejection. These checks are separate from game observation of applied scales and
from human assessment of animation and cloth quality.

## 2.54.0-rc.1 candidate

The candidate adds configurable player/enemy rules and non-c0000 EnemyIns routes.
It is not yet accepted in-game. Automatic checks completed on 2026-09-11:

- 191 default tests pass in debug and release; 8 optional tests pass in each profile.
- Clippy with warnings denied, rustfmt and the release DLL build pass.
- Three owned non-c0000 character heaps run 600 iterations at independent factors
  0.5, 1.5 and 3.0, retaining distinct native sizes, pose outputs and body collisions.
- Direct non-c0000 cloth ownership reaches the existing 2.53 collider rotation
  correction. Mock native calls verify corrected arguments, passthrough controls
  and rejection after the character identity is recycled.
- Shared cloth arrays preserve their original values when another instance loads
  later. Conflicting factors restore the affected group; unrelated geometry keeps
  its scale. Departing instances do not restore arrays still used by another unit.
- Subject-owned effects, constant rules, filters, malformed files, type/owner
  checks, dormant identities, model replacement and new handles are covered.
- The owned original-PE check validates 11 additional enemy-relation/model code
  witnesses and rejects a corruption in each. It never executes the mapped PE.
- The accepted 2.53 DLL and its 419-file source manifest remain unchanged.

Automatic evidence uses owned synthetic memory, existing local captured fixtures
and read-only original executable inspection. Synthetic model numbers are fixture
identities, not claims that those specific enemy types have passed in the game.
The new runtime is per instance; old 2.53 visual acceptance does not carry over.

Still required in-game: ordinary enemies, cloth users, nonhuman enemies and a Boss;
0.5 / 1.5 / 3.0 / restore, movement and attacks, death/reload/teleport, and the
BD9004 player regression with enemies enabled. Shared cloth with conflicting
factors is an explicit unsupported case in this candidate. Special grabs,
executions, attack volumes and Boss transitions are not automatically validated.

For a first test, copy `examples/ERCharacterScale.enemies-half.toml` next to the
DLL as `ERCharacterScale.toml`. It keeps the original player mappings and applies
0.5 to supported enemies without requiring enemy SpEffects. Record actual model,
NpcParam and event ID from `[ERCS-UNIT]` in the ordinary log when reporting results.

## Accepted runtime build

- Build: `er-2.53-rigid-collider-velocity` / release2.53.0.
- Game: Windows x64, WW2.7.1.0(game1.17.1).
- Human observation: BD9004 at0.5 scale no longer twitches and behaves normally.
- Returned runtime capture:3consecutive steps,42events,6integrations;complete footer.
- All15 collider velocity corrections were observed and matched isolated original
  function replay byte-for-byte. Position and native padding lanes were retained.
- Source development validation also included480 deterministic velocity controls,
  15 captured collider substep replays and a120-frame native rotation feedback loop.

This acceptance covers the reported scenario. It does not claim validation of
every enemy, armor set, scale or lifecycle sequence.

## Public source tests

The default suite uses owned synthetic memory and mock native callbacks. It does
not open a game process. Game-derived geometry, runtime dumps, original executables
and private development records are not distributed.

Seven integration tests retain their assertions but load local fixtures at run
time. To run them, set `ER_CHARACTER_SCALE_FIXTURES` to a directory containing your
own compatible capture fixtures, then run the selected test with `-- --ignored`.
The separate executable-layout check uses `ERPS_COMPAT_EXE` to name the matching
original executable. Neither environment variable is needed for the default suite.

```powershell
# Examples using paths you supply locally:
$env:ER_CHARACTER_SCALE_FIXTURES = 'D:/local-fixtures'
cargo test --locked slow_middle_read_still_records_three_complete_steps -- --ignored
$env:ERPS_COMPAT_EXE = 'D:/Games/ELDEN RING/Game/eldenring.exe'
cargo test --locked real_271_pe_runtime_guard_and_negative_controls -- --ignored
```

These7 tests and the1 original-executable check were also run successfully during
2.53 publication and again for the 2.54 candidate using the existing private fixtures.
The historical test-oracle Rust helpers under `src/test_support` are project code,
not game binaries or captured assets.

## Released binary

The GitHub release preserves the accepted binary rather than replacing it with a
new local build. ItsSHA256 is
`F40FC6B9793A08E280C0CAD21289D1C164B31764AC7297477A71BA4C684AC00B`.
That release's Cargo metadata and runtime identify version2.53. The candidate uses
version2.54.0-rc.3 and is a separate binary. Locally rebuilt files need not have the
same binary hash.
