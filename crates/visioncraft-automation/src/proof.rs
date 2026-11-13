//! Proving knowledge of the control token without sending it.
//!
//! The client opens with `challenge {nonce}`; the window answers with a nonce of its own, its
//! process id and `HMAC-SHA256(token, "visioncraft window" | port | pid | client nonce | window
//! nonce)`. The client checks it with the port it connected to, then authenticates with
//! `HMAC-SHA256(token, "visioncraft client" | port | pid | window nonce | client nonce)`. The token
//! never crosses the socket.
//!
//! Ported from 3dcraft `crates/automation/src/proof.rs` (MIT); only the role names differ, so a
//! proof for a 3DCraft window never passes for a VisionCraft one.

pub const WINDOW: &str = "visioncraft window";
pub const CLIENT: &str = "visioncraft client";

const BLOCK: usize = 64;

/// HMAC-SHA256 of `message` under `key` (RFC 2104), over 3dcraft's SHA-256.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0_u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&craft3d_doc::sha256(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = block.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(message);
    let mut outer: Vec<u8> = block.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&craft3d_doc::sha256(&inner));
    craft3d_doc::sha256(&outer)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The proof of `role` for the window listening on 127.0.0.1:`port` as process `pid`, over the
/// two nonces (lower case), as hexadecimal.
pub fn proof(token: &str, role: &str, port: u16, pid: u32, first: &str, second: &str) -> String {
    let message = format!(
        "{role}\nport {port}\npid {pid}\n{}\n{}",
        first.to_ascii_lowercase(),
        second.to_ascii_lowercase()
    );
    hex(&hmac_sha256(token.as_bytes(), message.as_bytes()))
}

/// Whether `a` and `b` are the same text, compared without stopping at the first difference.
pub fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut difference = a.len() ^ b.len();
    for (i, byte) in a.iter().enumerate() {
        difference |= usize::from(byte ^ b.get(i).copied().unwrap_or(0));
    }
    difference == 0 && !a.is_empty()
}

/// Whether `text` is a nonce: 32 to 128 hexadecimal characters.
pub fn is_nonce(text: &str) -> bool {
    (32..=128).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A fresh nonce (or token): 32 random bytes as hexadecimal.
#[cfg(not(target_arch = "wasm32"))]
pub fn nonce() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("cannot get random bytes: {e}"))?;
    Ok(hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_sha256_matches_rfc_4231() {
        let cases: [(Vec<u8>, &[u8], &str); 3] = [
            (
                vec![0x0b; 20],
                b"Hi There",
                "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
            ),
            (
                b"Jefe".to_vec(),
                b"what do ya want for nothing?",
                "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            ),
            (
                vec![0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First",
                "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
            ),
        ];
        for (key, message, expected) in cases {
            assert_eq!(hex(&hmac_sha256(&key, message)), expected);
        }
    }

    #[test]
    fn proofs_depend_on_token_role_port_pid_and_order() {
        let (token, a, b) = ("ab".repeat(32), "1".repeat(64), "2".repeat(64));
        let w = proof(&token, WINDOW, 7920, 42, &a, &b);
        assert_ne!(w, proof(&token, CLIENT, 7920, 42, &a, &b));
        assert_ne!(w, proof(&token, WINDOW, 7920, 42, &b, &a));
        assert_ne!(w, proof(&token, WINDOW, 7921, 42, &a, &b));
        assert_ne!(w, proof(&token, WINDOW, 7920, 43, &a, &b));
        assert!(same(
            &w,
            &proof(&token, WINDOW, 7920, 42, &a.to_uppercase(), &b)
        ));
        assert!(!same("", "") && !same(&w, &w[..63]));
        #[cfg(not(target_arch = "wasm32"))]
        assert!(is_nonce(&nonce().unwrap()));
    }
}
