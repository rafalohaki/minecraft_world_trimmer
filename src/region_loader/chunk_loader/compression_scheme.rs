#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompressionScheme {
    Gzip,
    Zlib,
    Lz4,
    Unknown(u8),
}

impl CompressionScheme {
    pub fn from_u8(byte: u8) -> Self {
        match byte {
            1 => CompressionScheme::Gzip,
            2 => CompressionScheme::Zlib,
            3 => CompressionScheme::Lz4,
            other => CompressionScheme::Unknown(other),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            CompressionScheme::Gzip => 1,
            CompressionScheme::Zlib => 2,
            CompressionScheme::Lz4 => 3,
            CompressionScheme::Unknown(v) => v,
        }
    }
}
