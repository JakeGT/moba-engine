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
//! position, attached through [`MeshData::with_skin`], which checks the count; and texture
//! coordinates (`TEXCOORD_0`, ADR-0010), one finite pair per position, attached through
//! [`MeshData::with_uvs`]; and tangents (glTF `TANGENT`, R2a), one finite vector with a
//! `±1` handedness per position, attached through [`MeshData::with_tangents`] or generated from
//! the UVs with [`MeshData::with_generated_tangents`]. See [`MeshData`] for the tangent
//! convention.

use crate::skin::{MeshSkin, SkinError};

/// Why a mesh was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
    /// A tangent's handedness (`w`) is not `+1` or `-1`.
    BadTangent,
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
            Self::BadTangent => "tangent handedness (w) must be +1 or -1",
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

/// A validated triangle mesh: positions, normals, `u32` indices, and optional texture
/// coordinates, tangents and skin.
///
/// # Tangent convention
///
/// Tangents follow glTF: `xyz` is the tangent (the direction of increasing `u`) and `w`
/// (exactly `+1` or `-1`) the handedness, with
/// `bitangent = cross(normal, tangent.xyz) * tangent.w`. glTF texture coordinates run `v`
/// *down* the image, and the bitangent points *up* the image (decreasing `v`), so a normal
/// map's green channel is "+Y up", as glTF specifies. For non-mirrored UVs this is the sign
/// `MikkTSpace` gives in DCC tools that export glTF (Blender computes it with `v` up before the
/// export flips `v`); a mirrored UV island gets `w = -1`.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshData {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
    uvs: Option<Vec<[f32; 2]>>,
    tangents: Option<Vec<[f32; 4]>>,
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
            uvs: None,
            tangents: None,
            skin: None,
        })
    }

    /// Attaches texture coordinates (glTF `TEXCOORD_0`: `u` right, `v` down the image), one
    /// pair per vertex. Values outside `0..=1` repeat.
    ///
    /// # Errors
    /// [`MeshError::LengthMismatch`] unless there is exactly one pair per vertex,
    /// [`MeshError::NonFinite`] for a NaN or infinite coordinate.
    pub fn with_uvs(mut self, uvs: Vec<[f32; 2]>) -> Result<Self, MeshError> {
        if uvs.len() != self.positions.len() {
            return Err(MeshError::LengthMismatch);
        }
        if uvs.iter().flatten().any(|value| !value.is_finite()) {
            return Err(MeshError::NonFinite);
        }
        self.uvs = Some(uvs);
        Ok(self)
    }

    /// Texture coordinates, one per position, if the mesh has them.
    #[must_use]
    pub fn uvs(&self) -> Option<&[[f32; 2]]> {
        self.uvs.as_deref()
    }

    /// Attaches tangents (glTF `TANGENT`; see [`MeshData`] for the convention),
    /// one per vertex. A `w` within `1e-3` of `±1` is stored as exactly `±1`; `xyz` is kept as
    /// given (the renderer normalises it).
    ///
    /// # Errors
    /// [`MeshError::LengthMismatch`] unless there is exactly one tangent per vertex,
    /// [`MeshError::NonFinite`] for a NaN or infinite component, [`MeshError::BadTangent`]
    /// when `w` is not `±1`.
    pub fn with_tangents(mut self, mut tangents: Vec<[f32; 4]>) -> Result<Self, MeshError> {
        if tangents.len() != self.positions.len() {
            return Err(MeshError::LengthMismatch);
        }
        if tangents.iter().flatten().any(|value| !value.is_finite()) {
            return Err(MeshError::NonFinite);
        }
        for tangent in &mut tangents {
            tangent[3] = if (tangent[3] - 1.0).abs() <= 1e-3 {
                1.0
            } else if (tangent[3] + 1.0).abs() <= 1e-3 {
                -1.0
            } else {
                return Err(MeshError::BadTangent);
            };
        }
        self.tangents = Some(tangents);
        Ok(self)
    }

    /// Tangents, one per position, if the mesh has them (from the file or generated).
    #[must_use]
    pub fn tangents(&self) -> Option<&[[f32; 4]]> {
        self.tangents.as_deref()
    }

    /// Generates tangents from the UVs, or `None` for a mesh without UVs.
    ///
    /// "`MikkTSpace`-style", not a `MikkTSpace` port: each triangle's tangent (`dP/du`) and
    /// bitangent (`-dP/dv`, see [`MeshData`]) are accumulated per vertex,
    /// weighted by the triangle's position × UV extent (degenerate UV triangles add
    /// nothing), then orthogonalised against the vertex normal (Gram-Schmidt); `w` is
    /// `-1` where the accumulated bitangent opposes `cross(normal, tangent)`, else `+1`. A
    /// vertex with no usable tangent gets a fixed unit vector perpendicular to its normal and
    /// `w = +1`. Vertices shared across a UV seam are not split. Only `+ - * /` and `sqrt`
    /// are used, so the result is bit-identical on every platform.
    #[must_use]
    pub fn generate_tangents(&self) -> Option<Vec<[f32; 4]>> {
        let uvs = self.uvs.as_deref()?;
        Some(generate_tangents(
            &self.positions,
            &self.normals,
            uvs,
            &self.indices,
        ))
    }

    /// Returns this mesh with tangents from [`Self::generate_tangents`], unless it already
    /// has tangents or has no UVs (then it is returned unchanged).
    #[must_use]
    pub fn with_generated_tangents(mut self) -> Self {
        if self.tangents.is_none() {
            self.tangents = self.generate_tangents();
        }
        self
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

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `a / |a|`, or `None` when `a` is (numerically) zero.
fn normalize(a: V3) -> Option<V3> {
    let length_sq = dot(a, a);
    (length_sq > 1e-30 && length_sq.is_finite()).then(|| scale(a, 1.0 / length_sq.sqrt()))
}

/// A fixed unit vector perpendicular to the unit vector `normal`.
fn perpendicular(normal: V3) -> V3 {
    let axis = if normal[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    // Never zero for the axis chosen above.
    normalize(sub(axis, scale(normal, dot(normal, axis)))).unwrap_or([0.0, 0.0, 1.0])
}

/// See [`MeshData::generate_tangents`]. The slices have one entry per vertex and every index
/// is in range (the `MeshData` invariants).
fn generate_tangents(
    positions: &[V3],
    normals: &[V3],
    uvs: &[[f32; 2]],
    indices: &[u32],
) -> Vec<[f32; 4]> {
    let mut tangents = vec![[0.0f32; 3]; positions.len()];
    let mut bitangents = vec![[0.0f32; 3]; positions.len()];
    for triangle in indices.as_chunks::<3>().0 {
        let [a, b, c] = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        let edge1 = sub(positions[b], positions[a]);
        let edge2 = sub(positions[c], positions[a]);
        // `v` is negated: the bitangent points up the image (glTF `v` runs down).
        let (du1, dv1) = (uvs[b][0] - uvs[a][0], uvs[a][1] - uvs[b][1]);
        let (du2, dv2) = (uvs[c][0] - uvs[a][0], uvs[a][1] - uvs[c][1]);
        let area = du1 * dv2 - du2 * dv1;
        if area == 0.0 || !area.is_finite() {
            continue;
        }
        // Lengyel's `sdir`/`tdir` times `|area|`: the same directions, weighted by the
        // triangle's size rather than divided by its UV area.
        let sign = if area < 0.0 { -1.0 } else { 1.0 };
        let tangent = scale(sub(scale(edge1, dv2), scale(edge2, dv1)), sign);
        let bitangent = scale(sub(scale(edge2, du1), scale(edge1, du2)), sign);
        for vertex in [a, b, c] {
            tangents[vertex] = add(tangents[vertex], tangent);
            bitangents[vertex] = add(bitangents[vertex], bitangent);
        }
    }
    tangents
        .iter()
        .zip(&bitangents)
        .zip(normals)
        .map(|((&tangent, &bitangent), &normal)| {
            let normal = normalize(normal).unwrap_or([0.0, 1.0, 0.0]);
            let projected = sub(tangent, scale(normal, dot(normal, tangent)));
            // Relative test: a tangent (nearly) parallel to the normal leaves only rounding.
            let usable = dot(projected, projected) > 1e-10 * dot(tangent, tangent);
            if let Some(t) = normalize(projected).filter(|_| usable) {
                let w = if dot(cross(normal, t), bitangent) < 0.0 {
                    -1.0
                } else {
                    1.0
                };
                [t[0], t[1], t[2], w]
            } else {
                let t = perpendicular(normal);
                [t[0], t[1], t[2], 1.0]
            }
        })
        .collect()
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
    fn uvs_need_one_finite_pair_per_vertex() {
        let (positions, normals, indices) = triangle();
        let mesh = MeshData::new(positions, normals, indices).expect("valid");
        assert_eq!(mesh.uvs(), None);
        assert_eq!(
            mesh.clone().with_uvs(vec![[0.0, 0.0]; 2]),
            Err(MeshError::LengthMismatch)
        );
        assert_eq!(
            mesh.clone().with_uvs(vec![[0.0, f32::NAN]; 3]),
            Err(MeshError::NonFinite)
        );
        let uvs = vec![[0.0, 0.0], [2.5, 0.0], [0.0, -1.0]];
        let textured = mesh.with_uvs(uvs.clone()).expect("valid uvs");
        assert_eq!(textured.uvs(), Some(&uvs[..]));
    }

    /// A unit quad in the XY plane facing +Z with glTF UVs (`v` down the image): the
    /// top-left corner `(0, 1, 0)` has UV `(0, 0)`.
    fn quad(uvs: [[f32; 2]; 4]) -> MeshData {
        MeshData::new(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            vec![[0.0, 0.0, 1.0]; 4],
            vec![0, 1, 2, 0, 2, 3],
        )
        .expect("valid")
        .with_uvs(uvs.to_vec())
        .expect("uvs")
    }

    const QUAD_UVS: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];

    #[test]
    fn generated_tangents_follow_the_gltf_convention() {
        // u runs +X and v runs down the image (-Y): tangent +X, bitangent (up the image) +Y
        // = cross(+Z, +X), so w = +1.
        let tangents = quad(QUAD_UVS).generate_tangents().expect("uvs");
        assert_eq!(tangents, vec![[1.0, 0.0, 0.0, 1.0]; 4]);
        // Mirrored in u: tangent -X, bitangent still +Y = -cross(+Z, -X), so w = -1.
        let mirrored = QUAD_UVS.map(|[u, v]| [1.0 - u, v]);
        let tangents = quad(mirrored).generate_tangents().expect("uvs");
        assert_eq!(tangents, vec![[-1.0, 0.0, 0.0, -1.0]; 4]);
        // Degenerate UVs: a fixed unit tangent perpendicular to the normal.
        let tangents = quad([[0.5, 0.5]; 4]).generate_tangents().expect("uvs");
        for t in &tangents {
            assert!((dot([t[0], t[1], t[2]], [t[0], t[1], t[2]]) - 1.0).abs() < 1e-6);
            assert_eq!(t[2], 0.0);
            assert_eq!(t[3], 1.0);
        }
        // No UVs: nothing to generate. Existing tangents are kept.
        let (positions, normals, indices) = triangle();
        let plain = MeshData::new(positions, normals, indices).expect("valid");
        assert_eq!(plain.generate_tangents(), None);
        assert_eq!(plain.clone().with_generated_tangents(), plain);
        let given = quad(QUAD_UVS)
            .with_tangents(vec![[0.0, 1.0, 0.0, -1.0]; 4])
            .expect("tangents")
            .with_generated_tangents();
        assert_eq!(given.tangents(), Some(&[[0.0, 1.0, 0.0, -1.0]; 4][..]));
    }

    #[test]
    fn box_projected_cube_tangents_are_unit_and_orthogonal() {
        // Six faces with their own vertices, box-projected like the sample meshes.
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::new();
        for axis in 0..3 {
            for side in [-1.0f32, 1.0] {
                let mut normal = [0.0; 3];
                normal[axis] = side;
                let (u_axis, v_axis) = ((axis + 1) % 3, (axis + 2) % 3);
                let base = positions.len() as u32;
                for (a, b) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                    let mut position = [0.0; 3];
                    position[axis] = side * 0.5;
                    position[u_axis] = a;
                    position[v_axis] = b;
                    positions.push(position);
                    normals.push(normal);
                    uvs.push([position[u_axis] * side, -position[v_axis]]);
                }
                if side > 0.0 {
                    indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
                } else {
                    indices.extend([base, base + 2, base + 1, base, base + 3, base + 2]);
                }
            }
        }
        let mesh = MeshData::new(positions, normals.clone(), indices)
            .expect("cube")
            .with_uvs(uvs)
            .expect("uvs")
            .with_generated_tangents();
        let tangents = mesh.tangents().expect("generated");
        for (t, n) in tangents.iter().zip(&normals) {
            let xyz = [t[0], t[1], t[2]];
            assert!((dot(xyz, xyz) - 1.0).abs() < 1e-6, "{t:?}");
            assert!(dot(xyz, *n).abs() < 1e-6, "{t:?} vs {n:?}");
            assert_eq!(t[3].abs(), 1.0, "{t:?}");
        }
        // Bit-reproducible: generating again gives the same bits.
        assert_eq!(mesh.generate_tangents().as_deref(), Some(tangents));
    }

    #[test]
    fn tangents_need_one_finite_signed_entry_per_vertex() {
        let mesh = quad(QUAD_UVS);
        assert_eq!(
            mesh.clone().with_tangents(vec![[1.0, 0.0, 0.0, 1.0]; 3]),
            Err(MeshError::LengthMismatch)
        );
        assert_eq!(
            mesh.clone()
                .with_tangents(vec![[f32::NAN, 0.0, 0.0, 1.0]; 4]),
            Err(MeshError::NonFinite)
        );
        assert_eq!(
            mesh.clone().with_tangents(vec![[1.0, 0.0, 0.0, 0.5]; 4]),
            Err(MeshError::BadTangent)
        );
        let snapped = mesh
            .with_tangents(vec![[1.0, 0.0, 0.0, -0.9995]; 4])
            .expect("within 1e-3");
        assert_eq!(snapped.tangents().map(|t| t[0][3]), Some(-1.0));
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
