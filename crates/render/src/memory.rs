//! VRAM accounting, budgets and load-time-only uploads (R8, ADR-0018 §4).
//!
//! Every [`GpuImage`](crate::resources::GpuImage) and [`GpuBuffer`](crate::resources::GpuBuffer)
//! adds its allocation's size to one of three classes when it is created and removes it when it
//! is dropped: **textures** (sampled images that are not attachments: material textures,
//! terrain layers, VAT, environment, LUTs, baked AO), **targets** (attachments: transients,
//! offscreen images, the shadow map) and **buffers**. The counters are process-wide atomics,
//! like the renderer's other diagnostics counters; a process runs one renderer.
//!
//! **Uploads.** Every staging upload (meshes, textures, terrain) calls [`count_upload`] and
//! every pipeline build [`count_pipelines`]. Between [`set_locked`]`(true)` (the match is
//! playing) and `set_locked(false)` they are also counted as *locked* and the first is logged:
//! the evidence runs require zero (nothing loads mid-match).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ash::vk;

static TEXTURE_BYTES: AtomicU64 = AtomicU64::new(0);
static TARGET_BYTES: AtomicU64 = AtomicU64::new(0);
static BUFFER_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_BYTES: AtomicU64 = AtomicU64::new(0);
static UPLOADS: AtomicU64 = AtomicU64::new(0);
static LOCKED: AtomicBool = AtomicBool::new(false);
static LOCKED_UPLOADS: AtomicU64 = AtomicU64::new(0);
static LOCKED_PIPELINES: AtomicU64 = AtomicU64::new(0);
static LOCKED_LOGGED: AtomicBool = AtomicBool::new(false);
static DEGRADED_TEXTURES: AtomicU64 = AtomicU64::new(0);
static REFUSED_TEXTURES: AtomicU64 = AtomicU64::new(0);

/// The accounting class of an allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemoryClass {
    Texture,
    Target,
    Buffer,
}

impl MemoryClass {
    /// An image's class from its usage: sampled without any attachment or storage use is a
    /// texture uploaded from an asset; anything else is a render target.
    pub(crate) fn of_image(usage: vk::ImageUsageFlags) -> Self {
        let target = vk::ImageUsageFlags::COLOR_ATTACHMENT
            | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
            | vk::ImageUsageFlags::STORAGE
            | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT;
        if usage.contains(vk::ImageUsageFlags::SAMPLED) && !usage.intersects(target) {
            Self::Texture
        } else {
            Self::Target
        }
    }

    fn counter(self) -> &'static AtomicU64 {
        match self {
            Self::Texture => &TEXTURE_BYTES,
            Self::Target => &TARGET_BYTES,
            Self::Buffer => &BUFFER_BYTES,
        }
    }
}

/// Records an allocation of `bytes` in `class`.
pub(crate) fn track_alloc(class: MemoryClass, bytes: u64) {
    class.counter().fetch_add(bytes, Ordering::Relaxed);
    let total = current_total();
    PEAK_BYTES.fetch_max(total, Ordering::Relaxed);
}

/// Records that an allocation of `bytes` in `class` was freed.
pub(crate) fn track_free(class: MemoryClass, bytes: u64) {
    // Every free pairs with the `track_alloc` of the same object, so this never underflows.
    class.counter().fetch_sub(bytes, Ordering::Relaxed);
}

fn current_total() -> u64 {
    TEXTURE_BYTES.load(Ordering::Relaxed)
        + TARGET_BYTES.load(Ordering::Relaxed)
        + BUFFER_BYTES.load(Ordering::Relaxed)
}

/// Bytes of textures allocated now.
pub(crate) fn texture_bytes() -> u64 {
    TEXTURE_BYTES.load(Ordering::Relaxed)
}

fn note_locked(what: &str) {
    if !LOCKED_LOGGED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "[vram] {what} while the match is playing: assets must load at match or lobby start \
             (ADR-0018)"
        );
    }
}

/// Records one staging upload (a mesh, texture or terrain).
pub(crate) fn count_upload() {
    UPLOADS.fetch_add(1, Ordering::Relaxed);
    if LOCKED.load(Ordering::Relaxed) {
        LOCKED_UPLOADS.fetch_add(1, Ordering::Relaxed);
        note_locked("an asset upload");
    }
}

/// Records `count` pipeline builds.
pub(crate) fn count_pipelines(count: u64) {
    if count > 0 && LOCKED.load(Ordering::Relaxed) {
        LOCKED_PIPELINES.fetch_add(count, Ordering::Relaxed);
        note_locked("a pipeline build");
    }
}

/// Starts (`true`) or ends (`false`) the locked, mid-match period.
pub(crate) fn set_locked(locked: bool) {
    LOCKED.store(locked, Ordering::Relaxed);
}

/// Records a texture uploaded with its largest mips dropped to fit the budget.
pub(crate) fn note_degraded() {
    DEGRADED_TEXTURES.fetch_add(1, Ordering::Relaxed);
}

/// Records a texture refused because it did not fit the budget even at its smallest level.
pub(crate) fn note_refused() {
    REFUSED_TEXTURES.fetch_add(1, Ordering::Relaxed);
}

