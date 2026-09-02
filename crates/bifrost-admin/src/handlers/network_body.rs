use base64::{engine::general_purpose::STANDARD, Engine};
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use std::io::Read;

const MAX_DECOMPRESSED_BODY_BYTES: usize = 10 * 1024 * 1024;

pub(super) struct ExportBody {
    pub text: Option<String>,
    pub base64: Option<String>,
}

pub(super) struct PreviewBody {
    pub text: Option<String>,
    pub warning: Option<String>,
}

pub(super) fn header_value(headers: &Option<Vec<(String, String)>>, name: &str) -> Option<String> {
    headers
        .as_ref()?
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

pub(super) fn export_body(bytes: Vec<u8>, headers: &Option<Vec<(String, String)>>) -> ExportBody {
    if let Ok(text) = std::str::from_utf8(&bytes) {
        return ExportBody {
            text: Some(text.to_string()),
            base64: None,
        };
    }

    let content_encoding = header_value(headers, "content-encoding");
    let decoded = content_encoding
        .as_deref()
        .and_then(|encoding| decompress(&bytes, encoding).ok());

    if let Some(decoded) = decoded {
        return ExportBody {
            text: String::from_utf8(decoded).ok(),
            base64: Some(STANDARD.encode(bytes)),
        };
    }

    ExportBody {
        text: None,
        base64: Some(STANDARD.encode(bytes)),
    }
}

pub(super) fn preview_body(
    text: Option<&str>,
    body_base64: Option<&str>,
    headers: &Option<Vec<(String, String)>>,
    label: &str,
) -> PreviewBody {
    if let Some(text) = text {
        if looks_like_legacy_lossy_body(text, headers) {
            return PreviewBody {
                text: None,
                warning: Some(format!(
                    "The {label} body was corrupted by an older Bifrost version during export. Re-export the request with an updated Bifrost to view its plaintext body."
                )),
            };
        }
        return PreviewBody {
            text: Some(text.to_string()),
            warning: None,
        };
    }

    let Some(body_base64) = body_base64 else {
        return PreviewBody {
            text: None,
            warning: None,
        };
    };
    let Ok(bytes) = STANDARD.decode(body_base64) else {
        return PreviewBody {
            text: None,
            warning: Some(format!("The {label} body contains invalid base64 data.")),
        };
    };
    let content_encoding = header_value(headers, "content-encoding");
    let decoded = content_encoding
        .as_deref()
        .and_then(|encoding| decompress(&bytes, encoding).ok())
        .unwrap_or(bytes);

    match String::from_utf8(decoded) {
        Ok(text) => PreviewBody {
            text: Some(text),
            warning: None,
        },
        Err(_) => PreviewBody {
            text: None,
            warning: Some(format!(
                "The {label} body is binary and cannot be shown as text; its original bytes remain in the package."
            )),
        },
    }
}

fn looks_like_legacy_lossy_body(text: &str, headers: &Option<Vec<(String, String)>>) -> bool {
    if header_value(headers, "content-encoding").is_none() {
        return false;
    }
    let replacement_count = text
        .chars()
        .filter(|character| *character == '\u{fffd}')
        .count();
    replacement_count >= 2
        || (replacement_count == 1
            && text
                .chars()
                .any(|character| character.is_control() && !character.is_whitespace()))
}

fn decompress(data: &[u8], content_encoding: &str) -> std::io::Result<Vec<u8>> {
    let encoding = content_encoding
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    match encoding.as_str() {
        "" | "identity" => Ok(data.to_vec()),
        "gzip" => read_limited(GzDecoder::new(data)),
        "deflate" => read_limited(ZlibDecoder::new(data))
            .or_else(|_| read_limited(DeflateDecoder::new(data))),
        "br" => read_limited(brotli::Decompressor::new(data, 4096)),
        "zstd" => read_limited(zstd::stream::read::Decoder::new(data)?),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsupported content-encoding: {content_encoding}"),
        )),
    }
}

fn read_limited(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut limited = reader
        .by_ref()
        .take((MAX_DECOMPRESSED_BODY_BYTES as u64) + 1);
    let mut output = Vec::new();
    limited.read_to_end(&mut output)?;
    if output.len() > MAX_DECOMPRESSED_BODY_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "decompressed body exceeds the preview limit",
        ));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn binary_body_uses_lossless_base64() {
        let exported = export_body(vec![0xff, 0x00, 0xfe], &None);

        assert!(exported.text.is_none());
        assert_eq!(
            STANDARD.decode(exported.base64.unwrap()).unwrap(),
            vec![0xff, 0x00, 0xfe]
        );
    }

    #[test]
    fn text_and_preview_edge_cases_are_handled() {
        let exported = export_body(b"plain text".to_vec(), &None);
        assert_eq!(exported.text.as_deref(), Some("plain text"));
        assert!(exported.base64.is_none());

        let invalid_base64 = preview_body(None, Some("%%%"), &None, "request");
        assert!(invalid_base64.text.is_none());
        assert!(invalid_base64.warning.unwrap().contains("invalid base64"));

        let binary = preview_body(None, Some("/wD+"), &None, "response");
        assert!(binary.text.is_none());
        assert!(binary.warning.unwrap().contains("binary"));

        let compressed_headers = Some(vec![("Content-Encoding".to_string(), "gzip".to_string())]);
        let legacy = preview_body(Some("\u{fffd}\u{1}"), None, &compressed_headers, "request");
        assert!(legacy.text.is_none());
        assert!(legacy.warning.unwrap().contains("older Bifrost"));
    }

    #[test]
    fn supported_content_encodings_are_decompressed() {
        let plaintext = b"encoded payload";

        let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        zlib.write_all(plaintext).unwrap();
        assert_eq!(
            decompress(&zlib.finish().unwrap(), "deflate").unwrap(),
            plaintext
        );

        let mut raw_deflate =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        raw_deflate.write_all(plaintext).unwrap();
        assert_eq!(
            decompress(&raw_deflate.finish().unwrap(), "deflate").unwrap(),
            plaintext
        );

        let mut brotli = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut brotli, 4096, 5, 22);
            encoder.write_all(plaintext).unwrap();
        }
        assert_eq!(decompress(&brotli, "br").unwrap(), plaintext);

        let zstd = zstd::stream::encode_all(plaintext.as_slice(), 1).unwrap();
        assert_eq!(decompress(&zstd, "zstd").unwrap(), plaintext);
        assert_eq!(decompress(plaintext, "identity").unwrap(), plaintext);
        assert!(decompress(plaintext, "compress").is_err());
    }

    #[test]
    fn decompression_limit_falls_back_to_lossless_base64() {
        let oversized = vec![b'a'; MAX_DECOMPRESSED_BODY_BYTES + 1];
        let mut compressed = Vec::new();
        {
            use flate2::{write::GzEncoder, Compression};
            let mut encoder = GzEncoder::new(&mut compressed, Compression::default());
            encoder.write_all(&oversized).unwrap();
            encoder.finish().unwrap();
        }
        let headers = Some(vec![("content-encoding".to_string(), "gzip".to_string())]);

        let exported = export_body(compressed.clone(), &headers);

        assert!(exported.text.is_none());
        assert_eq!(
            STANDARD.decode(exported.base64.unwrap()).unwrap(),
            compressed
        );
    }
}
