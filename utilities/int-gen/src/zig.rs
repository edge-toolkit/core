//! Post-process `openapi2zig`'s output into a `wasm32-unknown-unknown`-
//! compatible Zig REST client.
//!
//! `openapi2zig` generates a fully typed client that reaches the network
//! through `std.http.Client.fetch` -- which compiles for
//! `wasm32-freestanding` but can't actually reach the network from a
//! browser sandbox. As of openapi2zig 0.3 the emission has two shapes:
//!
//! * JSON / form / empty bodies route through a single shared
//!   `requestRawWithContentType` (with `requestRaw` a thin default-`content-type`
//!   wrapper over it), which holds the one `client.http.fetch` call.
//! * Binary / text bodies (e.g. `application/octet-stream`) *inline* their
//!   own `client.http.fetch` in the per-operation `*Raw` function rather
//!   than delegating.
//!
//! We funnel everything through one host import. First we swap the body of
//! the shared `requestRawWithContentType` for one that delegates to a single
//! `extern fn js_rest_request(...)` (host-implemented via `fetch()` and
//! `SharedArrayBuffer` in the JS shim). Then we rewrite each inlined binary
//! operation to delegate to that same shared function instead of calling
//! `client.http.fetch` directly. Finally we assert no reachable
//! `client.http.fetch` survived and append the extern declaration.
//!
//! Everything else -- schemas, `RawResponse`, `ApiResult`, per-operation
//! wrappers, the unreachable SSE `streamJson` helper -- is left untouched;
//! Zig's lazy evaluation + dead-code elimination shake out the now-unused
//! `std.http.Client`/`std.Io` machinery (verified: the resulting wasm has
//! a single `env.js_rest_request` import and is ~6 KB at `-O ReleaseSmall`).
//!
//! We use `tree-sitter-zig` to find `requestRawWithContentType` by name
//! rather than string-matching its body -- that way `openapi2zig` version
//! bumps that reshuffle the implementation don't break us.

use std::path::Path;
use std::process::Command;

use fs_err as fs;
use regex::Regex;
use tree_sitter::{Node, Parser};

use crate::Error;

/// Name of the single shared request function whose body we replace with the
/// host-import dispatch; `requestRaw` and every per-operation wrapper funnel
/// through it.
const SHARED_REQUEST_FN: &str = "requestRawWithContentType";

/// The replacement body spliced into [`SHARED_REQUEST_FN`] by [`rewrite`].
const REQUEST_RAW_BODY: &str = include_str!("zig.in/request_raw_body.zig");

/// The `extern fn js_rest_request(...)` declaration appended to the generated client by [`rewrite`].
const JS_REST_REQUEST_EXTERN: &str = include_str!("zig.in/js_rest_request_extern.zig");

/// Regex spanning the inlined request tail openapi2zig 0.3 emits for a binary/text body (its `.binary`/`.text`
/// arm, which -- unlike JSON bodies -- does not delegate to [`SHARED_REQUEST_FN`]). It anchors on the
/// `= requestBody;` payload assignment, which is unique to those ops (JSON ops assign `str.written()`, empty
/// bodies `null`), then skips to the two captures: 1 = content-type, 2 = HTTP method. `[A-Z]+` (not `\w`) keeps
/// it ASCII so no unicode regex feature is needed. [`reroute_inline_binary_ops`] splices both into a delegation.
const INLINE_FETCH_BLOCK_PATTERN: &str =
    r#"(?s)requestBody;.*?"([^"]+)".*?Method\.([A-Z]+).*?toOwnedSlice\(\),\n    \};"#;

/// Return `true` if the `openapi2zig` binary is on `PATH`.
///
/// Upstream doesn't publish a `linux/arm64` release (see `.mise/config.zig.toml`).
#[must_use]
pub fn is_available() -> bool {
    Command::new("openapi2zig").arg("--version").output().is_ok()
}

/// Invoke `openapi2zig` against the `OpenAPI` JSON intermediate, post-process
/// the result, and return the final Zig source.
///
/// Subprocess errors are flattened into `Error::ZigCodegen` since we don't
/// model them more precisely.
pub fn render(rest_json: &Path, raw_out: &Path) -> Result<String, Error> {
    run_openapi2zig(rest_json, raw_out)?;
    let raw = fs::read_to_string(raw_out)?;
    rewrite(&raw)
}

#[expect(
    clippy::single_call_fn,
    reason = "named helper; pairs with rewrite() as the two halves of render()"
)]
fn run_openapi2zig(rest_json: &Path, raw_out: &Path) -> Result<(), Error> {
    if let Some(parent) = raw_out.parent() {
        fs::create_dir_all(parent)?;
    }
    // Surface the spawn failure as `Error::Io` (the `#[from]` variant)
    // rather than a `ZigCodegen(format!(...))` wrap -- same diagnostic
    // detail (`std::io::Error` carries the "No such file or directory"
    // text) with the typed source preserved.
    let status = Command::new("openapi2zig")
        .args([
            "generate",
            "--resource-wrappers",
            "none",
            "-i",
            &rest_json.display().to_string(),
            "-o",
            &raw_out.display().to_string(),
        ])
        .status()?;
    if !status.success() {
        return Err(Error::ZigCodegen(format!("openapi2zig exited with {status}")));
    }
    Ok(())
}

