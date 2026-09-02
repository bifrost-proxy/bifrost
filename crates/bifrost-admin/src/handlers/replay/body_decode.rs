pub(super) fn decode_replay_body(
    headers: &[(String, String)],
    body: &[u8],
) -> Result<Option<String>, String> {
    if body.is_empty() {
        return Ok(None);
    }

    let headers = Some(headers.to_vec());
    let encoding = super::super::network_body::content_encoding_value(&headers);
    let decoded = match encoding.as_deref() {
        None | Some("") => body.to_vec(),
        Some(encoding) if !super::super::network_body::content_encoding_is_supported(encoding) => {
            body.to_vec()
        }
        Some(encoding) => super::super::network_body::decompress(body, encoding)
            .map_err(|error| format!("failed to decode {encoding} replay response: {error}"))?,
    };

    Ok(Some(String::from_utf8_lossy(&decoded).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::decode_replay_body;

    #[test]
    fn decode_gzip_response_body() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let raw = br#"{"ok":true,"msg":"hello"}"#;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(raw).unwrap();
        let gz = enc.finish().unwrap();

        let headers = vec![("content-encoding".to_string(), "gzip".to_string())];
        let decoded = decode_replay_body(&headers, &gz).unwrap().unwrap();
        assert_eq!(decoded, String::from_utf8_lossy(raw));
    }

    #[test]
    fn compressed_binary_response_does_not_fail_replay() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let raw = b"\xff\x00\xfe";
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(raw).unwrap();
        let gz = enc.finish().unwrap();

        let headers = vec![("content-encoding".to_string(), "gzip".to_string())];
        let decoded = decode_replay_body(&headers, &gz).unwrap().unwrap();
        assert_eq!(decoded, String::from_utf8_lossy(raw));
    }

    #[test]
    fn unencoded_binary_response_does_not_fail_replay() {
        let body = b"\xff\x00\xfe";
        let decoded = decode_replay_body(&[], body).unwrap().unwrap();
        assert_eq!(decoded, String::from_utf8_lossy(body));
    }

    #[test]
    fn identity_binary_response_does_not_fail_replay() {
        let headers = vec![("content-encoding".to_string(), "identity".to_string())];
        let body = b"\xff\x00\xfe";
        let decoded = decode_replay_body(&headers, body).unwrap().unwrap();
        assert_eq!(decoded, String::from_utf8_lossy(body));
    }

    #[test]
    fn standard_content_codings_roundtrip() {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;

        let raw = b"standard replay payload";
        let mut deflate = ZlibEncoder::new(Vec::new(), Compression::default());
        deflate.write_all(raw).unwrap();
        let deflate = deflate.finish().unwrap();
        assert_eq!(
            decode_replay_body(
                &[("content-encoding".to_string(), "deflate".to_string())],
                &deflate,
            )
            .unwrap()
            .as_deref(),
            Some("standard replay payload")
        );

        let mut br = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut br, 4096, 5, 22);
            encoder.write_all(raw).unwrap();
        }
        assert_eq!(
            decode_replay_body(&[("content-encoding".to_string(), "br".to_string())], &br,)
                .unwrap()
                .as_deref(),
            Some("standard replay payload")
        );

        let zstd = zstd::stream::encode_all(raw.as_slice(), 0).unwrap();
        assert_eq!(
            decode_replay_body(
                &[("content-encoding".to_string(), "zstd".to_string())],
                &zstd,
            )
            .unwrap()
            .as_deref(),
            Some("standard replay payload")
        );
    }

    #[test]
    fn complete_encoding_chain_and_gzip_members_are_decoded() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        fn gzip(data: &[u8]) -> Vec<u8> {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(data).unwrap();
            encoder.finish().unwrap()
        }

        let mut members = gzip(b"first ");
        members.extend_from_slice(&gzip(b"second"));
        assert_eq!(
            decode_replay_body(
                &[("content-encoding".to_string(), "gzip".to_string())],
                &members,
            )
            .unwrap()
            .as_deref(),
            Some("first second")
        );

        let raw = b"stacked";
        let gz = gzip(raw);
        let mut br = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut br, 4096, 5, 22);
            encoder.write_all(&gz).unwrap();
        }
        assert_eq!(
            decode_replay_body(
                &[("content-encoding".to_string(), "gzip, br".to_string())],
                &br,
            )
            .unwrap()
            .as_deref(),
            Some("stacked")
        );
    }

    #[test]
    fn malformed_standard_codings_are_rejected() {
        for encoding in ["gzip", "deflate", "br", "zstd"] {
            assert!(decode_replay_body(
                &[("Content-Encoding".to_string(), encoding.to_string())],
                b"not-valid",
            )
            .is_err());
        }
    }

    #[test]
    fn unknown_encoding_is_left_for_custom_decoder() {
        let body = b"custom bytes";
        assert_eq!(
            decode_replay_body(
                &[("content-encoding".to_string(), "x-private".to_string())],
                body,
            )
            .unwrap()
            .as_deref(),
            Some("custom bytes")
        );
        assert_eq!(
            decode_replay_body(&[("content-encoding".to_string(), "gzip".to_string())], b"",),
            Ok(None)
        );
    }
}
