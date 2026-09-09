# Validation

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
publication using the existing private fixtures. Changing fixture loading affects
only test builds; runtime scaling code is retained from the accepted2.53 version.
The historical test-oracle Rust helpers under `src/test_support` are project code,
not game binaries or captured assets.

## Released binary

The GitHub release preserves the accepted binary rather than replacing it with a
new local build. ItsSHA256 is
`F40FC6B9793A08E280C0CAD21289D1C164B31764AC7297477A71BA4C684AC00B`.
The public Cargo package metadata uses version2.53.0; the runtime build identifier
and behavior remain2.53. Locally rebuilt files need not have the same binary hash.
