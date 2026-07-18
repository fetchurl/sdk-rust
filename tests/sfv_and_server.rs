//! Public SFV / FETCHURL_SERVER helper coverage.
//!
//! Unit tests in lib.rs exercise basic encode/parse roundtrips. These cover the
//! protocol edge cases sibling SDKs already pin: unquoted single-server form,
//! empty env values, and quote/backslash escaping in X-Source-Urls.

use fetchurl_sdk::{encode_source_urls, parse_fetchurl_server};

#[test]
fn parse_fetchurl_server_empty_and_whitespace() {
    assert!(parse_fetchurl_server("").is_empty());
    assert!(parse_fetchurl_server("   ").is_empty());
    assert!(parse_fetchurl_server("\t\n").is_empty());
}

#[test]
fn parse_fetchurl_server_unquoted_single_url() {
    // Spec: if the first character is not `"`, the whole value is one server.
    assert_eq!(
        parse_fetchurl_server("http://cache.local:8080/api/fetchurl"),
        vec!["http://cache.local:8080/api/fetchurl"]
    );
    assert_eq!(
        parse_fetchurl_server("  http://cache/api/fetchurl  "),
        vec!["http://cache/api/fetchurl"]
    );
}

#[test]
fn parse_fetchurl_server_quoted_list() {
    assert_eq!(
        parse_fetchurl_server(r#""http://a/api/fetchurl", "http://b/api/fetchurl""#),
        vec!["http://a/api/fetchurl", "http://b/api/fetchurl"]
    );
}

#[test]
fn parse_fetchurl_server_ignores_parameters() {
    assert_eq!(
        parse_fetchurl_server(r#""http://a/api/fetchurl";q=0.9, "http://b/api/fetchurl""#),
        vec!["http://a/api/fetchurl", "http://b/api/fetchurl"]
    );
}

#[test]
fn encode_source_urls_escapes_quotes_and_backslashes() {
    let encoded = encode_source_urls(&[r#"https://ex.com/a"b\c"#]);
    assert_eq!(encoded, r#""https://ex.com/a\"b\\c""#);

    // Round-trip through the same SFV parser used for FETCHURL_SERVER lists.
    assert_eq!(
        parse_fetchurl_server(&encoded),
        vec![r#"https://ex.com/a"b\c"#]
    );
}

#[test]
fn encode_source_urls_roundtrip_multiple() {
    let urls = [
        "https://cdn.example.com/file.tar.gz",
        "https://mirror.org/archive.tgz",
    ];
    let encoded = encode_source_urls(&urls);
    assert_eq!(parse_fetchurl_server(&encoded), urls);
}
