//! Persistent `VkPipelineCache` (AAA graphics roadmap R1.5).
//!
//! Every pipeline (each shader permutation included) is built when the renderer is created,
//! through one pipeline cache, so nothing compiles mid-match. The cache's contents are saved
//! to a per-user file and fed back on the next start, so warm starts skip most driver
//! compilation.
//!
//! The file is untrusted input (a stale cache from another driver, a truncated write, or
//! garbage): drivers are required to validate the data, but several have crashed on
//! corrupted caches, so it is checked before the driver sees it. The file is
//!
//! ```text
//! "MOBAPSO1"  u64 payload length  u64 FNV-1a 64 of the payload  payload
//! ```
//!
//! and the payload must be at most [`MAX_PIPELINE_CACHE_BYTES`], with a
//! `VkPipelineCacheHeaderVersionOne` header whose vendor id, device id and
//! `pipelineCacheUUID` match this device. Anything else starts an empty cache (and says why).
//! Saving writes a temporary file next to the target and renames it over the old one, so a
//! crash mid-write leaves the previous cache intact.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use ash::vk;

/// Largest cache payload loaded or saved (64 MiB).
pub(crate) const MAX_PIPELINE_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"MOBAPSO1";
const WRAPPER_BYTES: usize = 8 + 8 + 8;
/// `VkPipelineCacheHeaderVersionOne`: four `u32`s and a 16-byte UUID.
const VK_HEADER_BYTES: usize = 32;

/// How the pipeline cache started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PipelineCacheState {
    /// No cache file configured ([`RenderConfig::pipeline_cache`](crate::RenderConfig)).
    Disabled,
    /// No file yet: a cold start; the cache is saved on shutdown.
    Cold,
    /// A valid file for this device was loaded.
    Warm,
    /// A file existed but was rejected; the cache started empty. A corrupt, truncated or
    /// oversized file is deleted at once; a file from another driver/device is kept. Either is
    /// replaced when this device's driver returns reusable cache data on save (some, such as
    /// lavapipe, never do, and then nothing is written).
    Rejected(PipelineCacheRejection),
}

/// Why a pipeline cache file was not used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PipelineCacheRejection {
    /// The file could not be read.
    Unreadable,
    /// Too short, wrong magic, a length that disagrees with the file, or over the cap.
    Malformed,
    /// The checksum does not match the payload.
    Checksum,
    /// The Vulkan header is not `VK_PIPELINE_CACHE_HEADER_VERSION_ONE`.
    HeaderVersion,
    /// Written by another vendor, device or driver build (`pipelineCacheUUID`).
    OtherDevice,
}

impl core::fmt::Display for PipelineCacheState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Disabled => f.write_str("disabled"),
            Self::Cold => f.write_str("cold"),
            Self::Warm => f.write_str("warm"),
            Self::Rejected(reason) => write!(f, "rejected ({reason:?})"),
        }
    }
}

/// FNV-1a, 64-bit: a cheap integrity check (not a security boundary; the driver still
/// validates the data it parses).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The identity a cache payload must match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CacheIdentity {
    pub(crate) vendor_id: u32,
    pub(crate) device_id: u32,
    pub(crate) uuid: [u8; vk::UUID_SIZE],
}

impl CacheIdentity {
    pub(crate) fn of(properties: &vk::PhysicalDeviceProperties) -> Self {
        Self {
            vendor_id: properties.vendor_id,
            device_id: properties.device_id,
            uuid: properties.pipeline_cache_uuid,
        }
    }
}

/// Checks a cache file's bytes and returns the payload to hand to the driver.
pub(crate) fn validate(
    file: &[u8],
    identity: CacheIdentity,
) -> Result<&[u8], PipelineCacheRejection> {
    use PipelineCacheRejection as R;
    if file.len() < WRAPPER_BYTES || &file[..8] != MAGIC {
        return Err(R::Malformed);
    }
    let length = u64::from_le_bytes(file[8..16].try_into().map_err(|_| R::Malformed)?);
    let checksum = u64::from_le_bytes(file[16..24].try_into().map_err(|_| R::Malformed)?);
    let payload = &file[WRAPPER_BYTES..];
    if length != payload.len() as u64 || payload.len() > MAX_PIPELINE_CACHE_BYTES {
        return Err(R::Malformed);
    }
    if fnv1a64(payload) != checksum {
        return Err(R::Checksum);
    }
    if payload.len() < VK_HEADER_BYTES {
        return Err(R::Malformed);
    }
    let word = |index: usize| {
        u32::from_le_bytes(
            payload[4 * index..4 * index + 4]
                .try_into()
                .expect("four bytes"),
        )
    };
    let header_size = word(0) as usize;
    if header_size < VK_HEADER_BYTES
        || header_size > payload.len()
        || word(1) != vk::PipelineCacheHeaderVersion::ONE.as_raw() as u32
    {
        return Err(R::HeaderVersion);
    }
    if word(2) != identity.vendor_id
        || word(3) != identity.device_id
        || payload[16..32] != identity.uuid
    {
        return Err(R::OtherDevice);
    }
    Ok(payload)
}

