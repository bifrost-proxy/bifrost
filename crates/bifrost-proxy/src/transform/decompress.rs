use bytes::Bytes;
use flate2::read::{DeflateDecoder, MultiGzDecoder, ZlibDecoder};
use std::io::{Read, Write};

const DEFAULT_MAX_DECOMPRESS_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

pub fn decompress_body(data: &[u8], content_encoding: Option<&str>) -> Bytes {
    decompress_body_with_limit(data, content_encoding, DEFAULT_MAX_DECOMPRESS_OUTPUT_BYTES)
}

/// 解压 HTTP body（gzip/deflate/br/zstd），并限制解压后的最大输出大小。
///
/// - 当 `max_output_bytes` 为 0 时，直接返回原始数据。
/// - 当解压输出超过上限时，放弃解压并回退到原始压缩数据（用于防止压缩炸弹）。
pub fn decompress_body_with_limit(
    data: &[u8],
    content_encoding: Option<&str>,
    max_output_bytes: usize,
) -> Bytes {
    if max_output_bytes == 0 {
        return Bytes::copy_from_slice(data);
    }

    let content_encoding = match content_encoding {
        Some(encoding) => encoding,
        None => return Bytes::copy_from_slice(data),
    };

    match try_decompress_body_with_limit(data, content_encoding, max_output_bytes) {
        Ok(decompressed) => Bytes::from(decompressed),
        Err(e) => {
            tracing::debug!(
                "Failed to decompress {} body (limit={}): {}",
                content_encoding,
                max_output_bytes,
                e
            );
            Bytes::copy_from_slice(data)
        }
    }
}

pub fn try_decompress_body_with_limit(
    data: &[u8],
    content_encoding: &str,
    max_output_bytes: usize,
) -> Result<Vec<u8>, std::io::Error> {
    if max_output_bytes == 0 {
        return Ok(data.to_vec());
    }

    let mut decoded = data.to_vec();
    let mut remaining_output_bytes = max_output_bytes;
    for encoding in content_encoding
        .split(',')
        .map(str::trim)
        .filter(|encoding| !encoding.is_empty())
        .rev()
    {
        let next = match encoding.to_ascii_lowercase().as_str() {
            "identity" => decoded,
            "gzip" | "x-gzip" => decompress_gzip_limited(&decoded, remaining_output_bytes)?,
            "deflate" => decompress_deflate_limited(&decoded, remaining_output_bytes)?,
            "br" => decompress_brotli_limited(&decoded, remaining_output_bytes)?,
            "zstd" => decompress_zstd_limited(&decoded, remaining_output_bytes)?,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unsupported content-encoding: {encoding}"),
                ));
            }
        };
        if !encoding.eq_ignore_ascii_case("identity") {
            remaining_output_bytes -= next.len();
        }
        decoded = next;
    }
    Ok(decoded)
}

fn decompress_gzip_limited(
    data: &[u8],
    max_output_bytes: usize,
) -> Result<Vec<u8>, std::io::Error> {
    let mut decoder = MultiGzDecoder::new(data);
    read_to_end_limited(&mut decoder, max_output_bytes)
}

fn decompress_deflate_limited(
    data: &[u8],
    max_output_bytes: usize,
) -> Result<Vec<u8>, std::io::Error> {
    if let Ok(result) = decompress_zlib_limited(data, max_output_bytes) {
        return Ok(result);
    }
    let mut decoder = DeflateDecoder::new(data);
    read_to_end_limited(&mut decoder, max_output_bytes)
}

fn decompress_zlib_limited(
    data: &[u8],
    max_output_bytes: usize,
) -> Result<Vec<u8>, std::io::Error> {
    let mut decoder = ZlibDecoder::new(data);
    read_to_end_limited(&mut decoder, max_output_bytes)
}

fn decompress_brotli_limited(
    data: &[u8],
    max_output_bytes: usize,
) -> Result<Vec<u8>, std::io::Error> {
    let mut decompressed = Vec::new();
    let mut writer = LimitedWriter::new(&mut decompressed, max_output_bytes);
    brotli::BrotliDecompress(&mut std::io::Cursor::new(data), &mut writer)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    Ok(decompressed)
}

fn decompress_zstd_limited(
    data: &[u8],
    max_output_bytes: usize,
) -> Result<Vec<u8>, std::io::Error> {
    let cursor = std::io::Cursor::new(data);
    let mut decoder = zstd::stream::read::Decoder::new(cursor)?;
    read_to_end_limited(&mut decoder, max_output_bytes)
}

