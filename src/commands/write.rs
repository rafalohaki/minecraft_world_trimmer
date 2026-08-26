use crate::commands::optimize_result::{OptimizeResult, reduce_optimize_results};
use crate::region_loader::region::{ParseRegionError, read_region_file, rewrite_region_bytes};
use crate::world::get_region_files::get_region_files;
use flate2::Compression;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::iter::ParallelIterator;
use rayon::prelude::IntoParallelRefIterator;
use std::error::Error;
use std::fs::{File, Permissions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic per-process counter used to disambiguate concurrent tempfile names
/// (was previously derived from parsing `Debug` of `ThreadId`, which is not stable API).
static TEMPFILE_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn execute_write(
    world_paths: &[PathBuf],
    compression: Compression,
    backup_dir: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    // Surface an invalid backup location before touching any world file.
    if let Some(backup_root) = backup_dir {
        std::fs::create_dir_all(backup_root)?;
    }

    let world = get_region_files(world_paths)?;
    let entries = &world.region_files;
    let pb = ProgressBar::new(entries.len() as u64);
    let style = ProgressStyle::with_template(
        "{percent}% {bar} {pos}/{len} [{elapsed_precise}>{eta_precise}, {per_sec}]",
    )
    .unwrap();
    pb.set_style(style);

    let mut results = entries
        .par_iter()
        .map(|entry| {
            let result = optimize_write(entry, compression, backup_dir, world_paths);
            pb.inc(1);
            result
        })
        .collect::<Vec<OptimizeResult>>();

    pb.finish();
    let result = reduce_optimize_results(&mut results);
    println!("{result}");
    if let Some(backup_root) = backup_dir {
        println!(
            "Originals preserved in {backup_root:?} — delete this directory only after verifying the world loads correctly."
        );
    }

    Ok(())
}

/// Path of `region_file` mirrored under the backup root. The longest matching
/// world root is stripped so files from different dimensions/worlds keep their
/// relative structure (`world/region/r.0.0.mca`, `world/DIM-1/region/...`).
fn backup_path_for(region_file: &Path, backup_root: &Path, world_paths: &[PathBuf]) -> PathBuf {
    let relative = world_paths
        .iter()
        .filter(|root| region_file.starts_with(root))
        .filter_map(|root| region_file.strip_prefix(root).ok())
        .max_by_key(|rel| rel.as_os_str().len())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            // Not under any known world root (should not happen); fall back
            // to the bare file name so nothing is lost.
            region_file
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("unknown.mca"))
        });
    backup_root.join(relative)
}

/// Move the original region file into the backup directory (same-volume rename:
/// O(1), no extra disk space beyond what the trimmed replacement will use).
/// Never overwrites an existing backup: a second run against the same backup
/// dir must not destroy the (older, closer-to-original) file from the first
/// run, so a numeric suffix is appended instead.
fn quarantine_original(
    region_file: &Path,
    backup_root: &Path,
    world_paths: &[PathBuf],
) -> std::io::Result<PathBuf> {
    let mut destination = backup_path_for(region_file, backup_root, world_paths);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if destination.exists() {
        let base_name = destination
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        destination = (1_u32..=10_000)
            .map(|n| {
                let mut name = base_name.clone();
                name.push(format!(".{n}"));
                destination.with_file_name(name)
            })
            .find(|candidate| !candidate.exists())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "no free backup name after 10000 attempts",
                )
            })?;
    }
    std::fs::rename(region_file, &destination)?;
    Ok(destination)
}

/// Remove the region file from the world: MOVE it into the backup dir when one
/// is configured, otherwise delete it.
fn remove_or_quarantine(
    region_file: &Path,
    backup_dir: Option<&Path>,
    world_paths: &[PathBuf],
    result: &mut OptimizeResult,
) {
    let removal = match backup_dir {
        Some(backup_root) => quarantine_original(region_file, backup_root, world_paths).map(|_| ()),
        None => std::fs::remove_file(region_file),
    };
    match removal {
        Ok(()) => result.deleted_regions += 1,
        Err(_) => result.io_errors += 1,
    }
}

