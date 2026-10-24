//! Capability tokens: compact HMAC-signed path caps with versioned keys and boot epochs (§2).
//!
//! `v1.<kid>.<payload>.<mac>`, all base64url (no padding), at most 300 bytes.
//! - `payload` is canonical CBOR (RFC 8949 §4.2.1: shortest integers, map keys in bytewise
//!   order of their encodings) of the claims `{p, h, s, r, ro?, dw?, g?, e, m, me, b, w, v?, d?}`.
//! - `mac = HMAC-SHA256(key[kid], "v1." ‖ kid ‖ "." ‖ payload)`, truncated to 16 bytes, compared
//!   in constant time.
//! - `kid` is the UTC day number; `key[kid] = HKDF-SHA256(PF_CAP_HMAC_KEY, info = "pf-cap/" ‖ kid)`.
//!   Verify accepts `kid` and `kid − 1`.
//! - Windows are deterministic: 12 h for people, links and agents (a cap minted in a window's last
//!   hour uses the next window), 60 s for `render`. No nonce, so the same inputs give the same cap.
//! - Verify needs no database: it reads the in-memory [`EpochStore`].
//!
//! Encoding choices that keep a cap under 300 bytes with a 58-character host and a pinned
//! version: a ULID part id (`p_…`) travels as its 16 raw bytes, an id of the form `kind:x_<ULID>`
//! (subject, minter, guest, draft) as `["kind:x", <16 bytes>]`, and a pinned version `v` as the
//! first 16 bytes of its sha256 (verify returns those 32 hex chars; the content listener checks
//! that the pinned version of this part starts with them). The part epoch claim `e` carries
//! `epoch[part] + epoch[global]`, so bumping either one rejects older caps.

mod epochs;
pub use epochs::{EpochStore, bump_epoch, minter_key, part_key};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ciborium::Value;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// The largest cap mint will produce.
pub const MAX_CAP_LEN: usize = 300;
/// Window length for `user`, `link` and `conn` subjects, in minutes (12 h).
pub const WINDOW_MIN: i64 = 720;
/// A cap minted with less than this left in its window uses the next window (seconds).
pub const ROLL_FORWARD_S: i64 = 3600;
const MAC_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapError {
    #[error("malformed cap")]
    Malformed,
    #[error("unknown key id")]
    UnknownKid,
    #[error("bad mac")]
    BadMac,
    #[error("cap is for another host")]
    WrongHost,
    #[error("part epoch is stale")]
    StalePartEpoch,
    #[error("minter epoch is stale")]
    StaleMinterEpoch,
    #[error("boot epoch is below the floor")]
    StaleBoot,
    #[error("window is over")]
    Expired,
    #[error("cap would be {0} bytes (max 300)")]
    TooLarge(usize),
    #[error("invalid claim: {0}")]
    Invalid(&'static str),
}

impl CapError {
    /// Whether the right answer is a silent re-grant (302 to `/grant`, or 401 + `X-PF-Reauth`)
    /// rather than a 404 that looks like a missing part.
    pub fn is_reauth(&self) -> bool {
        matches!(
            self,
            CapError::Expired
                | CapError::StalePartEpoch
                | CapError::StaleMinterEpoch
                | CapError::StaleBoot
        )
    }
}

/// The root secret (`PF_CAP_HMAC_KEY`) and the daily keys derived from it.
#[derive(Clone)]
pub struct CapKeys {
    root: Vec<u8>,
}

impl std::fmt::Debug for CapKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CapKeys(<redacted>)")
    }
}

impl CapKeys {
    pub fn new(root_secret: &[u8]) -> Self {
        Self {
            root: root_secret.to_vec(),
        }
    }

    /// HKDF-SHA256 (RFC 5869) with an empty salt: one 32-byte block of output.
    fn key(&self, kid: i64) -> [u8; 32] {
        let mut ext = <HmacSha256 as Mac>::new_from_slice(&[0u8; 32]).expect("hmac key");
        ext.update(&self.root);
        let prk = ext.finalize().into_bytes();
        let mut exp = <HmacSha256 as Mac>::new_from_slice(&prk).expect("hmac key");
        exp.update(format!("pf-cap/{kid}").as_bytes());
        exp.update(&[1u8]);
        exp.finalize().into_bytes().into()
    }

    fn mac(&self, kid: i64, signed: &str) -> HmacSha256 {
        let mut m = <HmacSha256 as Mac>::new_from_slice(&self.key(kid)).expect("hmac key");
        m.update(signed.as_bytes());
        m
    }
}

/// The decided grant a cap carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    /// Part id (`p_…`).
    pub p: String,
    /// The part host's label (the part name).
    pub h: String,
    /// `user:u_…` | `link:l_…` | `conn:c_…` | `render`.
    pub s: String,
    /// Role: 0 view, 1 comment, 2 edit, 3 manage.
    pub r: u8,
    pub ro: bool,
    pub dw: bool,
    pub g: Option<String>,
    /// `epoch[part] + epoch[global]` at mint.
    pub e: i64,
    /// `sess:<id>` | `link:<id>` | `conn:<id>` | `render`.
    pub m: String,
    pub me: i64,
    pub b: i64,
    /// Window end, unix minutes. Valid while `now < w * 60 s`.
    pub w: i64,
    /// Pinned version: the first 32 hex chars of its id.
    pub v: Option<String>,
    pub d: Option<String>,
}

