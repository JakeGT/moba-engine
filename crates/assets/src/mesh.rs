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
    /// Baked index-only levels of detail (R8, ADR-0018), LOD1 first; empty without a `.lod`.
    pub(crate) lods: Vec<crate::mesh_lod::MeshLod>,
    /// A culling sphere computed from another mesh with the same vertices (a reduced skin's
    /// full mesh, whose skeleton has the clips); see [`Self::cull_sphere`].
    pub(crate) cull_sphere: Option<[f32; 4]>,
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
            lods: Vec::new(),
            cull_sphere: None,
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
    /// The tangents are `MikkTSpace` (A0, ADR-0022), the tangent space normal-map bakers
    /// (Blender, xNormal, Substance) and glTF assume: the reference algorithm through
    /// `bevy_mikktspace`, a byte-identical port of the C original, with its default 180°
    /// grouping threshold. It runs on the unit normals and on `(u, -v)`: glTF `v` runs down
    /// the image, so negating it gives the tangent space a DCC tool computes with `v` up
    /// before its glTF export flips `v` (see [`MeshData`]); negation keeps every UV
    /// difference and equality exact.
    ///
    /// `MikkTSpace` answers per triangle corner. A vertex whose corners get different
    /// answers (a vertex shared across a mirrored UV seam, or two fans meeting at one
    /// index) takes its first corner's, in index order; exporters split such vertices, so a
    /// mesh exported with its seams split gets exact `MikkTSpace` everywhere. A vertex with
    /// no tangent space (only degenerate triangles touch it, or a zero tangent) gets a fixed
    /// unit vector perpendicular to its normal and `w = +1`. Only `+ - * /` and `sqrt` are
    /// used (the angle weights use [`acos`], a series in those operations), so the result is
    /// bit-identical on every platform.
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

    /// Attaches baked levels of detail ([`crate::MeshLods`], validated against this mesh when
    /// they were built or parsed). They index this mesh's vertices, so they survive
    /// [`Self::with_skin`], UVs and tangents, and a reduced skin keeps them
    /// ([`crate::SkinLod`]).
    #[must_use]
    pub fn with_lods(mut self, lods: crate::MeshLods) -> Self {
        let vertex_count = self.positions.len();
        // `MeshLods` is only built against a mesh with this vertex count; re-check so a level
        // set from another mesh can never index past the vertices.
        if lods
            .levels()
            .iter()
            .flat_map(crate::MeshLod::indices)
            .all(|&index| (index as usize) < vertex_count)
        {
            self.lods = lods.into_levels();
        }
        self
    }

    /// The baked levels of detail below LOD0 (LOD1 first); empty without any.
    #[must_use]
    pub fn lods(&self) -> &[crate::mesh_lod::MeshLod] {
        &self.lods
    }

    /// A bounding sphere `[centre x, y, z, radius]` of the positions: the AABB's centre and
    /// the distance to the farthest vertex.
    #[must_use]
    pub fn bounding_sphere(&self) -> [f32; 4] {
        bounding_sphere(&self.positions)
    }

    /// The sphere `[centre x, y, z, radius]` the renderer culls this mesh with (R8,
    /// ADR-0018 §1). A static mesh's is its [`Self::bounding_sphere`]. A skinned mesh's is
    /// centred on the bind-pose sphere, with the radius
    /// `max(farthest vertex over every clip's sampled poses × 1.05, bind radius ×`
    /// [`crate::SKINNED_CULL_FLOOR`]`)`: a clip with root motion or long reach widens it, and
    /// the floor covers spring bones, additive layers and clip blends, which sampling the
    /// clips does not see. Skins every vertex in up to [`crate::MAX_POSE_SAMPLES`] poses, so
    /// call it once, at load.
    #[must_use]
    pub fn cull_sphere(&self) -> [f32; 4] {
        if let Some(sphere) = self.cull_sphere {
            return sphere;
        }
        let [x, y, z, bind] = self.bounding_sphere();
        if self.skin.is_none() {
            return [x, y, z, bind];
        }
        let centre = [x, y, z];
        let mut farthest = 0.0f32;
        crate::for_each_sampled_pose(
            self,
            crate::POSE_SAMPLE_FPS,
            crate::MAX_POSE_SAMPLES,
            |positions| {
                for position in positions {
                    let distance = (0..3)
                        .map(|axis| (position[axis] - centre[axis]).powi(2))
                        .sum::<f32>()
                        .sqrt();
                    if distance.is_finite() {
                        farthest = farthest.max(distance);
                    }
                }
            },
        );
        let radius =
            (farthest * crate::vat::POSE_SAMPLE_MARGIN).max(bind * crate::SKINNED_CULL_FLOOR);
        [x, y, z, radius]
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

/// A bounding sphere `[x, y, z, radius]` of `positions` (the AABB's centre, radius to the
/// farthest point); a zero sphere at the origin for no points.
#[must_use]
pub fn bounding_sphere(positions: &[[f32; 3]]) -> [f32; 4] {
    if positions.is_empty() {
        return [0.0; 4];
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for position in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    let centre = scale(add(min, max), 0.5);
    let radius_squared = positions
        .iter()
        .map(|position| {
            let offset = sub(*position, centre);
            dot(offset, offset)
        })
        .fold(0.0f32, f32::max);
    [centre[0], centre[1], centre[2], radius_squared.sqrt()]
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
    let unit_normals: Vec<V3> = normals
        .iter()
        .map(|&normal| normalize(normal).unwrap_or([0.0, 1.0, 0.0]))
        .collect();
    let mut geometry = MikkGeometry {
        positions,
        normals: &unit_normals,
        uvs,
        indices,
        tangents: vec![None; positions.len()],
    };
    // `GenerateTangentSpaceError` is uninhabited: generation cannot fail.
    let _ = bevy_mikktspace::generate_tangents::<_, DeterministicOps>(&mut geometry);
    geometry
        .tangents
        .iter()
        .zip(&unit_normals)
        .map(|(tangent, &normal)| {
            tangent.unwrap_or_else(|| {
                let t = perpendicular(normal);
                [t[0], t[1], t[2], 1.0]
            })
        })
        .collect()
}

/// A [`MeshData`]'s streams as `bevy_mikktspace` reads them (one triangle per face), and the
/// first usable tangent written back for each vertex.
struct MikkGeometry<'a> {
    positions: &'a [V3],
    /// Unit length.
    normals: &'a [V3],
    uvs: &'a [[f32; 2]],
    indices: &'a [u32],
    tangents: Vec<Option<[f32; 4]>>,
}

impl MikkGeometry<'_> {
    fn vertex(&self, face: usize, corner: usize) -> usize {
        self.indices[face * 3 + corner] as usize
    }
}

