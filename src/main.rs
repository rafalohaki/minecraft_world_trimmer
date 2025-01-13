mod cli;
mod commands;
mod nbt;
mod region_loader;
mod world;

use crate::cli::{Cli, Mode};
use crate::commands::palette::execute_palette;

use crate::commands::read::execute_read;
use crate::commands::write::execute_write;
use clap::Parser;
use flate2::Compression;

fn main() {
    let cli = Cli::parse();

    let result = match cli.mode {
        Mode::Write => execute_write(&cli.world_paths, Compression::new(cli.compression_level)),
        Mode::Check => execute_read(&cli.world_paths),
        Mode::Palette => execute_palette(
            &cli.world_paths,
            &cli.csv_out,
            &cli.csv_in,
            cli.id.as_deref(),
            cli.count,
        ),
    };

    if let Err(err) = result {
        eprintln!("{err}");
    }
}
