//! Index-only mesh levels of detail (R8, ADR-0018).
//!
//! A level is a coarser **index list over the LOD0 vertices**: the vertex buffer, the skin,
//! every bone-LOD reduced skin and a VAT texture (all per LOD0 vertex) stay valid for it. The
//! offline baker (`moba-tools mesh-bake lod`, meshoptimizer) writes `models/<model>.lod` next
//! to `models/<model>.glb`; loaders attach it with [`MeshData::with_lods`] and a missing file
//! means "no levels".
//!
//! # File format (`MLOD`, version 1, little endian)
//!
//! ```text
//! b"MLOD"  u32 version = 1  u64 mesh_hash  u32 vertex_count  u32 level_count (1..=3)
//! level_count × { f32 error  u32 index_count }
//! the levels' indices, u32 each, in level order
//! ```
//!
//! `mesh_hash` is [`crate::mesh_hash`] of the mesh the levels were baked from (positions and
//! LOD0 indices), so a re-exported model never draws stale levels. `error` is the level's
//! object-space deviation in metres (model units), non-decreasing by level. Every level is a
//! non-empty triangle list with fewer indices than the previous one and every index below the
//! vertex count. Parsing is bounded ([`MAX_LOD_FILE_BYTES`]) and never panics.

use std::path::{Path, PathBuf};

use crate::mesh::MeshData;

/// Most baked levels below LOD0.
pub const MAX_MESH_LODS: usize = 3;
/// Largest `.lod` file accepted (the levels of a mesh at the index cap fit easily).
pub const MAX_LOD_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAGIC: &[u8; 4] = b"MLOD";
const VERSION: u32 = 1;
const HEADER: usize = 4 + 4 + 8 + 4 + 4;

/// One coarser level: its object-space error and its indices into the LOD0 vertices.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshLod {
    error: f32,
    indices: Vec<u32>,
}

impl MeshLod {
    /// Object-space deviation from LOD0, in model units (metres).
    #[must_use]
    pub fn error(&self) -> f32 {
        self.error
    }

    /// The level's triangle list over the LOD0 vertices.
    #[must_use]
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// Triangles in this level.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Why a level set was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LodError {
    /// The file is not an `MLOD` version 1 file, is truncated or has trailing bytes.
    Malformed(&'static str),
    /// The levels were baked from a different mesh (hash or vertex count).
    WrongMesh,
    /// A level breaks an invariant (see the module docs).
    Invalid(&'static str),
    /// Reading the file failed.
    Io(String),
}

impl core::fmt::Display for LodError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "malformed .lod file: {why}"),
            Self::WrongMesh => f.write_str(".lod file was baked from a different mesh"),
            Self::Invalid(why) => write!(f, "invalid LOD level: {why}"),
            Self::Io(error) => write!(f, "cannot read .lod file: {error}"),
        }
    }
}

impl std::error::Error for LodError {}

/// The baked levels of one mesh, in order (LOD1 first).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshLods {
    levels: Vec<MeshLod>,
}

impl MeshLods {
    /// Checks `levels` (`(error, indices)`, LOD1 first) against `mesh`.
    ///
    /// # Errors
    /// [`LodError::Invalid`] for more than [`MAX_MESH_LODS`] levels, an empty level, a level
    /// that is not a triangle list or does not shrink, an index past the vertex count, or a
    /// negative, non-finite or decreasing error.
    pub fn new(mesh: &MeshData, levels: Vec<(f32, Vec<u32>)>) -> Result<Self, LodError> {
        if levels.len() > MAX_MESH_LODS {
            return Err(LodError::Invalid("at most three levels below LOD0"));
        }
        let vertex_count = u32::try_from(mesh.vertex_count()).unwrap_or(u32::MAX);
        let mut previous_indices = mesh.indices().len();
        let mut previous_error = 0.0f32;
        let mut out = Vec::with_capacity(levels.len());
        for (error, indices) in levels {
            if indices.is_empty() || !indices.len().is_multiple_of(3) {
                return Err(LodError::Invalid(
                    "a level must be a non-empty triangle list",
                ));
            }
            if indices.len() >= previous_indices {
                return Err(LodError::Invalid(
                    "each level must have fewer indices than the previous",
                ));
            }
            if indices.iter().any(|&index| index >= vertex_count) {
                return Err(LodError::Invalid("an index names a missing vertex"));
            }
            if !error.is_finite() || error < previous_error {
                return Err(LodError::Invalid(
                    "errors must be finite, non-negative and non-decreasing",
                ));
            }
            previous_indices = indices.len();
            previous_error = error;
            out.push(MeshLod { error, indices });
        }
        Ok(Self { levels: out })
    }

