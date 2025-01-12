use clap::{Parser, ValueEnum};
use std::cmp::Ord;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "minecraft_world_optimizer",
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

    /// Compression level when writing region files
    #[arg(short, long, default_value = "6", value_parser = validate_compression_level)]
    pub compression_level: u32,

    /// Path to output a CSV file when using palette mode
    #[arg(long, requires = "palette")]
    pub csv_out: Option<PathBuf>,

    /// Path to input a CSV file when using palette mode
    #[arg(long, group = "palette_options")]
    pub csv_in: Option<PathBuf>,

    /// Block ID to filter by when using palette mode
    #[arg(long, group = "palette_options")]
    pub id: Option<String>,

    /// Minimum count of the block ID in a chunk to include it
    #[arg(long, requires = "id")]
    pub count: Option<u32>,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Mode {
    /// Only counts of region files and chunks that can be deleted without making any change to the world
    Check,

    /// Optimizes the world by deleting unused region files and chunks.
    /// This is a destructive process, make sure to make a backup of your worlds before running.
    /// Also make sure the world is not loaded by the game as this will corrupt the world.
    Write,

    /// Allows you to filter and delete chunks with specific block ids, also create and import a CSV file for easy deletion
    Palette,
}

fn validate_compression_level(s: &str) -> Result<u32, String> {
    match s.parse::<u32>() {
        Ok(level) if level <= 9 => Ok(level),
        _ => Err("Compression level must be an integer between 0 and 9".to_string()),
    }
}