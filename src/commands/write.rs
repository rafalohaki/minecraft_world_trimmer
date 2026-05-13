use crate::commands::optimize_result::{reduce_optimize_results, OptimizeResult};
use crate::region_loader::region::{ParseRegionError, Region};
use crate::world::get_region_files::get_region_files;
use flate2::Compression;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::iter::ParallelIterator;
use rayon::prelude::IntoParallelRefIterator;
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

pub fn execute_write(
    world_paths: &[PathBuf],
    compression: Compression,
) -> Result<(), Box<dyn Error>> {
    let entries = get_region_files(world_paths)?;
    let pb = ProgressBar::new(entries.len() as u64);
    let style = ProgressStyle::with_template(
        "{percent}% {bar} {pos}/{len} [{elapsed_precise}>{eta_precise}, {per_sec}]",
    )
    .unwrap();
    pb.set_style(style);

    let mut results = entries
        .par_iter()
        .filter_map(|entry| {
            let result = optimize_write(entry, compression);
            pb.inc(1);
            result.ok()
        })
        .collect::<Vec<OptimizeResult>>();

    let result = reduce_optimize_results(&mut results);
    println!("{result}");

    Ok(())
}

fn optimize_write(
    region_file_path: &Path,
    compression: Compression,
) -> std::io::Result<OptimizeResult> {
    let mut result = OptimizeResult::default();

    match Region::from_file_name(region_file_path) {
        Ok(mut region) => {
            result.total_chunks += region.get_chunk_count();

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
                result.deleted_regions += 1;
                std::fs::remove_file(region_file_path)?;
            } else if region.is_modified() {
                match region.to_bytes(compression) {
                    Ok(to_bytes) => {
                        // Atomic write: write to a sibling tempfile, then rename over the
                        // original. Guarantees the original is never left half-written if the
                        // process is killed or crashes mid-write. The tempfile lives in the
                        // same directory so `rename` stays on a single filesystem (atomic on
                        // POSIX; std::fs::rename uses ReplaceFile on Windows).
                        let tmp_path = tempfile_path_for(region_file_path);
                        let write_result = (|| -> std::io::Result<()> {
                            let file = File::create(&tmp_path)?;
                            let mut writer =
                                BufWriter::with_capacity(32 * 1024 * 1024, file);
                            writer.write_all(&to_bytes.bytes)?;
                            writer.flush()?;
                            Ok(())
                        })();
                        if let Err(e) = write_result {
                            let _ = std::fs::remove_file(&tmp_path);
                            return Err(e);
                        }
                        if let Err(e) = std::fs::rename(&tmp_path, region_file_path) {
                            let _ = std::fs::remove_file(&tmp_path);
                            return Err(e);
                        }
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
                    }
                    Err(_) => {
                        // Leave region file unchanged when serialization fails
                    }
                }
            }
        }
        Err(err) => match err {
            ParseRegionError::HeaderError => {
                result.deleted_regions += 1;
                std::fs::remove_file(region_file_path)?;
            }
            ParseRegionError::ReadError => {
                result.io_errors += 1;
            }
        },
    }

    Ok(result)
}

/// Build a sibling tempfile path: `r.X.Z.mca` → `r.X.Z.mca.tmp.<pid>.<thread-id>`.
/// Sibling (same directory) keeps `rename` on a single filesystem so it stays atomic.
/// pid + thread-id avoids collisions when rayon writes many regions in parallel.
fn tempfile_path_for(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    let thread_id = format!("{:?}", std::thread::current().id());
    let thread_id = thread_id
        .trim_start_matches("ThreadId(")
        .trim_end_matches(')');
    name.push(format!(".tmp.{}.{}", std::process::id(), thread_id));
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
        assert!(tmp.file_name().unwrap().to_string_lossy().starts_with("r.0.0.mca"));
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

        let result = optimize_write(&target, Compression::fast())
            .expect("optimize_write must succeed on a healthy sample");
        assert!(result.total_chunks > 0);

        // The file still exists (was not removed) and re-parses cleanly.
        let reparsed = Region::from_file_name(&target).expect("written file must re-parse");
        let expected_remaining = result.total_chunks - result.deleted_chunks;
        assert_eq!(reparsed.get_chunk_count(), expected_remaining);

        // No leftover tempfiles in the target directory.
        let leftovers: Vec<_> = std::fs::read_dir(&tmp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains(".tmp.")
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "atomic write must clean up tempfiles, found: {:?}",
            leftovers.iter().map(|e| e.path()).collect::<Vec<_>>()
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
        let payload = region.to_bytes(Compression::fast()).unwrap().bytes;
        println!("\npayload size: {} bytes ({:.2} MB)", payload.len(), payload.len() as f64 / 1_048_576.0);

        const ITERS: u32 = 50;

        // Warmup
        for _ in 0..5 {
            let tmp = tempfile_path_for(&target);
            let file = File::create(&tmp).unwrap();
            let mut w = BufWriter::with_capacity(32 * 1024 * 1024, file);
            w.write_all(&payload).unwrap();
            w.flush().unwrap();
            std::fs::rename(&tmp, &target).unwrap();
        }

        // A. Atomic write: tempfile + write + flush + rename
        let mut atomic_total = std::time::Duration::ZERO;
        for _ in 0..ITERS {
            let start = std::time::Instant::now();
            let tmp = tempfile_path_for(&target);
            let file = File::create(&tmp).unwrap();
            let mut w = BufWriter::with_capacity(32 * 1024 * 1024, file);
            w.write_all(&payload).unwrap();
            w.flush().unwrap();
            std::fs::rename(&tmp, &target).unwrap();
            atomic_total += start.elapsed();
        }

        // B. Direct write: truncate + write (old behavior)
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
        let overhead_ns =
            atomic_avg.as_nanos() as i128 - direct_avg.as_nanos() as i128;
        let overhead_pct = (overhead_ns as f64 / direct_avg.as_nanos() as f64) * 100.0;

        println!("=== I/O write path bench ({} iters) ===", ITERS);
        println!("  A. atomic (tempfile + rename): {:?}", atomic_avg);
        println!("  B. direct (truncate + write):  {:?}", direct_avg);
        println!("  overhead of atomic:            {} ns ({:+.2}%)", overhead_ns, overhead_pct);

        std::fs::remove_dir_all(&tmp_dir).ok();
    }
}