    /// Parses an `MLOD` file and checks it against `mesh` (the mesh it must have been baked
    /// from).
    ///
    /// # Errors
    /// [`LodError::Malformed`] for a bad header, size or layout, [`LodError::WrongMesh`] when
    /// the hash or vertex count does not match, and [`Self::new`]'s errors.
    pub fn parse(bytes: &[u8], mesh: &MeshData) -> Result<Self, LodError> {
        if bytes.len() > MAX_LOD_FILE_BYTES {
            return Err(LodError::Malformed("file too large"));
        }
        if bytes.len() < HEADER || &bytes[..4] != MAGIC {
            return Err(LodError::Malformed("missing MLOD header"));
        }
        let u32_at = |offset: usize| -> Option<u32> {
            let slice = bytes.get(offset..offset + 4)?;
            Some(u32::from_le_bytes(slice.try_into().ok()?))
        };
        let version = u32_at(4).ok_or(LodError::Malformed("truncated header"))?;
        if version != VERSION {
            return Err(LodError::Malformed("unsupported version"));
        }
        let hash = bytes
            .get(8..16)
            .and_then(|slice| slice.try_into().ok())
            .map(u64::from_le_bytes)
            .ok_or(LodError::Malformed("truncated header"))?;
        let vertex_count = u32_at(16).ok_or(LodError::Malformed("truncated header"))?;
        let level_count = u32_at(20).ok_or(LodError::Malformed("truncated header"))? as usize;
        if level_count == 0 || level_count > MAX_MESH_LODS {
            return Err(LodError::Malformed("level count must be 1..=3"));
        }
        if hash != crate::mesh_hash(mesh)
            || usize::try_from(vertex_count).ok() != Some(mesh.vertex_count())
        {
            return Err(LodError::WrongMesh);
        }
        let mut counts = [(0.0f32, 0usize); MAX_MESH_LODS];
        let mut offset = HEADER;
        let mut total = 0usize;
        for slot in counts.iter_mut().take(level_count) {
            let error = u32_at(offset)
                .map(f32::from_bits)
                .ok_or(LodError::Malformed("truncated level table"))?;
            let count =
                u32_at(offset + 4).ok_or(LodError::Malformed("truncated level table"))? as usize;
            if count > MeshData::MAX_INDICES {
                return Err(LodError::Malformed("level too large"));
            }
            *slot = (error, count);
            total += count;
            offset += 8;
        }
        if bytes.len() != offset + total * 4 {
            return Err(LodError::Malformed(
                "index data does not match the level table",
            ));
        }
        let mut levels = Vec::with_capacity(level_count);
        for &(error, count) in &counts[..level_count] {
            let indices = bytes[offset..offset + count * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&chunk| u32::from_le_bytes(chunk))
                .collect();
            offset += count * 4;
            levels.push((error, indices));
        }
        Self::new(mesh, levels)
    }