/// What to mint; the epochs and window are filled in by [`mint`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grant {
    pub part_id: String,
    pub host: String,
    pub subject: String,
    pub role: u8,
    pub read_only: bool,
    pub data_write: bool,
    pub guest: Option<String>,
    pub minter: String,
    pub version: Option<String>,
    pub draft: Option<String>,
}

/// UTC day number of `now_ms` (the key id).
pub fn kid_at(now_ms: i64) -> i64 {
    now_ms.div_euclid(86_400_000)
}

/// The window end (unix minutes) a cap minted at `now_ms` gets.
pub fn window_end(now_ms: i64, render: bool) -> i64 {
    let now_s = now_ms.div_euclid(1000);
    if render {
        // Ends at the minute boundary after next: 60–120 s of life.
        return now_s.div_euclid(60) + 2;
    }
    let end = (now_s.div_euclid(WINDOW_MIN * 60) + 1) * WINDOW_MIN;
    if end * 60 - now_s <= ROLL_FORWARD_S {
        end + WINDOW_MIN
    } else {
        end
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.to_string())
}

fn encode_part(p: &str) -> Value {
    match p.strip_prefix("p_").and_then(pf_db::ids::ulid_decode) {
        Some(u) => Value::Bytes(u.to_be_bytes().to_vec()),
        None => text(p),
    }
}

fn decode_part(v: &Value) -> Option<String> {
    match v {
        Value::Bytes(b) if b.len() == 16 => {
            let mut a = [0u8; 16];
            a.copy_from_slice(b);
            Some(format!(
                "p_{}",
                pf_db::ids::ulid_encode(u128::from_be_bytes(a))
            ))
        }
        Value::Text(t) => Some(t.clone()),
        _ => None,
    }
}

/// `kind:x_<ULID>` (a subject, minter, guest or draft id) travels as `["kind:x", <16 bytes>]`;
/// anything else as text.
fn encode_id(s: &str) -> Value {
    if let Some((head, tail)) = s.rsplit_once('_')
        && let Some(u) = pf_db::ids::ulid_decode(tail)
    {
        return Value::Array(vec![text(head), Value::Bytes(u.to_be_bytes().to_vec())]);
    }
    text(s)
}

fn decode_id(v: &Value) -> Option<String> {
    match v {
        Value::Text(t) => Some(t.clone()),
        Value::Array(a) if a.len() == 2 => {
            let head = a[0].as_text()?;
            let b = a[1].as_bytes()?;
            let arr: [u8; 16] = b.as_slice().try_into().ok()?;
            Some(format!(
                "{head}_{}",
                pf_db::ids::ulid_encode(u128::from_be_bytes(arr))
            ))
        }
        _ => None,
    }
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn canonical_cbor(c: &Claims) -> Result<Vec<u8>, CapError> {
    let mut entries: Vec<(&str, Value)> = vec![
        ("p", encode_part(&c.p)),
        ("h", text(&c.h)),
        ("s", encode_id(&c.s)),
        ("r", Value::Integer(c.r.into())),
        ("e", Value::Integer(c.e.into())),
        ("m", encode_id(&c.m)),
        ("me", Value::Integer(c.me.into())),
        ("b", Value::Integer(c.b.into())),
        ("w", Value::Integer(c.w.into())),
    ];
    if c.ro {
        entries.push(("ro", Value::Bool(true)));
    }
    if c.dw {
        entries.push(("dw", Value::Bool(true)));
    }
    if let Some(g) = &c.g {
        entries.push(("g", encode_id(g)));
    }
    if let Some(v) = &c.v {
        let raw = hex_decode(v).ok_or(CapError::Invalid("v must be hex"))?;
        let raw = raw[..raw.len().min(16)].to_vec();
        entries.push(("v", Value::Bytes(raw)));
    }
    if let Some(d) = &c.d {
        entries.push(("d", encode_id(d)));
    }
    // RFC 8949 §4.2.1: sort by the bytewise encoding of the keys. For short text keys that is
    // length first, then bytes.
    entries.sort_by(|a, b| (a.0.len(), a.0).cmp(&(b.0.len(), b.0)));
    let map = Value::Map(entries.into_iter().map(|(k, v)| (text(k), v)).collect());
    let mut out = Vec::new();
    ciborium::into_writer(&map, &mut out).map_err(|_| CapError::Invalid("cbor"))?;
    Ok(out)
}

fn int(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => i64::try_from(*i).ok(),
        _ => None,
    }
}

