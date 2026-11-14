// The linear exposure multiplier the temporal upscaler reads (ADR-0021 §4a): one texel of
// `push.exposure × 2^(adapted EV)` (`push.exposure` alone without auto exposure), the value
// the composite applies (`composite.frag`). The adapted EV is the previous frame's: the
// exposure pass meters bloom's last mip, which is built from the upscaler's output.

[[vk::binding(0, 0)]]
Texture2D<float> adapted_ev;

[[vk::binding(1, 0)]]
[format("r32f")]
RWTexture2D<float> exposure_linear;

struct ExposurePush
{
    float exposure;
    uint auto_exposure;
};

[[vk::push_constant]]
ConstantBuffer<ExposurePush> push;

[shader("compute")]
[numthreads(1, 1, 1)]
void main(uint3 thread : SV_DispatchThreadID)
{
    float ev = push.auto_exposure != 0 ? adapted_ev.Load(int3(0, 0, 0)) : 0.0;
    float value = push.exposure * exp2(ev);
    exposure_linear[uint2(0, 0)] = isfinite(value) ? value : push.exposure;
}