fn read_to_end_limited<R: Read>(
    reader: &mut R,
    max_output_bytes: usize,
) -> Result<Vec<u8>, std::io::Error> {
    let mut limited = reader.take((max_output_bytes as u64).saturating_add(1));
    let mut out = Vec::new();
    limited.read_to_end(&mut out)?;
    if out.len() > max_output_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "decompressed body too large ({} > {} bytes)",
                out.len(),
                max_output_bytes
            ),
        ));
    }
    Ok(out)
}

struct LimitedWriter<'a> {
    inner: &'a mut Vec<u8>,
    limit: usize,
}

impl<'a> LimitedWriter<'a> {
    fn new(inner: &'a mut Vec<u8>, limit: usize) -> Self {
        Self { inner, limit }
    }
}

impl Write for LimitedWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let next_len = self.inner.len().saturating_add(buf.len());
        if next_len > self.limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "decompressed body too large ({} > {} bytes)",
                    next_len, self.limit
                ),
            ));
        }
        self.inner.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn get_content_encoding(headers: &[(String, String)]) -> Option<String> {
    let values = headers
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case("content-encoding"))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decompress_no_encoding() {
        let data = b"hello world";
        let result = decompress_body(data, None);
        assert_eq!(result.as_ref(), data);
    }

    #[test]
    fn test_decompress_identity() {
        let data = b"hello world";
        let result = decompress_body(data, Some("identity"));
        assert_eq!(result.as_ref(), data);
    }

    #[test]
    fn test_content_encoding_headers_are_combined_and_x_gzip_is_supported() {
        let headers = vec![
            ("Content-Encoding".to_string(), "gzip".to_string()),
            ("content-encoding".to_string(), "br".to_string()),
        ];
        assert_eq!(get_content_encoding(&headers).as_deref(), Some("gzip, br"));

        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"alias").unwrap();
        let compressed = encoder.finish().unwrap();
        assert_eq!(
            decompress_body(&compressed, Some("x-gzip")).as_ref(),
            b"alias"
        );
    }

    #[test]
    fn test_decompress_gzip() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let original = b"hello world";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();

        let result = decompress_body(&compressed, Some("gzip"));
        assert_eq!(result.as_ref(), original);
    }

    #[test]
    fn test_decompresses_all_concatenated_gzip_members() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        fn gzip_member(data: &[u8]) -> Vec<u8> {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(data).unwrap();
            encoder.finish().unwrap()
        }

        let mut compressed = gzip_member(b"first member ");
        compressed.extend_from_slice(&gzip_member(b"second member"));

        let result = decompress_body(&compressed, Some("gzip"));
        assert_eq!(result.as_ref(), b"first member second member");
    }

    #[test]
    fn test_decompress_deflate() {
        use flate2::write::DeflateEncoder;
        use flate2::Compression;
        use std::io::Write;

        let original = b"hello world";
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();

        let result = decompress_body(&compressed, Some("deflate"));
        assert_eq!(result.as_ref(), original);
    }

    #[test]
    fn test_decompresses_multiple_content_codings_in_reverse_order() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let original = b"multiply encoded body";
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(original).unwrap();
        let gzip = gzip.finish().unwrap();

        let mut brotli = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut brotli, 4096, 5, 22);
            encoder.write_all(&gzip).unwrap();
        }

        let result =
            try_decompress_body_with_limit(&brotli, "gzip, br", 1024).expect("decode chain");
        assert_eq!(result, original);
    }

    #[test]
    fn test_multiple_content_codings_share_one_output_budget() {
        use flate2::write::{GzEncoder, ZlibEncoder};
        use flate2::Compression;
        use std::io::Write;

        let original = vec![b'a'; 1024];
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&original).unwrap();
        let gzip = gzip.finish().unwrap();
        let mut deflate = ZlibEncoder::new(Vec::new(), Compression::default());
        deflate.write_all(&gzip).unwrap();
        let wire = deflate.finish().unwrap();
        let shared_limit = original.len() + gzip.len() - 1;

        let error = try_decompress_body_with_limit(&wire, "gzip, deflate", shared_limit)
            .expect_err("successful layers must share one output budget");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("too large"));
    }

    #[test]
    fn test_unknown_content_coding_preserves_original_body() {
        let original = b"custom encoded body";

        let result = decompress_body_with_limit(original, Some("gzip, x-company-codec"), 1024);

        assert_eq!(result.as_ref(), original);
        let error = try_decompress_body_with_limit(original, "gzip, x-company-codec", 1024)
            .expect_err("custom coding must be left to custom decoders");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("x-company-codec"));
    }
}
