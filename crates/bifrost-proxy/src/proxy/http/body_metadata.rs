use hyper::header::HeaderValue;
use hyper::http::request::Parts as RequestParts;
use hyper::http::response::Parts as ResponseParts;
use hyper::StatusCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::proxy::http) enum BodyMode {
    Known(usize),
    Stream,
    StreamWithLength(usize),
    StreamWithTrailers,
}

pub(in crate::proxy::http) fn streaming_res_body_mode(
    content_length: Option<usize>,
    has_trailers: bool,
) -> BodyMode {
    if has_trailers {
        BodyMode::StreamWithTrailers
    } else if let Some(len) = content_length {
        BodyMode::StreamWithLength(len)
    } else {
        BodyMode::Stream
    }
}

pub(in crate::proxy::http) fn buffered_res_body_mode(
    content_length: usize,
    has_trailers: bool,
) -> BodyMode {
    if has_trailers {
        BodyMode::StreamWithTrailers
    } else {
        BodyMode::Known(content_length)
    }
}

pub(in crate::proxy::http) fn is_no_body_response(status: StatusCode, method: &str) -> bool {
    status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
        || method.eq_ignore_ascii_case("HEAD")
}

pub(in crate::proxy::http) fn response_content_encoding(parts: &ResponseParts) -> Option<String> {
    content_encoding_header_value(&parts.headers)
}

pub(in crate::proxy::http) fn header_content_encoding(
    headers: &hyper::HeaderMap,
) -> Option<String> {
    content_encoding_header_value(headers)
}

fn content_encoding_header_value(headers: &hyper::HeaderMap) -> Option<String> {
    let values = headers
        .get_all(hyper::header::CONTENT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join(", "))
}

pub(in crate::proxy::http) fn content_encoding_is_identity(content_encoding: &str) -> bool {
    let mut tokens = content_encoding
        .split(',')
        .map(str::trim)
        .filter(|encoding| !encoding.is_empty());
    let Some(first) = tokens.next() else {
        return false;
    };
    first.eq_ignore_ascii_case("identity")
        && tokens.all(|encoding| encoding.eq_ignore_ascii_case("identity"))
}

pub(in crate::proxy::http) fn set_content_encoding_header(
    headers: &mut hyper::HeaderMap,
    content_encoding: Option<&str>,
) {
    headers.remove(hyper::header::CONTENT_ENCODING);
    if let Some(content_encoding) = content_encoding {
        if let Ok(value) = HeaderValue::from_str(content_encoding) {
            headers.insert(hyper::header::CONTENT_ENCODING, value);
        }
    }
}

pub(in crate::proxy::http) fn normalize_req_headers(
    parts: &mut RequestParts,
    mode: BodyMode,
    had_content_length: bool,
) {
    match mode {
        BodyMode::Known(len) => {
            parts.headers.remove(hyper::header::TRANSFER_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
            if len > 0 || had_content_length {
                parts.headers.insert(
                    hyper::header::CONTENT_LENGTH,
                    HeaderValue::from_str(&len.to_string()).unwrap(),
                );
            }
        }
        BodyMode::Stream | BodyMode::StreamWithTrailers => {
            parts.headers.remove(hyper::header::TRANSFER_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
        }
        BodyMode::StreamWithLength(len) => {
            parts.headers.remove(hyper::header::TRANSFER_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
            parts.headers.insert(
                hyper::header::CONTENT_LENGTH,
                HeaderValue::from_str(&len.to_string()).unwrap(),
            );
        }
    }
}

pub(in crate::proxy::http) fn normalize_res_headers(
    parts: &mut ResponseParts,
    mode: BodyMode,
    method: &str,
) {
    if is_no_body_response(parts.status, method) {
        parts.headers.remove(hyper::header::TRANSFER_ENCODING);
        parts.headers.remove(hyper::header::CONTENT_LENGTH);
        return;
    }
    match mode {
        BodyMode::Known(len) => {
            parts.headers.remove(hyper::header::TRANSFER_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
            parts.headers.insert(
                hyper::header::CONTENT_LENGTH,
                HeaderValue::from_str(&len.to_string()).unwrap(),
            );
        }
        BodyMode::Stream | BodyMode::StreamWithTrailers => {
            parts.headers.remove(hyper::header::TRANSFER_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
        }
        BodyMode::StreamWithLength(len) => {
            parts.headers.remove(hyper::header::TRANSFER_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
            parts.headers.insert(
                hyper::header::CONTENT_LENGTH,
                HeaderValue::from_str(&len.to_string()).unwrap(),
            );
        }
    }
}

#[cfg(test)]
mod content_encoding_tests {
    use super::*;

    #[test]
    fn repeated_content_encoding_fields_are_combined_in_wire_order() {
        let mut headers = hyper::HeaderMap::new();
        headers.append(
            hyper::header::CONTENT_ENCODING,
            HeaderValue::from_static("gzip"),
        );
        headers.append(
            hyper::header::CONTENT_ENCODING,
            HeaderValue::from_static("br"),
        );

        assert_eq!(
            header_content_encoding(&headers).as_deref(),
            Some("gzip, br")
        );

        let response = hyper::Response::builder()
            .header(hyper::header::CONTENT_ENCODING, "gzip")
            .header(hyper::header::CONTENT_ENCODING, "br")
            .body(())
            .unwrap();
        let (parts, _) = response.into_parts();
        assert_eq!(
            response_content_encoding(&parts).as_deref(),
            Some("gzip, br")
        );
    }

    #[test]
    fn only_identity_tokens_are_classified_as_unencoded() {
        assert!(content_encoding_is_identity("identity"));
        assert!(content_encoding_is_identity(" identity, IDENTITY "));
        assert!(!content_encoding_is_identity(""));
        assert!(!content_encoding_is_identity("identity, gzip"));
        assert!(!content_encoding_is_identity("x-company-codec"));
    }
}