fn decode_claims(bytes: &[u8]) -> Result<Claims, CapError> {
    let v: Value = ciborium::from_reader(bytes).map_err(|_| CapError::Malformed)?;
    let Value::Map(entries) = v else {
        return Err(CapError::Malformed);
    };
    let get = |k: &str| {
        entries
            .iter()
            .find(|(kk, _)| kk.as_text() == Some(k))
            .map(|(_, v)| v)
    };
    let t = |k: &str| get(k).and_then(|v| v.as_text()).map(str::to_string);
    let id = |k: &str| get(k).and_then(decode_id);
    let req_i = |k: &str| get(k).and_then(int).ok_or(CapError::Malformed);
    Ok(Claims {
        p: get("p").and_then(decode_part).ok_or(CapError::Malformed)?,
        h: t("h").ok_or(CapError::Malformed)?,
        s: id("s").ok_or(CapError::Malformed)?,
        r: u8::try_from(req_i("r")?).map_err(|_| CapError::Malformed)?,
        ro: get("ro").and_then(|v| v.as_bool()).unwrap_or(false),
        dw: get("dw").and_then(|v| v.as_bool()).unwrap_or(false),
        g: id("g"),
        e: req_i("e")?,
        m: id("m").ok_or(CapError::Malformed)?,
        me: req_i("me")?,
        b: req_i("b")?,
        w: req_i("w")?,
        v: get("v").and_then(|v| v.as_bytes()).map(|b| hex_encode(b)),
        d: id("d"),
    })
}

/// Mint a cap for `grant` at `now_ms`, with the current epochs from `epochs`.
pub fn mint(
    keys: &CapKeys,
    epochs: &EpochStore,
    grant: &Grant,
    now_ms: i64,
) -> Result<String, CapError> {
    if grant.role > 3 {
        return Err(CapError::Invalid("role"));
    }
    if grant.host.is_empty()
        || grant.part_id.is_empty()
        || grant.subject.is_empty()
        || grant.minter.is_empty()
    {
        return Err(CapError::Invalid("empty field"));
    }
    let render = grant.subject == "render";
    let claims = Claims {
        p: grant.part_id.clone(),
        h: grant.host.clone(),
        s: grant.subject.clone(),
        r: grant.role,
        ro: grant.read_only,
        dw: grant.data_write,
        g: grant.guest.clone(),
        e: epochs.part_epoch(&grant.part_id),
        m: grant.minter.clone(),
        me: epochs.get(&minter_key(&grant.minter)),
        b: epochs.boot_floor(),
        w: window_end(now_ms, render),
        v: grant.version.clone(),
        d: grant.draft.clone(),
    };
    let payload = B64.encode(canonical_cbor(&claims)?);
    let kid = kid_at(now_ms);
    let signed = format!("v1.{kid}.{payload}");
    let tag = keys.mac(kid, &signed).finalize().into_bytes();
    let cap = format!("{signed}.{}", B64.encode(&tag[..MAC_LEN]));
    if cap.len() > MAX_CAP_LEN {
        return Err(CapError::TooLarge(cap.len()));
    }
    Ok(cap)
}

/// Verify `cap` for a request to `host` at `now_ms`, in the §2 order: known kid → MAC → host →
/// part epoch → minter epoch → boot floor → window. Reads only `epochs` (no database).
pub fn verify(
    keys: &CapKeys,
    epochs: &EpochStore,
    cap: &str,
    host: &str,
    now_ms: i64,
) -> Result<Claims, CapError> {
    if cap.len() > MAX_CAP_LEN {
        return Err(CapError::Malformed);
    }
    let mut it = cap.split('.');
    let (Some("v1"), Some(kid_s), Some(payload), Some(mac_s), None) =
        (it.next(), it.next(), it.next(), it.next(), it.next())
    else {
        return Err(CapError::Malformed);
    };
    let kid: i64 = kid_s.parse().map_err(|_| CapError::Malformed)?;
    if kid_s != kid.to_string() {
        return Err(CapError::Malformed);
    }
    let today = kid_at(now_ms);
    if kid != today && kid != today - 1 {
        return Err(CapError::UnknownKid);
    }
    let tag = B64.decode(mac_s).map_err(|_| CapError::Malformed)?;
    if tag.len() != MAC_LEN {
        return Err(CapError::Malformed);
    }
    let signed = &cap[..cap.len() - mac_s.len() - 1];
    keys.mac(kid, signed)
        .verify_truncated_left(&tag)
        .map_err(|_| CapError::BadMac)?;
    let raw = B64.decode(payload).map_err(|_| CapError::Malformed)?;
    let c = decode_claims(&raw)?;
    if c.h != host {
        return Err(CapError::WrongHost);
    }
    if c.e < epochs.part_epoch(&c.p) {
        return Err(CapError::StalePartEpoch);
    }
    if c.me < epochs.get(&minter_key(&c.m)) {
        return Err(CapError::StaleMinterEpoch);
    }
    if c.b < epochs.boot_floor() {
        return Err(CapError::StaleBoot);
    }
    if now_ms.div_euclid(1000) >= c.w * 60 {
        return Err(CapError::Expired);
    }
    Ok(c)
}

#[cfg(test)]
mod tests;
