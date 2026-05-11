mod badge;
mod body;
mod compress;
pub mod decompress;
mod request;
mod response;

pub use badge::{maybe_inject_bifrost_badge_html, BIFROST_BADGE_ELEMENT_ID};
pub use body::{
    apply_body_rules, apply_body_rules_preserving_encoding, apply_content_injection,
    apply_content_injection_preserving_encoding, ContentInjectionEncoding, ContentInjectionResult,
    Phase,
};
pub use compress::compress_body;
pub use decompress::{
    decompress_body, decompress_body_with_limit, get_content_encoding,
    try_decompress_body_with_limit,
};
pub use request::{
    apply_req_rules, collect_all_cookies_from_headers, format_cookie_header,
    merge_cookie_header_values, parse_cookie_string,
};
pub use response::{apply_res_rules, format_set_cookie, parse_set_cookie, SetCookieOptions};
