//! VRAM accounting, budgets and load-time-only uploads (R8, ADR-0018 §4).
//!
//! Every [`GpuImage`](crate::resources::GpuImage) and [`GpuBuffer`](crate::resources::GpuBuffer)
//! adds its allocation's size to one of three classes when it is created and removes it when it
//! is dropped: **textures** (sampled images that are not attachments: material textures,
//! terrain layers, VAT, environment, LUTs, baked AO), **targets** (attachments: transients,
//! offscreen images, the shadow map) and **buffers**. The **swapchain** images are owned by
//! the WSI, which reports no memory requirements; they are estimated as width × height ×
//! texel size × image count. The allocator's **reserved** bytes (every device memory block it
//! holds, including free space in them) are reported beside the live total. The counters are
//! process-wide atomics, like the renderer's other diagnostics counters; a process runs one
//! renderer at a time, and [`begin_renderer`] resets the per-renderer state.
//!
//! **Budget.** The first time the live total exceeds the tier's total budget, a warning is
//! logged (once per renderer); the peak is in the frame log and `perf-gate` fails a run whose
//! peak is over the budget.
//!
//! **Uploads.** Every staging upload (meshes, textures, terrain) calls [`count_upload`] and
//! every pipeline build [`count_pipelines`]. Between [`set_locked`]`(true)` (the match is
//! playing) and `set_locked(false)` they are also counted as *locked* and the first is logged:
//! the evidence runs require zero (nothing loads mid-match).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ash::vk;

static TEXTURE_BYTES: AtomicU64 = AtomicU64::new(0);
static SWAPCHAIN_BYTES: AtomicU64 = AtomicU64::new(0);
static TOTAL_BUDGET: AtomicU64 = AtomicU64::new(0);
static OVER_BUDGET_LOGGED: AtomicBool = AtomicBool::new(false);
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
    /// Swapchain images (estimated: the WSI owns their memory).
    Swapchain,
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
            Self::Swapchain => &SWAPCHAIN_BYTES,
        }
    }
}

/// Records an allocation of `bytes` in `class`.
pub(crate) fn track_alloc(class: MemoryClass, bytes: u64) {
    class.counter().fetch_add(bytes, Ordering::Relaxed);
    let total = current_total();
    PEAK_BYTES.fetch_max(total, Ordering::Relaxed);
    let budget = TOTAL_BUDGET.load(Ordering::Relaxed);
    if budget > 0 && total > budget && !OVER_BUDGET_LOGGED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "[vram] warning: {:.1} MiB allocated, over the tier's {:.1} MiB budget \
             (ADR-0018 §4: lower the quality tier or the content's texture sizes)",
            total as f64 / MIB,
            budget as f64 / MIB,
        );
    }
}

const MIB: f64 = 1024.0 * 1024.0;

/// Starts a renderer's accounting: the uploads are unlocked (a new renderer loads its assets;
/// a lock left by a previous renderer must not count them), the lock and over-budget
/// warnings are re-armed, and `total_budget` is the budget the warning compares against.
/// The byte counters stay: they follow live objects, which free themselves.
pub(crate) fn begin_renderer(total_budget: u64) {
    LOCKED.store(false, Ordering::Relaxed);
    LOCKED_LOGGED.store(false, Ordering::Relaxed);
    OVER_BUDGET_LOGGED.store(false, Ordering::Relaxed);
    TOTAL_BUDGET.store(total_budget, Ordering::Relaxed);
}

/// Bytes per texel of a swapchain format (4 unless it is a 64-bit format).
pub(crate) fn swapchain_texel_bytes(format: vk::Format) -> u64 {
    match format {
        vk::Format::R16G16B16A16_SFLOAT | vk::Format::R16G16B16A16_UNORM => 8,
        _ => 4,
    }
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
        + SWAPCHAIN_BYTES.load(Ordering::Relaxed)
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

/// Whether uploads are locked now.
pub(crate) fn locked() -> bool {
    LOCKED.load(Ordering::Relaxed)
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
    /// Swapchain images, bytes (estimated: width × height × texel size × images).
    pub swapchain_bytes: u64,
    /// Device memory the allocator holds in blocks, including their free space, bytes (the
    /// driver-side cost of the live total; the swapchain is not in it).
    pub reserved_bytes: u64,
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
    /// Every allocation now, swapchain included, bytes.
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.texture_bytes + self.target_bytes + self.buffer_bytes + self.swapchain_bytes
    }

    /// Whether the peak went over the tier's total budget.
    #[must_use]
    pub const fn over_budget(&self) -> bool {
        self.peak_bytes > self.total_budget
    }
}

/// The counters now, against `budget`, with the allocator's `reserved_bytes`.
pub(crate) fn stats(budget: &moba_assets::TierBudget, reserved_bytes: u64) -> MemoryStats {
    MemoryStats {
        texture_bytes: TEXTURE_BYTES.load(Ordering::Relaxed),
        target_bytes: TARGET_BYTES.load(Ordering::Relaxed),
        buffer_bytes: BUFFER_BYTES.load(Ordering::Relaxed),
        swapchain_bytes: SWAPCHAIN_BYTES.load(Ordering::Relaxed),
        reserved_bytes,
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

    /// R8 review: a lock left set (a renderer dropped mid-match, or a client that never
    /// unlocked) must not count the next renderer's load-time uploads as mid-match ones.
    #[test]
    fn a_new_renderer_starts_unlocked() {
        set_locked(true);
        LOCKED_LOGGED.store(true, Ordering::Relaxed);
        OVER_BUDGET_LOGGED.store(true, Ordering::Relaxed);
        begin_renderer(1 << 30);
        assert!(!locked());
        assert!(!LOCKED_LOGGED.load(Ordering::Relaxed));
        assert!(!OVER_BUDGET_LOGGED.load(Ordering::Relaxed));
        assert_eq!(TOTAL_BUDGET.load(Ordering::Relaxed), 1 << 30);
        let before = LOCKED_UPLOADS.load(Ordering::Relaxed);
        count_upload();
        assert_eq!(
            LOCKED_UPLOADS.load(Ordering::Relaxed),
            before,
            "a load-time upload"
        );
    }

    #[test]
    fn swapchain_texels_follow_the_format() {
        assert_eq!(swapchain_texel_bytes(vk::Format::B8G8R8A8_SRGB), 4);
        assert_eq!(
            swapchain_texel_bytes(vk::Format::A2B10G10R10_UNORM_PACK32),
            4
        );
        assert_eq!(swapchain_texel_bytes(vk::Format::R16G16B16A16_SFLOAT), 8);
    }
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
