# Prova's self-test suite (dogfooding)

Prova acceptance-tests **itself**: these `*_test.lua` files invoke the real `prova` binary (via the
`shell` module) against the inner `fixtures/`, and assert on exit codes and output. It's black-box
coverage of the assembled CLI — arg parsing, discovery, reporting, the `prova.toml` manifest — that
the Rust library tests can't reach, because it exercises the *binary*.

- `fixtures/` — inner suites the self-tests run `prova` against (named without `_test` so the outer
  run doesn't auto-discover them).
- `cli_test.lua` — exit codes, tally output, `--list`, `--format json`, error paths.
- `manifest_test.lua` — `prova.toml` profile selection + env injection, via the real CLI.

Driven by `tests/selftest.rs`, which runs `prova <selftest-dir>` (from `CARGO_BIN_EXE_prova`) with
`PROVA_FIXTURES` set. The inner binary is not passed in — each suite reads `prova.bin`, the
executable the runtime knows it is running, so the inner run is the same build as the outer one by
construction rather than by the launcher remembering to say so. So the flow is:

```
cargo test → prova (outer) → runs *_test.lua → each shells out to → prova (inner) → asserts
```

Writing these already paid off: they caught that the CLI advertised `--format json` (space form) but
only parsed `--format=json`. Fixed.
