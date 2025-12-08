# Minecraft World Trimmer

> [!WARNING]
> This software is highly experimental. Use at your own risks and report any bugs by submitting an issue on GitHub.

**Minecraft World Trimmer** is a command-line tool designed to reduce the size of your Minecraft world by parsing and
trimming region files. It's particularly useful for large worlds that have many generated but unexplored chunks.

## How it works

The trimmer employs the following steps to analyze and trim your world:

1. **Parallel Processing:** Leverage all available threads on your computer to load and parse region files
   simultaneously, improving processing speed.
2. **Invalid File Removal:** Deletes any invalid region files, such as those with insufficient size to hold chunk data.
3. **Chunk Parsing & Removal:**
    * Ignores and deletes invalid chunks within valid region files.
    * Removes not fully generated or unexplored chunks to reduce world size.
4. **Improved Compression:** Chunks are saved back using the best available compression level for further space
   savings.

The trimmer operates in-place, replacing original region files with trimmed ones. **Always make a backup before running!
**

## Supported Compression Algorithms

Minecraft stores chunk data in region files using multiple compression schemes.

- Read support: `Zlib`, `GZip`, and `LZ4`.
  - LZ4 is supported both as framed (`lz4 frame`) and as size-prepended block payloads. If framed decoding fails, we
    automatically fall back to size-prepended block decoding.
- Write (recompression) behavior:
  - By default, chunks are re-serialized and compressed using `Zlib` (falling back to `GZip` if `Zlib` fails) according
    to the configured compression level.
  - If recompression fails for a chunk, we do not drop your data: the original compressed bytes and their compression
    scheme are preserved and written back verbatim (including `LZ4` payloads). This is a fallback path intended to keep
    worlds safe.

Important:
- We do not proactively recompress chunks to `LZ4`. LZ4 appears only when the original chunk was already LZ4-compressed
  and we take the fallback path that preserves original bytes.
- Worlds using custom or unsupported compression schemes may still be at risk. While we do our best to decode
  recognized formats and preserve original bytes on errors, unusual formats could lead to skipped recompression or
  removal in trimming scenarios. Always keep backups.

The Minecraft World Trimmer has only been successfully tested on 1.20.6 and 1.21 vanilla worlds.

## Expected Results

The Minecraft World Trimmer as successfully been tested on the following worlds:

- **Server Worlds:** Backed-up world of my survival server with over 17.9 million generated chunks.
- **New World:** New amplified world pre-generated using Chunky with a square radius of 1024.

| World         | Before Trimming                          | After Trimming                         | Size Reduction |
|---------------|------------------------------------------|----------------------------------------|----------------|
| Server Worlds | 134,079,252 Kilobytes <br/> 29,861 Files | 27,220,248 Kilobytes <br/> 9,734 Files | **79.7%**      |
| New World     | 235,204 Kilobytes <br/> 36 Files         | 2,440 Kilobytes <br/> 4 Files          | **98.9%**      |

## Getting Started

### Prerequisites

- Rust toolchain (stable)
- A backed-up Minecraft world

### Pre-Built Binaries

Pre-built binaries (when available) are published in the
[releases](https://github.com/rafalohaki/minecraft_world_trimmer/releases) of this repository.

### With Cargo

```shell
cargo install --git https://github.com/rafalohaki/minecraft_world_trimmer.git
```

### From Source

1. Clone the repository:
   ```shell
   git clone https://github.com/rafalohaki/minecraft_world_trimmer.git
   cd minecraft_world_trimmer
   ```
2. Install the trimmer globally:
   ```shell
   cargo install --path .
   ```

## Usage

```shell
# View up-to-date usage of the command:
❯ minecraft_world_trimmer --help
❯ minecraft_world_trimmer <MODE> <WORLD_PATHS>...
```

Replace `<MODE>` with one of the following:

- `check`: the program will only check for chunks and region files that can be deleted without actually deleting any
  data. This mode is around two times faster than the write mode as it does not perform any file system operations.
- `write`: the program will delete unused chunks and region files.

Replace `<WORLD_PATHS>` with the path to your Minecraft world folders containing region files.
It will detect the 3 vanilla dimensions and trim them. Note that this has not been tested on modded worlds with
multiple dimensions.

Example:

```shell
❯ minecraft_world_trimmer check ~/.minecraft/saves/MyWorld
```

It can also be used to trim server worlds as dimensions are split in multiple worlds:

```shell
❯ minecraft_world_trimmer check /path/to/server/world*
# Or if your shell does not support wildcard:
❯ minecraft_world_trimmer check /path/to/server/world /path/to/server/world_nether /path/to/server/world_the_end
```

## Similar Tools

- [Querz/mcaselector](https://github.com/Querz/mcaselector) - has a graphical user interface
- [aternosorg/thanos](https://github.com/aternosorg/thanos) - PHP library

## Contributing

Contributions are welcome! If you encounter any issues or have suggestions for improvement, please submit an issue or
pull request on GitHub. Make sure to follow the existing code style and include relevant tests.

1. Fork the repository.
2. Create a new branch `git checkout -b <branch-name>`.
3. Make changes and commit `git commit -m 'Add some feature'`.
4. Push to your fork `git push origin <branch-name>`.
5. Submit a pull request.
