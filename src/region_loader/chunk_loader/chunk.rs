use crate::nbt::binary_reader::BinaryReader;
use crate::nbt::parse::parse_tag;
use crate::nbt::tag::Tag;
use crate::region_loader::chunk_loader::compression_scheme::CompressionScheme;
use crate::region_loader::get_u32::get_u32;
use crate::region_loader::location::Location;
use flate2::Compression;
use flate2::read::{GzDecoder, GzEncoder, ZlibDecoder, ZlibEncoder};
use lz4_flex::frame::FrameDecoder;
use std::io::Read;

#[derive(PartialEq, Debug, Clone)]
pub struct Chunk {
    /// Parsed chunk NBT. `None` for chunks we cannot decode (e.g. compression
    /// scheme 127, a server-specific custom algorithm) — those are preserved
    /// verbatim and never deleted, so no data is lost on rewrite.
    pub nbt: Option<Tag>,
    pub location: Location,
    /// Byte index of this chunk's entry in the region location table.
    table_position: usize,
    /// Original compression scheme byte as stored in the file (may be unknown).
    original_scheme_byte: u8,
    /// Original compressed payload and its scheme, used when recompression fails
    /// or the chunk is opaque (unknown scheme).
    original_payload: Vec<u8>,
}

impl Chunk {
    const STATUS_FULL: &'static str = "minecraft:full";

    pub fn from_location(
        buf: &[u8],
        location: Location,
        table_position: usize,
    ) -> Result<Self, &'static str> {
        // Chunk header parsing z ochroną zakresów
        let offset = location.get_offset() as usize;

        // Sprawdź dostępność 4 bajtów rozmiaru
        if offset + 4 > buf.len() {
            return Err("Chunk header out of bounds");
        }
        let chunk_size = get_u32(buf, offset) as usize;
        if chunk_size == 0 {
            return Err("Invalid chunk size (zero)");
        }

        // Bajt schematu kompresji
        let compression_scheme_index = offset + 4;
        if compression_scheme_index >= buf.len() {
            return Err("Compression scheme out of bounds");
        }
        let compression_scheme_byte = buf[compression_scheme_index];

        // Dane chunka: payload ma długość (chunk_size - 1)
        let header_size = 5; // 4 bajty rozmiaru + 1 bajt schematu
        let start = offset
            .checked_add(header_size)
            .ok_or("Chunk start offset overflow")?;
        // payload_len = chunk_size - 1 (chunk_size > 0 already verified)
        let payload_len = chunk_size - 1;
        let end = start
            .checked_add(payload_len)
            .ok_or("Chunk end offset overflow")?;
        if end > buf.len() {
            return Err("Chunk payload out of bounds");
        }
        let raw_first_chunk = &buf[start..end];
        let original_payload = raw_first_chunk.to_vec();

        let compression_scheme = match CompressionScheme::from_u8(compression_scheme_byte) {
            Ok(scheme) => scheme,
            Err(_) => {
                return Ok(Self {
                    nbt: None,
                    location,
                    table_position,
                    original_scheme_byte: compression_scheme_byte,
                    original_payload,
                });
            }
        };

