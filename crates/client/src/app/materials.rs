//! Client-local model materials (AAA graphics roadmap R1b, ADR-0010).
//!
//! A loaded hero model named `sentinel` picks up `<asset root>/materials/sentinel.toml` if it
//! exists, with its textures at `<asset root>/textures/<name>.ktx2`. Like models and map
//! visuals, materials are cosmetic and local: nothing here reaches the simulation, the wire
//! or the content hash, and a missing material simply keeps the vertex-colour look.
//!
//! - Every material of one bind (startup or content reload) is registered in **one**
//!   `Renderer::register_materials` call, so its descriptor pool is sized from exactly the
//!   content that was loaded (ADR-0009).
//! - A texture shared by several materials is loaded and uploaded once.
//! - Any failure (bad descriptor, missing or malformed texture, a format its slot does not
//!   allow, a model without UVs, a renderer limit) is logged once per model and leaves that
//!   model on the vertex-colour look; a match never fails because of a material.
//!
//! All I/O and uploads happen at bind time; nothing here runs per frame.

use std::path::PathBuf;

use moba_assets::{
    MaterialDesc, TextureFormat, TextureSlot, load_ktx2_file, material_path, texture_path,
};
use moba_render::{MaterialData, MaterialId, MeshId, TextureData, TextureId};

/// What the material binder needs from a renderer (a trait so tests need no GPU).
pub(crate) trait MaterialSink {
    fn register_texture(&mut self, data: &TextureData) -> Result<TextureId, String>;
    fn register_materials(&mut self, materials: &[MaterialData])
    -> Result<Vec<MaterialId>, String>;
    fn set_mesh_material(&mut self, mesh: MeshId, material: MaterialId) -> Result<(), String>;
}

impl MaterialSink for moba_render::Renderer {
    fn register_texture(&mut self, data: &TextureData) -> Result<TextureId, String> {
        moba_render::Renderer::register_texture(self, data).map_err(|error| error.to_string())
    }

    fn register_materials(
        &mut self,
        materials: &[MaterialData],
    ) -> Result<Vec<MaterialId>, String> {
        moba_render::Renderer::register_materials(self, materials)
            .map_err(|error| error.to_string())
    }

    fn set_mesh_material(&mut self, mesh: MeshId, material: MaterialId) -> Result<(), String> {
        moba_render::Renderer::set_mesh_material(self, mesh, material)
            .map_err(|error| error.to_string())
    }
}

/// A model whose material should be resolved: its name and every mesh drawn for it.
pub(crate) struct ModelMeshes<'a> {
    pub(crate) name: &'a str,
    /// The static mesh and, for a skinned model, the skinned one.
    pub(crate) meshes: [Option<MeshId>; 2],
}

/// Resolved materials and uploaded textures for the current renderer.
#[derive(Debug)]
pub(crate) struct MaterialLibrary {
    root: PathBuf,
    /// Model names already handled (applied, absent or failed): each is tried once.
    seen: Vec<String>,
    /// Uploaded textures by name, with their format (cleared with the renderer).
    textures: Vec<(String, TextureId, TextureFormat)>,
    /// Models drawn with a material, for the startup log.
    applied: Vec<String>,
}

