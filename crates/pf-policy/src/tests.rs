use crate::runtime::{PartFlags, RtOp, RtSubject, SharedDataWriters, allows};
use crate::*;
use pf_model::Principal;
use proptest::prelude::*;

#[path = "../tests/common/mod.rs"]
mod common;
use common::*;

fn decide(w: &World, p: &Principal, part: &str, now: i64) -> Option<u8> {
    w.with(|c| Engine::decide(c, p, part, now).unwrap())
}

#[test]
fn owner_manages_and_strangers_get_nothing() {
    let w = world();
    let a = w.person("alice", true);
    let b = w.person("bob", true);
    let p = w.part("notes", &a, "sp_ALICE");
    assert_eq!(decide(&w, &Principal::person(&a), &p, 1), Some(MANAGE));
    assert_eq!(decide(&w, &Principal::person(&b), &p, 1), None);
    assert_eq!(decide(&w, &Principal::person(&b), "p_NOPE", 1), None);
}

#[test]
fn effective_role_is_max_of_part_and_space_grants_live() {
    let w = world();
    let a = w.person("alice", true);
    let b = w.person("bob", true);
    let fam = w.shared_space("family");
    let p = w.part("list", &a, &fam);
    w.claim_group("household", &[&b]);
    w.grant("part", &p, &format!("user:{b}"), VIEW, None);
    assert_eq!(decide(&w, &Principal::person(&b), &p, 1), Some(VIEW));
    // A space grant added later applies at once, and the higher role wins.
    w.grant("space", &fam, "group:household", EDIT, None);
    assert_eq!(decide(&w, &Principal::person(&b), &p, 1), Some(EDIT));
}

#[test]
fn agents_never_exceed_their_person_or_their_spaces() {
    let w = world();
    let a = w.person("alice", true);
    let b = w.person("bob", true);
    let fam = w.shared_space("family");
    let p_fam = w.part("list", &b, &fam);
    let p_mine = w.part("mine", &a, "sp_ALICE");
    w.grant("part", &p_fam, &format!("user:{a}"), COMMENT, None);
    // Consent covers family only, with write scopes.
    w.agent("c_ONE", &a, &["parts:read", "parts:write"], &["family"]);
    let ag = Principal::agent(&a, "c_ONE");
    // The person has comment: the agent can't exceed it despite parts:write.
    assert_eq!(decide(&w, &ag, &p_fam, 1), Some(COMMENT));
    // Outside the consented spaces: nothing, even on the person's own part.
    assert_eq!(decide(&w, &ag, &p_mine, 1), None);
    // Read-only scopes cap at view.
    w.agent("c_TWO", &a, &["parts:read"], &["~alice", "family"]);
    let ro = Principal::agent(&a, "c_TWO");
    assert_eq!(decide(&w, &ro, &p_mine, 1), Some(VIEW));
    // Someone else's connection is refused.
    assert_eq!(decide(&w, &Principal::agent(&b, "c_TWO"), &p_fam, 1), None);
}

#[test]
fn expired_grant_stops_at_expiry() {
    let w = world();
    let a = w.person("alice", true);
    let b = w.person("bob", true);
    let p = w.part("x", &a, "sp_ALICE");
    w.grant("part", &p, &format!("user:{b}"), EDIT, Some(1000));
    assert_eq!(decide(&w, &Principal::person(&b), &p, 999), Some(EDIT));
    assert_eq!(decide(&w, &Principal::person(&b), &p, 1000), None);
}

#[test]
fn everyone_signed_in_is_the_audience_not_every_account() {
    let w = world();
    let a = w.person("alice", true);
    let out = w.person("outsider", false);
    let local = w.local_person("granny");
    let p = w.part("x", &a, "sp_ALICE");
    w.grant("part", &p, "everyone-signed-in", VIEW, None);
    assert_eq!(decide(&w, &Principal::person(&out), &p, 1), None);
    assert_eq!(decide(&w, &Principal::person(&local), &p, 1), Some(VIEW));
}

