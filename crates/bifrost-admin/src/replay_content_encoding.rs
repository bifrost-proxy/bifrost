use std::io::Write;

use flate2::write::{GzEncoder, ZlibEncoder};
use flate2::Compression;

pub(crate) fn content_encoding_value(headers: &[(String, String)]) -> Option<String> {
    let values = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-encoding"))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join(", "))
}

pub(crate) fn encode_content_encoded_body(
    data: &[u8],
    content_encoding: &str,
) -> std::io::Result<Vec<u8>> {
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
            "deflate" => {
                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(&encoded)?;
                encoder.finish()?
            }
            "br" => {
                let mut output = Vec::new();
                {
                    let mut encoder = brotli::CompressorWriter::new(&mut output, 4096, 5, 22);
                    encoder.write_all(&encoded)?;
                }
                output
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

/// Replay request DTOs carry body content as plaintext. Remove wire-level body
/// metadata before forwarding so headers cannot describe bytes that are no
/// longer compressed or whose length has changed.
pub(crate) fn normalize_plaintext_body_headers(headers: &mut Vec<(String, String)>) {
    headers.retain(|(name, _)| {
        !name.eq_ignore_ascii_case("content-encoding")
            && !name.eq_ignore_ascii_case("content-length")
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_content_encodings_roundtrip() {
        let original = br#"{"kind":"roundtrip"}"#;
        for encoding in [
            "identity", "gzip", "x-gzip", "deflate", "br", "zstd", "gzip, br",
        ] {
            let encoded = encode_content_encoded_body(original, encoding).unwrap();
            let decoded =
                crate::handlers::network_body::decompress_with_limit(&encoded, encoding, 1024)
                    .unwrap();
            assert_eq!(decoded, original, "encoding={encoding}");
        }
    }

    #[test]
    fn unsupported_content_encoding_is_rejected() {
        let error =
            encode_content_encoded_body(br#"{\"kind\":\"unsupported\"}"#, "x-private").unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(error.to_string(), "unsupported content-encoding: x-private");
    }

    #[test]
    fn plaintext_body_removes_stale_wire_headers() {
        let mut headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("CONTENT-ENCODING".to_string(), "gzip".to_string()),
            ("content-length".to_string(), "999".to_string()),
            ("X-Test".to_string(), "keep".to_string()),
        ];

        normalize_plaintext_body_headers(&mut headers);

        assert_eq!(
            headers,
            vec![
                ("Content-Type".to_string(), "application/json".to_string()),
                ("X-Test".to_string(), "keep".to_string()),
            ]
        );
    }
}
