// Tile classification for the deferred lighting (ADR-0023 §2a, N2b): one 16 x 16 tile per
// workgroup over this frame's rendered sub-rectangle. Each pixel reads its depth and the
// model in `gb2`; the tile is then appended to one of three lists, and that list's
// `VkDispatchIndirectCommand` counts it:
// - **sky**: every pixel at the far plane (`deferred_light` skips it; the sky pass draws it);
// - **standard**: only `STANDARD` and `UNLIT` pixels (the fast permutation);
// - **extended**: any other model (the full permutation).
// Each tile also records its model mask and depth range for the debug views. The header is
// reset by the `mask_clear` pass before this one (`TileLayout::RESET`: counts 0, arguments
// (0, 0, 1)).
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
// counts, then, from `TILE_HEADER_WORDS`, the three lists of `capacity` packed tiles
// (`x | y << 16`) and the per-tile info (`capacity` triples: model mask, the far and the near
// depth as float bits).

import gbuffer;

[[vk::binding(0, 0)]] Texture2D<float> scene_depth;
[[vk::binding(1, 0)]] Texture2D<float4> gb2;
[[vk::binding(2, 0)]] RWStructuredBuffer<uint> tiles;

// `ClassifyPush` in crates/render/src/passes/deferred.rs (16 bytes).
struct ClassifyPush
{
    uint2 render_size; // this frame's rendered sub-rectangle
    uint tiles_x;      // tiles per row (the info's stride)
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
// Models the standard permutation shades: UNLIT and STANDARD.
static const uint STANDARD_MODELS = (1u << GBUFFER_MODEL_UNLIT) | (1u << GBUFFER_MODEL_STANDARD);

groupshared uint model_mask;
groupshared uint depth_min;
groupshared uint depth_max;

[shader("compute")]
[numthreads(16, 16, 1)]
void main(uint3 group : SV_GroupID, uint3 local : SV_GroupThreadID, uint index : SV_GroupIndex)
{
    if (index == 0)
    {
        model_mask = 0;
        depth_min = 0x7F800000; // +inf: the bits of non-negative floats order as the floats
        depth_max = 0;
    }
    GroupMemoryBarrierWithGroupSync();
    uint2 pixel = group.xy * TILE + local.xy;
    // A tile at the edge of the sub-rectangle reads only the pixels inside it (§2a).
    if (all(pixel < push.render_size))
    {
        float depth = scene_depth.Load(int3(pixel, 0));
        // Reversed-Z (ADR-0023 §1c): the far plane is 0.
        if (depth > 0.0)
        {
            uint model = unorm_bits(gb2.Load(int3(pixel, 0)).a, 255.0) >> 4;
            InterlockedOr(model_mask, 1u << model);
            uint bits = asuint(depth);
            InterlockedMin(depth_min, bits);
            InterlockedMax(depth_max, bits);
        }
    }
    GroupMemoryBarrierWithGroupSync();
    if (index != 0)
    {
        return;
    }
    uint mask = model_mask;
    uint tile_class = mask == 0 ? CLASS_SKY
                    : (mask & ~STANDARD_MODELS) == 0 ? CLASS_STANDARD : CLASS_EXTENDED;
    uint slot;
    InterlockedAdd(tiles[TILE_COUNT_WORD + tile_class], 1, slot);
    if (slot < push.capacity)
    {
        tiles[TILE_HEADER_WORDS + tile_class * push.capacity + slot] = group.x | (group.y << 16);
        uint row = max(push.tiles_x, 1u);
        InterlockedMax(tiles[tile_class * 3], min(slot + 1, row));
        InterlockedMax(tiles[tile_class * 3 + 1], slot / row + 1);
    }
    uint tile_index = group.y * push.tiles_x + group.x;
    if (tile_index < push.capacity)
    {
        // Reversed-Z: the near depth is the largest. As float bits, so the shader needs no
        // 16-bit types.
        uint info = TILE_HEADER_WORDS + 3 * push.capacity + 3 * tile_index;
        tiles[info] = mask;
        tiles[info + 1] = mask == 0 ? 0 : depth_min;
        tiles[info + 2] = mask == 0 ? 0 : depth_max;
    }
}
