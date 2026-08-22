mod cli;
mod commands;
mod nbt;
mod region_loader;
mod world;

use crate::cli::{Cli, Mode};
use crate::commands::read::execute_read;
use crate::commands::write::execute_write;
use clap::Parser;
use flate2::Compression;
use rayon::ThreadPoolBuilder;

fn main() {
    let cli = Cli::parse();

    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    // Zostawiamy 2 rdzenie wolne — na hoście z pufferpanel/mc serwerem pełne
    // obciążenie wszystkich rdzeni wywołuje throttle i lag serwera. Koszt to
    // ~20-25% throughputu, ale system pozostaje responsywny. Przy 70k regionów
    // różnica to ~12 min vs ~9 min — warto.
    let threads = cpus.saturating_sub(2).max(1);
    let _ = ThreadPoolBuilder::new().num_threads(threads).build_global();

    let result = match cli.mode {
        Mode::Write => execute_write(
            &cli.world_paths,
            Compression::new(cli.compression_level),
            cli.backup_dir.as_deref(),
        ),
        Mode::Check => execute_read(&cli.world_paths),
    };

    if let Err(err) = result {
        eprintln!("{err}");
    }
}
