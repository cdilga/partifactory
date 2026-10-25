//! `everyone-signed-in` resolves to the `audience_group` members plus active local accounts, not
//! every IdP account, and its count matches that set.

mod common;
use common::*;
use pf_model::Principal;
use pf_policy::audience::{count, in_audience, members};
use pf_policy::{Engine, VIEW};

#[test]
fn audience_is_the_group_plus_local_accounts() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert!(in_audience(&s(&["household"]), "household"));
    assert!(!in_audience(&s(&["other-service"]), "household"));
    assert!(
        in_audience(&s(&[]), ""),
        "no audience group: everyone who signs in"
    );

    let w = world();
    let a = w.person("alice", true);
    let b = w.person("bob", true);
    let _out = w.person("outsider", false);
    let local = w.local_person("granny");
    let gone = w.person("gone", true);
    w.with(|tx| {
        tx.execute(
            "UPDATE users SET status = 'disabled' WHERE id = ?1",
            [&gone],
        )
        .unwrap()
    });
    let mut want = vec![a.clone(), b.clone(), local.clone()];
    want.sort();
    let got = w.with(|c| members(c).unwrap());
    assert_eq!(got, want);
    assert_eq!(w.with(|c| count(c).unwrap()), 3);

    // The engine's everyone-signed-in matches exactly that set.
    let p = w.part("board", &a, "sp_ALICE");
    w.grant("part", &p, "everyone-signed-in", VIEW, None);
    let all: Vec<String> = w.with(|c| {
        let mut st = c.prepare("SELECT id FROM users ORDER BY id").unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    });
    let mut allowed: Vec<String> = all
        .into_iter()
        .filter(|u| {
            w.with(|c| Engine::decide(c, &Principal::person(u), &p, 1).unwrap())
                .is_some()
        })
        .collect();
    allowed.sort();
    assert_eq!(allowed, want);
}
