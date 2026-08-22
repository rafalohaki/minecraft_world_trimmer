use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "minecraft_world_trimmer",
    version = "1.0",
    about = "Optimizing Minecraft worlds by deleting unused region files and chunks.",
    long_about = None,
)]
pub struct Cli {
    /// What mode to run the program in
    #[arg(value_enum, required = true)]
    pub mode: Mode,

    /// Path to your Minecraft Worlds containing `level.dat` file
    #[arg(required = true)]
    pub world_paths: Vec<PathBuf>,

    /// Compression level when writing region files. Levels 1–3 are significantly
    /// faster with only marginally larger output; 9 is extremely slow.
    #[arg(short, long, default_value = "3", value_parser = validate_compression_level)]
    pub compression_level: u32,

    /// Quarantine directory for original region files. When set, every region file
    /// that would be overwritten or deleted is MOVED there first (same-volume rename:
    /// instant and free). The world can be fully restored from this directory until
    /// you delete it. Recommended path: outside the world folder, on the same volume.
    #[arg(long, value_name = "DIR")]
    pub backup_dir: Option<PathBuf>,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Mode {
    /// Only counts of region files and chunks that can be deleted without making any change to the world
    Check,

    /// Optimizes the world by deleting unused region files and chunks.
    /// This is a destructive process, make sure to make a backup of your worlds before running.
    /// Also make sure the world is not loaded by the game as this will corrupt the world.
    Write,
}

fn validate_compression_level(s: &str) -> Result<u32, String> {
    match s.parse::<u32>() {
        Ok(level) if level <= 9 => Ok(level),
        _ => Err("Compression level must be an integer between 0 and 9".to_string()),
    }
}
