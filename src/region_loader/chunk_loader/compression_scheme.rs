#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompressionScheme {
    Gzip,
    Zlib,
    Uncompressed,
    Lz4,
}

impl CompressionScheme {
    /// Byte values as defined by the vanilla region file format:
    /// 1 = GZip, 2 = Zlib, 3 = Uncompressed, 4 = LZ4 (since 24w04a).
    /// 127 denotes a server-specific custom algorithm and cannot be decoded here;
    /// chunks using it must be preserved verbatim, never dropped.
    pub fn from_u8(byte: u8) -> Result<Self, &'static str> {
        match byte {
            1 => Ok(CompressionScheme::Gzip),
            2 => Ok(CompressionScheme::Zlib),
            3 => Ok(CompressionScheme::Uncompressed),
            4 => Ok(CompressionScheme::Lz4),
            _ => Err("Unsupported compression scheme"),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            CompressionScheme::Gzip => 1,
            CompressionScheme::Zlib => 2,
            CompressionScheme::Uncompressed => 3,
            CompressionScheme::Lz4 => 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_scheme_bytes_round_trip() {
        for byte in [1_u8, 2, 3, 4] {
            let scheme = CompressionScheme::from_u8(byte).expect("scheme must be known");
            assert_eq!(scheme.to_u8(), byte);
        }
    }

    #[test]
    fn test_custom_and_unknown_schemes_are_rejected() {
        assert!(CompressionScheme::from_u8(0).is_err());
        assert!(CompressionScheme::from_u8(127).is_err());
        assert!(CompressionScheme::from_u8(255).is_err());
    }
}
