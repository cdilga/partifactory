use super::*;
use proptest::prelude::*;

const H: i64 = 3_600_000;
const DAY: i64 = 24 * H;
/// 2026-10-23T00:00:00Z
const T0: i64 = 20_384 * DAY;

fn keys() -> CapKeys {
    CapKeys::new(b"0123456789abcdef0123456789abcdef")
}

fn grant(host: &str) -> Grant {
    Grant {
        part_id: "p_01JB4X2K9V3M8Q7R6T5Y4W3Z2A".into(),
        host: host.into(),
        subject: "user:u_01JB4X2K9V3M8Q7R6T5Y4W3Z2B".into(),
        role: 2,
        minter: "sess:9f2c4e7a1b3d5f8e".into(),
        ..Grant::default()
    }
}

fn host_strategy() -> impl Strategy<Value = String> {
    "[a-z0-9]{1,20}(-[a-z0-9]{1,10}){0,3}".prop_filter("<= 58", |s| s.len() <= 58)
}

#[test]
fn round_trip_and_claims() {
    let ep = EpochStore::default();
    let cap = mint(&keys(), &ep, &grant("sleep-chart"), T0 + 5 * H).unwrap();
    assert!(cap.starts_with("v1.20384."), "{cap}");
    let c = verify(&keys(), &ep, &cap, "sleep-chart", T0 + 6 * H).unwrap();
    assert_eq!(c.p, "p_01JB4X2K9V3M8Q7R6T5Y4W3Z2A");
    assert_eq!(c.s, "user:u_01JB4X2K9V3M8Q7R6T5Y4W3Z2B");
    assert_eq!(c.r, 2);
    assert_eq!(c.w * 60_000, T0 + 12 * H);
}

#[test]
fn max_cap_is_at_most_300_bytes() {
    let ep = EpochStore::default();
    for e in [0, 1_000_000_000] {
        ep.set("global", e);
        ep.set("boot", e);
        let mut g = grant(&"a".repeat(58));
        g.subject = "conn:c_01JB4X2K9V3M8Q7R6T5Y4W3Z2C".into();
        g.minter = "conn:c_01JB4X2K9V3M8Q7R6T5Y4W3Z2C".into();
        g.version = Some("9f2c4e7a1b3d5f8e0a2c4b6d8f1e3a5c7b9d0e2f4a6c8e1b3d5f7a9c0e2b4d6f".into());
        let cap = mint(&keys(), &ep, &g, T0).unwrap();
        assert!(cap.len() <= MAX_CAP_LEN, "{} bytes", cap.len());
        let c = verify(&keys(), &ep, &cap, &g.host, T0 + 1000).unwrap();
        assert_eq!(c.v.as_deref(), Some("9f2c4e7a1b3d5f8e0a2c4b6d8f1e3a5c"));
    }
}

#[test]
fn render_window_is_about_a_minute() {
    let ep = EpochStore::default();
    let mut g = grant("x");
    g.subject = "render".into();
    g.minter = "render".into();
    let now = T0 + 30_000;
    let cap = mint(&keys(), &ep, &g, now).unwrap();
    assert!(verify(&keys(), &ep, &cap, "x", now + 59_000).is_ok());
    assert_eq!(
        verify(&keys(), &ep, &cap, "x", now + 121_000),
        Err(CapError::Expired)
    );
}

#[test]
fn kid_rotation_25h() {
    // Minted at 23:00 on day K, its key verifies until the end of day K+1 (25 h later), then
    // the kid is unknown. The 12 h window is long over by then, so a verified key shows up as
    // `Expired`, an unverified one as `UnknownKid`.
    let ep = EpochStore::default();
    let minted = T0 + 23 * H;
    let cap = mint(&keys(), &ep, &grant("x"), minted).unwrap();
    assert_eq!(
        verify(&keys(), &ep, &cap, "x", minted + 25 * H - 1000),
        Err(CapError::Expired)
    );
    assert_eq!(
        verify(&keys(), &ep, &cap, "x", minted + 25 * H + 1000),
        Err(CapError::UnknownKid)
    );
    // A different root secret (rotation of PF_CAP_HMAC_KEY) rejects every existing cap.
    assert_eq!(
        verify(
            &CapKeys::new(b"another root secret, 32 bytes!!!"),
            &ep,
            &cap,
            "x",
            minted
        ),
        Err(CapError::BadMac)
    );
}

