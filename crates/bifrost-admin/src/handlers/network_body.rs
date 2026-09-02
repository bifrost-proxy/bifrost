use base64::{engine::general_purpose::STANDARD, Engine};
use flate2::read::{DeflateDecoder, MultiGzDecoder, ZlibDecoder};
use std::io::Read;

pub(crate) const DEFAULT_MAX_DECOMPRESSED_BODY_BYTES: usize = 10 * 1024 * 1024;

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

pub(super) fn body_size(text: Option<&str>, body_base64: Option<&str>) -> usize {
    text.map(str::len)
        .or_else(|| {
            body_base64.and_then(|encoded| STANDARD.decode(encoded).ok().map(|bytes| bytes.len()))
        })
        .unwrap_or(0)
}

pub(super) fn content_encoding_value(headers: &Option<Vec<(String, String)>>) -> Option<String> {
    let values = headers
        .as_ref()?
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case("content-encoding"))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join(", "))
}

#[cfg(test)]
pub(super) fn decode_body_for_display(
    bytes: Vec<u8>,
    headers: &Option<Vec<(String, String)>>,
) -> Vec<u8> {
    decode_content_encoded_body(bytes, content_encoding_value(headers).as_deref())
}

#[cfg(test)]
pub(crate) fn decode_content_encoded_body(
    bytes: Vec<u8>,
    content_encoding: Option<&str>,
) -> Vec<u8> {
    decode_content_encoded_body_with_limit(
        bytes,
        content_encoding,
        DEFAULT_MAX_DECOMPRESSED_BODY_BYTES,
    )
}

pub(crate) fn decode_content_encoded_body_with_limit(
    bytes: Vec<u8>,
    content_encoding: Option<&str>,
    max_output_bytes: usize,
) -> Vec<u8> {
    content_encoding
        .and_then(|encoding| decompress_with_limit(&bytes, encoding, max_output_bytes).ok())
        .unwrap_or(bytes)
}

#[cfg(test)]
pub(super) fn export_body(bytes: Vec<u8>, headers: &Option<Vec<(String, String)>>) -> ExportBody {
    export_content_encoded_body(bytes, content_encoding_value(headers).as_deref())
}

#[cfg(test)]
pub(super) fn export_content_encoded_body(
    bytes: Vec<u8>,
    content_encoding: Option<&str>,
) -> ExportBody {
    let mut budget = DEFAULT_MAX_DECOMPRESSED_BODY_BYTES;
    export_content_encoded_body_with_budget(bytes, content_encoding, &mut budget)
}