        // Depending on the compression scheme, read the data
        let decoded_bytes = match compression_scheme {
            CompressionScheme::Gzip => {
                let mut decoder = GzDecoder::new(raw_first_chunk);
                let mut bytes = Vec::new();
                decoder.read_to_end(&mut bytes).map(|_| bytes)
            }
            CompressionScheme::Zlib => {
                let mut decoder = ZlibDecoder::new(raw_first_chunk);
                let mut bytes = Vec::new();
                decoder.read_to_end(&mut bytes).map(|_| bytes)
            }
            CompressionScheme::Uncompressed => Ok(raw_first_chunk.to_vec()),
            CompressionScheme::Lz4 => {
                // Najpierw próbujemy dekodera "frame"
                let mut decoder = FrameDecoder::new(raw_first_chunk);
                let mut bytes = Vec::new();
                match decoder.read_to_end(&mut bytes) {
                    Ok(_) => Ok(bytes),
                    Err(_) => {
                        // Fallback: spróbuj trybu "block" z rozmiarem poprzedzającym (size-prepended)
                        lz4_flex::block::decompress_size_prepended(raw_first_chunk).map_err(|_| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "LZ4 block decompress failed",
                            )
                        })
                    }
                }
            }
        };

        // Convert to string
        let nbt = decoded_bytes
            .and_then(|bytes| {
                let mut binary_reader = BinaryReader::new(&bytes);
                parse_tag(&mut binary_reader).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("NBT parse error: {}", e),
                    )
                })
            })
            .map_err(|_| "Error while parsing NBT")?;

        Ok(Self {
            nbt: Some(nbt),
            location,
            table_position,
            original_scheme_byte: compression_scheme_byte,
            original_payload,
        })
    }

    pub fn to_bytes(&self, compression: Compression) -> Result<Vec<u8>, &'static str> {
        let Some(nbt) = &self.nbt else {
            // Opaque chunk (unknown/custom compression scheme): preserve verbatim
            return Ok(self.to_original_bytes());
        };
        let decoded_bytes = nbt.to_bytes();
        // Try Zlib first; if it fails, fall back to Gzip. If both fail,
        // do not write mismatched header/payload — propagate error to leave chunk unchanged.
        let mut zlib_encoder = ZlibEncoder::new(&decoded_bytes[..], compression);
        let mut zlib_bytes = Vec::new();
        match zlib_encoder.read_to_end(&mut zlib_bytes) {
            Ok(_) => Ok(Self::frame(CompressionScheme::Zlib.to_u8(), &zlib_bytes)),
            Err(_) => {
                let mut gzip_encoder = GzEncoder::new(&decoded_bytes[..], compression);
                let mut gzip_bytes = Vec::new();
                match gzip_encoder.read_to_end(&mut gzip_bytes) {
                    Ok(_) => Ok(Self::frame(CompressionScheme::Gzip.to_u8(), &gzip_bytes)),
                    Err(_) => Err("Compression failed for both Zlib and Gzip"),
                }
            }
        }
    }

    /// Chunk coordinates from the parsed NBT. Kept for diagnostics/tests; region
    /// serialization uses the stored location-table index instead.
    #[allow(dead_code)]
    pub fn get_position(&self) -> Result<(i32, i32), &'static str> {
        let nbt = self.nbt.as_ref().ok_or("Chunk has no parsed NBT")?;
        let x_pos_tag = nbt.find_tag("xPos").and_then(|v| v.get_int());
        let z_pos_tag = nbt.find_tag("zPos").and_then(|v| v.get_int());

        match (x_pos_tag, z_pos_tag) {
            (Some(x), Some(z)) => Ok((*x, *z)),
            _ => Err("No position for this chunk"),
        }
    }

    /// Byte index of this chunk's entry in the region location table.
    pub fn get_table_position(&self) -> usize {
        self.table_position
    }

    /// Checks if a chunk is not fully generated and has never been inhabited.
    /// Opaque chunks (unknown compression scheme) are never deleted.
    pub fn should_delete(&self) -> bool {
        self.nbt.is_some() && !self.is_fully_generated() && !self.has_been_inhabited()
    }

    fn is_fully_generated(&self) -> bool {
        self.nbt
            .as_ref()
            .and_then(|nbt| nbt.find_tag("Status"))
            .and_then(|tag| tag.get_string())
            .map(|status| status == Chunk::STATUS_FULL)
            // A chunk whose Status cannot be read is treated as fully generated:
            // every vanilla chunk carries this tag, so its absence means we do not
            // understand the format — err on the side of keeping the chunk.
            .unwrap_or(true)
    }

    fn has_been_inhabited(&self) -> bool {
        // The InhabitedTime value seems to be incremented for all 8 chunks around a player (including the one the player is standing in)
        match self
            .nbt
            .as_ref()
            .and_then(|nbt| nbt.find_tag("InhabitedTime"))
            .and_then(|tag| tag.get_long())
            .copied()
        {
            Some(inhabited_time) => inhabited_time > 0,
            // Every vanilla chunk carries this tag; its absence means we do not
            // understand the format — err on the side of keeping the chunk.
            None => true,
        }
    }

    pub fn to_original_bytes(&self) -> Vec<u8> {
        Self::frame(self.original_scheme_byte, &self.original_payload)
    }

    fn frame(scheme_byte: u8, payload: &[u8]) -> Vec<u8> {
        let size = (payload.len() + 1/* including the compression scheme byte */) as u32;
        let mut result = Vec::from(size.to_be_bytes());
        result.push(scheme_byte); // adding the compression scheme byte
        result.extend_from_slice(payload);
        result
    }
}
