// The depth convention (ADR-0023 §1c). Tiered builds define `MOBA_REVERSED_Z`: depth is
// cleared to 0, tested GREATER_OR_EQUAL, 1 at the near plane and 0 at the far plane. Compat
// (no define) keeps 0 near and 1 far, and these macros expand to its original expressions,
// so its SPIR-V is unchanged.
#ifndef MOBA_DEPTH_GLSL
#define MOBA_DEPTH_GLSL

#ifdef MOBA_REVERSED_Z
// The far plane's depth.
#define DEPTH_FAR 0.0
// Depth `d` is at (or beyond) the far plane: nothing was drawn there.
#define DEPTH_IS_FAR(d) ((d) <= 0.0)
// Depth `a` is nearer the camera than `b`.
#define DEPTH_NEARER(a, b) ((a) > (b))
// Depth `a` is at or behind `b`.
#define DEPTH_AT_OR_BEHIND(a, b) ((a) <= (b))
// Linear view depth of `d` for a perspective projection with planes `near` and `far`.
#define DEPTH_VIEW(d, near, far) ((near) * (far) / max((near) + (d) * ((far) - (near)), 1e-6))
#else
#define DEPTH_FAR 1.0
#define DEPTH_IS_FAR(d) ((d) >= 1.0)
#define DEPTH_NEARER(a, b) ((a) < (b))
#define DEPTH_AT_OR_BEHIND(a, b) ((a) >= (b))
#define DEPTH_VIEW(d, near, far) ((near) * (far) / max((far) - (d) * ((far) - (near)), 1e-6))
#endif

#endif
