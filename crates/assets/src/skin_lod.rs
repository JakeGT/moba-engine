//! Reduced skins for renderer bone-count LOD (R4b, ADR-0012 amendment).
//!
//! [`SkinLod::build`] derives a cheaper skinned mesh from a full one: fewer joints in its bone
//! palette and at most `max_influences` weights per vertex. Joints are dropped leaf first,
//! least skinned weight first (joints named in `drop_first`, such as spring chains, before
//! any other), so the kept set always contains every ancestor of a kept joint. A vertex
//! influence on a dropped joint moves to its nearest kept ancestor, which is exact for a
//! rigidly bound part and makes secondary parts follow their parent. Influences are merged,
//! sorted largest first (so a shader can stop at the first zero), cut to `max_influences` and
//! renormalised to 255.
//!
//! At draw time [`SkinLod::gather`] copies the kept joints' skinning matrices out of the full
//! set the animator computed; the reduced mesh indexes that shorter palette.

use crate::mesh::MeshData;
use crate::skin::{JointMatrix, MAX_JOINTS, MeshSkin, Skeleton, quantize_weights};

/// A reduced skinned mesh and the joints of the full skeleton its palette keeps.
#[derive(Clone, Debug, PartialEq)]
pub struct SkinLod {
    mesh: MeshData,
    /// Full-skeleton joint of each palette entry.
    joints: Vec<u8>,
    max_influences: usize,
}

impl SkinLod {
    /// Builds a reduced skin of `mesh` with at most `max_joints` joints (at least the roots
    /// are always kept) and `max_influences` (1..=4) weights per vertex. `drop_first` names
    /// joints to drop before any other (they are still kept if a kept joint descends from
    /// them).
    ///
    /// Returns `None` for a mesh without a skin or `max_influences` outside 1..=4.
    #[must_use]
    #[allow(clippy::too_many_lines)] // One pass: rank, drop, remap, re-weight, rebuild.
    pub fn build(
        mesh: &MeshData,
        max_joints: usize,
        max_influences: usize,
        drop_first: &[usize],
    ) -> Option<Self> {
        if !(1..=4).contains(&max_influences) {
            return None;
        }
        let skin = mesh.skin()?;
        let skeleton = skin.skeleton();
        let count = skeleton.joint_count();
        let parents = skeleton.parents();
        // Skinned weight per joint; `drop_first` joints rank below every weighted joint.
        let mut importance = [0u64; MAX_JOINTS];
        for (lanes, weights) in skin.joints().iter().zip(skin.weights()) {
            for (&joint, &weight) in lanes.iter().zip(weights) {
                importance[usize::from(joint)] += u64::from(weight);
            }
        }
        for &joint in drop_first.iter().filter(|&&joint| joint < count) {
            importance[joint] = 0;
        }
        let mut kept = [false; MAX_JOINTS];
        kept[..count].fill(true);
        let mut kept_count = count;
        while kept_count > max_joints.max(1) {
            // The least important kept leaf that is not a root; ties drop the later joint.
            let leaf = (0..count)
                .filter(|&joint| kept[joint] && parents[joint].is_some())
                .filter(|&joint| {
                    !(0..count)
                        .any(|child| kept[child] && parents[child].map(usize::from) == Some(joint))
                })
                .min_by_key(|&joint| (importance[joint], core::cmp::Reverse(joint)));
            let Some(leaf) = leaf else {
                break;
            };
            kept[leaf] = false;
            kept_count -= 1;
        }
        // Palette order follows the full skeleton's joint order.
        let mut palette = [u8::MAX; MAX_JOINTS];
        let mut joints = Vec::with_capacity(kept_count);
        for joint in 0..count {
            if kept[joint] {
                palette[joint] = u8::try_from(joints.len()).ok()?;
                joints.push(u8::try_from(joint).ok()?);
            }
        }
        // Nearest kept ancestor (or self) of every joint, as a palette index.
        let mut remap = [0u8; MAX_JOINTS];
        for (joint, target) in remap.iter_mut().enumerate().take(count) {
            let mut at = joint;
            while !kept[at] {
                at = usize::from(parents[at]?);
            }
            *target = palette[at];
        }
        let mut lanes_out = Vec::with_capacity(skin.joints().len());
        let mut weights_out = Vec::with_capacity(skin.weights().len());
        for (lanes, weights) in skin.joints().iter().zip(skin.weights()) {
            let mut merged: [(u8, f32); 4] = [(0, 0.0); 4];
            let mut used = 0;
            for (&joint, &weight) in lanes.iter().zip(weights) {
                if weight == 0 {
                    continue;
                }
                let target = remap[usize::from(joint)];
                if let Some(slot) = merged[..used].iter_mut().find(|(j, _)| *j == target) {
                    slot.1 += f32::from(weight);
                } else {
                    merged[used] = (target, f32::from(weight));
                    used += 1;
                }
            }
            let merged = &mut merged[..used];
            merged.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            let take = used.min(max_influences);
            let mut joint_lanes = [0u8; 4];
            let mut weight_lanes = [0.0f32; 4];
            for (lane, &(joint, weight)) in merged[..take].iter().enumerate() {
                joint_lanes[lane] = joint;
                weight_lanes[lane] = weight;
            }
            let mut quantized = quantize_weights(weight_lanes)?;
            // Keep lanes sorted after rounding (a tie can swap two lanes by one unit).
            let mut order = [0usize, 1, 2, 3];
            order.sort_by(|&a, &b| quantized[b].cmp(&quantized[a]).then(a.cmp(&b)));
            let (sorted_joints, sorted_weights) = (
                order.map(|lane| joint_lanes[lane]),
                order.map(|lane| quantized[lane]),
            );
            quantized = sorted_weights;
            lanes_out.push(sorted_joints);
            weights_out.push(quantized);
        }
        let reduced = Self::reduced_skeleton(skeleton, &joints, &palette)?;
        let reduced_skin = MeshSkin::new(lanes_out, weights_out, reduced).ok()?;
        let mut out = MeshData::new(
            mesh.positions().to_vec(),
            mesh.normals().to_vec(),
            mesh.indices().to_vec(),
        )
        .ok()?;
        if let Some(uvs) = mesh.uvs() {
            out = out.with_uvs(uvs.to_vec()).ok()?;
        }
        if let Some(tangents) = mesh.tangents() {
            out = out.with_tangents(tangents.to_vec()).ok()?;
        }
        Some(Self {
            mesh: out.with_skin(reduced_skin).ok()?,
            joints,
            max_influences,
        })
    }