#[test]
fn rt_matrix_table() {
    use RtOp::*;
    let ops = [
        PersonalState,
        SharedRead,
        SharedWrite,
        Comment,
        SelfPublish,
        SourceEdit,
        AssetUpload,
        Backend,
    ];
    // Rows from §3a, columns in `ops` order; `viewers` = shared_data_writers "viewers".
    let rows: &[(RtSubject, SharedDataWriters, bool, [bool; 8])] = &[
        (
            RtSubject::LinkViewer,
            SharedDataWriters::Viewers,
            false,
            [false, true, false, false, false, false, false, true],
        ),
        (
            RtSubject::LinkViewer,
            SharedDataWriters::Viewers,
            true,
            [false, true, false, true, false, false, false, true],
        ),
        (
            RtSubject::LinkGuest,
            SharedDataWriters::Viewers,
            true,
            [false, true, true, false, false, false, false, true],
        ),
        (
            RtSubject::LinkGuest,
            SharedDataWriters::Editors,
            false,
            [false, true, true, false, false, false, false, true],
        ),
        (
            RtSubject::Role(VIEW),
            SharedDataWriters::Viewers,
            false,
            [true, true, true, false, false, false, false, true],
        ),
        (
            RtSubject::Role(VIEW),
            SharedDataWriters::Editors,
            false,
            [true, true, false, false, false, false, false, true],
        ),
        (
            RtSubject::Role(COMMENT),
            SharedDataWriters::Viewers,
            false,
            [true, true, true, true, false, false, true, true],
        ),
        (
            RtSubject::Role(COMMENT),
            SharedDataWriters::Editors,
            false,
            [true, true, false, true, false, false, true, true],
        ),
        (
            RtSubject::Role(EDIT),
            SharedDataWriters::Editors,
            false,
            [true, true, true, true, true, true, true, true],
        ),
        (
            RtSubject::Role(MANAGE),
            SharedDataWriters::Editors,
            false,
            [true, true, true, true, true, true, true, true],
        ),
    ];
    for (subj, sdw, link_comments, want) in rows {
        let flags = PartFlags {
            shared_data_writers: *sdw,
            link_comments: *link_comments,
        };
        for (op, w) in ops.iter().zip(want) {
            assert_eq!(allows(*subj, *op, flags), *w, "{subj:?} {op:?} {flags:?}");
        }
    }
    assert_eq!(runtime::backend_role(RtSubject::LinkGuest), "guest");
    assert_eq!(runtime::backend_role(RtSubject::Role(VIEW)), "view");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// No subject gets a role above its best matching grant (or manage for the owner).
    #[test]
    fn prop_no_role_above_best_grant(
        grants in proptest::collection::vec((0u8..4, 0u8..4, 0u8..4, proptest::option::of(0i64..20)), 0..8),
        now in 0i64..20,
        member in any::<bool>(),
    ) {
        let w = world();
        let a = w.person("alice", true);
        let b = w.person("bob", true);
        let fam = w.shared_space("family");
        let p = w.part("x", &a, &fam);
        if member { w.claim_group("household", &[&b]); }
        let mut best: Option<u8> = None;
        let mut seen = std::collections::HashSet::new();
        for (who, target, role, exp) in grants {
            let subject = match who { 0 => format!("user:{b}"), 1 => "group:household".into(), 2 => "everyone-signed-in".into(), _ => format!("user:{a}") };
            let (kind, tid) = if target % 2 == 0 { ("part", p.clone()) } else { ("space", fam.clone()) };
            if !seen.insert((kind, subject.clone())) { continue; }
            w.grant(kind, &tid, &subject, role, exp);
            let matches = match who { 0 | 2 => true, 1 => member, _ => false };
            let live = exp.is_none_or(|e| e > now);
            if matches && live { best = best.max(Some(role)); }
        }
        prop_assert_eq!(decide(&w, &Principal::person(&b), &p, now), best);
        prop_assert_eq!(decide(&w, &Principal::person(&a), &p, now), Some(MANAGE));
    }

    /// A view grantee writes shared data only while the part is `viewers`; a Can-view guest never.
    #[test]
    fn prop_shared_write_rules(role in 0u8..4, editors in any::<bool>(), link_comments in any::<bool>()) {
        let flags = PartFlags {
            shared_data_writers: if editors { SharedDataWriters::Editors } else { SharedDataWriters::Viewers },
            link_comments,
        };
        prop_assert!(!allows(RtSubject::LinkViewer, RtOp::SharedWrite, flags));
        let w = allows(RtSubject::Role(role), RtOp::SharedWrite, flags);
        prop_assert_eq!(w, role >= EDIT || !editors);
    }
}
