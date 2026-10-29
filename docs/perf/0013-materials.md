# Materials, textures and the pipeline cache: cost and evidence (R1b)

Captured 2026-10-28 on the development workstation, not a CI runner, for
[ADR-0010](../adr/0010-texture-pipeline.md).

- GPU: NVIDIA GeForce RTX 5070 Ti, driver 615.78.08 (`nvidia_icd.json`); software: lavapipe
  from `/home/jaket/Projects/MOBA/.tools` with the bundled Khronos validation layer
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-2-cachyos, Wayland
- The host was shared with other agents' builds (load average up to ~13). A first set of
  RTX rounds overlapped this branch's own test builds and is discarded; the tables below are
  the second, clean set. Frame intervals are display-paced (240 Hz) as in
  [perf/0011](0011-render-graph.md); the GPU columns come from timestamp queries and are not
  affected by host load.
- **Before:** `main` at `6e809f0` (R1a + R4a). **After:** this branch.

## Per-pass GPU time (RTX 5070 Ti, 1920×1080, vsync off, validation off)

Commands as in perf/0011 (`MOBA_FRAME_LOG`, three alternating before/after rounds per scene).
The "sentinel" scene is the networked one with every hero drawn by the static
`sentinel.glb` (`MOBA_HERO_MODELS=sentinel`): vertex colour before, the textured sample
material (BC7 + BC5 + BC5 + BC4, 256², full mips) after.

GPU ms, p50 / p95, rounds 1–3:

| Scene | Build | scene | composite | ui | total |
|---|---|---|---|---|---|
| networked (skinned heroes) | before | 0.027–0.029 / 0.031–0.032 | 0.014 / 0.014 | 0.005 / 0.005 | 0.046–0.047 / 0.050–0.051 |
| | after | 0.027–0.029 / 0.028–0.032 | 0.014 / 0.014 | 0.005 / 0.005 | 0.046–0.047 / 0.047–0.051 |
| sentinel (textured after) | before | 0.029–0.032 / 0.032–0.036 | 0.014 / 0.014 | 0.005 / 0.005 | 0.047–0.051 / 0.051–0.055 |
| | after | 0.027–0.031 / 0.028–0.034 | 0.014 / 0.014 | 0.005 / 0.005 | 0.046–0.049 / 0.047–0.053 |
| local bot preview | before | 0.016 / 0.016–0.030 | 0.015 / 0.016 | 0.003 / 0.003 | 0.034 / 0.035–0.048 |
| | after | 0.016 / 0.016–0.026 | 0.015 / 0.016 | 0.003 / 0.003 | 0.034 / 0.035–0.043 |

`span` equals `total` within 1 µs in every run (barriers cost nothing measurable, as in
perf/0011). Frame interval p50/p95: 4.174–4.179 / 4.186–4.202 ms in every run of both builds.
CPU `render_*` p50: 0.204–0.209 ms (networked), 0.047–0.057 ms (local), both builds.

**Reading:**
- **The material costs nothing measurable here.** The textured permutation (four texture
  samples, a derivative tangent frame) on ten heroes is inside the run-to-run spread of the
  vertex-colour build. Heroes cover a few thousand pixels at the overview camera; the number
  to watch is the R5 teamfight scene at close zoom.
- **The submit split moves no GPU time.** With vsync off the acquired image is free at once,
  so the before build's scene pass rarely absorbed the acquire wait; the split guarantees it
  never does (the composite still can, when the presentation engine holds the image).
- **The frame log now records each GPU sample once.** The before build re-logged the latest
  sample whenever no newer frame had completed, which weighted repeated samples; both builds'
  p50s agree, so the old duplicates did not bias these numbers much.

## Pipeline cache: cold vs warm start (RTX 5070 Ti)

