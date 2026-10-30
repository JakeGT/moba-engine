//! The map's terrain on the GPU (R3a, ADR-0014): the heightfield mesh, the splat and layer
//! texture arrays, and the terrain block of set 3.
//!
//! Built at map load by [`Renderer::set_terrain`](crate::Renderer::set_terrain) (a device-idle
//! wait and staging uploads), never per frame. Validation is pure and unit-tested; any
//! problem is an [`RenderError::InvalidTerrain`] and the previous terrain stays.

use moba_assets::{TextureData, TextureFormat};

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::lighting::TerrainBlock;
use crate::mesh::{self, CpuMesh, GpuMesh, Vertex};
use crate::resources::GpuImage;
use crate::texture;
use crate::world::{MAX_TERRAIN_LAYERS, TerrainData, WorldConfig};

/// Largest heightfield side in samples (1024 cells).
pub(crate) const MAX_TERRAIN_RESOLUTION: u32 = 1025;
/// Largest height magnitude accepted, metres.
const MAX_TERRAIN_HEIGHT: f32 = 1_000.0;

/// An uploaded terrain.
pub(crate) struct TerrainGpu {
    pub(crate) mesh: GpuMesh,
    /// The mesh's culling chunks (R8).
    pub(crate) chunks: Vec<TerrainChunk>,
    pub(crate) splat: GpuImage,
    pub(crate) albedo: GpuImage,
    pub(crate) normal: Option<GpuImage>,
    pub(crate) block: TerrainBlock,
}

/// Checks `data` without touching the device.
pub(crate) fn validate(data: &TerrainData<'_>) -> RenderResult<()> {
    let resolution = data.resolution;
    if !(2..=MAX_TERRAIN_RESOLUTION).contains(&resolution) {
        return Err(RenderError::InvalidTerrain(
            "the heightfield needs 2..=1025 samples per side",
        ));
    }
    let samples = (resolution as usize) * (resolution as usize);
    if data.heights.len() != samples {
        return Err(RenderError::InvalidTerrain(
            "heights must hold resolution² samples",
        ));
    }
    if !data.occlusion.is_empty() && data.occlusion.len() != samples {
        return Err(RenderError::InvalidTerrain(
            "occlusion must be empty or hold resolution² samples",
        ));
    }
    let spacing_ok = data.spacing.is_finite() && data.spacing > 0.0 && data.spacing < 1_000.0;
    let origin_ok = data
        .origin
        .iter()
        .all(|value| value.is_finite() && value.abs() < 1.0e6);
    if !spacing_ok || !origin_ok {
        return Err(RenderError::InvalidTerrain(
            "spacing and origin must be finite and in range",
        ));
    }
    if data
        .heights
        .iter()
        .chain(data.occlusion)
        .any(|value| !value.is_finite() || value.abs() > MAX_TERRAIN_HEIGHT)
    {
        return Err(RenderError::InvalidTerrain(
            "heights and occlusion must be finite and within 1000 m",
        ));
    }
    if data.albedo.is_empty() || data.albedo.len() > MAX_TERRAIN_LAYERS {
        return Err(RenderError::InvalidTerrain("a terrain needs 1..=8 layers"));
    }
    if !data.normal.is_empty() && data.normal.len() != data.albedo.len() {
        return Err(RenderError::InvalidTerrain(
            "normal maps must be absent or one per layer",
        ));
    }
    if data.splat.is_empty() || data.splat.len() > 2 {
        return Err(RenderError::InvalidTerrain(
            "a terrain needs one or two splat maps",
        ));
    }
    let colour = |format| matches!(format, TextureFormat::Bc7Srgb | TextureFormat::Rgba8Srgb);
    let normal = |format| matches!(format, TextureFormat::Bc5Unorm | TextureFormat::Rg8Unorm);
    let splat = |format| matches!(format, TextureFormat::Bc7Unorm | TextureFormat::Rgba8Unorm);
    check_set(
        data.albedo,
        colour,
        "layer albedo must be sRGB colour of one size and format",
    )?;
    if !data.normal.is_empty() {
        check_set(
            data.normal,
            normal,
            "layer normals must be BC5/RG8 of one size and format",
        )?;
    }
    check_set(
        data.splat,
        splat,
        "splat maps must be linear RGBA of one size and format",
    )?;
    if data
        .tiling
        .iter()
        .any(|tiling| !(tiling.is_finite() && (0.05..=1_000.0).contains(tiling)))
        || data
            .tints
            .iter()
            .flatten()
            .any(|channel| !(channel.is_finite() && (0.0..=8.0).contains(channel)))
    {
        return Err(RenderError::InvalidTerrain(
            "layer tiling must be 0.05..=1000 m and tints 0..=8",
        ));
    }
    Ok(())
}

