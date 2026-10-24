//! Prefixed ULID ids (`p_01JB…`, `u_…`, `sp_…`) and unix-ms time, the schema's conventions.

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A new ULID: 48 bits of unix ms, then 80 random bits.
pub fn new_ulid() -> u128 {
    let mut rnd = [0u8; 10];
    getrandom::fill(&mut rnd).expect("os randomness");
    let mut r: u128 = 0;
    for b in rnd {
        r = (r << 8) | b as u128;
    }
    ((now_ms() as u128 & 0xFFFF_FFFF_FFFF) << 80) | r
}

/// The 26-character Crockford base32 form of a ULID.
pub fn ulid_encode(v: u128) -> String {
    let mut out = [0u8; 26];
    for (i, slot) in out.iter_mut().enumerate() {
        let shift = 125 - 5 * i as u32;
        *slot = CROCKFORD[((v >> shift) & 0x1F) as usize];
    }
    String::from_utf8(out.to_vec()).expect("ascii")
}

/// Parse a 26-character Crockford ULID (upper case, as [`ulid_encode`] writes it).
pub fn ulid_decode(s: &str) -> Option<u128> {
    let b = s.as_bytes();
    if b.len() != 26 || b[0] > b'7' {
        return None;
    }
    let mut v: u128 = 0;
    for &c in b {
        let d = CROCKFORD.iter().position(|&x| x == c)? as u128;
        v = (v << 5) | d;
    }
    Some(v)
}

/// A new prefixed id, e.g. `new_id("p")` → `p_01JB4X2K9V3M8Q7R6T5Y4W3Z2A`.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", ulid_encode(new_ulid()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulid_round_trip() {
        for _ in 0..100 {
            let v = new_ulid();
            let s = ulid_encode(v);
            assert_eq!(s.len(), 26);
            assert_eq!(ulid_decode(&s), Some(v));
        }
        assert_eq!(ulid_decode("not-a-ulid"), None);
        assert!(new_id("p").starts_with("p_"));
    }
}
