use crate::nbt::binary_reader::BinaryReader;
use crate::nbt::parse::parse_tag;
use crate::nbt::tag::Tag;
use crate::region_loader::chunk_loader::compression_scheme::CompressionScheme;
use crate::region_loader::get_u32::get_u32;
use crate::region_loader::location::Location;
use flate2::read::{GzDecoder, GzEncoder, ZlibDecoder, ZlibEncoder};
use flate2::Compression;
use lz4_flex::frame::FrameDecoder;
use std::io::Read;

#[derive(PartialEq, Debug, Clone)]
pub struct Chunk {
    pub nbt: Tag,
    pub location: Location,
    // Original compressed payload and its scheme, used when recompression fails
    original_compression_scheme: CompressionScheme,
    original_payload: Vec<u8>,
}

impl Chunk {
    const STATUS_FULL: &'static str = "minecraft:full";

    pub fn from_location(buf: &[u8], location: Location) -> Result<Self, &'static str> {
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
        let compression_scheme = CompressionScheme::from_u8(buf[compression_scheme_index])?;

        // Dane chunka: payload ma długość (chunk_size - 1)
        let header_size = 5; // 4 bajty rozmiaru + 1 bajt schematu
        let start = offset + header_size;
        let end = start + chunk_size - 1; // end jest ekskluzywne w slicingu
        if start > end || end > buf.len() {
            return Err("Chunk payload out of bounds");
        }
        let raw_first_chunk = &buf[start..end];
        let original_payload = raw_first_chunk.to_vec();

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
            CompressionScheme::Lz4 => {
                // Najpierw próbujemy dekodera "frame"
                let mut decoder = FrameDecoder::new(raw_first_chunk);
                let mut bytes = Vec::new();
                match decoder.read_to_end(&mut bytes) {
                    Ok(_) => Ok(bytes),
                    Err(_) => {
                        // Fallback: spróbuj trybu "block" z rozmiarem poprzedzającym (size-prepended)
                        lz4_flex::block::decompress_size_prepended(raw_first_chunk)
                            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "LZ4 block decompress failed"))
                    }
                }
            }
        };

        // Convert to string
        let nbt = decoded_bytes
            .and_then(|bytes| {
                let mut binary_reader = BinaryReader::new(&bytes);
                parse_tag(&mut binary_reader)
                    .map_err(|e| std::io::Error::new(
                        std::io::ErrorKind::InvalidData, 
                        format!("NBT parse error: {}", e)
                    ))
            })
            .map_err(|_| "Error while parsing NBT")?;

        Ok(Self {
            nbt,
            location,
            original_compression_scheme: compression_scheme,
            original_payload,
        })
    }

    pub fn to_bytes(&self, compression: Compression) -> Result<Vec<u8>, &'static str> {
        let decoded_bytes = self.nbt.to_bytes();
        // Try Zlib first; if it fails, fall back to Gzip. If both fail,
        // do not write mismatched header/payload — propagate error to leave chunk unchanged.
        let mut zlib_encoder = ZlibEncoder::new(&decoded_bytes[..], compression);
        let mut zlib_bytes = Vec::new();
        match zlib_encoder.read_to_end(&mut zlib_bytes) {
            Ok(_) => Ok(self.to_bytes_compression_scheme(CompressionScheme::Zlib, &zlib_bytes)),
            Err(_) => {
                let mut gzip_encoder = GzEncoder::new(&decoded_bytes[..], compression);
                let mut gzip_bytes = Vec::new();
                match gzip_encoder.read_to_end(&mut gzip_bytes) {
                    Ok(_) => Ok(self.to_bytes_compression_scheme(CompressionScheme::Gzip, &gzip_bytes)),
                    Err(_) => Err("Compression failed for both Zlib and Gzip"),
                }
            }
        }
    }

    pub fn get_position(&self) -> Result<(i32, i32), &'static str> {
        let x_pos_tag = self.nbt.find_tag("xPos").and_then(|v| v.get_int());
        let z_pos_tag = self.nbt.find_tag("zPos").and_then(|v| v.get_int());

        match (x_pos_tag, z_pos_tag) {
            (Some(x), Some(z)) => Ok((*x, *z)),
            _ => Err("No position for this chunk"),
        }
    }

    /// Checks if a chunk is not fully generated and has never been inhabited
    pub fn should_delete(&self) -> bool {
        !self.is_fully_generated() && !self.has_been_inhabited()
    }

    fn is_fully_generated(&self) -> bool {
        self.nbt
            .find_tag("Status")
            .and_then(|tag| tag.get_string())
            .map(|status| status == Chunk::STATUS_FULL)
            .unwrap_or(false) // if the tag is not present, we can assume that the chunk is not fully generated
    }

    fn has_been_inhabited(&self) -> bool {
        // The InhabitedTime value seems to be incremented for all 8 chunks around a player (including the one the player is standing in)
        let inhabited_time = self
            .nbt
            .find_tag("InhabitedTime")
            .and_then(|tag| tag.get_long())
            .copied()
            .unwrap_or(0); // If the tag is not present, we can assume that the chunk has never been inhabited

        inhabited_time > 0
    }

    pub fn to_original_bytes(&self) -> Vec<u8> {
        self.to_bytes_compression_scheme(self.original_compression_scheme, &self.original_payload)
    }

    fn to_bytes_compression_scheme(
        &self,
        compression_scheme: CompressionScheme,
        nbt_bytes: &[u8],
    ) -> Vec<u8> {
        let size = (nbt_bytes.len() + 1/* including the compression scheme byte */) as u32;
        let mut result = Vec::from(size.to_be_bytes());
        result.push(compression_scheme.to_u8()); // adding the compression scheme byte
        result.extend_from_slice(nbt_bytes);
        result
    }
}