fn optimize_write(
    region_file_path: &Path,
    compression: Compression,
    backup_dir: Option<&Path>,
    world_paths: &[PathBuf],
) -> OptimizeResult {
    let mut result = OptimizeResult::default();

    // Fast path: most regions in a snapshot world have 0 deletable chunks.
    // `rewrite_region_bytes` decompresses + re-serializes every kept chunk, so
    // calling it for every file wastes ~50% of CPU (the 59/s → 120/s gap the
    // user observed on world.SNAPSHOT with 70k regions). Do a cheap
    // decompress+parse-only scan first; only pay the recompression cost when
    // something actually needs deleting.
    let bytes = match read_region_file(region_file_path) {
        Ok(b) => b,
        Err(ParseRegionError::HeaderError) => {
            remove_or_quarantine(region_file_path, backup_dir, world_paths, &mut result);
            return result;
        }
        Err(ParseRegionError::ReadError) => {
            result.io_errors += 1;
            return result;
        }
    };

    // Cheap scan: no recompression, just decide if we need to rewrite at all.
    let stats = match crate::region_loader::region::analyze_region_bytes(&bytes) {
        Ok(s) => s,
        Err(ParseRegionError::HeaderError) => {
            remove_or_quarantine(region_file_path, backup_dir, world_paths, &mut result);
            return result;
        }
        Err(ParseRegionError::ReadError) => {
            result.io_errors += 1;
            return result;
        }
    };

    if stats.deletable_chunks == 0 {
        // Nothing to delete (and `analyze` already proved the file is
        // header-valid) — skip the expensive recompression entirely.
        result.total_chunks += stats.total_chunks;
        result.preserved_opaque_chunks += stats.opaque_chunks;
        result.corrupt_chunks += stats.corrupt_chunks;
        // A region holding no chunk data at all (empty location table) is
        // deleted as a whole file, matching check-mode accounting.
        if stats.total_chunks == 0 && stats.corrupt_chunks == 0 {
            remove_or_quarantine(region_file_path, backup_dir, world_paths, &mut result);
        }
        return result;
    }

    // At this point we know at least one chunk is deletable, so pay the cost.
    let outcome = match rewrite_region_bytes(&bytes, compression) {
        Ok(o) => o,
        Err(ParseRegionError::HeaderError) => {
            remove_or_quarantine(region_file_path, backup_dir, world_paths, &mut result);
            return result;
        }
        Err(ParseRegionError::ReadError) => {
            result.io_errors += 1;
            return result;
        }
    };

    result.total_chunks += outcome.total_chunks;
    result.deleted_chunks += outcome.deleted_chunks;
    result.preserved_opaque_chunks += outcome.opaque_chunks;
    result.corrupt_chunks += outcome.corrupt_chunks;

    if outcome.remaining_chunks == 0 && outcome.corrupt_chunks == 0 {
        // Every chunk was removed: the whole region file goes away. Never
        // whole-delete a file containing chunks we could not parse — matching
        // check-mode accounting, which refuses to count such regions as
        // deletable.
        remove_or_quarantine(region_file_path, backup_dir, world_paths, &mut result);
        return result;
    }

    if !outcome.modified {
        // Nothing changed — leave the original file untouched.
        return result;
    }

    // With a backup dir configured, never destroy the original without
    // having moved it aside first.
    if let Some(backup_root) = backup_dir
        && quarantine_original(region_file_path, backup_root, world_paths).is_err()
    {
        result.io_errors += 1;
        eprintln!(
            "Failed to quarantine original before rewrite: {:?}",
            region_file_path
        );
        return result;
    }
    if outcome.compression_fallbacks > 0 {
        result.compression_failures += outcome.compression_fallbacks;
        result.regions_with_compression_issues += 1;
        eprintln!(
            "Compression fallback in {} chunk(s) for {:?}",
            outcome.compression_fallbacks, region_file_path
        );
    }
    if outcome.header_write_failures > 0 {
        result.header_write_failures += outcome.header_write_failures;
        result.regions_with_header_issues += 1;
        eprintln!(
            "Header write failure: skipped payload for {} chunk(s) in {:?}",
            outcome.header_write_failures, region_file_path
        );
    }
    if atomic_write_region(region_file_path, &outcome.bytes).is_err() {
        result.io_errors += 1;
    }

    result
}

