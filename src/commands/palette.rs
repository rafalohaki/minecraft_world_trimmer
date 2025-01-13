// src/commands/palette.rs
use crate::commands::optimize_result::{reduce_optimize_results, OptimizeResult};
use crate::region_loader::region::Region;
use crate::world::get_region_files::get_region_files;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::iter::ParallelIterator;
use rayon::prelude::IntoParallelRefIterator;
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;

pub fn execute_palette(
    world_paths: &Vec<PathBuf>,
    csv_out: &Option<PathBuf>,
    csv_in: &Option<PathBuf>,
    id_filter: Option<&str>,
    count_threshold: Option<u32>,
) -> Result<(), Box<dyn Error>> {
    if let Some(csv_in_path) = csv_in {
        execute_palette_import(csv_in_path, id_filter, count_threshold)
    } else if let Some(csv_out_path) = csv_out {
        execute_palette_export(world_paths, csv_out_path, id_filter, count_threshold)
    } else {
        println!("You must provide either a CSV output or input path.");
        Ok(())
    }
}

fn execute_palette_export(
    world_paths: &Vec<PathBuf>,
    csv_out_path: &PathBuf,
    id_filter: Option<&str>,
    count_threshold: Option<u32>,
) -> Result<(), Box<dyn Error>> {
    let entries = get_region_files(world_paths)?;
    let pb = ProgressBar::new(entries.len() as u64);
    let style = ProgressStyle::with_template(
        "{percent}% {bar} {pos}/{len} [{elapsed_precise}>{eta_precise}, {per_sec}]",
    )
    .unwrap();
    pb.set_style(style);

    let results = entries
        .par_iter()
        .map(|entry| {
            let result = optimize_palette_export(entry, id_filter, count_threshold);
            pb.inc(1);
            result
        })
        .filter_map(Result::ok)
        .flatten()
        .collect::<Vec<PaletteResult>>();

    let csv_file = File::create(csv_out_path)?;
    let mut writer = BufWriter::new(csv_file);

    writeln!(writer, "region_file_path,chunk_x,chunk_z")?;
    for result in results {
        writeln!(
            writer,
            "{},{},{}",
            result.region_file_path.display(),
            result.chunk_x,
            result.chunk_z
        )?;
    }

    let result = reduce_optimize_results(&mut [OptimizeResult::default()]);
    println!("{result}");

    Ok(())
}

fn optimize_palette_export(
    region_file_path: &PathBuf,
    id_filter: Option<&str>,
    count_threshold: Option<u32>,
) -> std::io::Result<Vec<PaletteResult>> {
    match Region::from_file_name(region_file_path) {
        Ok(region) => {
            let results = region
                .get_chunks()
                .iter()
                .filter_map(|chunk| {
                    if let Some(id) = id_filter {
                        let count = chunk.count_block(id);
                        if let Some(threshold) = count_threshold {
                            if count >= threshold {
                                let position = chunk.get_position().ok()?;
                                return Some(PaletteResult {
                                    region_file_path: region_file_path.clone(),
                                    chunk_x: position.0,
                                    chunk_z: position.1,
                                });
                            }
                        } else if count > 0 {
                            let position = chunk.get_position().ok()?;
                            return Some(PaletteResult {
                                region_file_path: region_file_path.clone(),
                                chunk_x: position.0,
                                chunk_z: position.1,
                            });
                        }
                    } else if chunk.should_delete() {
                        let position = chunk.get_position().ok()?;
                        return Some(PaletteResult {
                            region_file_path: region_file_path.clone(),
                            chunk_x: position.0,
                            chunk_z: position.1,
                        });
                    }
                    None
                })
                .collect::<Vec<_>>();

            Ok(results)
        }
        Err(_) => Ok(vec![]),
    }
}

fn execute_palette_import(
    csv_in_path: &PathBuf,
    id_filter: Option<&str>,
    count_threshold: Option<u32>,
) -> Result<(), Box<dyn Error>> {
    let file = File::open(csv_in_path)?;
    let reader = BufReader::new(file);
    let mut rdr = csv::Reader::from_reader(reader);

    let mut results = Vec::new();
    for result in rdr.deserialize() {
        let record: PaletteResult = result?;
        results.push(record);
    }

    let filtered_results = results
        .into_iter()
        .filter(|res| {
            if let Some(id) = id_filter {
                if let Ok(region) = Region::from_file_name(&res.region_file_path) {
                    if let Some(chunk) = region.get_chunk(res.chunk_x, res.chunk_z) {
                        let count = chunk.count_block(id);
                        if let Some(threshold) = count_threshold {
                            return count >= threshold;
                        } else {
                            return count > 0;
                        }
                    }
                }
                return false;
            }
            true
        })
        .collect::<Vec<_>>();

    let pb = ProgressBar::new(filtered_results.len() as u64);
    let style = ProgressStyle::with_template(
        "{percent}% {bar} {pos}/{len} [{elapsed_precise}>{eta_precise}, {per_sec}]",
    )
    .unwrap();
    pb.set_style(style);

    let mut optimize_results = filtered_results
        .par_iter()
        .map(|result| {
            let optimize_result = optimize_palette_import(result);
            pb.inc(1);
            optimize_result
        })
        .flatten()
        .collect::<Vec<OptimizeResult>>();

    let result = reduce_optimize_results(&mut optimize_results);
    println!("{result}");

    Ok(())
}

fn optimize_palette_import(palette_result: &PaletteResult) -> std::io::Result<OptimizeResult> {
    let mut result = OptimizeResult::default();

    match Region::from_file_name(&palette_result.region_file_path) {
        Ok(mut region) => {
            let chunks_to_delete_indices: Vec<_> = region
                .get_chunks()
                .iter()
                .enumerate()
                .filter_map(|(i, chunk)| {
                    if let Ok(position) = chunk.get_position() {
                        if position.0 == palette_result.chunk_x
                            && position.1 == palette_result.chunk_z
                        {
                            Some(i)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect();

            if !chunks_to_delete_indices.is_empty() {
                for &index in chunks_to_delete_indices.iter().rev() {
                    region.remove_chunk_by_index(index);
                }
                result.deleted_chunks += chunks_to_delete_indices.len();
            }

            if region.is_empty() {
                result.deleted_regions += 1;
                fs::remove_file(&palette_result.region_file_path)?;
            } else if region.is_modified() {
                let bytes = region.to_bytes(flate2::Compression::new(6));
                fs::write(&palette_result.region_file_path, bytes)?;
            }
        }
        Err(_) => {
            result.deleted_regions += 1;
            fs::remove_file(&palette_result.region_file_path)?;
        }
    }

    Ok(result)
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct PaletteResult {
    region_file_path: PathBuf,
    chunk_x: i32,
    chunk_z: i32,
}