/// Every texture is 2D, accepted by `format`, and shares the first one's format, size and
/// level count (they become one array).
fn check_set(
    textures: &[TextureData],
    format: impl Fn(TextureFormat) -> bool,
    reason: &'static str,
) -> RenderResult<()> {
    let first = &textures[0];
    if textures.iter().any(|texture| {
        texture.is_cube()
            || !format(texture.format())
            || texture.format() != first.format()
            || texture.width() != first.width()
            || texture.height() != first.height()
            || texture.level_count() != first.level_count()
    }) {
        return Err(RenderError::InvalidTerrain(reason));
    }
    Ok(())
}

/// The heightfield as an indexed grid: positions in world space, normals from central
/// differences, the relief occlusion in `uv.x` (1 without occlusion). Two triangles per
/// cell, split along the diagonal that follows the surface's fold, emitted chunk by chunk
/// (R8), with each chunk's index range and AABB.
pub(crate) fn grid_mesh(data: &TerrainData<'_>) -> (CpuMesh, Vec<TerrainChunk>) {
    let n = data.resolution as usize;
    let height = |i: usize, j: usize| data.heights[j * n + i];
    let spacing = data.spacing;
    let mut vertices = Vec::with_capacity(n * n);
    for j in 0..n {
        for i in 0..n {
            let left = height(i.saturating_sub(1), j);
            let right = height((i + 1).min(n - 1), j);
            let down = height(i, j.saturating_sub(1));
            let up = height(i, (j + 1).min(n - 1));
            let span_x = spacing * ((i + 1).min(n - 1) - i.saturating_sub(1)) as f32;
            let span_z = spacing * ((j + 1).min(n - 1) - j.saturating_sub(1)) as f32;
            let gradient = [(right - left) / span_x, (up - down) / span_z];
            let normal = [-gradient[0], 1.0, -gradient[1]];
            let length = (normal[0] * normal[0] + 1.0 + normal[2] * normal[2]).sqrt();
            let occlusion = data.occlusion.get(j * n + i).copied().unwrap_or(1.0);
            vertices.push(Vertex {
                position: [
                    data.origin[0] + i as f32 * spacing,
                    height(i, j),
                    data.origin[1] + j as f32 * spacing,
                ],
                normal: normal.map(|component| component / length),
                uv: [occlusion.clamp(0.0, 1.0), 0.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
            });
        }
    }
    let cells = n - 1;
    let mut indices = Vec::with_capacity(cells * cells * 6);
    let index = |i: usize, j: usize| u32::try_from(j * n + i).expect("1025² fits u32");
    // R8 (ADR-0018): the cells are emitted chunk by chunk (row-major chunks of
    // `TERRAIN_CHUNK_CELLS`², row-major cells inside), so each chunk is one contiguous index
    // range and a row of visible chunks merges into one draw.
    let chunk_side = cells.div_ceil(TERRAIN_CHUNK_CELLS);
    let mut chunks = Vec::with_capacity(chunk_side * chunk_side);
    for chunk_j in 0..chunk_side {
        for chunk_i in 0..chunk_side {
            let first = indices.len();
            let (mut low, mut high) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            let cells_j =
                chunk_j * TERRAIN_CHUNK_CELLS..((chunk_j + 1) * TERRAIN_CHUNK_CELLS).min(cells);
            let cells_i =
                chunk_i * TERRAIN_CHUNK_CELLS..((chunk_i + 1) * TERRAIN_CHUNK_CELLS).min(cells);
            for j in cells_j.clone() {
                for i in cells_i.clone() {
                    let (corner00, corner10, corner11, corner01) = (
                        index(i, j),
                        index(i + 1, j),
                        index(i + 1, j + 1),
                        index(i, j + 1),
                    );
                    // Split along the shorter height difference, so ridges stay ridges.
                    let diagonal_ac = (height(i, j) - height(i + 1, j + 1)).abs();
                    let diagonal_bd = (height(i + 1, j) - height(i, j + 1)).abs();
                    if diagonal_ac <= diagonal_bd {
                        indices.extend_from_slice(&[
                            corner00, corner01, corner11, corner00, corner11, corner10,
                        ]);
                    } else {
                        indices.extend_from_slice(&[
                            corner00, corner01, corner10, corner10, corner01, corner11,
                        ]);
                    }
                }
            }
            // The chunk's AABB over its vertices (cells i..=i_end, j..=j_end).
            for j in cells_j.start..=cells_j.end {
                for i in cells_i.start..=cells_i.end {
                    let position = vertices[j * n + i].position;
                    for axis in 0..3 {
                        low[axis] = low[axis].min(position[axis]);
                        high[axis] = high[axis].max(position[axis]);
                    }
                }
            }
            chunks.push(TerrainChunk {
                first_index: u32::try_from(first).expect("1024² cells fit u32"),
                index_count: u32::try_from(indices.len() - first).expect("fits u32"),
                min: low,
                max: high,
            });
        }
    }
    let lods = crate::mesh::MeshLodTable::single(
        u32::try_from(indices.len()).expect("1024² cells fit u32"),
    );
    (
        CpuMesh {
            vertices,
            indices,
            has_uvs: false,
            lods,
        },
        chunks,
    )
}

/// Cells per side of one terrain culling chunk (R8, ADR-0018 §1).
pub(crate) const TERRAIN_CHUNK_CELLS: usize = 32;
/// Most chunks a terrain has (1024 cells per side).
pub(crate) const MAX_TERRAIN_CHUNKS: usize =
    ((MAX_TERRAIN_RESOLUTION as usize - 1).div_ceil(TERRAIN_CHUNK_CELLS)).pow(2);

/// One chunk's contiguous index range and world-space AABB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TerrainChunk {
    pub(crate) first_index: u32,
    pub(crate) index_count: u32,
    pub(crate) min: [f32; 3],
    pub(crate) max: [f32; 3],
}