/// Wraps a driver payload for saving.
pub(crate) fn wrap(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(WRAPPER_BYTES + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(&fnv1a64(payload).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Reads the cache file at `path`: `Ok(None)` when there is none (cold start).
pub(crate) fn read(path: &Path) -> Result<Option<Vec<u8>>, PipelineCacheRejection> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(PipelineCacheRejection::Unreadable),
    };
    if !metadata.is_file() {
        return Err(PipelineCacheRejection::Unreadable);
    }
    let cap = (MAX_PIPELINE_CACHE_BYTES + WRAPPER_BYTES) as u64;
    if metadata.len() > cap {
        return Err(PipelineCacheRejection::Malformed);
    }
    // `take` bounds the read even if the file grows after the metadata check.
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(path)
        .and_then(|file| file.take(cap + 1).read_to_end(&mut bytes))
        .map_err(|_| PipelineCacheRejection::Unreadable)?;
    if bytes.len() as u64 > cap {
        return Err(PipelineCacheRejection::Malformed);
    }
    Ok(Some(bytes))
}

/// Removes a cache file that was rejected as corrupt (so a driver whose own data is not
/// reusable, such as lavapipe, does not keep re-reading garbage). A file written for another
/// device is kept: it is valid for that device, and one file serves whichever GPU last saved.
pub(crate) fn discard_rejected(path: &Path, reason: PipelineCacheRejection) {
    if !matches!(
        reason,
        PipelineCacheRejection::OtherDevice | PipelineCacheRejection::Unreadable
    ) {
        let _ = std::fs::remove_file(path);
    }
}

/// Writes `payload` atomically (temporary file + rename) to `path`.
pub(crate) fn write(path: &Path, payload: &[u8]) -> std::io::Result<()> {
    if payload.len() > MAX_PIPELINE_CACHE_BYTES {
        return Err(std::io::Error::other("pipeline cache larger than the cap"));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut temporary: PathBuf = path.to_path_buf();
    temporary
        .as_mut_os_string()
        .push(format!(".tmp{}", std::process::id()));
    // A stale temporary (or a planted symlink) is removed, not followed: `create_new` refuses
    // to open anything that already exists.
    let _ = std::fs::remove_file(&temporary);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(&wrap(payload))?;
            // Durable before the rename, so a crash cannot leave a renamed but empty file.
            file.sync_all()
        });
    written
        .and_then(|()| std::fs::rename(&temporary, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&temporary);
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: CacheIdentity = CacheIdentity {
        vendor_id: 0x10de,
        device_id: 0x2c05,
        uuid: [7; vk::UUID_SIZE],
    };

    fn payload(identity: CacheIdentity) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&32u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&identity.vendor_id.to_le_bytes());
        bytes.extend_from_slice(&identity.device_id.to_le_bytes());
        bytes.extend_from_slice(&identity.uuid);
        bytes.extend_from_slice(b"driver data");
        bytes
    }

    #[test]
    fn a_wrapped_cache_for_this_device_round_trips() {
        let data = payload(IDENTITY);
        let file = wrap(&data);
        assert_eq!(validate(&file, IDENTITY), Ok(&data[..]));
    }

    #[test]
    fn corrupt_stale_and_foreign_caches_are_rejected() {
        use PipelineCacheRejection as R;
        let file = wrap(&payload(IDENTITY));
        assert_eq!(validate(&file[..10], IDENTITY), Err(R::Malformed));
        assert_eq!(
            validate(&file[..file.len() - 1], IDENTITY),
            Err(R::Malformed)
        );
        let mut magic = file.clone();
        magic[0] = b'X';
        assert_eq!(validate(&magic, IDENTITY), Err(R::Malformed));
        let mut flipped = file.clone();
        *flipped.last_mut().unwrap() ^= 1;
        assert_eq!(validate(&flipped, IDENTITY), Err(R::Checksum));
        let other_driver = CacheIdentity {
            uuid: [8; vk::UUID_SIZE],
            ..IDENTITY
        };
        assert_eq!(validate(&file, other_driver), Err(R::OtherDevice));
        let other_vendor = CacheIdentity {
            vendor_id: 0x1002,
            ..IDENTITY
        };
        assert_eq!(validate(&file, other_vendor), Err(R::OtherDevice));
        let mut version = payload(IDENTITY);
        version[4] = 2;
        assert_eq!(validate(&wrap(&version), IDENTITY), Err(R::HeaderVersion));
        let mut short_header = payload(IDENTITY);
        short_header[0] = 8;
        assert_eq!(
            validate(&wrap(&short_header), IDENTITY),
            Err(R::HeaderVersion)
        );
        assert_eq!(validate(&wrap(&[1, 2, 3]), IDENTITY), Err(R::Malformed));
        // Garbage of any length never panics.
        for length in 0..64 {
            let _ = validate(&vec![0xAB; length], IDENTITY);
        }
    }

    #[test]
    fn files_are_written_atomically_and_read_back() {
        let dir = std::env::temp_dir().join(format!("moba-pso-{}", std::process::id()));
        let path = dir.join("nested").join("pipeline-cache.bin");
        assert_eq!(read(&path), Ok(None), "missing file is a cold start");
        let data = payload(IDENTITY);
        write(&path, &data).expect("write");
        let file = read(&path).expect("readable").expect("present");
        assert_eq!(validate(&file, IDENTITY), Ok(&data[..]));
        assert_eq!(
            read(&dir),
            Err(PipelineCacheRejection::Unreadable),
            "a directory"
        );
        // A stale temporary from a crashed save does not block the next one.
        let mut stale = path.clone();
        stale
            .as_mut_os_string()
            .push(format!(".tmp{}", std::process::id()));
        std::fs::write(&stale, b"stale").expect("stale temp");
        write(&path, &data).expect("write over a stale temporary");
        assert!(!stale.exists());
        // Corrupt files are discarded; files from another device are kept.
        discard_rejected(&path, PipelineCacheRejection::OtherDevice);
        assert!(path.exists());
        discard_rejected(&path, PipelineCacheRejection::Checksum);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