pub(super) fn export_content_encoded_body_with_budget(
    bytes: Vec<u8>,
    content_encoding: Option<&str>,
    remaining_decompress_bytes: &mut usize,
) -> ExportBody {
    let standard_compressed = content_encoding.is_some_and(|encoding| {
        content_encoding_is_supported(encoding)
            && encoding
                .split(',')
                .map(str::trim)
                .any(|coding| !coding.eq_ignore_ascii_case("identity"))
    });
    let decoded = if standard_compressed && *remaining_decompress_bytes > 0 {
        match decompress_with_limit_metered(
            &bytes,
            content_encoding.unwrap(),
            *remaining_decompress_bytes,
        ) {
            Ok((decoded, consumed)) => {
                *remaining_decompress_bytes = remaining_decompress_bytes.saturating_sub(consumed);
                Some(decoded)
            }
            Err(_) => {
                *remaining_decompress_bytes = 0;
                None
            }
        }
    } else if standard_compressed {
        None
    } else {
        content_encoding.and_then(|encoding| decompress(&bytes, encoding).ok())
    };

    if let Some(decoded) = decoded {
        let was_decoded = decoded != bytes;
        let text = String::from_utf8(decoded).ok();
        return ExportBody {
            base64: (was_decoded || text.is_none()).then(|| STANDARD.encode(bytes)),
            text,
        };
    }

    if let Ok(text) = std::str::from_utf8(&bytes) {
        return ExportBody {
            text: Some(text.to_string()),
            base64: None,
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
        if let Some(warning) = legacy_lossy_body_warning(text, body_base64, headers, label) {
            return PreviewBody {
                text: None,
                warning: Some(warning),
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
    let content_encoding = content_encoding_value(headers);
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

pub(super) fn legacy_lossy_body_warning(
    text: &str,
    body_base64: Option<&str>,
    headers: &Option<Vec<(String, String)>>,
    label: &str,
) -> Option<String> {
    (body_base64.is_none() && looks_like_legacy_lossy_body(text, headers)).then(|| {
        format!(
            "The {label} body was corrupted by an older Bifrost version during export. Re-export the request with an updated Bifrost to view its plaintext body."
        )
    })
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

pub(crate) fn content_encoding_is_supported(content_encoding: &str) -> bool {
    content_encoding
        .split(',')
        .map(str::trim)
        .filter(|encoding| !encoding.is_empty())
        .all(|encoding| {
            matches!(
                encoding.to_ascii_lowercase().as_str(),
                "identity" | "gzip" | "x-gzip" | "deflate" | "br" | "zstd"
            )
        })
}

pub(super) fn decompress(data: &[u8], content_encoding: &str) -> std::io::Result<Vec<u8>> {
    decompress_with_limit(data, content_encoding, DEFAULT_MAX_DECOMPRESSED_BODY_BYTES)
}

pub(crate) fn decompress_with_limit(
    data: &[u8],
    content_encoding: &str,
    max_output_bytes: usize,
) -> std::io::Result<Vec<u8>> {
    decompress_with_limit_metered(data, content_encoding, max_output_bytes)
        .map(|(decoded, _)| decoded)
}

pub(crate) fn decompress_with_limit_metered(
    data: &[u8],
    content_encoding: &str,
    max_output_bytes: usize,
) -> std::io::Result<(Vec<u8>, usize)> {
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
            "gzip" | "x-gzip" => read_limited(
                MultiGzDecoder::new(decoded.as_slice()),
                remaining_output_bytes,
            )?,
            "deflate" => read_limited(ZlibDecoder::new(decoded.as_slice()), remaining_output_bytes)
                .or_else(|_| {
                    read_limited(
                        DeflateDecoder::new(decoded.as_slice()),
                        remaining_output_bytes,
                    )
                })?,
            "br" => read_limited(
                brotli::Decompressor::new(decoded.as_slice(), 4096),
                remaining_output_bytes,
            )?,
            "zstd" => read_limited(
                zstd::stream::read::Decoder::new(decoded.as_slice())?,
                remaining_output_bytes,
            )?,
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
    Ok((decoded, max_output_bytes - remaining_output_bytes))
}

pub(crate) struct DecompressedPrefix {
    pub bytes: Vec<u8>,
    pub consumed: usize,
    pub truncated: bool,
}

/// Decode at most `max_output_bytes` and preserve the decoded prefix when the
/// final representation is larger than the caller's preview allowance.
pub(crate) fn decompress_prefix_with_limit_metered(
    data: &[u8],
    content_encoding: &str,
    max_output_bytes: usize,
) -> std::io::Result<DecompressedPrefix> {
    let encodings = content_encoding
        .split(',')
        .map(str::trim)
        .filter(|encoding| !encoding.is_empty())
        .collect::<Vec<_>>();
    let mut decoded = data.to_vec();
    let mut remaining_output_bytes = max_output_bytes;
    let mut final_truncated = false;

    for (index, encoding) in encodings.iter().rev().enumerate() {
        if encoding.eq_ignore_ascii_case("identity") {
            continue;
        }
        let (next, truncated) = match encoding.to_ascii_lowercase().as_str() {
            "gzip" | "x-gzip" => read_limited_prefix(
                MultiGzDecoder::new(decoded.as_slice()),
                remaining_output_bytes,
            )?,
            "deflate" => {
                read_limited_prefix(ZlibDecoder::new(decoded.as_slice()), remaining_output_bytes)
                    .or_else(|_| {
                        read_limited_prefix(
                            DeflateDecoder::new(decoded.as_slice()),
                            remaining_output_bytes,
                        )
                    })?
            }
            "br" => read_limited_prefix(
                brotli::Decompressor::new(decoded.as_slice(), 4096),
                remaining_output_bytes,
            )?,
            "zstd" => read_limited_prefix(
                zstd::stream::read::Decoder::new(decoded.as_slice())?,
                remaining_output_bytes,
            )?,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unsupported content-encoding: {encoding}"),
                ));
            }
        };
        remaining_output_bytes = remaining_output_bytes.saturating_sub(next.len());
        decoded = next;

        if truncated {
            let has_later_decoder = encodings
                .iter()
                .rev()
                .skip(index + 1)
                .any(|later| !later.eq_ignore_ascii_case("identity"));
            if has_later_decoder {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "intermediate decompressed body exceeds the preview limit",
                ));
            }
            final_truncated = true;
            break;
        }
    }

    Ok(DecompressedPrefix {
        bytes: decoded,
        consumed: max_output_bytes - remaining_output_bytes,
        truncated: final_truncated,
    })
}

/// Decode the currently available prefix of a streaming HTTP body.
///
/// Streaming compressors commonly omit their final trailer until the
/// connection closes. The regular decoder intentionally treats that as an
/// error; SSE tailing needs the plaintext produced before that EOF instead.
#[cfg(test)]
pub(crate) fn decompress_partial_with_limit(
    data: &[u8],
    content_encoding: &str,
    max_output_bytes: usize,
) -> std::io::Result<Vec<u8>> {
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
            "gzip" | "x-gzip" => read_limited_partial(
                MultiGzDecoder::new(decoded.as_slice()),
                remaining_output_bytes,
            )?,
            "deflate" => {
                match read_limited_partial(
                    ZlibDecoder::new(decoded.as_slice()),
                    remaining_output_bytes,
                ) {
                    Ok(bytes) if !bytes.is_empty() => bytes,
                    _ => read_limited_partial(
                        DeflateDecoder::new(decoded.as_slice()),
                        remaining_output_bytes,
                    )?,
                }
            }
            "br" => read_limited_partial(
                brotli::Decompressor::new(decoded.as_slice(), 4096),
                remaining_output_bytes,
            )?,
            "zstd" => read_limited_partial(
                zstd::stream::read::Decoder::new(decoded.as_slice())?,
                remaining_output_bytes,
            )?,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unsupported content-encoding: {encoding}"),
                ));
            }
        };
        if !encoding.eq_ignore_ascii_case("identity") {
            remaining_output_bytes =
                remaining_output_bytes
                    .checked_sub(next.len())
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "decompressed body exceeds the preview limit",
                        )
                    })?;
        }
        decoded = next;
    }
    Ok(decoded)
}

