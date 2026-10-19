# 0002: Platform (M2)

**Milestone:** M2   **Crates touched:** platform, client   **Reviewers:** perf + /code-review

## Goal
A window, keyboard/mouse input, and a fixed-timestep application loop that every client
build and every later renderer can rely on. The platform crate is the only place that
links `winit`; simulation crates stay free of windowing and wall clocks.

## Scope
- `WindowConfig` + typed `PlatformError`; window creation on `resumed`, clean suspend.
- Input snapshot: physical-key mapping (WASD, abilities, modifiers), mouse buttons, cursor,
  wheel, focus-loss safety (held keys are released so units cannot get stuck).
- Frame loop: integer `FixedStep` at 30 Hz, `frame.ticks()` and `frame.alpha()` for
  interpolation, configurable frame-rate cap, `WaitUntil` pacing.
- `App` trait with `on_start` / `on_frame` -> `FrameOutcome` / `on_resize` / `on_focus` /
  `on_suspend`; renderer re-creation on resume.
- Client smoke loop exits after `MOBA_SMOKE_FRAMES` so CI can prove the loop without a human.

## Non-goals
- Gamepad, IME/text entry and clipboard (M6 UI).
- Cursor grab/free-look and multi-window (M6).
- Renderer surface creation (M3).

## Acceptance tests
- [x] `input::tests::press_edges_ignore_autorepeat`: held keys, edges and autorepeat.
- [x] `input::tests::focus_loss_releases_held_input`: no stuck movement after alt-tab.
- [x] `input::tests::mouse_and_wheel_accumulate`: mouse state and per-frame wheel delta.
- [x] Client smoke run (`MOBA_SMOKE_FRAMES=60`) exits 0 after 60 frames on Linux/X11.
- [~] Windows/macOS client smoke: macOS `macos-frame.yml` runs the client and records a frame
  log; the hosted Windows runner has no Vulkan device. Real platform GPU validation is #9/#127.

## Budgets touched
Input handling is event-driven and allocation-free per event. The loop caps at 240 FPS by
default; the renderer's FIFO present mode will be the real limiter from M3.