impl bevy_mikktspace::Geometry<DeterministicOps> for MikkGeometry<'_> {
    fn num_faces(&self) -> usize {
        self.indices.len() / 3
    }

    fn num_vertices_of_face(&self, _face: usize) -> usize {
        3
    }

    fn position(&self, face: usize, vert: usize) -> [f32; 3] {
        self.positions[self.vertex(face, vert)]
    }

    fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
        self.normals[self.vertex(face, vert)]
    }

    fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
        // `v` negated: MikkTSpace's bitangent then points up the image (glTF `v` runs down).
        let [u, v] = self.uvs[self.vertex(face, vert)];
        [u, -v]
    }

    fn set_tangent(
        &mut self,
        tangent_space: Option<bevy_mikktspace::TangentSpace>,
        face: usize,
        vert: usize,
    ) {
        let vertex = self.vertex(face, vert);
        if self.tangents[vertex].is_some() {
            return;
        }
        let Some(space) = tangent_space else {
            return;
        };
        let [x, y, z, w] = space.tangent_encoded();
        // MikkTSpace leaves a tangent it could not orient at zero; keep looking.
        if normalize([x, y, z]).is_some() {
            self.tangents[vertex] = Some([x, y, z, w]);
        }
    }
}

/// `sqrt` and `acos` for `bevy_mikktspace` that give the same bits on every platform:
/// `sqrt` is correctly rounded (IEEE 754), and [`acos`] uses only `+ - * /` and `sqrt`.
/// The reference build uses the platform's `f64::acos`; both round to the same `f32`
/// except where the platform's own last bit differs.
struct DeterministicOps;

impl bevy_mikktspace::Ops for DeterministicOps {
    fn sqrt(x: f32) -> f32 {
        x.sqrt()
    }

    fn acos(x: f32) -> f32 {
        acos(f64::from(x)) as f32
    }
}

