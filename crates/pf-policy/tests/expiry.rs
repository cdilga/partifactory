//! An expired grant stops working at expiry (test clock); the expiry timer deletes it, audits it
//! as the system and bumps the parts it covered, so caps minted under it stop verifying.

mod common;
use common::*;
use pf_caps::{CapKeys, EpochStore, Grant, mint, verify};
use pf_model::Principal;
use pf_policy::grants::{Actor, Target, expire_due, set_grant};
use pf_policy::{EDIT, Engine, mirror};

#[test]
fn expired_grant_stops_and_the_timer_revokes_its_caps() {
    let w = world();
    let a = w.person("alice", true);
    let b = w.person("bob", true);
    let fam = w.shared_space("family");
    let p = w.part("x", &a, &fam);
    let q = w.part("y", &a, &fam);
    let t0 = 1_800_000_000_000i64;
    let exp = t0 + 60_000;
    w.with(|tx| {
        set_grant(
            tx,
            &Actor::person(&a),
            &Target::Space(fam.clone()),
            &format!("user:{b}"),
            EDIT,
            Some(exp),
            t0,
        )
        .unwrap()
    });
    let bob = Principal::person(&b);
    let role = |now| w.with(|c| Engine::decide(c, &bob, &p, now).unwrap());
    assert_eq!(role(exp - 1), Some(EDIT));
    assert_eq!(role(exp), None, "stops at expiry, before any timer runs");

    // A cap minted before expiry would live for its window; the timer's bump kills it.
    let epochs = w.with(|c| EpochStore::load(c).unwrap());
    let keys = CapKeys::new(b"k");
    let g = Grant {
        part_id: p.clone(),
        host: "x".into(),
        subject: format!("user:{b}"),
        role: EDIT,
        minter: "sess:s_01J00000000000000000000000".into(),
        ..Default::default()
    };
    let cap = mint(&keys, &epochs, &g, exp - 1).unwrap();
    assert!(verify(&keys, &epochs, &cap, "x", exp + 1).is_ok());

    // Not yet due: nothing happens.
    let (bumped, n) = w.with(|tx| expire_due(tx, exp - 1).unwrap());
    assert_eq!((bumped.len(), n), (0, 0));
    let (bumped, n) = w.with(|tx| expire_due(tx, exp).unwrap());
    assert_eq!(n, 1);
    let keys_bumped: Vec<_> = bumped.iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(keys_bumped, vec![format!("part:{p}"), format!("part:{q}")]);
    mirror(&epochs, &bumped, "grant_expired");
    assert!(verify(&keys, &epochs, &cap, "x", exp + 1).is_err());
    assert_eq!(w.count("SELECT COUNT(*) FROM grants"), 0);
    assert_eq!(
        w.count("SELECT COUNT(*) FROM audit WHERE action = 'grant.expire' AND actor = 'system'"),
        1
    );
}