    /// Serialises the levels for `mesh` (the mesh they were built for).
    #[must_use]
    pub fn to_bytes(&self, mesh: &MeshData) -> Vec<u8> {
        let total: usize = self.levels.iter().map(|level| level.indices.len()).sum();
        let mut out = Vec::with_capacity(HEADER + self.levels.len() * 8 + total * 4);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&crate::mesh_hash(mesh).to_le_bytes());
        out.extend_from_slice(
            &u32::try_from(mesh.vertex_count())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        out.extend_from_slice(
            &u32::try_from(self.levels.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        for level in &self.levels {
            out.extend_from_slice(&level.error.to_bits().to_le_bytes());
            out.extend_from_slice(
                &u32::try_from(level.indices.len())
                    .unwrap_or(u32::MAX)
                    .to_le_bytes(),
            );
        }
        for level in &self.levels {
            for index in &level.indices {
                out.extend_from_slice(&index.to_le_bytes());
            }
        }
        out
    }

    /// The levels, LOD1 first.
    #[must_use]
    pub fn levels(&self) -> &[MeshLod] {
        &self.levels
    }

    /// The levels, finest first, by value.
    #[must_use]
    pub fn into_levels(self) -> Vec<MeshLod> {
        self.levels
    }

    /// Whether there are no levels.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }
}

/// `<dir>/<stem>.lod` for a model at `<dir>/<stem>.glb`.
#[must_use]
pub fn lod_path(glb: &Path) -> PathBuf {
    glb.with_extension("lod")
}

/// Loads `<model>.lod` next to `glb` for `mesh`: `Ok(None)` when there is no file.
///
/// # Errors
/// Any [`LodError`] from reading or [`MeshLods::parse`].
pub fn load_lods_for(glb: &Path, mesh: &MeshData) -> Result<Option<MeshLods>, LodError> {
    let path = lod_path(glb);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(LodError::Io(error.to_string())),
    };
    MeshLods::parse(&bytes, mesh).map(Some)
}

#[cfg(feature = "glb")]
/// Loads `glb` and attaches its `.lod` levels when they exist and are valid. A bad `.lod` file
/// is reported in the second value and the mesh is returned without levels.
///
/// # Errors
/// The [`crate::GlbError`] of loading the model itself.
pub fn load_glb_file_with_lods(
    glb: &Path,
) -> Result<(MeshData, Option<LodError>), crate::GlbError> {
    let mesh = crate::load_glb_file(glb)?;
    Ok(match load_lods_for(glb, &mesh) {
        Ok(Some(lods)) => (mesh.with_lods(lods), None),
        Ok(None) => (mesh, None),
        Err(error) => (mesh, Some(error)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 grid of quads (9 vertices, 8 triangles).
    fn grid() -> MeshData {
        let mut positions = Vec::new();
        for z in 0..3 {
            for x in 0..3 {
                positions.push([x as f32, 0.0, z as f32]);
            }
        }
        let normals = vec![[0.0, 1.0, 0.0]; 9];
        let mut indices = Vec::new();
        for z in 0..2u32 {
            for x in 0..2u32 {
                let a = z * 3 + x;
                indices.extend_from_slice(&[a, a + 3, a + 1, a + 1, a + 3, a + 4]);
            }
        }
        MeshData::new(positions, normals, indices).expect("grid")
    }

    fn coarse() -> Vec<(f32, Vec<u32>)> {
        vec![(0.0, vec![0, 6, 2, 2, 6, 8])]
    }

    #[test]
    fn levels_round_trip_through_the_file_format() {
        let mesh = grid();
        let lods = MeshLods::new(&mesh, coarse()).expect("valid");
        let bytes = lods.to_bytes(&mesh);
        assert_eq!(&bytes[..4], b"MLOD");
        let parsed = MeshLods::parse(&bytes, &mesh).expect("parses");
        assert_eq!(parsed, lods);
        assert_eq!(parsed.levels()[0].triangle_count(), 2);
        let attached = mesh.clone().with_lods(parsed);
        assert_eq!(attached.lods().len(), 1);
    }

    #[test]
    fn levels_from_another_mesh_or_a_damaged_file_are_rejected() {
        let mesh = grid();
        let bytes = MeshLods::new(&mesh, coarse())
            .expect("valid")
            .to_bytes(&mesh);
        let mut other = mesh.positions().to_vec();
        other[4][1] = 0.5;
        let moved =
            MeshData::new(other, mesh.normals().to_vec(), mesh.indices().to_vec()).expect("mesh");
        assert_eq!(MeshLods::parse(&bytes, &moved), Err(LodError::WrongMesh));
        for cut in [0, 3, 10, HEADER, bytes.len() - 1] {
            assert!(MeshLods::parse(&bytes[..cut], &mesh).is_err(), "cut {cut}");
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(MeshLods::parse(&trailing, &mesh).is_err());
        let mut version = bytes.clone();
        version[4] = 2;
        assert!(MeshLods::parse(&version, &mesh).is_err());
        let mut index = bytes;
        let last = index.len() - 4;
        index[last..].copy_from_slice(&99u32.to_le_bytes());
        assert!(matches!(
            MeshLods::parse(&index, &mesh),
            Err(LodError::Invalid(_))
        ));
    }

    #[test]
    fn invalid_levels_are_rejected() {
        let mesh = grid();
        for levels in [
            vec![(0.0, vec![])],
            vec![(0.0, vec![0, 1])],
            vec![(0.0, mesh.indices().to_vec())],
            vec![(0.0, vec![0, 1, 9])],
            vec![(f32::NAN, vec![0, 1, 2])],
            vec![(-1.0, vec![0, 1, 2])],
            vec![(0.5, vec![0, 1, 2, 3, 4, 5]), (0.1, vec![0, 1, 2])],
            vec![(0.1, vec![0, 1, 2, 3, 4, 5]), (0.2, vec![0, 1, 2, 3, 4, 5])],
            vec![
                (0.1, vec![0; 18]),
                (0.2, vec![0; 15]),
                (0.3, vec![0; 12]),
                (0.4, vec![0; 9]),
            ],
        ] {
            assert!(MeshLods::new(&mesh, levels.clone()).is_err(), "{levels:?}");
        }
    }

    #[test]
    fn a_missing_lod_file_is_no_levels() {
        let mesh = grid();
        let path = std::env::temp_dir().join(format!(
            "moba-lod-missing-{}-{:?}.glb",
            std::process::id(),
            std::thread::current().id()
        ));
        assert_eq!(load_lods_for(&path, &mesh), Ok(None));
        assert_eq!(
            lod_path(Path::new("models/a.glb")),
            Path::new("models/a.lod")
        );
    }
}
