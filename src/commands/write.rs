use crate::commands::optimize_result::{OptimizeResult, reduce_optimize_results};
use crate::region_loader::region::Region;
use crate::world::get_region_files::get_region_files;
use flate2::Compression;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::iter::ParallelIterator;
use rayon::prelude::IntoParallelRefIterator;
use std::error::Error;
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
        .flat_map(|entry| {
            let result = optimize_write(entry, compression);
            pb.inc(1);
            result
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
                std::fs::write(region_file_path, bytes)?;
            }
        }
        Err(_) => {
            result.deleted_regions += 1;
            std::fs::remove_file(region_file_path)?;
        }
    }

    Ok(result)
}
