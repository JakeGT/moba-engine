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
carry the repository's licence. AMD FidelityFX FSR is **not** included
([ADR-0015](docs/adr/0015-post-processing.md)).

The texture loader's CPU fallback and the baker's quality check decode BC blocks with
`bcdec_rs` (MIT), a Rust port of `bcdec`; the baker reads and writes PNG with `png`
(MIT OR Apache-2.0). See [ADR-0010](docs/adr/0010-texture-pipeline.md).

The full repository license is dual MIT/Apache-2.0; see `LICENSE-MIT` and `LICENSE-APACHE`.
