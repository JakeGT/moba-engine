//! `MOBA_MATERIAL_SHEET=<set>`: the material-model sheet (ADR-0023 verification gate 5, N2c).
//!
//! The local preview draws one sphere per descriptor in `<asset root>/materials/<set>/`
//! (`*.toml` except `provenance.toml`, sorted by file name, at most [`MAX_SPHERES`]), each with its own material, in a
//! row along +x centred on [`SHEET_CENTRE`]. `content/materials/samples/` holds one sample
//! material per shading model, so `MOBA_MATERIAL_SHEET=samples` is the sheet the tiered
//! goldens capture. The spheres are client-only instances like the stress scene's minions:
//! nothing here touches the simulation, the wire or the content hash. A descriptor that does
//! not load leaves its sphere on the vertex-colour look (logged once, as for a model).
//!
//! The meshes are uploaded at bind time (startup, content reload, a new renderer); drawing
//! pushes one instance per sphere and never allocates.

use std::path::{Path, PathBuf};

use moba_assets::{MeshData, valid_mesh_name};
use moba_core::math::{Mat4, Vec3, Vec4};
use moba_render::{MeshId, UnitInstance};

/// One sphere's material binding: its label, its descriptor and its meshes, as
/// [`crate::app::materials::MaterialLibrary::bind_files`] takes them.
pub(crate) type SheetMaterial = (String, Option<PathBuf>, [Option<MeshId>; 4]);

/// Environment variable naming the sample set (a directory under `materials/`).
pub(crate) const SHEET_ENV: &str = "MOBA_MATERIAL_SHEET";
/// Most spheres on a sheet.
pub(crate) const MAX_SPHERES: usize = 16;
/// The row's centre on the map (x, z), metres: open, flat ground near the mid lane.
pub(crate) const SHEET_CENTRE: [f32; 2] = [0.0, -12.0];
/// Sphere radius and the distance between centres, metres.
const RADIUS: f32 = 1.0;
const SPACING: f32 = 2.5;
/// Segments around and rings from pole to pole of the UV sphere.
const SEGMENTS: u32 = 48;
const RINGS: u32 = 24;

/// The sheet's set and, once bound, its spheres.
#[derive(Debug)]
pub(crate) struct MaterialSheet {
    set: String,
    spheres: Vec<MeshId>,
    bound: bool,
}

impl MaterialSheet {
    /// From [`SHEET_ENV`]: `None` when unset, empty or not a plain directory name.
    pub(crate) fn from_env() -> Option<Self> {
        let set = std::env::var(SHEET_ENV).ok()?;
        let set = set.trim();
        if !valid_mesh_name(set) {
            if !set.is_empty() {
                eprintln!("[assets] {SHEET_ENV}={set}: not a plain directory name; no sheet");
            }
            return None;
        }
        Some(Self {
            set: set.to_owned(),
            spheres: Vec::with_capacity(MAX_SPHERES),
            bound: false,
        })
    }

    /// Forgets the uploaded spheres (a new renderer has none); the next bind uploads them.
    pub(crate) fn renderer_recreated(&mut self) {
        self.spheres.clear();
        self.bound = false;
    }

    /// The set's descriptors, sorted by file name: `(label, path)`.
    fn descriptors(&self, root: &Path) -> Vec<(String, PathBuf)> {
        let dir = root.join("materials").join(&self.set);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!("[assets] material sheet {}: {error}", dir.display());
                return Vec::new();
            }
        };
        let mut found: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter_map(|path| {
                let stem = path.file_stem()?.to_str()?;
                let toml = path.extension().is_some_and(|ext| ext == "toml");
                // `provenance.toml` is the directory's licence record (ADR-0022), not a material.
                (toml && stem != "provenance" && valid_mesh_name(stem) && path.is_file())
                    .then(|| (format!("{}/{stem}", self.set), path.clone()))
            })
            .collect();
        found.sort();
        found.truncate(MAX_SPHERES);
        found
    }

    /// Uploads one sphere per descriptor through `register` (once per renderer) and returns
    /// what [`crate::app::materials::MaterialLibrary::bind_files`] binds to them: `(label, descriptor, meshes)`.
    pub(crate) fn upload<R>(&mut self, root: &Path, mut register: R) -> Vec<SheetMaterial>
    where
        R: FnMut(&MeshData) -> Result<MeshId, String>,
    {
        if self.bound {
            return Vec::new();
        }
        self.bound = true;
        let descriptors = self.descriptors(root);
        let Ok(sphere) = uv_sphere(SEGMENTS, RINGS) else {
            eprintln!("[assets] material sheet: the sphere mesh is invalid");
            return Vec::new();
        };
        let mut files = Vec::with_capacity(descriptors.len());
        for (label, path) in descriptors {
            match register(&sphere) {
                Ok(mesh) => {
                    self.spheres.push(mesh);
                    files.push((label, Some(path), [Some(mesh), None, None, None]));
                }
                Err(reason) => {
                    eprintln!("[assets] material sheet {label}: upload failed ({reason})");
                    break;
                }
            }
        }
        eprintln!(
            "[assets] material sheet '{}': {} spheres",
            self.set,
            self.spheres.len()
        );
        files
    }

    /// Pushes the spheres, left to right in file-name order. Never allocates past `units`'
    /// capacity (spheres past it are skipped).
    pub(crate) fn push(&self, units: &mut Vec<UnitInstance>) {
        let count = self.spheres.len();
        let offset = (count.max(1) - 1) as f32 * SPACING * 0.5;
        for (index, &mesh) in self.spheres.iter().enumerate() {
            if units.len() >= units.capacity() {
                break;
            }
            let centre = Vec3::new(
                SHEET_CENTRE[0] + index as f32 * SPACING - offset,
                RADIUS + 0.1,
                SHEET_CENTRE[1],
            );
            let transform = Mat4::from_translation(centre) * Mat4::from_scale(Vec3::splat(RADIUS));
            units.push(UnitInstance::new(transform, Vec4::ONE).with_mesh(mesh));
        }
    }
}

