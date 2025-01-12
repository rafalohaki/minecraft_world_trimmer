use crate::commands::optimize_result::{reduce_optimize_results, OptimizeResult};
use crate::region_loader::region::Region;
use crate::world::get_region_files::get_region_files;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::iter::ParallelIterator;
use rayon::prelude::IntoParallelRefIterator;
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write, Read};
use std::path::PathBuf;

pub fn execute_palette(
    world_paths: &Vec<PathBuf>,
    csv_out: &Option<PathBuf>,
    csv_in: &Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    if let Some(csv_in_path) = csv_in {
        execute_palette_import(world_paths, csv_in_path)
    } else if let Some(csv_out_path) = csv_out {
        execute_palette_export(world_paths, csv_out_path)
    } else {
        println!("You must provide either a CSV output or input path.");
        Ok(())
    }
}

fn execute_palette_export(world_paths: &Vec<PathBuf>, csv_out_path: &PathBuf) -> Result<(), Box<dyn Error>> {
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
            let result = optimize_palette_export(entry);
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
            writeln!(writer, "{},{},{}", result.region_file_path.display(), result.chunk_x, result.chunk_z)?;
    }

    let result = reduce_optimize_results(&mut vec![OptimizeResult::default()]);
    println!("{result}");

    Ok(())
}

fn optimize_palette_export(
    region_file_path: &PathBuf,
) -> std::io::Result<Vec<PaletteResult>> {

    match Region::from_file_name(region_file_path) {
        Ok(region) => {
                let results = region
                .get_chunks()
                .iter()
                 .filter_map(|chunk| {
                    if chunk.should_delete() {
                        let position = chunk.get_position().ok()?;
                        Some(PaletteResult {
                            region_file_path: region_file_path.clone(),
                            chunk_x: position.0,
                            chunk_z: position.1,
                        })
                    } else {
                        None
                    }
                })
                 .collect::<Vec<_>>();

            Ok(results)
        }
        Err(_) => {
             Ok(vec![])
        }
    }
}

fn execute_palette_import(_world_paths: &Vec<PathBuf>, csv_in_path: &PathBuf) -> Result<(), Box<dyn Error>> {

    let mut file = File::open(csv_in_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let mut rdr = csv::Reader::from_reader(contents.as_bytes());

    let mut results = Vec::new();
    for result in rdr.records() {
        let record = result?;
        let region_file_path = PathBuf::from(record.get(0).ok_or("Missing column 0")?);
        let chunk_x = record
            .get(1)
            .ok_or("Missing column 1")?
            .parse::<i32>()?;
        let chunk_z = record
            .get(2)
            .ok_or("Missing column 2")?
            .parse::<i32>()?;

        results.push(PaletteResult {
            region_file_path,
            chunk_x,
            chunk_z
         })
    }

    let pb = ProgressBar::new(results.len() as u64);
    let style = ProgressStyle::with_template(
        "{percent}% {bar} {pos}/{len} [{elapsed_precise}>{eta_precise}, {per_sec}]",
    )
        .unwrap();
    pb.set_style(style);

    let mut optimize_results = results
        .par_iter()
         .map(|result| {
            let optimize_result =  optimize_palette_import(result);
            pb.inc(1);
             optimize_result
         })
        .flatten()
        .collect::<Vec<OptimizeResult>>();

    let result = reduce_optimize_results(&mut optimize_results);
    println!("{result}");

    Ok(())
}

fn optimize_palette_import(
    palette_result: &PaletteResult,
) -> std::io::Result<OptimizeResult> {

    let mut result = OptimizeResult::default();

    match Region::from_file_name(&palette_result.region_file_path) {
        Ok(mut region) => {

              let chunks_to_delete_indices: Vec<_> = region
                .get_chunks()
                .iter()
                .enumerate()
                  .filter_map(|(i, chunk)| {
                      if let Ok(position) = chunk.get_position() {
                          if position.0 == palette_result.chunk_x && position.1 == palette_result.chunk_z {
                               Some(i)
                          } else {
                             None
                          }
                     }  else {
                        None
                    }
                })
                  .collect();

            if region.is_empty() {
                 result.deleted_regions += 1;
                 std::fs::remove_file(&palette_result.region_file_path)?;
             } else if region.is_modified() {
                 let bytes = region.to_bytes(flate2::Compression::new(6));
                 std::fs::write(&palette_result.region_file_path, bytes)?;
             }

            result.deleted_chunks += chunks_to_delete_indices.len();

             for &index in chunks_to_delete_indices.iter().rev() {
                region.remove_chunk_by_index(index);
             }
        }
        Err(_) => {
            result.deleted_regions += 1;
            std::fs::remove_file(&palette_result.region_file_path)?;
         }
    }

    Ok(result)
}

#[derive(Clone, Debug)]
struct PaletteResult {
    region_file_path: PathBuf,
    chunk_x: i32,
    chunk_z: i32,
}