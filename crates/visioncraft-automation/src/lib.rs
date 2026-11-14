//! Automation for VisionCraft: the JSON-lines protocol and the MCP server over one [`Host`] (a
//! session plus optional pictures), so scripts and AI agents drive everything the app can do
//! (docs/control-protocol.md, docs/agent.md).
//!
//! Modelled on 3dcraft's `craft3d-automation` (MIT, v0.1.1): the same envelope, method names,
//! tool names, limits and error conventions. Where code is ported the source file is named.
//!
//! - [`rpc`]: `{"id", "method", "params"}` in, `{"id", "ok", "result" | "error"}` out.
//! - [`mcp`]: the Model Context Protocol (JSON-RPC 2.0, one message per line).
//! - [`commands`]: what both run: the core's commands plus the host's own (`file.*`,
//!   `edit.*`, `document.inspect`, `view.render`, queries), all-or-nothing batches.
//! - [`bridge`], [`control`], [`discovery`] (native only): the loopback control channel of a
//!   running window, client and server side.
//!
//! Each server is a pure function from one input line to at most one reply line
//! (`handle_line`), plus a loop over a reader and a writer (`serve`). No line, however
//! malformed, makes a handler panic or stops a server.

#[cfg(not(target_arch = "wasm32"))]
pub mod bridge;
pub mod commands;
#[cfg(not(target_arch = "wasm32"))]
pub mod control;
#[cfg(not(target_arch = "wasm32"))]
pub mod discovery;
pub mod docs;
mod fields;
pub mod host;
mod inspect;
mod lines;
pub mod mcp;
pub mod proof;
pub mod rpc;

pub use host::{
    Access, Headless, Host, HostState, LoadedModel, PhotoRequest, Pictures, RenderRequest, Rendered,
};
pub use lines::{Line, read_line};

/// Longest request line a server reads, in bytes (as 3dcraft).
pub const MAX_REQUEST_BYTES: usize = 1 << 20;
/// Longest reply line a server writes, in bytes.
pub const MAX_REPLY_BYTES: usize = 8 << 20;
/// Most steps in one batch.
pub const MAX_BATCH_STEPS: usize = 256;

/// Standard base64 with padding (RFC 4648), for PNGs in replies.
pub fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(A[(n >> shift) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_rfc_4648() {
        for (raw, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(super::base64(raw.as_bytes()), coded);
        }
    }
}