#[test]
fn verify_needs_no_database() {
    // `verify` takes only keys, the in-memory epochs, the cap, the host and the time: there is
    // no database handle to query. A default (never loaded) store works.
    let ep = EpochStore::default();
    let cap = mint(&keys(), &ep, &grant("x"), T0).unwrap();
    assert!(verify(&keys(), &ep, &cap, "x", T0 + 1).is_ok());
}

#[test]
fn epochs_load_from_db_and_boot_bumps() {
    let db = pf_db::temp_db();
    let ep = EpochStore::load_and_bump_boot(&db).unwrap();
    assert_eq!(ep.boot_floor(), 1);
    let cap = mint(&keys(), &ep, &grant("x"), T0).unwrap();
    // A restart bumps the floor: the old cap fails.
    let ep2 = EpochStore::load_and_bump_boot(&db).unwrap();
    assert_eq!(ep2.boot_floor(), 2);
    assert_eq!(
        verify(&keys(), &ep2, &cap, "x", T0 + 1),
        Err(CapError::StaleBoot)
    );
    // A part bump through the writer persists.
    let rt = tokio_rt();
    rt.block_on(ep2.bump(&db, &part_key("p_01JB4X2K9V3M8Q7R6T5Y4W3Z2A")))
        .unwrap();
    let ep3 = db
        .read_blocking(|c| EpochStore::load(c).map_err(pf_db::DbError::from))
        .unwrap();
    assert_eq!(ep3.get("part:p_01JB4X2K9V3M8Q7R6T5Y4W3Z2A"), 1);
}

fn tokio_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

proptest! {
    #[test]
    fn host_bound(host in host_strategy(), other in host_strategy(), off in 0i64..(11 * H)) {
        prop_assume!(host != other);
        let ep = EpochStore::default();
        let cap = mint(&keys(), &ep, &grant(&host), T0 + off).unwrap();
        prop_assert_eq!(verify(&keys(), &ep, &cap, &other, T0 + off), Err(CapError::WrongHost));
        prop_assert!(verify(&keys(), &ep, &cap, &host, T0 + off).is_ok());
    }

    #[test]
    fn deterministic(host in host_strategy(), a in 0i64..(11 * H), b in 0i64..(11 * H)) {
        // Two mints in the same window, both before its last hour, give the same cap.
        let ep = EpochStore::default();
        let c1 = mint(&keys(), &ep, &grant(&host), T0 + a).unwrap();
        let c2 = mint(&keys(), &ep, &grant(&host), T0 + b).unwrap();
        prop_assert_eq!(c1, c2);
    }

    #[test]
    fn last_hour_rolls_forward(now in T0..(T0 + 30 * DAY)) {
        let w = window_end(now, false) * 60_000;
        let left = w - now;
        prop_assert!(left > H, "a fresh cap has more than an hour left ({left} ms)");
        prop_assert!(left <= 13 * H);
        let in_last_hour = (now.rem_euclid(12 * H)) >= 11 * H;
        let natural_end = (now.div_euclid(12 * H) + 1) * 12 * H;
        if in_last_hour {
            prop_assert_eq!(w, natural_end + 12 * H);
        } else {
            prop_assert_eq!(w, natural_end);
        }
    }

    #[test]
    fn epoch_rejection(which in 0usize..4, off in 0i64..(11 * H)) {
        let ep = EpochStore::default();
        let g = grant("x");
        let cap = mint(&keys(), &ep, &g, T0 + off).unwrap();
        let (key, want) = match which {
            0 => (part_key(&g.part_id), CapError::StalePartEpoch),
            1 => (minter_key(&g.minter), CapError::StaleMinterEpoch),
            2 => ("global".to_string(), CapError::StalePartEpoch),
            _ => ("boot".to_string(), CapError::StaleBoot),
        };
        ep.set(&key, 1);
        prop_assert_eq!(verify(&keys(), &ep, &cap, "x", T0 + off), Err(want));
        // A cap minted after the bump verifies.
        let fresh = mint(&keys(), &ep, &g, T0 + off).unwrap();
        prop_assert!(verify(&keys(), &ep, &fresh, "x", T0 + off).is_ok());
    }

    #[test]
    fn tamper(pos in 0usize..300, byte in any::<u8>()) {
        let ep = EpochStore::default();
        let cap = mint(&keys(), &ep, &grant("sleep-chart"), T0).unwrap();
        let mut b = cap.clone().into_bytes();
        let i = pos % b.len();
        let replacement = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_."[byte as usize % 65];
        prop_assume!(b[i] != replacement);
        b[i] = replacement;
        let t = String::from_utf8(b).unwrap();
        prop_assert!(verify(&keys(), &ep, &t, "sleep-chart", T0).is_err(), "tampered cap verified: {t}");
    }
}