/// Atomic + durable replacement of a region file.
///
/// Flow:
///   1. Read original file metadata (to preserve permissions across rename).
///   2. Create sibling tempfile in the same directory (so `rename` is on the same
///      filesystem and atomic on POSIX; `ReplaceFile` on Windows).
///   3. Write payload, `flush` the `BufWriter`, then `sync_all` the file descriptor.
///      `sync_all` forces data + metadata to disk (`fsync`), which is required for
///      durability across kernel/power loss. Without this, after a crash the rename
///      could be visible while the file content is still zero-bytes or stale.
///   4. Restore original permissions on the tempfile (rename keeps the new inode).
///   5. `rename(tmp, target)`. POSIX guarantees this is atomic and durable for the
///      *directory entry*, but the *parent directory's* metadata still has to be
///      fsynced to make the rename itself survive a crash on POSIX.
///   6. Best-effort `sync_all` on the parent directory handle. Silent failure is
///      acceptable here — on platforms where opening a directory or fsyncing it is
///      not supported (some Windows configurations), the journaling filesystem
///      already provides equivalent ordering guarantees.
fn atomic_write_region(region_file_path: &Path, payload: &[u8]) -> std::io::Result<()> {
    let tmp_path = tempfile_path_for(region_file_path);
    let original_permissions: Option<Permissions> = std::fs::metadata(region_file_path)
        .ok()
        .map(|m| m.permissions());

    let write_result = (|| -> std::io::Result<()> {
        let file = File::create(&tmp_path)?;
        // 128 KiB is plenty: `payload` is written via a single `write_all`, so the
        // buffer only needs to cover syscall coalescing. 32 MiB previously used
        // here multiplied peak RSS by ~32 MiB per rayon writer thread (≈512 MiB
        // on a 16-core machine) with no measurable throughput gain.
        let mut writer = BufWriter::with_capacity(128 * 1024, file);
        writer.write_all(payload)?;
        writer.flush()?;
        let file = writer.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    if let Some(perms) = original_permissions {
        // Best-effort: do not abort the trim if perm-restore fails (e.g. cross-platform diffs).
        let _ = std::fs::set_permissions(&tmp_path, perms);
    }

    if let Err(e) = std::fs::rename(&tmp_path, region_file_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    // Best-effort directory fsync for durability of the rename itself.
    // On POSIX this is the standard atomic-rename idiom. On Windows/some FSes
    // opening a directory or fsyncing it may not be supported — we treat any
    // failure as non-fatal because journaled filesystems already enforce the
    // necessary ordering.
    if let Some(dir) = region_file_path.parent()
        && let Ok(dir_handle) = File::open(dir)
    {
        let _ = dir_handle.sync_all();
    }

    Ok(())
}

/// Build a sibling tempfile path: `r.X.Z.mca` → `r.X.Z.mca.tmp.<pid>.<seq>`.
/// Sibling (same directory) keeps `rename` on a single filesystem so it stays atomic.
/// pid + monotonic counter avoids collisions when rayon writes many regions in parallel
/// and is stable across platforms (unlike Debug-formatted `ThreadId`).
fn tempfile_path_for(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    let seq = TEMPFILE_SEQ.fetch_add(1, Ordering::Relaxed);
    name.push(format!(".tmp.{}.{}", std::process::id(), seq));
    target.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::tag::Tag;
    use crate::region_loader::region::Region;
    use flate2::read::ZlibEncoder;
    use std::io::Read;

    fn unique_tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mwt_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn chunk_nbt(x: i32, z: i32, inhabited_time: i64) -> Tag {
        Tag::Compound {
            name: Some(String::new()),
            value: vec![
                Tag::Int {
                    name: Some(String::from("xPos")),
                    value: x,
                },
                Tag::Int {
                    name: Some(String::from("zPos")),
                    value: z,
                },
                Tag::String {
                    name: Some(String::from("Status")),
                    value: String::from("minecraft:full"),
                },
                Tag::Long {
                    name: Some(String::from("InhabitedTime")),
                    value: inhabited_time,
                },
            ],
        }
    }

    fn zlib_chunk_frame(nbt: &Tag) -> Vec<u8> {
        let mut compressed = Vec::new();
        ZlibEncoder::new(nbt.to_bytes().as_slice(), Compression::fast())
            .read_to_end(&mut compressed)
            .unwrap();
        let mut frame = ((compressed.len() + 1) as u32).to_be_bytes().to_vec();
        frame.push(2_u8); // zlib
        frame.extend_from_slice(&compressed);
        frame
    }

    /// Builds a region file with the given chunk frames placed in consecutive
    /// location-table slots starting at slot 0.
    fn build_region(frames: &[Vec<u8>]) -> Vec<u8> {
        let mut region = vec![0_u8; 8192];
        let mut sector = 2_u32;
        for (slot, frame) in frames.iter().enumerate() {
            let sectors = frame.len().div_ceil(4096) as u32;
            let entry = (sector << 8) | sectors;
            region[slot * 4..slot * 4 + 4].copy_from_slice(&entry.to_be_bytes());
            let mut padded = frame.clone();
            padded.resize((sectors * 4096) as usize, 0);
            region.extend_from_slice(&padded);
            sector += sectors;
        }
        region
    }

    /// Regression (missing `return` in the fast path): a region whose chunks are
    /// all inhabited must be left byte-for-byte untouched, with stats counted
    /// exactly once.
    #[test]
    fn test_fast_path_leaves_region_without_deletable_chunks_untouched() {
        let tmp = unique_tmp_dir("fastpath");
        let target = tmp.join("r.0.0.mca");
        let region_bytes = build_region(&[
            zlib_chunk_frame(&chunk_nbt(0, 0, 1200)),
            zlib_chunk_frame(&chunk_nbt(1, 0, 7)),
        ]);
        std::fs::write(&target, &region_bytes).unwrap();

        let result = optimize_write(&target, Compression::fast(), None, &[]);
        assert_eq!(result.total_chunks, 2, "stats must not be double-counted");
        assert_eq!(result.deleted_chunks, 0);
        assert_eq!(result.deleted_regions, 0);
        assert_eq!(result.io_errors, 0);
        assert_eq!(
            std::fs::read(&target).unwrap(),
            region_bytes,
            "file with nothing to delete must stay byte-identical"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Regression (corrupt chunks dropped on rewrite): a region with one
    /// deletable and one corrupt chunk must keep the corrupt chunk's raw bytes
    /// in the rewritten file.
    #[test]
    fn test_corrupt_chunk_survives_rewrite() {
        let tmp = unique_tmp_dir("corrupt");
        let target = tmp.join("r.0.0.mca");
        // Distinctive garbage payload: valid frame header, invalid zlib stream.
        let marker: Vec<u8> = (0..64).map(|i| 0xA0 ^ i as u8).collect();
        let mut corrupt_frame = ((marker.len() + 1) as u32).to_be_bytes().to_vec();
        corrupt_frame.push(2_u8);
        corrupt_frame.extend_from_slice(&marker);

        let region_bytes = build_region(&[
            zlib_chunk_frame(&chunk_nbt(0, 0, 0)), // deletable
            corrupt_frame.clone(),
        ]);
        std::fs::write(&target, &region_bytes).unwrap();

        let result = optimize_write(&target, Compression::fast(), None, &[]);
        assert_eq!(result.deleted_chunks, 1);
        assert_eq!(result.corrupt_chunks, 1);
        assert_eq!(result.deleted_regions, 0, "file must not be whole-deleted");
        assert_eq!(result.io_errors, 0);

        let rewritten = std::fs::read(&target).unwrap();
        assert!(
            rewritten
                .windows(corrupt_frame.len())
                .any(|w| w == corrupt_frame.as_slice()),
            "corrupt chunk frame must be preserved verbatim in the rewritten file"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A region containing only corrupt chunks must be left completely untouched.
    #[test]
    fn test_corrupt_only_region_is_left_untouched() {
        let tmp = unique_tmp_dir("corruptonly");
        let target = tmp.join("r.0.0.mca");
        let mut corrupt_frame = 65_u32.to_be_bytes().to_vec();
        corrupt_frame.push(2_u8);
        corrupt_frame.extend_from_slice(&[0xFF; 64]);
        let region_bytes = build_region(&[corrupt_frame]);
        std::fs::write(&target, &region_bytes).unwrap();

        let result = optimize_write(&target, Compression::fast(), None, &[]);
        assert_eq!(result.corrupt_chunks, 1);
        assert_eq!(result.deleted_regions, 0);
        assert_eq!(
            std::fs::read(&target).unwrap(),
            region_bytes,
            "corrupt-only region must stay byte-identical"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A header-only region file with an empty location table holds no data and
    /// is removed as a whole, matching check-mode accounting.
    #[test]
    fn test_empty_region_file_is_removed() {
        let tmp = unique_tmp_dir("emptyregion");
        let target = tmp.join("r.0.0.mca");
        std::fs::write(&target, vec![0_u8; 8192]).unwrap();

        let result = optimize_write(&target, Compression::fast(), None, &[]);
        assert_eq!(result.deleted_regions, 1);
        assert_eq!(result.io_errors, 0);
        assert!(!target.exists(), "empty region file must be deleted");

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A second quarantine of a same-named region file must not overwrite the
    /// existing backup from an earlier run.
    #[test]
    fn test_quarantine_does_not_overwrite_existing_backup() {
        let tmp = unique_tmp_dir("noclobber");
        let world = tmp.join("world");
        let region_dir = world.join("region");
        std::fs::create_dir_all(&region_dir).unwrap();
        let backup_root = tmp.join("backup");
        let world_paths = vec![world.clone()];
        let target = region_dir.join("r.0.0.mca");

        std::fs::write(&target, b"FIRST RUN ORIGINAL").unwrap();
        quarantine_original(&target, &backup_root, &world_paths).unwrap();
        std::fs::write(&target, b"SECOND RUN CONTENT").unwrap();
        let second_dest = quarantine_original(&target, &backup_root, &world_paths).unwrap();

        assert_eq!(
            std::fs::read(backup_root.join("region/r.0.0.mca")).unwrap(),
            b"FIRST RUN ORIGINAL",
            "the older backup must survive a second run"
        );
        assert_eq!(std::fs::read(&second_dest).unwrap(), b"SECOND RUN CONTENT");
        assert_ne!(second_dest, backup_root.join("region/r.0.0.mca"));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_tempfile_path_is_sibling() {
        let target = Path::new("/tmp/world/region/r.0.0.mca");
        let tmp = tempfile_path_for(target);
        assert_eq!(tmp.parent(), target.parent(), "tempfile must be a sibling");
        assert!(tmp.file_name().unwrap().to_string_lossy().contains(".tmp."));
        assert!(
            tmp.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("r.0.0.mca")
        );
    }

    #[test]
    fn test_tempfile_path_is_unique_across_calls() {
        let target = Path::new("/tmp/world/region/r.0.0.mca");
        let a = tempfile_path_for(target);
        let b = tempfile_path_for(target);
        assert_ne!(
            a, b,
            "monotonic counter must produce distinct tempfile names within a process"
        );
    }

    /// End-to-end: copy the real 11 MB sample to a temp dir, run `optimize_write` on it,
    /// verify the result is a valid region file with the same chunk count, and verify no
    /// stray `.tmp.*` files are left behind.
    #[test]
    fn test_optimize_write_atomic_on_real_sample() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "mwt_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let target = tmp_dir.join("r.-1.-1.mca");
        let original_bytes = include_bytes!("../../test_files/r.-1.-1.mca");
        std::fs::write(&target, original_bytes).unwrap();

        let result = optimize_write(&target, Compression::fast(), None, &[]);
        assert!(result.total_chunks > 0);
        assert_eq!(
            result.io_errors, 0,
            "no I/O errors expected on healthy sample"
        );

        // The file still exists (was not removed) and re-parses cleanly.
        let reparsed = Region::from_file_name(&target).expect("written file must re-parse");
        let expected_remaining = result.total_chunks - result.deleted_chunks;
        assert_eq!(reparsed.get_chunk_count(), expected_remaining);

        // No leftover tempfiles in the target directory.
        let leftovers: Vec<_> = std::fs::read_dir(&tmp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "atomic write must clean up tempfiles, found: {:?}",
            leftovers.iter().map(|e| e.path()).collect::<Vec<_>>()
        );

        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    /// With `--backup-dir`, an original region file that would be deleted (all
    /// chunks removed) must be MOVED into the backup directory instead, so the
    /// world can be restored without a full pre-copy of the world.
    #[test]
    fn test_backup_dir_quarantines_deleted_region() {
        use crate::nbt::tag::Tag;
        use flate2::read::ZlibEncoder;
        use std::io::Read;

        let tmp_dir = std::env::temp_dir().join(format!(
            "mwt_quarantine_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let world = tmp_dir.join("world");
        let region_dir = world.join("region");
        std::fs::create_dir_all(&region_dir).unwrap();
        let target = region_dir.join("r.0.0.mca");
        let backup_root = tmp_dir.join("backup");

        // Minimal region with a single deletable chunk (Status != full, InhabitedTime == 0)
        let nbt = Tag::Compound {
            name: Some(String::new()),
            value: vec![
                Tag::Int {
                    name: Some(String::from("xPos")),
                    value: 0,
                },
                Tag::Int {
                    name: Some(String::from("zPos")),
                    value: 0,
                },
                Tag::String {
                    name: Some(String::from("Status")),
                    value: String::from("minecraft:empty"),
                },
                Tag::Long {
                    name: Some(String::from("InhabitedTime")),
                    value: 0,
                },
            ],
        };
        let mut compressed = Vec::new();
        ZlibEncoder::new(nbt.to_bytes().as_slice(), Compression::fast())
            .read_to_end(&mut compressed)
            .unwrap();
        let mut chunk_frame = ((compressed.len() + 1) as u32).to_be_bytes().to_vec();
        chunk_frame.push(2_u8); // zlib
        chunk_frame.extend_from_slice(&compressed);
        let aligned_len = chunk_frame.len().div_ceil(4096) * 4096;

        let mut region_bytes = vec![0_u8; 8192];
        region_bytes[0..4].copy_from_slice(&[0, 0, 2, (aligned_len / 4096) as u8]);
        region_bytes.extend_from_slice(&chunk_frame);
        region_bytes.resize(8192 + aligned_len, 0);
        std::fs::write(&target, &region_bytes).unwrap();

        let world_paths = vec![world.clone()];
        let result = optimize_write(
            &target,
            Compression::fast(),
            Some(&backup_root),
            &world_paths,
        );
        assert_eq!(result.deleted_chunks, 1);
        assert_eq!(result.deleted_regions, 1);
        assert_eq!(result.io_errors, 0);

        // The region file is gone from the world…
        assert!(
            !target.exists(),
            "region file must be removed from the world"
        );
        // …and the byte-identical original must sit in the backup directory,
        // mirroring the world-relative path.
        let backup_file = backup_root.join("region/r.0.0.mca");
        assert!(
            backup_file.is_file(),
            "original must be quarantined at {:?}",
            backup_file
        );
        assert_eq!(
            std::fs::read(&backup_file).unwrap(),
            region_bytes,
            "quarantined original must be byte-identical"
        );

        // Rollback works by renaming the backup file back.
        std::fs::rename(&backup_file, &target).unwrap();
        let restored = Region::from_file_name(&target);
        assert!(restored.is_ok(), "restored file must re-parse");

        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    /// Verifies that atomic_write_region preserves the file mode of the original file.
    /// Critical for server worlds where region files have non-default permissions
    /// (e.g. group-readable for a `minecraft` system user).
    #[cfg(unix)]
    #[test]
    fn test_atomic_write_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp_dir = std::env::temp_dir().join(format!(
            "mwt_perms_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let target = tmp_dir.join("r.0.0.mca");
        std::fs::write(&target, b"placeholder").unwrap();

        // Set a distinctive mode that wouldn't come from default umask.
        let mut perms = std::fs::metadata(&target).unwrap().permissions();
        perms.set_mode(0o640);
        std::fs::set_permissions(&target, perms).unwrap();
        let mode_before = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode_before, 0o640);

        atomic_write_region(&target, b"replacement payload").unwrap();

        let mode_after = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode_after, 0o640,
            "atomic_write_region must preserve file mode across rename"
        );

        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    /// Throwaway micro-bench: `cargo test --release -- --ignored bench_optimize_write --nocapture`.
    /// Times atomic-write `optimize_write` on the 11 MB sample, plus the same I/O without
    /// the tempfile dance, to isolate atomic-write overhead from compression cost.
    #[test]
    #[ignore]
    fn bench_optimize_write() {
        let original_bytes = include_bytes!("../../test_files/r.-1.-1.mca");
        let tmp_dir = std::env::temp_dir().join(format!("mwt_bench_{}", std::process::id()));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let target = tmp_dir.join("r.-1.-1.mca");
        std::fs::write(&target, original_bytes).unwrap();

        // Pre-build the payload we'll write — we want to isolate I/O cost, not compression.
        let region = Region::from_file_name(&target).unwrap();
        let payload = region.to_bytes(Compression::fast()).bytes;
        println!(
            "\npayload size: {} bytes ({:.2} MB)",
            payload.len(),
            payload.len() as f64 / 1_048_576.0
        );

        const ITERS: u32 = 50;

        // Warmup
        for _ in 0..5 {
            atomic_write_region(&target, &payload).unwrap();
        }

        // A. Atomic write: tempfile + write + flush + fsync + rename + dir-fsync
        let mut atomic_total = std::time::Duration::ZERO;
        for _ in 0..ITERS {
            let start = std::time::Instant::now();
            atomic_write_region(&target, &payload).unwrap();
            atomic_total += start.elapsed();
        }

        // B. Direct write: truncate + write (old behavior, no fsync)
        let mut direct_total = std::time::Duration::ZERO;
        for _ in 0..ITERS {
            let start = std::time::Instant::now();
            let file = File::create(&target).unwrap();
            let mut w = BufWriter::with_capacity(32 * 1024 * 1024, file);
            w.write_all(&payload).unwrap();
            w.flush().unwrap();
            direct_total += start.elapsed();
        }

        let atomic_avg = atomic_total / ITERS;
        let direct_avg = direct_total / ITERS;
        let overhead_ns = atomic_avg.as_nanos() as i128 - direct_avg.as_nanos() as i128;
        let overhead_pct = (overhead_ns as f64 / direct_avg.as_nanos() as f64) * 100.0;

        println!("=== I/O write path bench ({} iters) ===", ITERS);
        println!("  A. atomic (tempfile + fsync + rename): {:?}", atomic_avg);
        println!("  B. direct (truncate + write, no fsync): {:?}", direct_avg);
        println!(
            "  overhead of atomic+durable:            {} ns ({:+.2}%)",
            overhead_ns, overhead_pct
        );

        std::fs::remove_dir_all(&tmp_dir).ok();
    }
}

#[cfg(test)]
mod nbt_safety_regression {
    use crate::nbt::binary_reader::BinaryReader;
    use crate::nbt::parse::parse_tag;

    fn truncated_compound_nbt() -> Vec<u8> {
        let mut nbt: Vec<u8> = Vec::new();
        nbt.push(10); // TAG_Compound
        nbt.extend_from_slice(&[0, 4]);
        nbt.extend_from_slice(b"Test");
        nbt.push(3); // TAG_Int "A" — parses fine
        nbt.extend_from_slice(&[0, 1]);
        nbt.extend_from_slice(b"A");
        nbt.extend_from_slice(&123_i32.to_be_bytes());
        nbt.push(8); // TAG_String "B" claiming 200 bytes but truncated
        nbt.extend_from_slice(&[0, 1]);
        nbt.extend_from_slice(b"B");
        nbt.extend_from_slice(&[0, 200]);
        nbt
    }

    /// Regression: a compound whose payload is truncated mid-child must FAIL
    /// to parse. Previously the parser returned a partial tree (only child A),
    /// which re-serializes shorter than the original and silently drops the
    /// chunk's tail on rewrite.
    #[test]
    fn truncated_compound_is_an_error_not_partial_parse() {
        let data = truncated_compound_nbt();
        let mut reader = BinaryReader::new(&data);
        assert!(
            parse_tag(&mut reader).is_err(),
            "truncated compound must fail instead of returning a partial tree"
        );
    }

    /// Regression: a list declaring more elements than the payload holds must
    /// fail instead of silently yielding fewer elements.
    #[test]
    fn truncated_list_is_an_error() {
        let mut nbt: Vec<u8> = Vec::new();
        nbt.push(9); // TAG_List
        nbt.extend_from_slice(&[0, 2]); // name len 2
        nbt.extend_from_slice(b"Ls");
        nbt.push(3); // element type TAG_Int
        nbt.extend_from_slice(&5_i32.to_be_bytes()); // declares 5 elements
        nbt.extend_from_slice(&1_i32.to_be_bytes()); // only one present
        let mut reader = BinaryReader::new(&nbt);
        assert!(parse_tag(&mut reader).is_err());
    }

    /// Regression: an array declaring more bytes than the payload holds must
    /// fail instead of returning a short read.
    #[test]
    fn truncated_byte_array_is_an_error() {
        let mut nbt: Vec<u8> = Vec::new();
        nbt.push(7); // TAG_Byte_Array
        nbt.extend_from_slice(&[0, 1]); // name len
        nbt.extend_from_slice(b"B");
        nbt.extend_from_slice(&10_i32.to_be_bytes()); // claims 10 bytes
        nbt.extend_from_slice(&[1, 2, 3]); // only 3 present
        let mut reader = BinaryReader::new(&nbt);
        assert!(parse_tag(&mut reader).is_err());
    }

    /// A crafted deeply-nested payload must hit the depth limit error rather
    /// than overflowing the stack (which would abort the process mid-run).
    #[test]
    fn deep_nesting_hits_depth_limit_instead_of_stack_overflow() {
        const DEPTH: usize = crate::nbt::MAX_NBT_DEPTH as usize + 64;
        let mut data = Vec::with_capacity(DEPTH * 5);
        for _ in 0..DEPTH {
            data.push(10); // TAG_Compound
            data.extend_from_slice(&[0, 1]); // name len 1
            data.push(b'N');
        }
        let mut reader = BinaryReader::new(&data);
        match parse_tag(&mut reader) {
            Err(crate::nbt::NbtError::DepthLimit) => {}
            other => panic!("expected DepthLimit, got {other:?}"),
        }
    }
}
