// Toolchain smoke module (ADR-0020 §4c, N0): exercises the Slang build end to end (import from
// `lib/`, explicit bindings, `slangc -reflection-json`, `spirv-val`). The renderer does not
// create it; the run-time smoke passes (`bindless_smoke`, `rt_smoke`) arrive with bindless
// and the capability gate.
//
// It copies the frame's vision circles into `circles[0..16)` and writes
// `mul(view_proj, camera_pos)` to `circles[16]`, so the frame-constants block is a real,
// reflected parameter, and a GPU run (`moba_render::slang_smoke`) checks the data path and
// the matrix convention (column-major, `mul(M, v)` == glam `M * v`) end to end.
import frame_constants;

[[vk::binding(0, 0)]]
ConstantBuffer<FrameConstants> camera;

/// `MOBA_VISION_CIRCLES + 1` entries.
[[vk::binding(1, 0)]]
RWStructuredBuffer<float4> circles;

[shader("compute")]
[numthreads(16, 1, 1)]
void main(uint3 thread : SV_DispatchThreadID)
{
    uint index = thread.x;
    if (index < min(camera.vision_count.x, MOBA_VISION_CIRCLES))
    {
        circles[index] = camera.vision_circles[index];
    }
    if (index == 0)
    {
        circles[MOBA_VISION_CIRCLES] = mul(camera.view_proj, camera.camera_pos);
    }
}
