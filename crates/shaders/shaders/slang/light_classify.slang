// Tile classification for the deferred lighting (ADR-0023 §2a, N2b): one 16 x 16 tile per
// workgroup over this frame's rendered sub-rectangle. Each pixel reads its depth and the
// model in `gb2`, the models are ORed across each subgroup and then the workgroup, and the
// tile is classed:
// - **sky**: every pixel at the far plane: counted nowhere (`deferred_light` never sees it;
//   the G-buffer pass drew the sky);
// - **standard**: only `UNLIT`, `STANDARD`, `FOLIAGE` and `TRANSMISSIVE_FALLBACK` pixels
//   (the fast permutation; `FOLIAGE` joined it in N2c);
// - **extended**: any other model (the full permutation with the models' lobes).
// A lit tile is appended to its class's list, and that class's `VkDispatchIndirectCommand`
// counts it. The header is reset by the `mask_clear` pass before this one
// (`TileLayout::RESET`: counts 0, arguments (0, 0, 1)).
//
// The arguments are two-dimensional, rows of `tiles_x` groups: a class can hold more tiles
// than `maxComputeWorkGroupCount[0]` guarantees (65,535: 79,712 at 6016 x 3384), while
// `tiles_x` and the row count stay within it. Each appended tile raises its class's `x` to
// min(slot + 1, tiles_x) and `y` to slot / tiles_x + 1 with atomic maxima, so after the pass
// they are min(n, tiles_x) and ceil(n / tiles_x) for n tiles; `deferred_light` skips the
// groups of the last row past n.
//
// `TileLayout` in crates/render/src/passes/deferred.rs mirrors the buffer's layout:
// words 0-8 the sky, standard and extended dispatch arguments, 9-11 the three classes' tile
// counts (the sky's stay at their reset values), then, from `TILE_HEADER_WORDS`, the standard
// and the extended lists of `capacity` packed tiles (`x | y << 16`).

import gbuffer;

[[vk::binding(0, 0)]] Texture2D<float> scene_depth;
[[vk::binding(1, 0)]] Texture2D<float4> gb2;
[[vk::binding(2, 0)]] RWStructuredBuffer<uint> tiles;

// `ClassifyPush` in crates/render/src/passes/deferred.rs (16 bytes).
struct ClassifyPush
{
    uint2 render_size; // this frame's rendered sub-rectangle
    uint tiles_x;      // tiles per row (the dispatch's row width)
    uint capacity;     // tiles each list holds
};

[[vk::push_constant]]
ConstantBuffer<ClassifyPush> push;

static const uint TILE = 16;
static const uint TILE_HEADER_WORDS = 12;
static const uint TILE_COUNT_WORD = 9;
static const uint CLASS_SKY = 0;
static const uint CLASS_STANDARD = 1;
static const uint CLASS_EXTENDED = 2;
// Models the standard permutation shades: UNLIT, STANDARD, FOLIAGE (its back light) and
// TRANSMISSIVE_FALLBACK (lit as STANDARD); N2c, ADR-0023 amendment 3.
static const uint STANDARD_MODELS = (1u << GBUFFER_MODEL_UNLIT) | (1u << GBUFFER_MODEL_STANDARD)
                                  | (1u << GBUFFER_MODEL_FOLIAGE)
                                  | (1u << GBUFFER_MODEL_TRANSMISSIVE_FALLBACK);

groupshared uint model_mask;

// `WaveActiveBitOr`: subgroup arithmetic, which the renderer checks before it builds this
// pass (`deferred::unsupported`).
[require(spvGroupNonUniform, spvGroupNonUniformArithmetic)]
[shader("compute")]
[numthreads(16, 16, 1)]
void main(uint3 group : SV_GroupID, uint3 local : SV_GroupThreadID, uint index : SV_GroupIndex)
{
    if (index == 0)
    {
        model_mask = 0;
    }
    GroupMemoryBarrierWithGroupSync();
    uint2 pixel = group.xy * TILE + local.xy;
    uint bit = 0;
    // A tile at the edge of the sub-rectangle reads only the pixels inside it (§2a).
    if (all(pixel < push.render_size))
    {
        // Reversed-Z (ADR-0023 §1c): the far plane is 0.
        if (scene_depth.Load(int3(pixel, 0)) > 0.0)
        {
            bit = 1u << (unorm_bits(gb2.Load(int3(pixel, 0)).a, 255.0) >> 4);
        }
    }
    // One shared atomic per subgroup rather than per pixel.
    uint subgroup_mask = WaveActiveBitOr(bit);
    if (WaveIsFirstLane() && subgroup_mask != 0)
    {
        InterlockedOr(model_mask, subgroup_mask);
    }
    GroupMemoryBarrierWithGroupSync();
    if (index != 0)
    {
        return;
    }
    uint mask = model_mask;
    uint tile_class = mask == 0 ? CLASS_SKY
                    : (mask & ~STANDARD_MODELS) == 0 ? CLASS_STANDARD : CLASS_EXTENDED;
    if (tile_class == CLASS_SKY)
    {
        return;
    }
    uint slot;
    InterlockedAdd(tiles[TILE_COUNT_WORD + tile_class], 1, slot);
    if (slot < push.capacity)
    {
        tiles[TILE_HEADER_WORDS + (tile_class - 1) * push.capacity + slot] =
            group.x | (group.y << 16);
        uint row = max(push.tiles_x, 1u);
        InterlockedMax(tiles[tile_class * 3], min(slot + 1, row));
        InterlockedMax(tiles[tile_class * 3 + 1], slot / row + 1);
    }
}