/// Replace the shared request function's body with our extern-backed
/// implementation, reroute the inlined binary operations through it, assert
/// no reachable `client.http.fetch` survived, and append the
/// `extern fn js_rest_request` declaration. Everything else passes through
/// verbatim.
#[expect(
    clippy::single_call_fn,
    reason = "named helper; pairs with run_openapi2zig() as the two halves of render()"
)]
fn rewrite(source: &str) -> Result<String, Error> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_zig::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| Error::ZigCodegen("tree-sitter parse returned None".into()))?;

    let shared_fn = find_fn(tree.root_node(), source, SHARED_REQUEST_FN)
        .ok_or_else(|| Error::ZigCodegen(format!("{SHARED_REQUEST_FN} function not found in openapi2zig output")))?;
    let body = shared_fn
        .child_by_field_name("body")
        .ok_or_else(|| Error::ZigCodegen(format!("{SHARED_REQUEST_FN} has no body field")))?;

    let body_start = body.start_byte();
    let body_end = body.end_byte();

    // 1024 is a comfortable upper bound for the replacement body + extern
    // declaration we splice in below; if the additions ever exceed it the
    // worst case is one extra reallocation, not a panic.
    let mut out = String::with_capacity(source.len().saturating_add(1024));
    // Indexing into `source` is safe here because tree-sitter byte offsets
    // sit on UTF-8 boundaries by construction (it tokenises a UTF-8
    // string and emits byte-aligned spans).
    #[expect(
        clippy::string_slice,
        reason = "tree-sitter byte offsets are UTF-8 boundary-aligned; we round-trip the source by splicing those spans"
    )]
    {
        out.push_str(&source[..body_start]);
        out.push_str(REQUEST_RAW_BODY.trim_end());
        out.push_str(&source[body_end..]);
    }
    out = reroute_inline_binary_ops(&out)?;
    if out.contains("client.http.fetch") {
        return Err(Error::ZigCodegen(
            concat!(
                "openapi2zig output still contains a reachable `client.http.fetch` after rewriting ",
                "- an operation shape changed; it would attempt real HTTP from wasm. Verify the codegen.",
            )
            .into(),
        ));
    }
    out.push_str(JS_REST_REQUEST_EXTERN);
    Ok(out)
}

/// Reroute openapi2zig's inlined binary/text-body operations through the
/// shared, extern-backed request function.
///
/// Since openapi2zig 0.3, an operation whose request body is not JSON/form
/// (e.g. `application/octet-stream`) does not delegate to
/// `requestRawWithContentType`; it inlines the whole
/// header-build + `std.Uri.parse` + `client.http.fetch` + `RawResponse`
/// dance directly (`src/generators/unified/api_generator.zig`'s `.binary`/
/// `.text` arm). That inlined `client.http.fetch` would try to reach the
/// network from wasm. We rewrite the inlined tail back into a single
/// `return requestRawWithContentType(...)` call so it funnels through the
/// host import like every other operation; the captured method and
/// content-type are preserved. The `rewrite` caller then asserts no
/// `client.http.fetch` survived, so an emission shape we don't recognise
/// fails loudly rather than silently emitting a real-HTTP path.
#[expect(
    clippy::single_call_fn,
    reason = "named helper called once by rewrite(); kept separate for the long comment + clear scope"
)]
fn reroute_inline_binary_ops(source: &str) -> Result<String, Error> {
    let block = Regex::new(INLINE_FETCH_BLOCK_PATTERN)?;
    Ok(block
        .replace_all(source, |caps: &regex::Captures<'_>| {
            format!(
                r#"requestBody;

    return {SHARED_REQUEST_FN}(client, std.http.Method.{}, uri_buf.written(), payload, "{}");"#,
                &caps[2], &caps[1],
            )
        })
        .into_owned())
}

/// Recursive walk: return the first `function_declaration` whose `name`
/// child matches `wanted`.
fn find_fn<'tree>(node: Node<'tree>, source: &str, wanted: &str) -> Option<Node<'tree>> {
    // Indexing into `source` is safe because tree-sitter node byte spans
    // are always UTF-8 boundary-aligned for a UTF-8 input.
    #[expect(
        clippy::string_slice,
        reason = "tree-sitter byte spans are UTF-8 boundary-aligned by construction"
    )]
    if node.kind() == "function_declaration"
        && let Some(name) = node.child_by_field_name("name")
        && &source[name.start_byte()..name.end_byte()] == wanted
    {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_fn(child, source, wanted) {
            return Some(found);
        }
    }
    None
}
