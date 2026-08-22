use crate::commands::optimize_result::{OptimizeResult, reduce_optimize_results};
use crate::region_loader::region::{ParseRegionError, analyze_region_bytes, read_region_file};
use crate::world::get_region_files::get_region_files;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::iter::ParallelIterator;
use rayon::prelude::IntoParallelRefIterator;
use std::error::Error;
use std::path::{Path, PathBuf};

pub fn execute_read(world_paths: &[PathBuf]) -> Result<(), Box<dyn Error>> {
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
            let result = optimize_read(entry);
            pb.inc(1);
            result
        })
        .collect::<Vec<OptimizeResult>>();

    let result = reduce_optimize_results(&mut results);
    println!("{result}");

    Ok(())
}

fn optimize_read(region_file_path: &Path) -> OptimizeResult {
    let mut result = OptimizeResult::default();

    match read_region_file(region_file_path).and_then(|bytes| analyze_region_bytes(&bytes)) {
        Ok(stats) => {
            result.total_chunks += stats.total_chunks;
            result.deleted_chunks += stats.deletable_chunks;
            result.preserved_opaque_chunks += stats.opaque_chunks;
            if stats.deletable_chunks >= stats.total_chunks {
                result.deleted_regions += 1;
            }
        }
        Err(ParseRegionError::HeaderError) => {
            // Plik za mały / uszkodzony nagłówek — w trybie write zostanie skasowany.
            result.deleted_regions += 1;
        }
        Err(ParseRegionError::ReadError) => {
            // Błąd I/O (np. brak uprawnień, zerwane łącze sieciowe) — nie do skasowania.
            result.io_errors += 1;
        }
    }

    result
}
