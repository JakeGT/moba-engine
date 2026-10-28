# Third-party notices

The engine's Rust dependencies are declared in the workspace `Cargo.toml`; `cargo-deny`
checks their licenses in CI. The following assets are embedded in source form.

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

The full repository license is dual MIT/Apache-2.0; see `LICENSE-MIT` and `LICENSE-APACHE`.
