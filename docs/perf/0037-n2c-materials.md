# The material model on Tiered (N2c)

Captured 2026-11-17 for ADR-0023 (N2c) and its
[Amendment 3](../adr/0023-frame-architecture-materials.md#amendment-3-2026-11-17-n2c-the-material-model-as-built-and-the-frozen-material-spec).

- **GPU:** NVIDIA GeForce RTX 5070 Ti (driver 620.70.06), used for timings and validation.
- **Software:** lavapipe (Mesa 26.2.5 llvmpipe, LLVM 23.1.3) with the Khronos validation layer
  and `MOBA_VULKAN_VALIDATION=required` (validation and synchronisation validation).
- **CPU:** AMD Ryzen 7 9800X3D. **OS:** Linux 7.3.4-1-cachyos, Wayland (output scale 1.5:
  a 1280×720 window is a 1920×1080 swapchain).
- **Toolchain:** rustc stable, the pinned `slangc`.
- **Branch:** `feat/n2c-material-model`, on origin/main `ceca947` (N2b).
- **Baselines:** detached release builds in their own temporary worktrees, deleted afterwards:
  `ab42718` (the N1 tiered forward frame, ADR-0023 §9b's baseline) and `ceca947` (N2b).

TODO