/// A unit UV sphere with `segments` around (`u`, once around from +x towards −z) and `rings`
/// from the north pole (`v` 0) to the south pole (`v` 1), with `MikkTSpace` tangents. The seam
/// and pole vertices are duplicated so `u` and `v` stay continuous within each triangle.
pub(crate) fn uv_sphere(segments: u32, rings: u32) -> Result<MeshData, moba_assets::MeshError> {
    let (segments, rings) = (segments.max(3), rings.max(2));
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    for ring in 0..=rings {
        let v = ring as f32 / rings as f32;
        let polar = v * core::f32::consts::PI;
        for segment in 0..=segments {
            let u = segment as f32 / segments as f32;
            let azimuth = u * core::f32::consts::TAU;
            positions.push([
                polar.sin() * azimuth.cos(),
                polar.cos(),
                -polar.sin() * azimuth.sin(),
            ]);
            uvs.push([u, v]);
        }
    }
    let normals = positions.clone();
    let row = segments + 1;
    let mut indices = Vec::new();
    for ring in 0..rings {
        for segment in 0..segments {
            let a = ring * row + segment;
            let b = a + row;
            // Counter-clockwise seen from outside.
            if ring > 0 {
                indices.extend_from_slice(&[a, b, a + 1]);
            }
            if ring + 1 < rings {
                indices.extend_from_slice(&[a + 1, b, b + 1]);
            }
        }
    }
    Ok(MeshData::new(positions, normals, indices)?
        .with_uvs(uvs)?
        .with_generated_tangents())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sphere_is_closed_unit_and_outward_facing() {
        let sphere = uv_sphere(SEGMENTS, RINGS).expect("valid mesh");
        let positions = sphere.positions();
        assert!(positions.iter().all(|p| {
            let length = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            (length - 1.0).abs() < 1e-5
        }));
        // Every triangle winds counter-clockwise seen from outside: its face normal points
        // away from the centre.
        for triangle in sphere.indices().as_chunks::<3>().0 {
            let [a, b, c] = [0, 1, 2].map(|i| Vec3::from(positions[triangle[i] as usize]));
            let normal = (b - a).cross(c - a);
            assert!(normal.dot(a + b + c) > 0.0, "{triangle:?}");
        }
        assert_eq!(
            sphere.indices().len() as u32,
            3 * SEGMENTS * (2 * RINGS - 2),
            "two triangles per quad, one per pole quad"
        );
        assert!(sphere.tangents().is_some(), "tangents for the normal map");
    }

    #[test]
    fn the_shipped_samples_cover_every_model_and_load() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let sheet = MaterialSheet {
            set: String::from("samples"),
            spheres: Vec::new(),
            bound: false,
        };
        let descriptors = sheet.descriptors(&root);
        let mut models: Vec<moba_assets::ShadingModel> = descriptors
            .iter()
            .map(|(label, path)| {
                moba_assets::MaterialDesc::load_file(path)
                    .unwrap_or_else(|error| panic!("{label}: {error}"))
                    .shading_model()
            })
            .collect();
        models.sort_by_key(|model| model.gbuffer_id());
        models.dedup();
        assert_eq!(models, moba_assets::ShadingModel::ALL);
    }
}