impl MaterialLibrary {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            seen: Vec::new(),
            textures: Vec::new(),
            applied: Vec::new(),
        }
    }

    /// Forgets uploaded textures and materials (a new renderer has none). Models are
    /// re-resolved on the next bind, so a material that failed is logged again only if its
    /// model is reloaded too.
    pub(crate) fn renderer_recreated(&mut self) {
        self.seen.clear();
        self.textures.clear();
        self.applied.clear();
    }

    /// Model names currently drawn with a material.
    #[cfg(test)]
    pub(crate) fn applied(&self) -> &[String] {
        &self.applied
    }

    /// Resolves the material of every model not handled yet and applies them all with one
    /// registration call.
    pub(crate) fn bind<'a>(
        &mut self,
        sink: &mut impl MaterialSink,
        models: impl IntoIterator<Item = ModelMeshes<'a>>,
    ) {
        let mut pending: Vec<(String, [Option<MeshId>; 2], MaterialData)> = Vec::new();
        for model in models {
            if self.seen.iter().any(|seen| seen == model.name) {
                continue;
            }
            self.seen.push(model.name.to_owned());
            match self.resolve(sink, model.name) {
                Ok(Some(material)) => {
                    pending.push((model.name.to_owned(), model.meshes, material));
                }
                Ok(None) => {}
                Err(reason) => eprintln!(
                    "[assets] material for model '{}': {reason}; using the vertex-colour look",
                    model.name
                ),
            }
        }
        if pending.is_empty() {
            return;
        }
        let data: Vec<MaterialData> = pending.iter().map(|(_, _, material)| *material).collect();
        let ids = match sink.register_materials(&data) {
            Ok(ids) => ids,
            Err(reason) => {
                for (name, _, _) in &pending {
                    eprintln!(
                        "[assets] material for model '{name}': {reason}; using the vertex-colour look"
                    );
                }
                return;
            }
        };
        for ((name, meshes, _), id) in pending.into_iter().zip(ids) {
            let mut applied = false;
            for mesh in meshes.into_iter().flatten() {
                match sink.set_mesh_material(mesh, id) {
                    Ok(()) => applied = true,
                    Err(reason) => eprintln!(
                        "[assets] material for model '{name}': {reason}; using the vertex-colour look"
                    ),
                }
            }
            if applied {
                eprintln!("[assets] model '{name}' drawn with materials/{name}.toml");
                self.applied.push(name);
            }
        }
    }

    /// The material for `model`: `Ok(None)` when it has no descriptor.
    fn resolve(
        &mut self,
        sink: &mut impl MaterialSink,
        model: &str,
    ) -> Result<Option<MaterialData>, String> {
        let Some(path) = material_path(&self.root, model) else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        let desc = MaterialDesc::load_file(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let mut textures = [None; TextureSlot::COUNT];
        for (slot, name) in desc.bound_textures() {
            textures[slot.index()] = Some(self.texture(sink, slot, name)?);
        }
        Ok(Some(MaterialData::from_desc(&desc, textures)))
    }

    /// Uploads (or reuses) texture `name` for `slot`.
    fn texture(
        &mut self,
        sink: &mut impl MaterialSink,
        slot: TextureSlot,
        name: &str,
    ) -> Result<TextureId, String> {
        let path =
            texture_path(&self.root, name).ok_or_else(|| format!("bad texture name {name}"))?;
        let check = |format: TextureFormat| {
            if slot.accepts(format) {
                Ok(())
            } else {
                Err(format!(
                    "{}: format {format} is not allowed for the {} slot",
                    path.display(),
                    slot.name()
                ))
            }
        };
        // A texture already uploaded for another material is reused, but must still suit
        // this slot (a normal map is not a base colour).
        if let Some(&(_, id, format)) = self.textures.iter().find(|(known, ..)| known == name) {
            check(format)?;
            return Ok(id);
        }
        let data = load_ktx2_file(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        check(data.format())?;
        let id = sink
            .register_texture(&data)
            .map_err(|error| format!("{}: upload failed ({error})", path.display()))?;
        self.textures.push((name.to_owned(), id, data.format()));
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_render::MaterialShader;

    const BASE: &[u8] = include_bytes!("../../../../content/textures/sentinel-base.ktx2");
    const NORMAL: &[u8] = include_bytes!("../../../../content/textures/sentinel-normal.ktx2");

    /// Records calls; ids are handed out in order.
    #[derive(Default)]
    struct FakeRenderer {
        textures: usize,
        batches: Vec<usize>,
        assigned: Vec<(MeshId, MaterialId)>,
        fail_materials: bool,
    }

    impl MaterialSink for FakeRenderer {
        fn register_texture(&mut self, _: &TextureData) -> Result<TextureId, String> {
            self.textures += 1;
            Ok(TextureId::from_index(self.textures as u16))
        }

        fn register_materials(
            &mut self,
            materials: &[MaterialData],
        ) -> Result<Vec<MaterialId>, String> {
            if self.fail_materials {
                return Err(String::from("material limit reached"));
            }
            assert!(
                materials
                    .iter()
                    .all(|material| material.shader == MaterialShader::Textured)
            );
            self.batches.push(materials.len());
            Ok((0..materials.len())
                .map(|index| MaterialId::from_index(index as u16))
                .collect())
        }

        fn set_mesh_material(&mut self, mesh: MeshId, material: MaterialId) -> Result<(), String> {
            self.assigned.push((mesh, material));
            Ok(())
        }
    }

    fn root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "moba-materials-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("materials")).expect("materials dir");
        std::fs::create_dir_all(root.join("textures")).expect("textures dir");
        std::fs::write(root.join("textures/base.ktx2"), BASE).expect("write base");
        std::fs::write(root.join("textures/normal.ktx2"), NORMAL).expect("write normal");
        root
    }

    fn model(name: &str) -> ModelMeshes<'_> {
        ModelMeshes {
            name,
            meshes: [Some(MeshId::CUBE), None],
        }
    }

    #[test]
    fn materials_are_batched_textures_shared_and_failures_fall_back_once() {
        let root = root("batch");
        let good = "shader = \"textured\"\nbase_color = \"base\"\nnormal = \"normal\"\n";
        std::fs::write(root.join("materials/a.toml"), good).expect("write");
        std::fs::write(root.join("materials/b.toml"), good).expect("write");
        // Normal map in the base-colour slot: rejected by the slot rules.
        std::fs::write(
            root.join("materials/swapped.toml"),
            "shader = \"textured\"\nbase_color = \"normal\"\n",
        )
        .expect("write");
        std::fs::write(
            root.join("materials/missing.toml"),
            "shader = \"textured\"\nbase_color = \"gone\"\n",
        )
        .expect("write");
        std::fs::write(root.join("materials/broken.toml"), "shader = 3").expect("write");
        let mut library = MaterialLibrary::new(root.clone());
        let mut renderer = FakeRenderer::default();
        let models = || {
            [
                model("a"),
                model("b"),
                model("swapped"),
                model("missing"),
                model("broken"),
                model("plain"),
            ]
        };
        library.bind(&mut renderer, models());
        assert_eq!(renderer.batches, [2], "one registration call for the bind");
        assert_eq!(renderer.textures, 2, "shared textures upload once");
        assert_eq!(renderer.assigned.len(), 2);
        assert_eq!(library.applied(), ["a", "b"]);
        // A rebind (content reload) does not retry or re-upload anything.
        library.bind(&mut renderer, models());
        assert_eq!(renderer.batches, [2]);
        // A new renderer re-resolves everything.
        library.renderer_recreated();
        library.bind(&mut renderer, models());
        assert_eq!(renderer.batches, [2, 2]);
        assert_eq!(renderer.textures, 4);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_renderer_refusal_keeps_the_vertex_colour_look() {
        let root = root("refuse");
        std::fs::write(
            root.join("materials/a.toml"),
            "shader = \"textured\"\nbase_color = \"base\"\n",
        )
        .expect("write");
        let mut library = MaterialLibrary::new(root.clone());
        let mut renderer = FakeRenderer {
            fail_materials: true,
            ..FakeRenderer::default()
        };
        library.bind(&mut renderer, [model("a")]);
        assert_eq!(renderer.assigned, []);
        assert_eq!(library.applied(), [] as [String; 0]);
        let _ = std::fs::remove_dir_all(root);
    }
}