/// Device memory the renderer uses and its tier budgets ([`Renderer::memory_stats`](crate::Renderer::memory_stats)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MemoryStats {
    /// Textures uploaded from assets, bytes.
    pub texture_bytes: u64,
    /// Render targets (attachments, transients, offscreen images, the shadow map), bytes.
    pub target_bytes: u64,
    /// Buffers (meshes, per-frame uniforms and instances, staging while it lives), bytes.
    pub buffer_bytes: u64,
    /// The largest total seen so far, bytes.
    pub peak_bytes: u64,
    /// The tier's texture budget, bytes.
    pub texture_budget: u64,
    /// The tier's total budget, bytes.
    pub total_budget: u64,
    /// Textures uploaded with mips dropped to fit the texture budget.
    pub degraded_textures: u64,
    /// Textures refused because they did not fit at any level.
    pub refused_textures: u64,
    /// Staging uploads since start.
    pub uploads: u64,
    /// Staging uploads while uploads were locked (the match playing); 0 is the contract.
    pub locked_uploads: u64,
    /// Pipeline builds while uploads were locked; 0 is the contract.
    pub locked_pipelines: u64,
    /// Whether uploads are locked now.
    pub locked: bool,
}

impl MemoryStats {
    /// Every allocation now, bytes.
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.texture_bytes + self.target_bytes + self.buffer_bytes
    }
}

/// The counters now, against `budget`.
pub(crate) fn stats(budget: &moba_assets::TierBudget) -> MemoryStats {
    MemoryStats {
        texture_bytes: TEXTURE_BYTES.load(Ordering::Relaxed),
        target_bytes: TARGET_BYTES.load(Ordering::Relaxed),
        buffer_bytes: BUFFER_BYTES.load(Ordering::Relaxed),
        peak_bytes: PEAK_BYTES.load(Ordering::Relaxed),
        texture_budget: budget.texture_bytes,
        total_budget: budget.total_bytes,
        degraded_textures: DEGRADED_TEXTURES.load(Ordering::Relaxed),
        refused_textures: REFUSED_TEXTURES.load(Ordering::Relaxed),
        uploads: UPLOADS.load(Ordering::Relaxed),
        locked_uploads: LOCKED_UPLOADS.load(Ordering::Relaxed),
        locked_pipelines: LOCKED_PIPELINES.load(Ordering::Relaxed),
        locked: LOCKED.load(Ordering::Relaxed),
    }
}

/// How many of `data`'s largest mip levels to drop so its upload fits `free` bytes: `Some(0)`
/// when it fits as is, `None` when even its smallest level does not (or it is a cube map,
/// whose levels are never dropped). Pure, for the degradation policy's tests.
pub(crate) fn mips_to_drop(data: &moba_assets::TextureData, free: u64) -> Option<u32> {
    let mut bytes: u64 = data.data().len() as u64;
    for drop in 0..data.level_count() {
        if bytes <= free {
            return Some(drop);
        }
        if data.is_cube() {
            return None;
        }
        let (_, _, level) = data.level(drop)?;
        bytes = bytes.saturating_sub(level.len() as u64);
    }
    None
}

/// `data` without its `drop` largest mip levels (`drop < level_count`).
pub(crate) fn without_top_mips(
    data: &moba_assets::TextureData,
    drop: u32,
) -> Option<moba_assets::TextureData> {
    let (width, height, _) = data.level(drop)?;
    let levels: Vec<Vec<u8>> = (drop..data.level_count())
        .map(|level| data.level(level).map(|(_, _, bytes)| bytes.to_vec()))
        .collect::<Option<_>>()?;
    moba_assets::TextureData::new(data.format(), width, height, &levels).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_assets::{TextureData, TextureFormat};

    fn mipped(size: u32) -> TextureData {
        let levels: Vec<Vec<u8>> = (0..moba_assets::full_mip_chain(size, size))
            .map(|level| {
                let (w, h) = moba_assets::mip_size(size, size, level);
                vec![level as u8; TextureFormat::Rgba8Unorm.level_bytes(w, h).expect("size")]
            })
            .collect();
        TextureData::new(TextureFormat::Rgba8Unorm, size, size, &levels).expect("texture")
    }

    #[test]
    fn images_are_textures_only_when_sampled_and_never_attached() {
        use vk::ImageUsageFlags as U;
        assert_eq!(
            MemoryClass::of_image(U::SAMPLED | U::TRANSFER_DST),
            MemoryClass::Texture
        );
        assert_eq!(
            MemoryClass::of_image(U::SAMPLED | U::COLOR_ATTACHMENT),
            MemoryClass::Target
        );
        assert_eq!(
            MemoryClass::of_image(U::DEPTH_STENCIL_ATTACHMENT),
            MemoryClass::Target
        );
    }

    #[test]
    fn over_budget_textures_drop_their_largest_mips_until_they_fit() {
        let texture = mipped(64);
        let full = texture.data().len() as u64; // 64² + 32² + … RGBA8.
        assert_eq!(mips_to_drop(&texture, full), Some(0));
        assert_eq!(mips_to_drop(&texture, full - 1), Some(1));
        assert_eq!(mips_to_drop(&texture, full - 64 * 64 * 4), Some(1));
        assert_eq!(
            mips_to_drop(&texture, 4),
            Some(6),
            "only the 1×1 level is left"
        );
        assert_eq!(mips_to_drop(&texture, 3), None, "nothing fits");
        let smaller = without_top_mips(&texture, 2).expect("16² chain");
        assert_eq!((smaller.width(), smaller.height()), (16, 16));
        assert_eq!(smaller.level_count(), texture.level_count() - 2);
        assert_eq!(smaller.level(0).map(|(_, _, bytes)| bytes[0]), Some(2));
    }
}
