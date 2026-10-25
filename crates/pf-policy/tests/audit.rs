//! Every grant change writes exactly one audit row; narrowing changes bump epochs.

mod common;
use common::*;
use pf_policy::grants::{Actor, Target, authorize, revoke_grant, set_grant};
use pf_policy::groups::{create_local, set_local_member};
use pf_policy::{EDIT, MANAGE, VIEW};

#[test]
fn every_grant_change_writes_one_audit_row() {
    let w = world();
    let a = w.person("alice", true);
    let b = w.person("bob", true);
    let p = w.part("x", &a, "sp_ALICE");
    let t = Target::Part(p.clone());
    let sub = format!("user:{b}");
    let act = Actor::person(&a);
    let audits = || w.count("SELECT COUNT(*) FROM audit");

    let bumped = w.with(|tx| set_grant(tx, &act, &t, &sub, EDIT, None, 10).unwrap());
    assert!(bumped.is_empty(), "widening needs no bump");
    assert_eq!(audits(), 1);
    // Lowering the role narrows: bump.
    let bumped = w.with(|tx| set_grant(tx, &act, &t, &sub, VIEW, None, 11).unwrap());
    assert_eq!(bumped, vec![(format!("part:{p}"), 1)]);
    assert_eq!(audits(), 2);
    // Revoke: one row, one bump.
    let bumped = w.with(|tx| revoke_grant(tx, &act, &t, &sub, 12).unwrap());
    assert_eq!(bumped, vec![(format!("part:{p}"), 2)]);
    assert_eq!(audits(), 3);
    // Revoking nothing writes nothing.
    let bumped = w.with(|tx| revoke_grant(tx, &act, &t, &sub, 13).unwrap());
    assert!(bumped.is_empty());
    assert_eq!(audits(), 3);
    // An agent's grant is labelled with its connection.
    w.with(|tx| set_grant(tx, &Actor::agent(&a, "c_X"), &t, &sub, VIEW, None, 14).unwrap());
    assert_eq!(
        w.count("SELECT COUNT(*) FROM audit WHERE actor = 'conn:c_X' AND action = 'grant.set'"),
        1
    );
    assert_eq!(
        w.count("SELECT COUNT(*) FROM grants WHERE created_by_conn = 'c_X'"),
        1
    );
    // The before/after are recorded.
    assert_eq!(
        w.count("SELECT COUNT(*) FROM audit WHERE action = 'grant.revoke' AND json_extract(before, '$.role') = 'view' AND after IS NULL"),
        1
    );
}

#[test]
fn invalid_grants_are_refused_without_audit() {
    let w = world();
    let a = w.person("alice", true);
    let p = w.part("x", &a, "sp_ALICE");
    let t = Target::Part(p);
    let act = Actor::person(&a);
    for bad in ["link:l_1", "conn:c_1", "user:", "nobody"] {
        let e = w.with(|tx| set_grant(tx, &act, &t, bad, VIEW, None, 1).unwrap_err());
        assert_eq!(e.code, "invalid_input", "{bad}");
    }
    assert_eq!(w.count("SELECT COUNT(*) FROM audit"), 0);
}

#[test]
fn who_may_grant() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    // Owners/managers share inward; viewers can't; strangers see not_found.
    assert!(authorize(Some(MANAGE), false, None, "user:u_B", EDIT).is_ok());
    assert_eq!(
        authorize(Some(VIEW), false, None, "user:u_B", VIEW)
            .unwrap_err()
            .code,
        "forbidden"
    );
    assert_eq!(
        authorize(None, false, None, "user:u_B", VIEW)
            .unwrap_err()
            .code,
        "not_found"
    );
    // everyone-signed-in is the admin's alone, never an agent's.
    assert!(authorize(Some(MANAGE), false, None, "everyone-signed-in", VIEW).is_err());
    assert!(authorize(None, true, None, "everyone-signed-in", VIEW).is_ok());
    let sh = s(&["parts:write", "share:household"]);
    assert!(authorize(Some(MANAGE), true, Some(&sh), "everyone-signed-in", VIEW).is_err());
    // Agents need share:household and give at most edit.
    assert!(
        authorize(
            Some(MANAGE),
            false,
            Some(&s(&["parts:write"])),
            "user:u_B",
            VIEW
        )
        .is_err()
    );
    assert!(authorize(Some(MANAGE), false, Some(&sh), "group:household", EDIT).is_ok());
    assert!(authorize(Some(MANAGE), false, Some(&sh), "group:household", MANAGE).is_err());
}

#[test]
fn local_groups_are_pf_only_and_removal_bumps_at_once() {
    let w = world();
    let a = w.person("alice", true);
    let b = w.person("bob", true);
    let p = w.part("x", &a, "sp_ALICE");
    let q = w.part("y", &a, "sp_ALICE");
    let admin = Actor::person(&a);
    // A local group not named pf-* is refused by the API and by the schema.
    let e = w.with(|tx| create_local(tx, &admin, "family", 1).unwrap_err());
    assert_eq!(e.code, "invalid_input");
    let schema = w.with(|tx| {
        tx.execute(
            "INSERT INTO groups (name, source, managed, synced_at) VALUES ('family', 'local', 1, 0)",
            [],
        )
    });
    assert!(schema.is_err(), "schema rejects a local group outside pf-*");
    w.with(|tx| create_local(tx, &admin, "pf-photos", 1).unwrap());
    w.grant("part", &p, "group:pf-photos", VIEW, None);
    w.with(|tx| set_local_member(tx, &admin, "pf-photos", &b, true, 2).unwrap());
    let bumped = w.with(|tx| set_local_member(tx, &admin, "pf-photos", &b, false, 3).unwrap());
    // Only the part the group reaches.
    assert_eq!(bumped, vec![(format!("part:{p}"), 1)]);
    let _ = q;
    assert_eq!(
        w.count("SELECT COUNT(*) FROM audit WHERE target_kind = 'group'"),
        3
    );
}
