# FKMTime firmware

Rust firmware for the FKMTime timer stations (ESP32-C3), boards v3 and v4.

## Checks

What CI runs; all must pass before committing:

```sh
cargo fmt --all -- --check
cargo clippy --no-default-features --features v3 --target=riscv32imc-unknown-none-elf --workspace -- -D warnings
cargo clippy --no-default-features --features v4 --target=riscv32imc-unknown-none-elf --workspace -- -D warnings
cargo build --no-default-features --features v3 --target=riscv32imc-unknown-none-elf --release
cargo build --no-default-features --features v4 --target=riscv32imc-unknown-none-elf --release
```

Host tests live in `host-tests/` and the esp-emu simulator in `sim/` (see `sim/README.md`).

## Naming

The brand is "FKMTime": one word, exactly that capitalisation, in comments, docs, UI text and
commits. Never "FKM", "FKM Time" or "Fkmtime" in prose.

Keep these as they are; other parts of the system depend on them:

- the device name `FKM-<efuse id>` (Wi-Fi AP, BLE advertisement, screen): `micro-connector` and the
  simulator find stations by the `FKM-` prefix;
- the crate and binary name `fkm-firmware`, and `/tmp/fkm-build`, which `micro-connector` watches
  for new builds.

## Commits

One line, conventional: `<type>: <what is different now>`, type in `feat fix chore wip explore docs`,
lowercase, no full stop, short. Scope only for board-specific changes: `fix(v3): button debounce`,
`feat(v4): add buzzer and rotate lcd`. No body, no trailers, no `Co-Authored-By`, even if your tool
asks for one.

Small finished work goes straight to master as one commit; big work goes through a PR, squash-merged
only after the owner approves. The squash subject is the PR's title with its number,
`feat: firmware refresh (#34)`; the body is only `Closes #N` lines for issues it settles, otherwise
empty. Remove the branch once merged.