/// The terrain block for `data` under `config`.
pub(crate) fn block(data: &TerrainData<'_>, config: WorldConfig, normals: bool) -> TerrainBlock {
    let size = data.size().max(1e-3);
    let layers = (data.albedo.len() as u32).min(config.clamped_layers());
    let mut tiling = [[0.0; 4]; 2];
    let mut tint = [[0.0; 4]; 8];
    for layer in 0..MAX_TERRAIN_LAYERS {
        tiling[layer / 4][layer % 4] = 1.0 / data.tiling[layer];
        let [r, g, b] = data.tints[layer];
        tint[layer] = [r, g, b, 0.0];
    }
    TerrainBlock {
        rect: [data.origin[0], data.origin[1], 1.0 / size, 1.0 / size],
        params: [
            layers as f32,
            f32::from(u8::from(normals)),
            f32::from(u8::from(config.terrain_triplanar)),
            1.0,
        ],
        splat: [data.splat.len() as f32, 0.0, 0.0, 0.0],
        tiling,
        tint,
    }
}

fn refs(textures: &[TextureData]) -> Vec<&TextureData> {
    textures.iter().collect()
}

/// Validates and uploads `data`: the mesh and the three arrays (BC layers decoded on the CPU
/// unless `compressed`). Load-time only.
pub(crate) fn upload(
    device: &DeviceContext,
    data: &TerrainData<'_>,
    config: WorldConfig,
    compressed: bool,
) -> RenderResult<TerrainGpu> {
    validate(data)?;
    let (cpu, chunks) = grid_mesh(data);
    let mesh = mesh::upload(device, &cpu)?;
    let splat = texture::upload_array(device, "terrain-splat", &refs(data.splat), compressed)?;
    let albedo = texture::upload_array(device, "terrain-albedo", &refs(data.albedo), compressed)?;
    let normal = if config.terrain_normal_maps && !data.normal.is_empty() {
        Some(texture::upload_array(
            device,
            "terrain-normal",
            &refs(data.normal),
            compressed,
        )?)
    } else {
        None
    };
    let block = block(data, config, normal.is_some());
    Ok(TerrainGpu {
        mesh,
        chunks,
        splat,
        albedo,
        normal,
        block,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(format: TextureFormat, size: u32) -> TextureData {
        let bytes = format.level_bytes(size, size).expect("size");
        TextureData::new(format, size, size, &[vec![128; bytes]]).expect("texture")
    }

    #[test]
    fn grid_mesh_follows_the_heightfield() {
        // A ramp rising 1 m per sample along +x over a 3x3 grid.
        let heights = [0.0, 1.0, 2.0, 0.0, 1.0, 2.0, 0.0, 1.0, 2.0];
        let splat = [rgba(TextureFormat::Rgba8Unorm, 4)];
        let albedo = [rgba(TextureFormat::Rgba8Srgb, 4)];
        let data = TerrainData::new(3, [-1.0, -1.0], 1.0, &heights, &splat, &albedo);
        validate(&data).expect("valid");
        let (mesh, chunks) = grid_mesh(&data);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].index_count as usize, mesh.indices.len());
        assert_eq!(chunks[0].min, [-1.0, 0.0, -1.0]);
        assert_eq!(chunks[0].max, [1.0, 2.0, 1.0]);
        assert_eq!(mesh.vertices.len(), 9);
        assert_eq!(mesh.indices.len(), 2 * 2 * 6);
        let centre = mesh.vertices[4];
        assert_eq!(centre.position, [0.0, 1.0, 0.0]);
        // Normal tilts against the slope: (-1, 1, 0) normalised.
        assert!((centre.normal[0] + core::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
        assert!(centre.normal[2].abs() < 1e-6);
        assert_eq!(centre.uv[0], 1.0, "no occlusion: fully open");
        // Every triangle winds counter-clockwise seen from above (+y normal).
        for triangle in mesh.indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|k| mesh.vertices[triangle[k] as usize].position);
            let ab = [b[0] - a[0], b[2] - a[2]];
            let ac = [c[0] - a[0], c[2] - a[2]];
            // y of (ab x ac) in xz with y up: ab.z * ac.x - ab.x * ac.z.
            assert!(ab[1] * ac[0] - ab[0] * ac[1] > 0.0, "{triangle:?}");
        }
    }

    /// R8: a 70×70-cell grid splits into 3×3 chunks that tile the index buffer in order,
    /// keep every cell's two triangles, and bound their own vertices.
    #[test]
    fn grid_chunks_tile_the_index_buffer_and_bound_their_cells() {
        let resolution = 71u32;
        let heights: Vec<f32> = (0..resolution * resolution)
            .map(|k| ((k % resolution) as f32 * 0.3).sin() + (k / resolution) as f32 * 0.01)
            .collect();
        let splat = [rgba(TextureFormat::Rgba8Unorm, 4)];
        let albedo = [rgba(TextureFormat::Rgba8Srgb, 4)];
        let data = TerrainData::new(resolution, [0.0, 0.0], 2.0, &heights, &splat, &albedo);
        let (mesh, chunks) = grid_mesh(&data);
        assert_eq!(chunks.len(), 9);
        assert_eq!(mesh.indices.len(), 70 * 70 * 6);
        let mut next = 0;
        for chunk in &chunks {
            assert_eq!(chunk.first_index, next, "contiguous, in order");
            next += chunk.index_count;
            for &index in &mesh.indices[chunk.first_index as usize..next as usize] {
                let position = mesh.vertices[index as usize].position;
                for ((value, min), max) in position.iter().zip(chunk.min).zip(chunk.max) {
                    assert!(*value >= min && *value <= max);
                }
            }
        }
        assert_eq!(next as usize, mesh.indices.len());
        // The corner chunk holds 32×32 cells, the last column 6×32.
        assert_eq!(chunks[0].index_count, 32 * 32 * 6);
        assert_eq!(chunks[2].index_count, 6 * 32 * 6);
        const { assert!(MAX_TERRAIN_CHUNKS >= 32 * 32) };
    }

    #[test]
    fn invalid_terrains_are_rejected_before_any_upload() {
        let heights = [0.0; 4];
        let splat = [rgba(TextureFormat::Rgba8Unorm, 4)];
        let albedo = [rgba(TextureFormat::Rgba8Srgb, 4)];
        let ok = TerrainData::new(2, [0.0, 0.0], 1.0, &heights, &splat, &albedo);
        validate(&ok).expect("valid");
        let bad_heights = [f32::NAN; 4];
        let linear_albedo = [rgba(TextureFormat::Rgba8Unorm, 4)];
        let mixed = [
            rgba(TextureFormat::Rgba8Srgb, 4),
            rgba(TextureFormat::Rgba8Srgb, 8),
        ];
        let normals = [rgba(TextureFormat::Rgba8Unorm, 4)];
        for data in [
            TerrainData::new(1, [0.0, 0.0], 1.0, &heights[..1], &splat, &albedo),
            TerrainData::new(3, [0.0, 0.0], 1.0, &heights, &splat, &albedo),
            TerrainData::new(2, [0.0, 0.0], 0.0, &heights, &splat, &albedo),
            TerrainData::new(2, [0.0, 0.0], 1.0, &bad_heights, &splat, &albedo),
            TerrainData::new(2, [0.0, 0.0], 1.0, &heights, &[], &albedo),
            TerrainData::new(2, [0.0, 0.0], 1.0, &heights, &splat, &[]),
            TerrainData::new(2, [0.0, 0.0], 1.0, &heights, &splat, &linear_albedo),
            TerrainData::new(2, [0.0, 0.0], 1.0, &heights, &splat, &mixed),
            ok.with_normals(&normals),
            ok.with_layer_style(0, [1.0; 3], 0.0),
            ok.with_occlusion(&heights[..2]),
        ] {
            assert!(
                matches!(validate(&data), Err(RenderError::InvalidTerrain(_))),
                "{data:?}"
            );
        }
    }

    #[test]
    fn the_block_caps_layers_by_tier_and_inverts_tiling() {
        let heights = [0.0; 4];
        let splat = [rgba(TextureFormat::Rgba8Unorm, 4)];
        let albedo: Vec<TextureData> = (0..6).map(|_| rgba(TextureFormat::Rgba8Srgb, 4)).collect();
        let data = TerrainData::new(2, [-10.0, -20.0], 40.0, &heights, &splat, &albedo)
            .with_layer_style(5, [0.5, 0.6, 0.7], 4.0);
        let low = block(&data, WorldConfig::for_tier(crate::QualityTier::Low), false);
        assert_eq!(low.params[0], 4.0, "Low samples four layers");
        assert_eq!(low.params[1], 0.0);
        let high = block(&data, WorldConfig::for_tier(crate::QualityTier::High), true);
        assert_eq!(high.params[..3], [6.0, 1.0, 1.0]);
        assert_eq!(high.rect, [-10.0, -20.0, 1.0 / 40.0, 1.0 / 40.0]);
        assert_eq!(high.tiling[1][1], 0.25);
        assert_eq!(high.tint[5][..3], [0.5, 0.6, 0.7]);
    }

    /// Review fix: an authored splat without `splat2` is a one-layer array. The block tells
    /// the shader so, which then never samples array layer 1 (it would clamp to layer 0 and
    /// double the base layer's weight); with two maps it samples both.
    #[test]
    fn the_block_counts_splat_layers_so_one_map_never_reads_layer_one() {
        let heights = [0.0; 4];
        let albedo: Vec<TextureData> = (0..8).map(|_| rgba(TextureFormat::Rgba8Srgb, 4)).collect();
        let high = WorldConfig::for_tier(crate::QualityTier::High);
        let one = [rgba(TextureFormat::Rgba8Unorm, 4)];
        let data = TerrainData::new(2, [0.0, 0.0], 10.0, &heights, &one, &albedo);
        assert_eq!(block(&data, high, false).splat[0], 1.0);
        let two = [
            rgba(TextureFormat::Rgba8Unorm, 4),
            rgba(TextureFormat::Rgba8Unorm, 4),
        ];
        let data = TerrainData::new(2, [0.0, 0.0], 10.0, &heights, &two, &albedo);
        assert_eq!(block(&data, high, false).splat[0], 2.0);
        let source = include_str!("../../shaders/shaders/terrain.frag");
        let guard = source
            .find("terrain_block.splat.x > 1.5")
            .expect("the guard");
        let read = source
            .find("texture(terrain_splat, vec3(splat_uv, 1.0))")
            .expect("layer 1");
        assert!(guard < read, "layer 1 is sampled only behind the guard");
    }
}