fn read_limited(mut reader: impl Read, max_output_bytes: usize) -> std::io::Result<Vec<u8>> {
    let mut limited = reader
        .by_ref()
        .take((max_output_bytes as u64).saturating_add(1));
    let mut output = Vec::new();
    limited.read_to_end(&mut output)?;
    if output.len() > max_output_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "decompressed body exceeds the preview limit",
        ));
    }
    Ok(output)
}

fn read_limited_prefix(
    mut reader: impl Read,
    max_output_bytes: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut limited = reader
        .by_ref()
        .take((max_output_bytes as u64).saturating_add(1));
    let mut output = Vec::new();
    limited.read_to_end(&mut output)?;
    let truncated = output.len() > max_output_bytes;
    output.truncate(max_output_bytes);
    Ok((output, truncated))
}

#[cfg(test)]
fn read_limited_partial(
    mut reader: impl Read,
    max_output_bytes: usize,
) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                if output.len().saturating_add(read) > max_output_bytes {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "decompressed body exceeds the preview limit",
                    ));
                }
                output.extend_from_slice(&chunk[..read]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error),
        }
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

        let identity_headers = Some(vec![(
            "Content-Encoding".to_string(),
            "identity".to_string(),
        )]);
        let identity = export_body(vec![0xff, 0x00, 0xfe], &identity_headers);
        assert!(identity.text.is_none());
        assert_eq!(
            STANDARD.decode(identity.base64.unwrap()).unwrap(),
            vec![0xff, 0x00, 0xfe]
        );
    }

    #[test]
    fn base64_body_size_is_lossless_and_invalid_base64_is_empty() {
        assert_eq!(body_size(None, Some("AAECAw==")), 4);
        assert_eq!(body_size(Some("plain"), Some("AAECAw==")), 5);
        assert_eq!(body_size(None, Some("not base64")), 0);
    }

    #[test]
    fn lossless_base64_prevents_legacy_replacement_character_false_positive() {
        let headers = Some(vec![("Content-Encoding".to_string(), "gzip".to_string())]);
        let preview = preview_body(
            Some("valid \u{fffd}\u{fffd} text"),
            Some("AAECAw=="),
            &headers,
            "request",
        );

        assert_eq!(preview.text.as_deref(), Some("valid \u{fffd}\u{fffd} text"));
        assert!(preview.warning.is_none());
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

        assert!(content_encoding_is_supported("gzip, br, identity"));
        assert!(!content_encoding_is_supported("gzip, x-company-codec"));

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
    fn decompressed_prefix_supports_every_standard_coding_and_rejects_invalid_chains() {
        let plaintext = b"decoded preview longer than its limit";

        let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        zlib.write_all(plaintext).unwrap();
        let zlib = zlib.finish().unwrap();
        assert_eq!(
            decompress_prefix_with_limit_metered(&zlib, "identity, deflate", 7)
                .unwrap()
                .bytes,
            &plaintext[..7]
        );

        let mut raw =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        raw.write_all(plaintext).unwrap();
        assert_eq!(
            decompress_prefix_with_limit_metered(&raw.finish().unwrap(), "deflate", 7)
                .unwrap()
                .bytes,
            &plaintext[..7]
        );

        let mut br = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut br, 4096, 5, 22);
            encoder.write_all(plaintext).unwrap();
        }
        assert_eq!(
            decompress_prefix_with_limit_metered(&br, "br", 7)
                .unwrap()
                .bytes,
            &plaintext[..7]
        );

        let zstd = zstd::stream::encode_all(plaintext.as_slice(), 1).unwrap();
        assert_eq!(
            decompress_prefix_with_limit_metered(&zstd, "zstd", 7)
                .unwrap()
                .bytes,
            &plaintext[..7]
        );
        assert_eq!(
            decompress_prefix_with_limit_metered(plaintext, "identity", 128)
                .unwrap()
                .bytes,
            plaintext
        );
        assert!(decompress_prefix_with_limit_metered(plaintext, "x-company", 7).is_err());

        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(plaintext).unwrap();
        let gzip = gzip.finish().unwrap();
        let mut outer = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        outer.write_all(&gzip).unwrap();
        assert!(
            decompress_prefix_with_limit_metered(&outer.finish().unwrap(), "gzip, deflate", 7)
                .is_err()
        );
    }

    #[test]
    fn export_budget_handles_malformed_and_binary_decoded_bodies_losslessly() {
        let malformed = b"not actually gzip".to_vec();
        let mut malformed_budget = 64;
        let exported = export_content_encoded_body_with_budget(
            malformed.clone(),
            Some("gzip"),
            &mut malformed_budget,
        );
        assert_eq!(malformed_budget, 0);
        assert_eq!(exported.text.as_deref(), Some("not actually gzip"));

        let binary = vec![0xff, 0, 0xfe];
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&binary).unwrap();
        let wire = gzip.finish().unwrap();
        let mut binary_budget = 64;
        let exported =
            export_content_encoded_body_with_budget(wire.clone(), Some("gzip"), &mut binary_budget);
        assert!(exported.text.is_none());
        assert_eq!(STANDARD.decode(exported.base64.unwrap()).unwrap(), wire);
    }

    #[test]
    fn partial_gzip_stream_exposes_plaintext_before_the_trailer_arrives() {
        let plaintext = b"data: still-open\n\n";
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(plaintext).unwrap();
        encoder.flush().unwrap();
        let growing_wire = encoder.get_ref().clone();

        assert!(decompress(&growing_wire, "gzip").is_err());
        assert_eq!(
            decompress_partial_with_limit(&growing_wire, "gzip", 1024).unwrap(),
            plaintext
        );
    }

    #[test]
    fn partial_decoding_supports_standard_stream_codings_and_bounds_output() {
        let plaintext = b"data: standard coding\n\n";

        let mut zlib = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        zlib.write_all(plaintext).unwrap();
        assert_eq!(
            decompress_partial_with_limit(&zlib.finish().unwrap(), "deflate", 1024).unwrap(),
            plaintext
        );

        let mut raw_deflate =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        raw_deflate.write_all(plaintext).unwrap();
        assert_eq!(
            decompress_partial_with_limit(&raw_deflate.finish().unwrap(), "deflate", 1024).unwrap(),
            plaintext
        );

        let mut brotli = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut brotli, 4096, 5, 22);
            encoder.write_all(plaintext).unwrap();
        }
        assert_eq!(
            decompress_partial_with_limit(&brotli, "br", 1024).unwrap(),
            plaintext
        );

        let zstd = zstd::stream::encode_all(plaintext.as_slice(), 1).unwrap();
        assert_eq!(
            decompress_partial_with_limit(&zstd, "zstd", 1024).unwrap(),
            plaintext
        );
        assert_eq!(
            decompress_partial_with_limit(plaintext, "identity", 1024).unwrap(),
            plaintext
        );
        assert!(decompress_partial_with_limit(plaintext, "x-company-codec", 1024).is_err());

        let oversized = vec![b'a'; 16 * 1024];
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&oversized).unwrap();
        let error = decompress_partial_with_limit(&gzip.finish().unwrap(), "gzip", 8)
            .expect_err("partial decoding must enforce its output limit");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn decompression_limit_accepts_usize_max_without_overflow() {
        assert_eq!(
            decompress_with_limit(b"plain body", "identity", usize::MAX).unwrap(),
            b"plain body"
        );
    }

    #[test]
    fn content_encoding_layers_share_the_preview_budget() {
        let original = vec![b'a'; DEFAULT_MAX_DECOMPRESSED_BODY_BYTES];
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&original).unwrap();
        let gzip = gzip.finish().unwrap();
        let mut deflate =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        deflate.write_all(&gzip).unwrap();
        let wire = deflate.finish().unwrap();

        let error = decompress(&wire, "gzip, deflate")
            .expect_err("successful layers must share one preview budget");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("preview limit"));
    }

    #[test]
    fn multiple_content_codings_decode_in_reverse_and_custom_codings_stay_raw() {
        let plaintext = b"multiply encoded package body";
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(plaintext).unwrap();
        let gzip = gzip.finish().unwrap();

        let mut brotli = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut brotli, 4096, 5, 22);
            encoder.write_all(&gzip).unwrap();
        }
        assert_eq!(
            decompress(&brotli, "gzip, br").expect("decode chain"),
            plaintext
        );

        let custom_headers = Some(vec![(
            "content-encoding".to_string(),
            "gzip, x-company-codec".to_string(),
        )]);
        let exported = export_body(brotli.clone(), &custom_headers);
        assert!(exported.text.is_none());
        assert_eq!(STANDARD.decode(exported.base64.unwrap()).unwrap(), brotli);
    }

    #[test]
    fn repeated_content_encoding_headers_and_x_gzip_are_decoded() {
        let plaintext = b"repeated header body";
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(plaintext).unwrap();
        let gzip = gzip.finish().unwrap();

        let mut brotli = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut brotli, 4096, 5, 22);
            encoder.write_all(&gzip).unwrap();
        }
        let repeated_headers = Some(vec![
            ("Content-Encoding".to_string(), "gzip".to_string()),
            ("content-encoding".to_string(), "br".to_string()),
        ]);
        assert_eq!(
            decode_body_for_display(brotli.clone(), &repeated_headers),
            plaintext
        );
        let repeated = export_body(brotli.clone(), &repeated_headers);
        assert_eq!(repeated.text.as_deref(), Some("repeated header body"));

        let custom_headers = Some(vec![(
            "Content-Encoding".to_string(),
            "x-company-codec".to_string(),
        )]);
        assert_eq!(
            decode_body_for_display(brotli.clone(), &custom_headers),
            brotli
        );

        let alias_headers = Some(vec![("Content-Encoding".to_string(), "x-gzip".to_string())]);
        let alias = export_body(gzip, &alias_headers);
        assert_eq!(alias.text.as_deref(), Some("repeated header body"));
    }

    #[test]
    fn concatenated_gzip_members_are_all_decoded() {
        fn gzip_member(data: &[u8]) -> Vec<u8> {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(data).unwrap();
            encoder.finish().unwrap()
        }

        let mut wire = gzip_member(b"first member ");
        wire.extend_from_slice(&gzip_member(b"second member"));

        assert_eq!(
            decompress(&wire, "gzip").expect("decode all gzip members"),
            b"first member second member"
        );
    }

    #[test]
    fn decompression_limit_falls_back_to_lossless_base64() {
        let oversized = vec![b'a'; DEFAULT_MAX_DECOMPRESSED_BODY_BYTES + 1];
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
