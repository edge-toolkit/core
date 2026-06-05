//! Post-process `openapi2zig`'s output into a `wasm32-unknown-unknown`-
//! compatible Zig REST client.
//!
//! `openapi2zig` generates a fully typed client whose `requestRaw` calls
//! `std.http.Client.fetch` — which compiles for `wasm32-freestanding` but
//! can't actually reach the network from a browser sandbox. We swap the
//! body of `requestRaw` for one that delegates to a single
//! `extern fn js_rest_request(...)` import (host-implemented via `fetch()`
//! and `SharedArrayBuffer` in the JS shim), and append the extern
//! declaration. Everything else — schemas, `RawResponse`, `ApiResult`,
//! per-operation wrappers, SSE helpers — is left untouched; Zig's lazy
//! evaluation + dead-code elimination shake out the now-unused
//! `std.http.Client`/`std.Io` machinery (verified: the resulting wasm has
//! a single `env.js_rest_request` import and is ~6 KB at `-O ReleaseSmall`).
//!
//! We use `tree-sitter-zig` to find `requestRaw` by name rather than
//! string-matching its body — that way `openapi2zig` version bumps that
//! reshuffle the implementation don't break us.

use std::path::Path;
use std::process::Command;

use fs_err as fs;
use tree_sitter::{Node, Parser};

use crate::Error;

/// The replacement body spliced into `requestRaw` by [`rewrite`].
const REQUEST_RAW_BODY: &str = include_str!("zig.in/request_raw_body.zig");

/// The `extern fn js_rest_request(...)` declaration appended to the generated client by [`rewrite`].
const JS_REST_REQUEST_EXTERN: &str = include_str!("zig.in/js_rest_request_extern.zig");

/// Return `true` if the `openapi2zig` binary is on `PATH`.
///
/// Upstream doesn't publish a `linux/arm64` release (see `.mise.toml`).
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
    // rather than a `ZigCodegen(format!(...))` wrap — same diagnostic
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

/// Replace `requestRaw`'s body with our extern-backed implementation, fix
/// the JSON-encoding of binary request bodies (openapi2zig blindly applies
/// `std.json.Stringify.value` even when the `OpenAPI` content-type is
/// `application/octet-stream`), and append the `extern fn js_rest_request`
/// declaration. Everything else passes through verbatim.
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

    let request_raw = find_fn(tree.root_node(), source, "requestRaw")
        .ok_or_else(|| Error::ZigCodegen("requestRaw function not found in openapi2zig output".into()))?;
    let body = request_raw
        .child_by_field_name("body")
        .ok_or_else(|| Error::ZigCodegen("requestRaw has no body field".into()))?;

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
    out = fix_binary_request_body(&out)?;
    out.push_str(JS_REST_REQUEST_EXTERN);
    Ok(out)
}

/// Replace openapi2zig's `std.json.Stringify.value(requestBody, ...)` block
/// with a direct `requestBody` pass-through.
#[expect(
    clippy::single_call_fn,
    reason = "named helper called once by rewrite(); kept separate for the long comment + clear scope"
)]
fn fix_binary_request_body(source: &str) -> Result<String, Error> {
    // openapi2zig generates the `bad` block for every operation that has a
    // `requestBody`, regardless of content-type.
    //
    // For `application/octet-stream` endpoints (every body in our spec) this
    // corrupts the wire bytes — `"Hello"` ships as `"\"Hello\""`. Replace
    // the block with a direct `requestBody` pass-through. If openapi2zig
    // changes the pattern we fail loudly rather than silently emitting
    // JSON-encoded bodies.
    //
    // Upstream bug: <https://github.com/christianhelle/openapi2zig/issues/53>
    // (emission sites: `src/generators/unified/api_generator.zig:611-617` and
    // `:1431-1436`, both unconditional on content-type). Drop this workaround
    // once the issue is fixed and we bump the pinned openapi2zig version.
    let bad = concat!(
        "    var str: std.Io.Writer.Allocating = .init(allocator);\n",
        "    defer str.deinit();\n",
        "    try std.json.Stringify.value(requestBody, .{ .emit_null_optional_fields = false }, &str.writer);\n",
        "    const payload: ?[]const u8 = str.written();",
    );
    let good = "    const payload: ?[]const u8 = requestBody;";
    let count = source.matches(bad).count();
    if count == 0 {
        return Err(Error::ZigCodegen(
            concat!(
                "openapi2zig output no longer contains the std.json.Stringify(requestBody) pattern ",
                "- its body-encoding may have changed; verify before relying on the binary fix-up",
            )
            .into(),
        ));
    }
    Ok(source.replace(bad, good))
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
