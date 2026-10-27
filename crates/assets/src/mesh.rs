//! Validated CPU-side static meshes.
//!
//! [`MeshData`] is the hand-off type between asset loaders and the renderer. Its invariants
//! are checked once, at construction. The renderer's indexed draws depend on them for memory
//! safety (it also re-checks index ranges at registration), so never add a constructor,
//! `Default`, `Deserialize` or mutable accessor that bypasses [`MeshData::new`]:
//!
//! - at least one triangle, at most [`MeshData::MAX_VERTICES`] vertices and
//!   [`MeshData::MAX_INDICES`] indices;
//! - one normal per position;
//! - a triangle list (`indices.len() % 3 == 0`) whose every index names a vertex;
//! - finite positions and normals.
//!
//! The fields are private so a mesh that violates them cannot be built outside this module.
//! A mesh may also carry a [`MeshSkin`] (WP-4.1): per-vertex joints and weights, one per
//! position, attached through [`MeshData::with_skin`], which checks the count.

use crate::skin::{MeshSkin, SkinError};

/// Why a mesh was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshError {
    /// No vertices or no triangles.
    Empty,
    /// More than [`MeshData::MAX_VERTICES`] vertices.
    TooManyVertices,
    /// More than [`MeshData::MAX_INDICES`] indices.
    TooManyIndices,
    /// Position and normal streams have different lengths.
    LengthMismatch,
    /// The index count is not a multiple of three.
    NotTriangles,
    /// An index names a vertex that does not exist.
    IndexOutOfRange,
    /// A position or normal component is NaN or infinite.
    NonFinite,
    /// The skin, skeleton or clip is invalid, or its streams do not match the vertex count.
    Skin(SkinError),
}

impl core::fmt::Display for MeshError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let text = match self {
            Self::Empty => "mesh has no triangles",
            Self::TooManyVertices => "mesh exceeds the vertex cap",
            Self::TooManyIndices => "mesh exceeds the index cap",
            Self::LengthMismatch => "position and normal counts differ",
            Self::NotTriangles => "index count is not a multiple of three",
            Self::IndexOutOfRange => "index out of range",
            Self::NonFinite => "non-finite vertex data",
            Self::Skin(error) => return write!(f, "skin: {error}"),
        };
        f.write_str(text)
    }
}

impl std::error::Error for MeshError {}

impl From<SkinError> for MeshError {
    fn from(error: SkinError) -> Self {
        Self::Skin(error)
    }
}

/// A validated triangle mesh: positions, normals, `u32` indices and an optional skin.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshData {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
    skin: Option<MeshSkin>,
}

impl MeshData {
    /// Maximum vertices per mesh (64 Ki). A hero-sized model needs a few thousand.
    pub const MAX_VERTICES: usize = 65_536;
    /// Maximum indices per mesh (128 Ki triangles).
    pub const MAX_INDICES: usize = 3 * 131_072;

    /// Builds a mesh after checking every invariant listed in the module docs.
    ///
    /// # Errors
    /// Returns the first violated invariant.
    pub fn new(
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        indices: Vec<u32>,
    ) -> Result<Self, MeshError> {
        if positions.is_empty() || indices.is_empty() {
            return Err(MeshError::Empty);
        }
        if positions.len() > Self::MAX_VERTICES {
            return Err(MeshError::TooManyVertices);
        }
        if indices.len() > Self::MAX_INDICES {
            return Err(MeshError::TooManyIndices);
        }
        if positions.len() != normals.len() {
            return Err(MeshError::LengthMismatch);
        }
        if !indices.len().is_multiple_of(3) {
            return Err(MeshError::NotTriangles);
        }
        // `positions.len() <= MAX_VERTICES`, so the count always fits u32.
        let vertex_count =
            u32::try_from(positions.len()).map_err(|_| MeshError::TooManyVertices)?;
        if indices.iter().any(|&index| index >= vertex_count) {
            return Err(MeshError::IndexOutOfRange);
        }
        if positions
            .iter()
            .chain(&normals)
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(MeshError::NonFinite);
        }
        Ok(Self {
            positions,
            normals,
            indices,
            skin: None,
        })
    }

    /// Attaches skinning streams. Positions and normals are then the bind pose.
    ///
    /// # Errors
    /// [`MeshError::Skin`] with [`SkinError::LengthMismatch`] unless the skin has exactly one
    /// joint/weight entry per vertex.
    pub fn with_skin(mut self, skin: MeshSkin) -> Result<Self, MeshError> {
        if skin.joints().len() != self.positions.len() {
            return Err(SkinError::LengthMismatch.into());
        }
        self.skin = Some(skin);
        Ok(self)
    }

    /// The skin, if the mesh is skinned.
    #[must_use]
    pub fn skin(&self) -> Option<&MeshSkin> {
        self.skin.as_ref()
    }

    /// Vertex positions in model space (metres, +Y up).
    #[must_use]
    pub fn positions(&self) -> &[[f32; 3]] {
        &self.positions
    }

    /// Per-vertex normals, one per position.
    #[must_use]
    pub fn normals(&self) -> &[[f32; 3]] {
        &self.normals
    }

    /// Triangle-list indices into [`Self::positions`].
    #[must_use]
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// Number of vertices.
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    /// Number of triangles.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Axis-aligned bounds `(min, max)` of the positions.
    #[must_use]
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for position in &self.positions {
            for axis in 0..3 {
                min[axis] = min[axis].min(position[axis]);
                max[axis] = max[axis].max(position[axis]);
            }
        }
        (min, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle() -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<u32>) {
        (
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0.0, 0.0, 1.0]; 3],
            vec![0, 1, 2],
        )
    }

    #[test]
    fn accepts_a_valid_triangle_and_reports_bounds() {
        let (positions, normals, indices) = triangle();
        let mesh = MeshData::new(positions, normals, indices).expect("valid");
        assert_eq!(mesh.vertex_count(), 3);
        assert_eq!(mesh.triangle_count(), 1);
        assert_eq!(mesh.bounds(), ([0.0, 0.0, 0.0], [1.0, 1.0, 0.0]));
    }

    #[test]
    fn rejects_each_broken_invariant() {
        let (positions, normals, _) = triangle();
        assert_eq!(
            MeshData::new(Vec::new(), Vec::new(), vec![0, 1, 2]),
            Err(MeshError::Empty)
        );
        assert_eq!(
            MeshData::new(positions.clone(), normals.clone(), Vec::new()),
            Err(MeshError::Empty)
        );
        assert_eq!(
            MeshData::new(positions.clone(), normals[..2].to_vec(), vec![0, 1, 2]),
            Err(MeshError::LengthMismatch)
        );
        assert_eq!(
            MeshData::new(positions.clone(), normals.clone(), vec![0, 1]),
            Err(MeshError::NotTriangles)
        );
        assert_eq!(
            MeshData::new(positions.clone(), normals.clone(), vec![0, 1, 3]),
            Err(MeshError::IndexOutOfRange)
        );
        let mut nan = positions.clone();
        nan[1][2] = f32::NAN;
        assert_eq!(
            MeshData::new(nan, normals.clone(), vec![0, 1, 2]),
            Err(MeshError::NonFinite)
        );
        let many = vec![[0.0; 3]; MeshData::MAX_VERTICES + 1];
        assert_eq!(
            MeshData::new(many.clone(), many, vec![0, 1, 2]),
            Err(MeshError::TooManyVertices)
        );
        assert_eq!(
            MeshData::new(positions, normals, vec![0; MeshData::MAX_INDICES + 3]),
            Err(MeshError::TooManyIndices)
        );
    }
}