`MOBA_SMOKE_FRAMES=10`, default scene, validation off, three rounds. `__GL_SHADER_DISK_CACHE=0`
turns off the NVIDIA driver's own shader disk cache, so "cold" is really cold. 7 pipelines
are built at creation (terrain, unit, unit-textured, skinned, skinned-textured, composite,
UI); `startup_ms` is the whole `Renderer::new` (device, swapchain, passes, pipelines, the
built-in meshes and the 1×1 default textures).

| Run | pipelines built in | renderer ready in |
|---|---|---|
| no cache file, driver cache off | 90.5–98.2 ms | 174.9–254.1 ms |
| no cache file, driver cache on | 3.3–4.2 ms | 77.8–83.4 ms |
| **cold** cache file (first start), driver cache off | 90.7–97.0 ms | 173.0–180.1 ms |
| **warm** cache file, driver cache off | **0.5 ms** | **83.3–84.4 ms** |
| corrupted file (7 bytes overwritten) → `rejected (Checksum)` | 97.0 ms | 179.3 ms |
| next start after the rewrite | 0.5 ms (warm) | 83.8 ms |
| 100 random bytes → `rejected (Malformed)` | 97.6 ms | 180.3 ms |

The saved cache is 128,736 bytes for the 7 pipelines. On lavapipe the driver returns a
20-byte blob with a truncated header; the renderer refuses to save it (`not saved ... not
reusable`), so lavapipe always starts cold (7 pipelines in about 6–8 ms) and a lavapipe run
never overwrites another device's cache. A cache written on the RTX and then offered to
lavapipe is rejected as `OtherDevice` before the driver sees it, and kept for the RTX; a
corrupt file (checksum, header, size) is deleted when it is rejected.

## Textures: CPU fallback cost and parity

`texture-bake info` (release build, one core) decodes each shipped texture, every level:

| Texture | Format | Texel bytes | CPU decode |
|---|---|---|---|
| `sentinel-base` | BC7 sRGB, 256², 9 levels | 87,408 | 0.515 ms |
| `sentinel-normal` | BC5, 256², 9 levels | 87,408 | 0.195 ms |
| `sentinel-mr` | BC5, 256², 9 levels | 87,408 | 0.167 ms |
| `sentinel-mask` | BC4, 256², 9 levels | 43,704 | 0.116 ms |

About 1 ms per material at this size, at load, and only on devices without
`textureCompressionBC`. Parity: the networked sentinel capture on lavapipe with
`MOBA_COMPRESSED_TEXTURES=0` differs from the native-BC capture in 4,189 of 2,764,800 bytes,
by at most 1 (lavapipe's BC decoder and `bcdec_rs` round differently in the last bit).

Bake quality (`texture-bake bake`, level-0 PSNR decoded with `bcdec_rs`): base colour BC7
51.70 dB, normal BC5 50.55 dB, metallic/roughness BC5 53.85 dB, mask BC4 exact.

## Validation and goldens

- **Lavapipe**, `MOBA_VULKAN_VALIDATION=required` (validation + synchronisation validation),
  the `render-golden.yml` commands: the default capture and the networked capture report **0**
  messages and are **byte-identical** to `main` at `6e809f0` (`cmp` on the 1280×720 PPMs), so
  the fingerprints are unchanged (`image-check`: default 0.000 / 0; networked 0.009 / 5, the
  same as `main`). The textured networked sentinel capture and its CPU-decoded twin: 0
  messages each.
- An early build declared the UV attribute on the vertex-colour pipelines, and the layer
  reported `Vertex attribute at location 4 not consumed by vertex shader` (a performance
  warning). Fixed with per-permutation vertex layouts before the runs above.
- **RTX 5070 Ti** with the Khronos layer and synchronisation validation: the default capture,
  the networked capture and the textured networked sentinel capture report **0** messages.
- **Hot reload** (lavapipe, validation required; measured with a release build before the
  review made overrides debug-build-only): with `MOBA_SHADER_DIR` holding
  `unit.frag.spv`, replacing it mid-run with a variant that outputs red logged
  `shaders reloaded ... in 7.6 ms`, the final capture had 6,657 red-dominant pixels, and the
  run reported 0 validation messages.