    /// The kept joints as a skeleton (hierarchy, rest pose, inverse binds and names; no
    /// clips: the animator keeps evaluating the full skeleton).
    fn reduced_skeleton(
        full: &Skeleton,
        joints: &[u8],
        palette: &[u8; MAX_JOINTS],
    ) -> Option<Skeleton> {
        let parents = joints
            .iter()
            .map(|&joint| {
                full.parents()[usize::from(joint)].map(|parent| palette[usize::from(parent)])
            })
            .collect();
        let rest = joints
            .iter()
            .map(|&joint| full.rest()[usize::from(joint)])
            .collect();
        let base = joints
            .iter()
            .map(|&joint| full.base(usize::from(joint)))
            .collect();
        let inverse_bind = joints
            .iter()
            .map(|&joint| full.inverse_bind(usize::from(joint)))
            .collect();
        let names = joints
            .iter()
            .map(|&joint| full.joint_name(usize::from(joint)).map(str::to_owned))
            .collect();
        Skeleton::new(parents, rest, base, inverse_bind, None)
            .ok()?
            .with_joint_names(names)
            .ok()
    }

    /// The reduced mesh (register it as a skinned mesh).
    #[must_use]
    pub fn mesh(&self) -> &MeshData {
        &self.mesh
    }

    /// Joints in the reduced palette.
    #[must_use]
    pub fn joint_count(&self) -> usize {
        self.joints.len()
    }

    /// Most influences per vertex.
    #[must_use]
    pub fn max_influences(&self) -> usize {
        self.max_influences
    }

    /// The full-skeleton joint of each palette entry.
    #[must_use]
    pub fn joints(&self) -> &[u8] {
        &self.joints
    }

