use crate::commands::optimize_result::{OptimizeResult, reduce_optimize_results};
use crate::region_loader::region::{ParseRegionError, Region};
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

    let entries = get_region_files(world_paths)?;
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
fn quarantine_original(
    region_file: &Path,
    backup_root: &Path,
    world_paths: &[PathBuf],
) -> std::io::Result<PathBuf> {
    let destination = backup_path_for(region_file, backup_root, world_paths);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(region_file, &destination)?;
    Ok(destination)
}

fn optimize_write(
    region_file_path: &Path,
    compression: Compression,
    backup_dir: Option<&Path>,
    world_paths: &[PathBuf],
) -> OptimizeResult {
    let mut result = OptimizeResult::default();

    match Region::from_file_name(region_file_path) {
        Ok(mut region) => {
            result.total_chunks += region.get_chunk_count();
            result.preserved_opaque_chunks += region
                .get_chunks()
                .iter()
                .filter(|chunk| chunk.nbt.is_none())
                .count();

            let chunks_to_delete_indices: Vec<_> = region
                .get_chunks()
                .iter()
                .enumerate()
                .filter_map(|(i, chunk)| if chunk.should_delete() { Some(i) } else { None })
                .collect();
            result.deleted_chunks += chunks_to_delete_indices.len();

            for &index in chunks_to_delete_indices.iter().rev() {
                region.remove_chunk_by_index(index);
            }

            if region.is_empty() {
                let removal = match backup_dir {
                    Some(backup_root) => {
                        quarantine_original(region_file_path, backup_root, world_paths).map(|_| ())
                    }
                    None => std::fs::remove_file(region_file_path),
                };
                match removal {
                    Ok(()) => result.deleted_regions += 1,
                    Err(_) => result.io_errors += 1,
                }
            } else if region.is_modified() {
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
                let to_bytes = region.to_bytes(compression);
                if to_bytes.compression_fallbacks > 0 {
                    result.compression_failures += to_bytes.compression_fallbacks;
                    result.regions_with_compression_issues += 1;
                    eprintln!(
                        "Compression fallback in {} chunk(s) for {:?}",
                        to_bytes.compression_fallbacks, region_file_path
                    );
                }
                if to_bytes.header_write_failures > 0 {
                    result.header_write_failures += to_bytes.header_write_failures;
                    result.regions_with_header_issues += 1;
                    eprintln!(
                        "Header write failure: skipped payload for {} chunk(s) in {:?}",
                        to_bytes.header_write_failures, region_file_path
                    );
                }
                if atomic_write_region(region_file_path, &to_bytes.bytes).is_err() {
                    result.io_errors += 1;
                }
            }
        }
        Err(ParseRegionError::HeaderError) => {
            let removal = match backup_dir {
                Some(backup_root) => {
                    quarantine_original(region_file_path, backup_root, world_paths).map(|_| ())
                }
                None => std::fs::remove_file(region_file_path),
            };
            match removal {
                Ok(()) => result.deleted_regions += 1,
                Err(_) => result.io_errors += 1,
            }
        }
        Err(ParseRegionError::ReadError) => {
            result.io_errors += 1;
        }
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
        let mut writer = BufWriter::with_capacity(32 * 1024 * 1024, file);
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
