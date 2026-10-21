# FKM firmware

Rust firmware for the FKM timer stations (ESP32-C3), boards v3 and v4.

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

## Commits

One line, conventional: `<type>: <what is different now>`, type in `feat fix chore wip explore docs`,
lowercase, no full stop, short. Scope only for board-specific changes: `fix(v3): button debounce`,
`feat(v4): add buzzer and rotate lcd`. No body, no trailers, no `Co-Authored-By`, even if your tool
asks for one.

Small finished work goes straight to master as one commit; big work goes through a PR, squash-merged
only after the owner approves. The squash subject is the PR's title with its number,
`feat: firmware refresh (#34)`; the body is only `Closes #N` lines for issues it settles, otherwise
empty. Remove the branch once merged.
