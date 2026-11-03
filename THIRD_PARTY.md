# Third-party notices

The engine's Rust dependencies are declared in the workspace `Cargo.toml`; `cargo-deny`
checks their licenses in CI. The following assets are embedded in source form. The offline
font baker, `crates/tools/font-bake`, is a standalone crate with its own lockfile, like
`fuzz/`. Its dependencies (`fdsm` MIT, `skrifa` MIT OR Apache-2.0, and their `image`/`nalgebra`
trees) are never built by the engine workspace or linked into the client.

## 5×7 bitmap font

`crates/render/src/font_data.rs` contains the printable-ASCII portion of the Adafruit-GFX
5×7 font.

```text
Software License Agreement (BSD License)

Copyright (c) 2012 Adafruit Industries.  All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

- Redistributions of source code must retain the above copyright notice,
  this list of conditions and the following disclaimer.
- Redistributions in binary form must reproduce the above copyright
  notice, this list of conditions and the following disclaimer in the
  documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

## Open Sans Condensed SemiBold (HUD font)

`crates/tools/fonts/OpenSans-CondensedSemiBold.ttf` is Open Sans Condensed SemiBold, version
3.003, from the Open Sans project (<https://github.com/googlefonts/opensans>), unmodified. Its
licence ships next to it as `crates/tools/fonts/OFL.txt`. The `bake-ui-font` tool
(`cargo run --release --locked --manifest-path crates/tools/font-bake/Cargo.toml`) renders its printable-ASCII
glyphs into the multi-channel signed-distance-field atlas `crates/render/assets/ui-font.rgba`
and the metrics table `crates/render/src/font_metrics.rs`, which the client embeds. The font is
used under the SIL Open Font License 1.1, which has no Reserved Font Name for Open Sans.
Release archives ship this file, the licence as `OFL-OpenSans.txt` and the repository licences
next to the binaries (OFL §2):

```text
Copyright 2020 The Open Sans Project Authors (https://github.com/googlefonts/opensans)

-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font
creation efforts of academic and linguistic communities, and to
provide a free and open framework in which fonts may be shared and
improved in partnership with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply to
any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software
components as distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to,
deleting, or substituting -- in part or in whole -- any of the
components of the Original Version, by changing formats or by porting
the Font Software to a new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed,
modify, redistribute, and sell modified and unmodified copies of the
Font Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components, in
Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the
corresponding Copyright Holder. This restriction only applies to the
primary font name as presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created using
the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

## Sample hero model

`content/models/sentinel.glb` and its rigged, animated variant
`content/models/sentinel-skinned.glb` (and their copies in `fuzz/corpus/glb/`), and the map
meshes `content/models/crystal.glb` and `content/models/rampart.glb`, are not third-party
art. All are generated from code in this repository by
`cargo run -p moba-tools --bin sample-mesh` (`crates/tools/src/bin/sample-mesh.rs`), so they
carry the repository's MIT OR Apache-2.0 licence. Unit tests check that the committed bytes
match the generator.

The `.glb` loader uses the `gltf` crate family (MIT OR Apache-2.0) with default features
disabled; see [ADR-0007](docs/adr/0007-mesh-format.md).

## Mesh simplification (meshoptimizer)

The offline mesh LOD baker (`moba-tools mesh-bake lod`, `crates/tools/src/bin/mesh-bake.rs`)
simplifies meshes with the `meshopt` crate 0.7.0 (MIT OR Apache-2.0), which compiles and
statically links the bundled C++ [meshoptimizer](https://github.com/zeux/meshoptimizer)
library, version 1.3 (MIT, Copyright (C) 2016-2026 Arseny Kapoulkine), and its
dependency `float-cmp` (MIT). Only the tools crate links it, and only with its optional
`mesh-bake` feature: the client and renderer read the baked `models/<model>.lod` files and
never link meshoptimizer. The generated
`content/models/*.lod` files are derived from this repository's own sample meshes and carry
the repository's MIT OR Apache-2.0 licence. See [ADR-0018](docs/adr/0018-culling-lods-memory.md).

## Sample textures and material

`content/textures/sentinel-{base,normal,mr,mask}.ktx2` (and the copy in `fuzz/corpus/ktx2/`)
and `content/materials/sentinel.toml` are not third-party art. The textures are generated
procedurally by `moba-tools texture-bake generate sentinel`
(`crates/tools/src/bin/texture-bake/patterns.rs`) and baked by the same tool
(`scripts/bake-sample-textures.sh`), so they carry the repository's MIT OR Apache-2.0 licence.
A unit test checks that level 0 of each committed file matches the generator.

## VFX atlas and effects

`content/vfx/atlas.ktx2` and the effect descriptors in `content/vfx/` (and their copies in
`fuzz/corpus/vfx/`) are not third-party art either. The atlas is generated procedurally by
`moba_assets::procedural_atlas` (`crates/assets/src/vfx.rs`; the renderer's built-in atlas)
and baked to BC5 by `texture-bake` (`scripts/bake-sample-textures.sh`); the effects are
hand-written TOML. Both carry the repository's MIT OR Apache-2.0 licence. See
[ADR-0013](docs/adr/0013-vfx-runtime.md).

## Colour-grading LUTs and post-processing shaders

`content/grading/{ember,frost,meadow}.cube` (and `fuzz/corpus/grading/`) are generated by
`moba-tools grade-bake` from analytic grades (`crates/tools/src/bin/grade-bake/grade.rs`,
`scripts/bake-sample-textures.sh`) and carry the repository's MIT OR Apache-2.0 licence. The
`.cube` text format (Adobe/Resolve) is a public format; no reference LUTs are included.

The post-processing shaders (`crates/shaders/shaders/{bloom_down,bloom_up,exposure,gtao,
ao_blur,fxaa,upscale}.frag`) are this repository's own implementations of published
algorithms — the 13-tap bloom filter and Karis average (Jimenez, SIGGRAPH 2014), GTAO
(Jimenez et al. 2016), specular occlusion (Lagarde & de Rousiers 2014), FXAA 3.11's
edge search (Lottes 2011) and Catmull-Rom filtering — not copies of any reference source, and
carry the repository's licence. They include no AMD FidelityFX code
([ADR-0015](docs/adr/0015-post-processing.md)); the FSR 3.1 upscaler port is listed below.

The texture loader's CPU fallback and the baker's quality check decode BC blocks with
`bcdec_rs` (MIT), a Rust port of `bcdec`; the baker reads and writes PNG with `png`
(MIT OR Apache-2.0). See [ADR-0010](docs/adr/0010-texture-pipeline.md).

## AMD FidelityFX FSR 3.1.6 upscaler (`fsr3`, on by default)

`third_party/fidelityfx/` holds the GPU sources of the FSR 3.1.6 upscaler from the AMD
FidelityFX SDK v2.4.0 (tag `v2.4.0`, commit `7539ae8b6568ad29ed78a753404351daf3b5cca7`),
unmodified, with the SDK's `Kits/FidelityFX/docs/license.md` beside them and a SHA-256 manifest
([README](third_party/fidelityfx/README.md)). They are compiled into the open build's SPIR-V
(`moba-shaders`, feature `fsr3`), and `crates/render/src/fsr3/` is a Rust port of the SDK's
upscaler host code (`upscalers/fsr3/internal/ffx_fsr3upscaler.cpp`) and of the parts of its
Vulkan back end it uses (`backend/vk/ffx_vk.cpp`). The SDK licence's default terms are
binary-only, but it lists every one of these files under the MIT licence below, and that is
the licence they are used under ([ADR-0021](docs/adr/0021-temporal-upscaling-overlay.md)
amendment 1, [docs/dev/fsr3.md](docs/dev/fsr3.md)). Frame generation is not included.

```text
Copyright (C) Advanced Micro Devices, Inc. All rights reserved.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software
and associated documentation files (the "Software"), to deal in the Software without
restriction, including without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

## Vendor SDKs (optional cargo features, off by default)

Nothing in this section is in the repository or in the open build (default features). Each
SDK is fetched by `scripts/fetch-sdks.sh` into `target/vendor-sdks/` and pinned with its
licence file's SHA-256 in `vendor-sdks.toml`; FFI declarations are generated into `OUT_DIR` at
build time. A build that enables a feature carries that SDK's obligations, summarised here
from the pinned licence (the licence file itself is authoritative; [ADR-0020](docs/adr/0020-next-gen-foundations.md)
§6 and [docs/dev/vendor-sdks.md](docs/dev/vendor-sdks.md)). Public releases ship the open
build only ([RELEASE.md](RELEASE.md) step 8).

| Feature | SDK (pinned) | Licence file in the archive | Obligations when compiled in |
|---|---|---|---|
| `fsr3` (default) | none fetched: FSR 3.1.6's MIT sources are vendored in `third_party/fidelityfx/` (section above) | `third_party/fidelityfx/Kits/FidelityFX/docs/license.md` | the MIT notice above |
| `dlss` | NVIDIA DLSS SDK 310.9.16 | `LICENSE.txt` (NVIDIA RTX SDKs License) | object code only; NVIDIA GPUs only; **notify NVIDIA before any commercial release**; attribution and NVIDIA marks in the credits, splash screen and about box (supplement §7.1(b)), with NVIDIA's prior approval of the marks (§7.2(c)); NVIDIA may update or disable the feature over the air |
| `streamline` (`dlss-fg`) | NVIDIA Streamline 2.14.16 | `license.txt` (MIT) | MIT notice; `sl_nvperf` files fall under the Nsight Perf SDK License and are not shipped; the DLSS-G binaries it loads fall under the RTX SDKs License as for `dlss` |
| `nrd` | NVIDIA NRD 4.18.0 (with ShaderMake and MathLib, MIT) | `LICENSE.txt` (NVIDIA RTX SDKs License) | attribution in the credits (supplement §7.1(c)); stay interoperable with NVIDIA hardware; MIT notices for ShaderMake and MathLib |
| `rtxdi` | NVIDIA RTXDI 3.1.0 (RTXDI-Library) | `LICENSE.txt` (NVIDIA RTX SDKs License) | as for `nrd` |
| `sharc` | NVIDIA SHaRC 1.8.3 | `License.md` (NVIDIA RTX SDKs License) | as for `nrd` |
| `fsr4` | AMD FidelityFX SDK 2.4.0 (FSR 4) | `Kits/FidelityFX/docs/license.md` | **development-only**: binary-only redistribution without reverse engineering by default, though the signed DLLs are also listed under MIT; no distributed build enables it until ADR-0020 Q6 is settled |
| `xess` | Intel XeSS SDK 3.0.2 | `LICENSE.txt` (Intel Simplified Software License) | redistribute the binaries unmodified, with the notice; no reverse engineering, including at run time; the headers are under the same Intel licence (not MIT as ADR-0020 §6a says; amendment 6) |

The in-game credits do not exist yet; when a credits screen is added, it must show the notices
and marks above for every feature compiled in (tracked as a follow-up in
[docs/dev/vendor-sdks.md](docs/dev/vendor-sdks.md#notices-and-credits)).

The full repository license is dual MIT/Apache-2.0; see `LICENSE-MIT` and `LICENSE-APACHE`.
