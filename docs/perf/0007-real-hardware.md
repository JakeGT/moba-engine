# Real-hardware frame timings

Captured 2026-10-18 on the development workstation, not a CI runner.

- GPU: NVIDIA GeForce RTX 5070 Ti (GB203), driver 615.71.09, Vulkan 1.4.351
- CPU: AMD Ryzen 7 9800X3D
- OS: Linux (Wayland/XWayland), Mesa validation layer 1.3.x
- Resolution: 1920×1080 physical (1280×720 logical at 1.5 scale)
- Scene: local bot preview after 3,000 warmup ticks, 840 measured frames, vsync disabled
- Command:
  ```bash
  VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/nvidia_icd.json \
  MOBA_WARMUP_TICKS=3000 MOBA_SMOKE_FRAMES=900 MOBA_VSYNC=0 \
  MOBA_FRAME_LOG=/tmp/frame_nv.log cargo run -p moba-client
  ```
- Result: `mean 4.183 ms`, `p50 4.181 ms`, `p95 4.197 ms`, `p99 4.212 ms`, `max 4.538 ms`.
  540-frame repeat: `p95 4.198 ms`, `max 5.518 ms`.

Validation-required capture at the same scene produced zero validation messages; artifact
`docs/images/m8-nvidia-teamfight.png`. This is well inside the 6.9 ms p95 / 16.6 ms max budget.

## Windows / macOS

- Windows dedicated GPU results are not available in this environment. CI Windows runners have
  no Vulkan-capable GPU; the cross-OS `gates` job still compiles/tests the renderer.
- macOS: `macos-vulkan.yml` runs the MoltenVK probe and, when the runner allows window creation,
  the client smoke/frame-log steps. Apple-GPU timings from that job are attached as workflow
  artifacts rather than claimed here.
- Commands are the same as above with the platform's `VK_ICD_FILENAMES`/MoltenVK ICD path.

## Headless/low-end path

The existing instanced-cube fallback and lavapipe golden path are unchanged; software Vulkan
validation is covered by the separate golden workflow.
