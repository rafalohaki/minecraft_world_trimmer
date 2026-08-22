use crate::region_loader::chunk_loader::chunk::Chunk;
use crate::region_loader::get_u32::get_u32;
use crate::region_loader::location::Location;
use flate2::Compression;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use thiserror::Error;

#[derive(PartialEq, Debug)]
pub struct Region {
    chunks: Vec<Chunk>,
    is_modified: bool,
}

pub struct ToBytesResult {
    pub bytes: Vec<u8>,
    pub compression_fallbacks: usize,
    pub header_write_failures: usize,
}

#[derive(Error, Debug)]
pub enum ParseRegionError {
    #[error("error while reading the file")]
    ReadError,
    #[error("cannot read header of region file")]
    HeaderError,
}

impl Region {
    pub fn from_file_name(file_name: &Path) -> Result<Self, ParseRegionError> {
        let bytes = try_read_bytes(file_name).map_err(|_| ParseRegionError::ReadError)?;
        Region::from_bytes(&bytes)
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, ParseRegionError> {
        let mut chunks = Vec::with_capacity(1024);
        if bytes.len() < 8192 {
            return Err(ParseRegionError::HeaderError);
        }

        let location_table = &bytes[0..4096];
        let timestamp_table = &bytes[4096..8192];

        for i in (0..4096).step_by(4) {
            let l = get_u32(location_table, i);
            let timestamp = get_u32(timestamp_table, i);
            let location = Location::from_bytes(l, timestamp);

            if location.is_valid() {
                if let Ok(chunk) = Chunk::from_location(bytes, location, i) {
                    chunks.push(chunk);
                }
                // Chunks with an unknown compression scheme (e.g. 127, a server-specific
                // custom algorithm) are loaded as opaque chunks and preserved verbatim;
                // only truly corrupt chunks are dropped here.
            }
        }

        Ok(Self {
            chunks,
            is_modified: false,
        })
    }

    pub fn to_bytes(&self, compression: Compression) -> ToBytesResult {
        let mut data: Vec<u8> = Vec::with_capacity(self.chunks.len() * 4096);
        let mut location_table = [0_u8; 4096];
        let mut timestamp_table = [0_u8; 4096];
        let mut compression_fallbacks = 0usize;
        let mut header_write_failures = 0usize;

        for chunk in &self.chunks {
            let mut serialized = match chunk.to_bytes(compression) {
                Ok(bytes) => bytes,
                Err(_) => {
                    compression_fallbacks += 1;
                    chunk.to_original_bytes()
                }
            };
            align_vec_size(&mut serialized);

            let new_position = (data.len() + 8192) as u32;
            let new_size = serialized.len() as u32;
            let original_timestamp = chunk.location.get_timestamp();
            let new_location = Location::new(new_position, new_size, original_timestamp);

            // Use the table index the chunk was loaded from instead of deriving it from
            // xPos/zPos, so opaque chunks (unknown compression scheme, no parsed NBT)
            // are written back to their original slot.
            let position_in_table = chunk.get_table_position();

            if let Ok(new_location) = new_location {
                if position_in_table + 4 <= location_table.len() {
                    let location_bytes = new_location.to_location_bytes();
                    location_table[position_in_table..(4 + position_in_table)]
                        .copy_from_slice(&location_bytes);

                    let timestamp_bytes = new_location.to_timestamp_bytes();
                    timestamp_table[position_in_table..(4 + position_in_table)]
                        .copy_from_slice(&timestamp_bytes);

                    data.extend(serialized);
                } else {
                    header_write_failures += 1;
                }
            } else {
                // Do not append payload if we cannot produce a valid header entry
                header_write_failures += 1;
            }
        }

        let mut bytes = Vec::with_capacity(8192 + data.len());
        bytes.extend_from_slice(&location_table);
        bytes.extend_from_slice(&timestamp_table);
        bytes.extend(data);
        ToBytesResult {
            bytes,
            compression_fallbacks,
            header_write_failures,
        }
    }

    pub fn get_chunks(&self) -> &[Chunk] {
        &self.chunks
    }

    pub fn get_chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn remove_chunk_by_index(&mut self, index: usize) {
        self.chunks.remove(index);
        if !self.is_modified {
            self.is_modified = true;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn is_modified(&self) -> bool {
        self.is_modified
    }
}

fn align_vec_size(vec: &mut Vec<u8>) {
    let aligned_size = vec.len().div_ceil(4096) * 4096;
    vec.resize(aligned_size, 0);
}

fn try_read_bytes(file_path: &Path) -> std::io::Result<Vec<u8>> {
    let estimated_len = std::fs::metadata(file_path)
        .map(|m| m.len() as usize)
        .unwrap_or(0)
        .min(1_000_000_000);

    let mut file = File::open(file_path)?;
    let mut buf = Vec::with_capacity(estimated_len);
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_align_vec_size() {
        let mut vec_500 = vec![0; 500];
        align_vec_size(&mut vec_500);
        assert_eq!(4096, vec_500.len());

        let mut vec_4096 = vec![0; 4096];
        align_vec_size(&mut vec_4096);
        assert_eq!(4096, vec_4096.len());

        let mut vec_4097 = vec![0; 4097];
        align_vec_size(&mut vec_4097);
        assert_eq!(8192, vec_4097.len());
    }

    #[test]
    fn test_small_region() {
        let original_bytes = include_bytes!("../../test_files/r.-1.-1.mca");

        let original_parsed_region_file = Region::from_bytes(original_bytes)
            .expect("Failed to parse original region file");
        let result = original_parsed_region_file.to_bytes(Compression::fast());

        // We cannot validate the header as the compression and chunk order in the payload may differ
        // resulting in a modification of the offset bytes, so as long as the re-parsed region file is
        // the same as the parsed original, we should be fine

        let parsed_again = Region::from_bytes(&result.bytes)
            .expect("Failed to parse serialized region file");

        let original_chunks = original_parsed_region_file.get_chunks();
        let parsed_chunks = parsed_again.get_chunks();
        assert_eq!(parsed_chunks.len(), original_chunks.len());

        for i in 0..original_chunks.len() {
            let original_chunk = &original_chunks[i];
            let parsed_chunk = &parsed_chunks[i];
            assert_eq!(original_chunk.nbt, parsed_chunk.nbt);
        }
    }

    /// Byte-for-byte round-trip check on the *decompressed* NBT payload of every chunk
    /// in a real Minecraft region file. Guards against:
    ///   - `flate2` bumps producing a lossy deflate/inflate path
    ///   - NBT serializer drift (tag order, padding, endianness)
    ///   - `lz4_flex` bumps for chunks with scheme byte = 4 (if present in sample)
    ///
    /// Comparison strategy: serialize each chunk's parsed NBT to bytes, then re-parse
    /// after a region-level write→read cycle and compare the serialized NBT bytes.
    /// We compare serialized NBT (not raw compressed bytes) because zlib-ng heuristics
    /// may legitimately produce different compressed output across versions while
    /// preserving the decompressed payload — which is what Minecraft actually consumes.
    #[test]
    fn test_roundtrip_decompressed_nbt_byte_for_byte() {
        let original_bytes = include_bytes!("../../test_files/r.-1.-1.mca");

        let original_region = Region::from_bytes(original_bytes)
            .expect("Failed to parse original region file");
        assert!(
            !original_region.get_chunks().is_empty(),
            "sample region file must contain at least one chunk"
        );

        for compression in [
            Compression::fast(),
            Compression::default(),
            Compression::best(),
        ] {
            let result = original_region.to_bytes(compression);

            assert_eq!(
                result.compression_fallbacks, 0,
                "zlib should not fall back to gzip on a healthy sample"
            );
            assert_eq!(
                result.header_write_failures, 0,
                "every chunk should produce a valid header entry"
            );

            let parsed_again = Region::from_bytes(&result.bytes)
                .expect("Failed to parse serialized region file");

            let original_chunks = original_region.get_chunks();
            let parsed_chunks = parsed_again.get_chunks();
            assert_eq!(
                parsed_chunks.len(),
                original_chunks.len(),
                "chunk count must be preserved (compression level {:?})",
                compression
            );

            for (i, (original, parsed)) in
                original_chunks.iter().zip(parsed_chunks.iter()).enumerate()
            {
                let original_nbt_bytes = original.nbt.as_ref().expect("sample chunks must parse").to_bytes();
                let parsed_nbt_bytes = parsed.nbt.as_ref().expect("sample chunks must parse").to_bytes();
                assert_eq!(
                    original_nbt_bytes, parsed_nbt_bytes,
                    "chunk #{i} decompressed NBT differs after round-trip (compression {:?})",
                    compression
                );
                assert_eq!(
                    original.get_position().ok(),
                    parsed.get_position().ok(),
                    "chunk #{i} (x,z) position differs after round-trip",
                );
            }
        }
    }

    /// A chunk stored with an unknown compression scheme (e.g. 127 = server-specific
    /// custom algorithm) must be loaded as an opaque chunk, never deleted, and
    /// written back byte-for-byte including its original scheme byte.
    #[test]
    fn test_unknown_compression_scheme_chunk_is_preserved_verbatim() {
        let payload: [u8; 16] = [
            0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
            0x0A, 0x0B,
        ];
        // Chunk frame: 4-byte size (payload + scheme byte), scheme byte 127 (custom)
        let mut chunk_bytes = Vec::new();
        chunk_bytes.extend_from_slice(&((payload.len() + 1) as u32).to_be_bytes());
        chunk_bytes.push(127_u8);
        chunk_bytes.extend_from_slice(&payload);

        // Build a minimal region file: location entry points at sector 2, one sector long
        let mut bytes = vec![0_u8; 8192];
        bytes[0..4].copy_from_slice(&[0, 0, 2, 1]);
        bytes.extend_from_slice(&chunk_bytes);

        let region =
            Region::from_bytes(&bytes).expect("region with opaque chunk must parse");
        assert_eq!(region.get_chunk_count(), 1);
        let chunk = &region.get_chunks()[0];
        assert!(chunk.nbt.is_none(), "opaque chunk must have no parsed NBT");
        assert!(
            !chunk.should_delete(),
            "opaque chunk must never be deleted"
        );

        // Round-trip: rewritten region keeps the chunk verbatim at a valid slot
        let result = region.to_bytes(Compression::fast());
        assert_eq!(result.compression_fallbacks, 0);
        assert_eq!(result.header_write_failures, 0);

        let reparsed =
            Region::from_bytes(&result.bytes).expect("rewritten region must re-parse");
        assert_eq!(reparsed.get_chunk_count(), 1);
        let reparsed_chunk = &reparsed.get_chunks()[0];
        assert!(reparsed_chunk.nbt.is_none());
        assert!(!reparsed_chunk.should_delete());

        // Extract the raw framed chunk from the rewritten file and compare byte-for-byte
        let offset = reparsed_chunk.location.get_offset() as usize;
        let written_frame = &result.bytes[offset..offset + 5 + payload.len()];
        assert_eq!(
            written_frame,
            &chunk_bytes[..],
            "opaque chunk payload and scheme byte must survive a write unchanged"
        );
    }
}
