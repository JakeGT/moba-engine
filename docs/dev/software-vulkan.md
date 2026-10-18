# Running the renderer without a GPU (lavapipe)

The development container has no hardware Vulkan device. Mesa's **lavapipe** CPU driver
implements Vulkan 1.4 and, together with the Khronos validation layers, gives us real API
coverage for the renderer and CI.

## Linux/Arch (what this workspace uses)

Packages are extracted under `.tools/` (not committed) to avoid needing root:

```bash
mkdir -p .tools/pkgs .tools/swrast .tools/validation .tools/llvm
# Download from an Arch mirror:
#   extra/vulkan-swrast
#   extra/vulkan-validation-layers
#   extra/llvm-libs   (if the installed LLVM is older than lavapipe needs)
cd .tools/pkgs
curl -LO https://geo.mirror.pkgbuild.com/extra/os/x86_64/vulkan-swrast-<ver>-x86_64.pkg.tar.zst
curl -LO https://geo.mirror.pkgbuild.com/extra/os/x86_64/vulkan-validation-layers-<ver>-x86_64.pkg.tar.zst
tar --zstd -xf vulkan-swrast-*.pkg.tar.zst -C ../swrast
tar --zstd -xf vulkan-validation-layers-*.pkg.tar.zst -C ../validation
```

Environment used for validation runs:

```bash
export VK_ICD_FILENAMES=$PWD/.tools/swrast/usr/share/vulkan/icd.d/lvp_icd.json
export VK_LAYER_PATH=$PWD/.tools/validation/usr/share/vulkan/explicit_layer.d
export LD_LIBRARY_PATH=$PWD/.tools/swrast/usr/lib:$PWD/.tools/validation/usr/lib:$PWD/.tools/llvm/usr/lib:$LD_LIBRARY_PATH
export MOBA_VULKAN_VALIDATION=required
```

## Ubuntu CI

```bash
sudo apt-get install -y mesa-vulkan-drivers vulkan-validationlayers glslang-tools
```

Then run the client smoke/capture mode under Xvfb (or a headless surface once the
renderer grows one):

```bash
MOBA_VULKAN_VALIDATION=required MOBA_SMOKE_FRAMES=8 \
MOBA_CAPTURE=frame.ppm MOBA_CAPTURE_FRAME=3 xvfb-run -a cargo run -p moba-client
```

## Notes

- The window's X11 output does not refresh under this lavapipe build; **use the readback
  capture** (`MOBA_CAPTURE`) to verify pixels. The renderer itself is fully exercised.
- `MOBA_VULKAN_VALIDATION=off|required` (default `IfAvailable`) controls the layer.
- `MOBA_SMOKE_FRAMES=<n>` exits the client after `n` frames for CI.

## Golden-image check

The manual/label-triggered `render golden` workflow runs the client under Mesa lavapipe with
validation **and synchronization validation** required, captures frame 90, and compares a
64×36 downscaled fingerprint. The capture path fails if Vulkan reports any validation error.

Local command (no `just` required):

```bash
VK_ICD_FILENAMES=$PWD/.tools/swrast/usr/share/vulkan/icd.d/lvp_icd.json \
VK_LAYER_PATH=$PWD/.tools/validation/usr/share/vulkan/explicit_layer.d \
LD_LIBRARY_PATH=$PWD/.tools/swrast/usr/lib:$PWD/.tools/validation/usr/lib:$PWD/.tools/llvm/usr/lib \
MOBA_VULKAN_VALIDATION=required \
MOBA_SMOKE_FRAMES=120 MOBA_CAPTURE=/tmp/render.ppm MOBA_CAPTURE_FRAME=90 \
cargo run --release -p moba-client

cargo run --release -p moba-tools --bin image-check -- /tmp/render.ppm \
  --expected tests/golden/render-fingerprint.rgb --diff /tmp/render-diff.ppm
```

Regenerate deliberately with `--write` only for intentional visual changes; upload the actual
and diff PPM artifacts from CI when reviewing a mismatch. Trigger on a PR by adding the
`render-golden` label.

### Known validation-layer workaround

Ubuntu's VVL 1.3.275 reports a submit-time `SYNC-HAZARD-PRESENT-AFTER-WRITE` for the
swapchain→present layout transition even though present waits on the render-finished
semaphore; upstream fixed this in [VVL #7480](https://github.com/KhronosGroup/Vulkan-ValidationLayers/pull/7480)
([issue #7479](https://github.com/KhronosGroup/Vulkan-ValidationLayers/issues/7479)). The
golden workflow sets `MOBA_IGNORE_KNOWN_PRESENT_HAZARD=1`, which suppresses **only** that exact
message from the error count/log. Every other validation error still fails the capture.
Remove the variable when the runner ships a newer layer.