/// `acos(x)` in radians for `x` in `-1..=1` (clamped; NaN stays NaN), to within a few
/// `f64` ulps, from `+ - * /` and `sqrt` only. `|x| <= 0.5` uses `π/2 - asin(x)`; beyond
/// that, `acos(x) = 2 asin(sqrt((1 - x) / 2))` (and its mirror for negative `x`) keeps the
/// series argument at most `0.5`.
fn acos(x: f64) -> f64 {
    use core::f64::consts::{FRAC_PI_2, PI};
    let x = x.clamp(-1.0, 1.0);
    if x.abs() <= 0.5 {
        FRAC_PI_2 - asin_series(x)
    } else if x > 0.0 {
        2.0 * asin_series(f64::midpoint(1.0, -x).sqrt())
    } else {
        PI - 2.0 * asin_series(f64::midpoint(1.0, x).sqrt())
    }
}

/// The Maclaurin series of `asin(z)` for `|z| <= 0.5`:
/// `sum (2n)! / (4^n (n!)^2 (2n + 1)) z^(2n + 1)`. Term `n + 1` is term `n` times
/// `z² (2n + 1)² / ((2n + 2)(2n + 3))`, at most a quarter of it, so 30 terms leave under
/// `1e-19`.
fn asin_series(z: f64) -> f64 {
    let z2 = z * z;
    let mut term = z;
    let mut sum = z;
    for n in 0..30_u32 {
        let k = f64::from(2 * n + 1);
        term *= z2 * k * k / ((k + 1.0) * (k + 2.0));
        sum += term;
    }
    sum
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

    /// The deterministic `acos` agrees with the platform's to `f64` rounding, and to the
    /// `f32` bit nearly always (at most one ulp: platform libms differ in the last bit).
    #[test]
    fn deterministic_acos_matches_the_platform() {
        let steps = 200_000_u32;
        for step in 0..=steps {
            let x = -1.0 + 2.0 * f64::from(step) / f64::from(steps);
            let ours = acos(x);
            assert!(
                (ours - x.acos()).abs() < 4e-15,
                "acos({x}) = {ours} vs {}",
                x.acos()
            );
            let narrow = x as f32;
            let ours = acos(f64::from(narrow)) as f32;
            let platform = f64::from(narrow).acos() as f32;
            assert!(
                ours.to_bits().abs_diff(platform.to_bits()) <= 1,
                "acos({narrow}): {ours} vs {platform}"
            );
        }
        assert_eq!(acos(1.0), 0.0);
        assert_eq!(acos(2.0), 0.0, "clamped");
        assert!((acos(-1.0) - core::f64::consts::PI).abs() < 1e-15);
        assert!(acos(f64::NAN).is_nan());
    }

    /// A smooth, closed three-triangle fan with non-affine UVs and very different triangle
    /// sizes and corner angles: area weighting (the pre-A0 generator) and `MikkTSpace`'s
    /// angle weighting disagree here.
    fn fan() -> MeshData {
        let positions = vec![
            [0.0, 0.0, 0.3],
            [1.0, 0.0, 0.0],
            [0.0, 2.0, 0.0],
            [-1.0, -1.0, 0.0],
        ];
        let normals = positions
            .iter()
            .map(|&[x, y, _]| normalize([x * 0.3, y * 0.3, 1.0]).expect("unit"))
            .collect();
        let uvs = positions
            .iter()
            .map(|&[x, y, _]| [0.5 + 0.3 * x + 0.02 * y * y, 0.5 - 0.25 * y + 0.05 * x * x])
            .collect();
        MeshData::new(positions, normals, vec![0, 1, 2, 0, 2, 3, 0, 3, 1])
            .expect("valid")
            .with_uvs(uvs)
            .expect("uvs")
    }

    type D3 = [f64; 3];

    fn d3(a: V3) -> D3 {
        a.map(f64::from)
    }

    fn dsub(a: D3, b: D3) -> D3 {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    fn dscale(a: D3, s: f64) -> D3 {
        a.map(|value| value * s)
    }

    fn ddot(a: D3, b: D3) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn dunit(a: D3) -> D3 {
        dscale(a, 1.0 / ddot(a, a).sqrt())
    }

    /// `a` projected onto the plane of the unit `n`.
    fn dflat(a: D3, n: D3) -> D3 {
        dsub(a, dscale(n, ddot(n, a)))
    }

    /// Per-vertex tangents written straight from Mikkelsen's definition (thesis §4.3, the
    /// reference `GenerateTSpaces`/`EvalTspace`) in `f64`, for a mesh where every vertex has
    /// one orientation-preserving group: each triangle's unit `dP/ds` (with `t = -v`), at
    /// each corner projected onto the vertex's tangent plane, normalised and weighted by the
    /// corner's angle in that plane; the sum normalised. `area_weighted` gives the pre-A0
    /// generator instead: un-normalised `dP/ds` times `|UV area|`, projected once at the end.
    fn reference_tangents(mesh: &MeshData, area_weighted: bool) -> Vec<D3> {
        let (positions, normals, uvs) =
            (mesh.positions(), mesh.normals(), mesh.uvs().expect("uvs"));
        let mut sums = vec![[0.0f64; 3]; positions.len()];
        for triangle in mesh.indices().as_chunks::<3>().0 {
            let corners = triangle.map(|index| index as usize);
            let [a, b, c] = corners;
            let st = |vertex: usize| [f64::from(uvs[vertex][0]), -f64::from(uvs[vertex][1])];
            let (t1, t2, t3) = (st(a), st(b), st(c));
            let (t21, t31) = (
                [t2[0] - t1[0], t2[1] - t1[1]],
                [t3[0] - t1[0], t3[1] - t1[1]],
            );
            let d1 = dsub(d3(positions[b]), d3(positions[a]));
            let d2 = dsub(d3(positions[c]), d3(positions[a]));
            let area = t21[0] * t31[1] - t21[1] * t31[0];
            assert!(
                area > 0.0,
                "the reference handles orientation-preserving fans only"
            );
            let os = dsub(dscale(d1, t31[1]), dscale(d2, t21[1]));
            for (corner, &vertex) in corners.iter().enumerate() {
                if area_weighted {
                    // os / area * |area|: Lengyel's sdir weighted by the triangle's size.
                    sums[vertex] = [0, 1, 2].map(|axis| sums[vertex][axis] + os[axis]);
                    continue;
                }
                let n = dunit(d3(normals[vertex]));
                let tangent = dunit(dflat(dunit(os), n));
                let previous = corners[(corner + 2) % 3];
                let next = corners[(corner + 1) % 3];
                let here = d3(positions[vertex]);
                let e1 = dunit(dflat(dsub(d3(positions[previous]), here), n));
                let e2 = dunit(dflat(dsub(d3(positions[next]), here), n));
                let angle = ddot(e1, e2).clamp(-1.0, 1.0).acos();
                sums[vertex] = [0, 1, 2].map(|axis| sums[vertex][axis] + angle * tangent[axis]);
            }
        }
        sums.iter()
            .zip(normals)
            .map(|(&sum, &normal)| dunit(dflat(sum, dunit(d3(normal)))))
            .collect()
    }

    /// The A0 fix, pinned against an independent reference: the generated tangents are
    /// Mikkelsen's angle-weighted average, not the area-weighted one the engine used before.
    #[test]
    fn generated_tangents_are_mikktspace() {
        let mesh = fan();
        let tangents = mesh.generate_tangents().expect("uvs");
        let mikk = reference_tangents(&mesh, false);
        let area = reference_tangents(&mesh, true);
        let mut largest_change = 0.0f64;
        for ((tangent, expected), old) in tangents.iter().zip(&mikk).zip(&area) {
            assert_eq!(tangent[3], 1.0, "orientation preserving");
            for axis in 0..3 {
                assert!(
                    (f64::from(tangent[axis]) - expected[axis]).abs() < 2e-6,
                    "{tangent:?} vs MikkTSpace {expected:?}"
                );
                largest_change = largest_change.max((expected[axis] - old[axis]).abs());
            }
        }
        // The fixture discriminates: the two weightings disagree by far more than rounding.
        assert!(largest_change > 1e-2, "{largest_change}");
    }

    impl bevy_mikktspace::Geometry<bevy_mikktspace::StdOps> for MikkGeometry<'_> {
        fn num_faces(&self) -> usize {
            self.indices.len() / 3
        }
        fn num_vertices_of_face(&self, _face: usize) -> usize {
            3
        }
        fn position(&self, face: usize, vert: usize) -> [f32; 3] {
            self.positions[self.vertex(face, vert)]
        }
        fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
            self.normals[self.vertex(face, vert)]
        }
        fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
            let [u, v] = self.uvs[self.vertex(face, vert)];
            [u, -v]
        }
        fn set_tangent(
            &mut self,
            tangent_space: Option<bevy_mikktspace::TangentSpace>,
            face: usize,
            vert: usize,
        ) {
            <Self as bevy_mikktspace::Geometry<DeterministicOps>>::set_tangent(
                self,
                tangent_space,
                face,
                vert,
            );
        }
    }

    /// The deterministic build agrees with the reference build (the platform's `f64::acos`,
    /// which is what makes `bevy_mikktspace` byte-identical to the C original) on a mesh
    /// with a smooth fan, a mirrored UV island, a UV-degenerate triangle and a
    /// position-degenerate one.
    #[test]
    fn deterministic_mikktspace_matches_the_reference_build() {
        let fan = fan();
        let mut positions = fan.positions().to_vec();
        let mut normals = fan.normals().to_vec();
        let mut uvs = fan.uvs().expect("uvs").to_vec();
        let mut indices = fan.indices().to_vec();
        let mut push = |p: V3, n: V3, uv: [f32; 2], positions: &mut Vec<V3>| {
            positions.push(p);
            normals.push(n);
            uvs.push(uv);
            u32::try_from(positions.len() - 1).expect("small")
        };
        // A mirrored island (u runs -X), its own vertices.
        let up = [0.0, 0.0, 1.0];
        let m = [
            push([3.0, 0.0, 0.0], up, [0.0, 1.0], &mut positions),
            push([4.0, 0.0, 0.0], up, [-1.0, 1.0], &mut positions),
            push([4.0, 1.0, 0.0], up, [-1.0, 0.0], &mut positions),
        ];
        indices.extend_from_slice(&m);
        // Zero UV area, then two coincident corners.
        let d = [
            push([6.0, 0.0, 0.0], up, [0.5, 0.5], &mut positions),
            push([7.0, 0.0, 0.0], up, [0.5, 0.5], &mut positions),
            push([7.0, 1.0, 0.0], up, [0.5, 0.5], &mut positions),
            push([9.0, 0.0, 0.0], up, [0.0, 0.0], &mut positions),
            push([9.0, 0.0, 0.0], up, [0.0, 0.0], &mut positions),
            push([9.0, 1.0, 0.0], up, [0.0, 1.0], &mut positions),
        ];
        indices.extend_from_slice(&d);
        let mesh = MeshData::new(positions, normals, indices)
            .expect("valid")
            .with_uvs(uvs)
            .expect("uvs");
        let ours = mesh.generate_tangents().expect("uvs");
        let mut reference = MikkGeometry {
            positions: mesh.positions(),
            normals: mesh.normals(),
            uvs: mesh.uvs().expect("uvs"),
            indices: mesh.indices(),
            tangents: vec![None; mesh.vertex_count()],
        };
        let _ = bevy_mikktspace::generate_tangents::<_, bevy_mikktspace::StdOps>(&mut reference);
        for (vertex, (ours, reference)) in ours.iter().zip(&reference.tangents).enumerate() {
            match reference {
                Some(reference) => assert!(
                    ours.iter()
                        .zip(reference)
                        .all(|(a, b)| (a - b).abs() < 1e-6),
                    "vertex {vertex}: {ours:?} vs {reference:?}"
                ),
                // No tangent space: the fallback, perpendicular to the (+Z) normal.
                None => assert_eq!(ours, &[1.0, 0.0, 0.0, 1.0], "vertex {vertex}"),
            }
        }
        // The mirrored island is left-handed.
        for vertex in m {
            assert_eq!(ours[vertex as usize][3], -1.0, "vertex {vertex}");
        }
    }

    /// `MikkTSpace` answers per corner; a vertex index shared by two triangles that disagree
    /// (here: mirrored UVs across the shared edge) takes its first corner's tangent space.
    #[test]
    fn a_shared_vertex_takes_its_first_corners_tangent_space() {
        let mesh = MeshData::new(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [-1.0, 0.0, 0.0],
            ],
            vec![[0.0, 0.0, 1.0]; 4],
            // Both triangles face +Z and share the edge 0-2.
            vec![0, 1, 2, 0, 2, 3],
        )
        .expect("valid")
        // u mirrors about x = 0: right-handed on the first triangle, left on the second.
        .with_uvs(vec![[0.0, 1.0], [1.0, 1.0], [0.0, 0.0], [1.0, 1.0]])
        .expect("uvs");
        let tangents = mesh.generate_tangents().expect("uvs");
        assert_eq!(tangents[1], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(tangents[3], [-1.0, 0.0, 0.0, -1.0]);
        // The shared vertices keep the first triangle's right-handed space.
        assert_eq!(tangents[0], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(tangents[2], [1.0, 0.0, 0.0, 1.0]);
    }
}
