# fetchurl Rust SDK

Protocol-level client for [fetchurl](https://github.com/fetchurl/spec) content-addressable cache servers.

This crate does **not** perform HTTP by itself. It provides a state machine (`FetchSession`) that drives protocol logic while you use any HTTP library for I/O.

## Install

```toml
[dependencies]
fetchurl-sdk = "0.2"
```

## Protocol

Normative behavior: **[fetchurl/spec](https://github.com/fetchurl/spec)** (`SPEC.md`).

Reference server: **[fetchurl/fetchurl](https://github.com/fetchurl/fetchurl)**.

## Usage

See crate docs and `examples/get.rs`. Clients **must** treat the server as untrusted and verify the hash (the session/verifier APIs are built for that).

## Environment

| Variable | Meaning |
|----------|---------|
| `FETCHURL_SERVER` | Server base URL(s) per the [spec](https://github.com/fetchurl/spec/blob/main/SPEC.md). Empty/absent disables server use. |

## Development

```bash
cargo test
cargo run --example get -- --help
# Integration (Docker + image):
# FETCHURL_TEST_IMAGE=fetchurl:local cargo test --test integration
```
