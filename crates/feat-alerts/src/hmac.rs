//! HMAC-SHA256 (RFC 2104) on top of `sha2`, for checking Twitch's webhook signatures.

use sha2::{Digest, Sha256};

const BLOCK: usize = 64;

/// HMAC-SHA256 of the concatenation of `parts`.
pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    for p in parts {
        inner.update(p);
    }
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner);
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer.finalize());
    out
}

/// Compare without leaking where the first difference is.
pub fn equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Twitch's `Twitch-Eventsub-Message-Signature`: `sha256=` and the hex HMAC of
/// message id + timestamp + raw body, keyed with the EventSub secret.
pub fn twitch_signature_ok(secret: &str, id: &str, timestamp: &str, body: &[u8], header: &str) -> bool {
    let Some(given) = header.strip_prefix("sha256=") else { return false };
    let mac = hmac_sha256(secret.as_bytes(), &[id.as_bytes(), timestamp.as_bytes(), body]);
    equal(hex::encode(mac).as_bytes(), given.to_ascii_lowercase().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4231_vectors() {
        // Test case 2.
        let mac = hmac_sha256(b"Jefe", &[b"what do ya want ", b"for nothing?"]);
        assert_eq!(hex::encode(mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        // Test case 6: a key longer than the block size.
        let key = [0xaau8; 131];
        let mac = hmac_sha256(&key, &[b"Test Using Larger Than Block-Size Key - Hash Key First"]);
        assert_eq!(hex::encode(mac), "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
    }

    #[test]
    fn twitch_signature() {
        let secret = "s3cre7-for-tests";
        let (id, ts, body) = ("e76c6bd4-55c9-4987-8304-da1588d8988b", "2026-10-30T18:00:00.000Z", br#"{"challenge":"abc"}"#);
        let good = format!("sha256={}", hex::encode(hmac_sha256(secret.as_bytes(), &[id.as_bytes(), ts.as_bytes(), body])));
        assert!(twitch_signature_ok(secret, id, ts, body, &good));
        assert!(!twitch_signature_ok("other", id, ts, body, &good));
        assert!(!twitch_signature_ok(secret, id, ts, b"{}", &good));
        assert!(!twitch_signature_ok(secret, id, ts, body, good.trim_start_matches("sha256=")));
        assert!(!twitch_signature_ok(secret, id, ts, body, "sha256=00"));
    }
}
