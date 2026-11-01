// Toolchain smoke module (ADR-0020 §4c, N0): exercises the Slang build end to end (import from
// `lib/`, explicit bindings, `slangc -reflection-json`, `spirv-val`). The renderer does not
// create it; the run-time smoke passes (`bindless_smoke`, `rt_smoke`) arrive with bindless
// and the capability gate.
//
// It copies the frame's vision circles into a buffer, so the frame-constants block is a real,
// reflected parameter of the module.
import frame_constants;

[[vk::binding(0, 0)]]
ConstantBuffer<FrameConstants> camera;

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
}
