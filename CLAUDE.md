# OpenShareMouse

Spec and acceptance baseline: `ShareMouse-Professional-Reconstruction.md` ("the report"). Section numbers like §6.2 and IDs like F24/AT14/U07 refer to it.

## Build

- `make app` / `make run` / `make test` / `make lint`. Rust lives at `~/.cargo/bin` which may not be on PATH in non-login shells; the scripts prepend it.
- After changing anything `#[uniffi::export]`ed in `crates/ffi`, rerun `scripts/build-rust.sh` — Swift sees the old API until the xcframework and `Sources/OSMCore/Generated/` are regenerated (both git-ignored).
- Test permission-dependent behaviour only in the signed `.app` from `scripts/bundle-app.sh`. TCC grants follow the code signature; `swift run` and ad-hoc builds give misleading results.
- Network to static.rust-lang.org is flaky here; keep `rust-toolchain.toml` on `stable`, not a pinned version (pinning makes rustup download a manifest and time out).

## Conventions

- `osm-core` is sans-IO: no clocks, sockets, threads or OS types. Timers and transport live in the driver and feed events back in. Keep it that way so state machines stay deterministic under test.
- Input hot path (event tap, suppression, injection) is Rust in `osm-platform-macos`, not Swift. See `docs/adr/0001-input-hot-path-in-rust.md`.
- The UniFFI surface is control plane only. Don't route per-event input through it.
- Report values marked D (timeouts, limits, ports) are starting points; when changing one, note the measurement that justified it.
- Never log keystrokes, clipboard contents, file contents or passwords (§7.2). Device IDs in logs use `DeviceId::short()`.
- Comments in code are English; user-facing strings in the app are Chinese.
