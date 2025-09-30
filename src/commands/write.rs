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
use std::path::PathBuf;

pub fn execute_write(
    world_paths: &Vec<PathBuf>,
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
    region_file_path: &PathBuf,
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
                // Only write the region file if it has been modified
                let bytes = region.to_bytes(compression);
                let file = File::create(region_file_path)?;
                // Use a buffered writer to reduce syscall overhead; 32 MB buffer
                let mut writer = BufWriter::with_capacity(32 * 1024 * 1024, file);
                writer.write_all(&bytes)?;
            }
        }
        Err(err) => match err {
            ParseRegionError::HeaderError => {
                // Plik ma uszkodzony nagłówek, można bezpiecznie usunąć
                result.deleted_regions += 1;
                std::fs::remove_file(region_file_path)?;
            }
            ParseRegionError::ReadError => {
                // Błąd I/O – nie usuwaj pliku, tylko zlicz błąd
                result.io_errors += 1;
            }
        },
    }

    Ok(result)
}