    /// Copies the kept joints' matrices from `full` (one per full-skeleton joint) into
    /// `out[..joint_count]`. Returns `false` (and writes nothing) if either is too short.
    /// Never allocates.
    pub fn gather(&self, full: &[JointMatrix], out: &mut [JointMatrix]) -> bool {
        if out.len() < self.joints.len()
            || self
                .joints
                .iter()
                .any(|&joint| usize::from(joint) >= full.len())
        {
            return false;
        }
        for (target, &joint) in out.iter_mut().zip(&self.joints) {
            *target = full[usize::from(joint)];
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::SkinLod;
    use crate::mesh::MeshData;
    use crate::skin::{JointTransform, MeshSkin, Skeleton};
    use moba_core::math::{Mat4, Vec3};

    /// `root(0) → spine(1) → arm(2)`; `root → cape(3) → cape_tip(4)`.
    fn mesh() -> MeshData {
        let rest = vec![JointTransform::IDENTITY; 5];
        let parents = vec![None, Some(0), Some(1), Some(0), Some(3)];
        let skeleton = Skeleton::new(
            parents,
            rest,
            vec![Mat4::IDENTITY; 5],
            (0..5)
                .map(|joint| Mat4::from_translation(Vec3::splat(-(joint as f32))))
                .collect(),
            None,
        )
        .expect("skeleton")
        .with_joint_names(
            ["root", "spine", "arm", "cape", "cape_tip"]
                .map(|name| Some(name.to_owned()))
                .to_vec(),
        )
        .expect("names");
        let positions = vec![[0.0; 3]; 4];
        let normals = vec![[0.0, 1.0, 0.0]; 4];
        let skin = MeshSkin::new(
            vec![[2, 1, 0, 0], [1, 0, 0, 0], [4, 3, 0, 0], [0, 0, 0, 0]],
            vec![
                [128, 64, 63, 0],
                [255, 0, 0, 0],
                [200, 55, 0, 0],
                [255, 0, 0, 0],
            ],
            skeleton,
        )
        .expect("skin");
        MeshData::new(positions, normals, vec![0, 1, 2, 1, 2, 3])
            .expect("mesh")
            .with_skin(skin)
            .expect("skinned")
    }

    #[test]
    fn drops_secondary_leaves_first_and_collapses_to_ancestors() {
        let full = mesh();
        // Drop the cape chain first: 3 joints left (root, spine, arm), 2 influences.
        let lod = SkinLod::build(&full, 3, 2, &[3, 4]).expect("lod");
        assert_eq!(lod.joints(), &[0, 1, 2]);
        let skin = lod.mesh().skin().expect("skin");
        assert_eq!(skin.skeleton().joint_count(), 3);
        assert_eq!(skin.skeleton().joint_name(2), Some("arm"));
        // Vertex 0: arm 128, spine 64, root 63 → top two renormalised, sorted.
        assert_eq!(skin.joints()[0][..2], [2, 1]);
        assert_eq!(skin.weights()[0], [170, 85, 0, 0]);
        // Vertex 2 was all cape: both influences collapse into the root.
        assert_eq!(
            (skin.joints()[2][0], skin.weights()[2]),
            (0, [255, 0, 0, 0])
        );
        // One influence, two joints: the arm (least weight among leaves) goes too.
        let coarse = SkinLod::build(&full, 2, 1, &[3, 4]).expect("lod");
        assert_eq!(coarse.joints(), &[0, 1]);
        assert!(
            coarse
                .mesh()
                .skin()
                .expect("skin")
                .weights()
                .iter()
                .all(|w| w[1..] == [0, 0, 0])
        );
        // Gather picks the kept joints' matrices.
        let matrices: Vec<_> = (0..5).map(|joint| [[joint as f32; 4]; 3]).collect();
        let mut out = [[[0.0; 4]; 3]; 3];
        assert!(lod.gather(&matrices, &mut out));
        assert_eq!(out.map(|m| m[0][0]), [0.0, 1.0, 2.0]);
        assert!(!lod.gather(&matrices[..2], &mut out));
        assert!(SkinLod::build(&full, 3, 0, &[]).is_none());
    }

    #[test]
    fn the_full_budget_keeps_every_joint() {
        let lod = SkinLod::build(&mesh(), 64, 4, &[]).expect("lod");
        assert_eq!(lod.joint_count(), 5);
        assert_eq!(lod.max_influences(), 4);
    }
}
