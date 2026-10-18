# 0004 — Open-source distribution and extension surface

- **Status:** Accepted
- **Date:** 2026-10-17
- **Deciders:** engine maintainers

## Context

The engine is ready to be published as an open-source project that others can embed, extend
and build games or tools on. That requires more than code: licensing, a documented public
surface, optional dependency boundaries, examples and a release/stability policy. The
workspace is still pre-1.0 (and content was compile-time when this decision was written), so
the distribution surface must not pretend to be a frozen 1.0 API.

## Decision

1. **Dual MIT/Apache-2.0 licensing**, the Rust ecosystem convention. Contributions are inbound
   under the same terms; third-party notices are recorded in `THIRD_PARTY.md`.
2. **Layer-preserving crate graph**, documented in `docs/architecture.md` and enforced by CI.
   Headless users must be able to depend on simulation/networking without Vulkan.
3. **Umbrella crate `moba-engine`** with additive features:
   - `sim` (default): core, ECS, game, nav, simulation;
   - `net` (default): UDP/protocol/prediction (implies `sim`);
   - `render`: Vulkan renderer + winit platform, off by default for headless consumers.
   Individual crates stay public for narrower boundaries.
4. ~~**Content remains compile-time for 0.1.**~~ **Superseded:** runtime TOML content packs
   landed in M8; `moba-game` tables remain the default pack. See `docs/content-authoring.md`.
5. **Stability tiers** (foundation / engine / applications) with a pre-1.0 breakage policy,
   changelog discipline and wire-protocol version bumps. `cargo-semver-checks` is the intended
   release gate once crate names/ownership are settled.
6. **Publishing is opt-in** (`publish = false`) until names, repository URL and ownership are
   settled. Git tags are the release artifact today; the release checklist covers metadata,
   packaging, docs and semver checks.
7. **Examples and docs are part of the product**, not an afterthought: runnable embedding
   examples, architecture/content/API guides, contribution and security policies, issue/PR
   templates and a documentation CI gate.

## Consequences

- Downstream users get a small default dependency graph and a clear "not 1.0" signal.
- Maintaining feature combinations (`sim`, `net`, `render`) and documented feature checks adds
  CI time, but catches boundary regressions.
- Compile-time content limits moddability until M8; documenting it avoids users building on an
  API that cannot yet support runtime packs.
- Crate publishing remains a deliberate step rather than an accident.

## References

- Rust API guidelines: <https://rust-lang.github.io/api-guidelines/>
- Cargo semver: <https://doc.rust-lang.org/cargo/reference/semver.html>
- Bevy plugin/ecosystem guidance: <https://bevy.org/learn/quick-start/plugin-development/>
