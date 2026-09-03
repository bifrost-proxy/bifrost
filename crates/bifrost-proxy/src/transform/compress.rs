use flate2::write::{GzEncoder, ZlibEncoder};
use flate2::Compression;
use std::io::Write;

pub fn compress_body(data: &[u8], content_encoding: &str) -> std::io::Result<Vec<u8>> {
    let mut encoded = data.to_vec();
    for encoding in content_encoding
        .split(',')
        .map(str::trim)
        .filter(|encoding| !encoding.is_empty())
    {
        encoded = match encoding.to_ascii_lowercase().as_str() {
            "identity" => encoded,
            "gzip" | "x-gzip" => {
                let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(&encoded)?;
                encoder.finish()?
            }
            // Most clients interpret "deflate" as zlib wrapped stream.
            "deflate" => {
                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(&encoded)?;
                encoder.finish()?
            }
            "br" => {
                let mut out = Vec::new();
                {
                    let mut encoder = brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
                    encoder.write_all(&encoded)?;
                }
                out
            }
            "zstd" => zstd::stream::encode_all(std::io::Cursor::new(encoded), 0)?,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unsupported content-encoding: {encoding}"),
                ));
            }
        };
    }
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::decompress::try_decompress_body_with_limit;

    #[test]
    fn test_gzip_roundtrip() {
        let original = b"<html><body>Hello</body></html>";
        let compressed = compress_body(original, "gzip").unwrap();
        let decompressed = try_decompress_body_with_limit(&compressed, "gzip", 1024).unwrap();
        assert_eq!(decompressed, original);

        let compressed_alias = compress_body(original, "x-gzip").unwrap();
        let decompressed_alias =
            try_decompress_body_with_limit(&compressed_alias, "x-gzip", 1024).unwrap();
        assert_eq!(decompressed_alias, original);
    }

    #[test]
    fn test_identity_and_empty_encoding_return_input() {
        let data = b"plain text";
        assert_eq!(compress_body(data, "").unwrap(), data);
        assert_eq!(compress_body(data, "identity").unwrap(), data);
        let encoded = compress_body(data, "identity, gzip").unwrap();
        assert_eq!(
            try_decompress_body_with_limit(&encoded, "identity, gzip", 1024).unwrap(),
            data
        );
    }

    #[test]
    fn test_deflate_roundtrip() {
        let original = b"hello deflate";
        let compressed = compress_body(original, "deflate").unwrap();
        let decompressed = try_decompress_body_with_limit(&compressed, "deflate", 1024).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_brotli_roundtrip() {
        let original = b"hello brotli";
        let compressed = compress_body(original, "br").unwrap();
        let decompressed = try_decompress_body_with_limit(&compressed, "br", 1024).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_zstd_roundtrip() {
        let original = b"hello zstd";
        let compressed = compress_body(original, "zstd").unwrap();
        let decompressed = try_decompress_body_with_limit(&compressed, "zstd", 1024).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_multiple_content_codings_roundtrip() {
        let original = b"stacked encoding roundtrip";

        let compressed = compress_body(original, "gzip, br").unwrap();
        let decompressed = try_decompress_body_with_limit(&compressed, "gzip, br", 1024).unwrap();

        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_unsupported_encoding_returns_error() {
        let err = compress_body(b"data", "unknown-encoding").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err
            .to_string()
            .contains("unsupported content-encoding: unknown-encoding"));
    }
}
