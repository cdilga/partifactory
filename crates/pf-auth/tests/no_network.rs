//! Sign in as the fixture people and groups with no network at all, and check the two ways a
//! refresh can fail: `invalid_grant` fails closed, an unreachable IdP keeps the last membership.

use pf_auth::*;
use std::sync::{Arc, Mutex};

fn now() -> i64 {
    pf_db::ids::now_ms()
}

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn fixture_people_and_groups_sign_in() {
    let db = pf_db::temp_db();
    let dir = StaticDirectory::repo_fixture();
    assert_eq!(dir.group_names(), ["household", "adults", "kids"]);
    assert_eq!(
        dir.usernames(),
        ["admin", "member-a", "member-b", "outsider", "disabled"]
    );

    let admin = TestSession::sign_in(&db, &dir, "admin").await.unwrap();
    assert!(admin.principal.starts_with("user:u_"));
    assert_eq!(
        session_user(&db, &admin.token, now()).await.unwrap(),
        Some(admin.user_id.clone())
    );
    assert_eq!(
        groups_of(&db, &admin.user_id).await.unwrap(),
        ["adults", "household"]
    );

    let kid = TestSession::sign_in(&db, &dir, "member-b").await.unwrap();
    assert_eq!(
        groups_of(&db, &kid.user_id).await.unwrap(),
        ["household", "kids"]
    );
    let out = TestSession::sign_in(&db, &dir, "outsider").await.unwrap();
    assert!(groups_of(&db, &out.user_id).await.unwrap().is_empty());

    // The disabled account can't sign in; an unknown one neither.
    assert!(matches!(
        TestSession::sign_in(&db, &dir, "disabled").await,
        Err(AuthError::Directory(DirectoryError::Disabled))
    ));
    assert!(matches!(
        TestSession::sign_in(&db, &dir, "nobody").await,
        Err(AuthError::Directory(DirectoryError::UnknownUser))
    ));

    // Signing in again keeps the same user row; a wrong token has no session.
    let again = TestSession::sign_in(&db, &dir, "admin").await.unwrap();
    assert_eq!(again.user_id, admin.user_id);
    assert_eq!(session_user(&db, "not-a-token", now()).await.unwrap(), None);
    // The session value is stored only as its sha256.
    let tok = admin.token.clone();
    let stored: i64 = db
        .read(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM sessions WHERE id_hash = ?1",
                [secret_hash(&tok)],
                |r| r.get(0),
            )
            .map_err(pf_db::DbError::from)
        })
        .await
        .unwrap();
    assert_eq!(stored, 1);
}

#[tokio::test]
async fn session_created_is_logged_with_principal() {
    let db = pf_db::temp_db();
    let dir = StaticDirectory::repo_fixture();
    let buf = Buf::default();
    let w = buf.clone();
    let sub = tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_writer(move || w.clone())
        .finish();
    // A process-wide subscriber keeps callsite interest stable while other tests sign in on
    // their own threads. A thread-local subscriber could race a cached "no interest" decision.
    tracing::subscriber::set_global_default(sub).unwrap();
    tracing::callsite::rebuild_interest_cache();
    let s = TestSession::sign_in(&db, &dir, "member-a").await.unwrap();
    let logs = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    let line = logs
        .lines()
        .find(|l| l.contains("\"event\":\"session.created\"") && l.contains(&s.principal))
        .expect(&logs);
    assert!(
        line.contains(&format!("\"principal\":\"{}\"", s.principal)),
        "{line}"
    );
}

#[tokio::test]
async fn refresh_updates_groups() {
    let db = pf_db::temp_db();
    let dir = StaticDirectory::repo_fixture();
    let s = TestSession::sign_in(&db, &dir, "member-a").await.unwrap();
    dir.set_groups("member-a", &["household", "kids"]);
    let r = refresh_membership(&db, &dir, &s.user_id, now())
        .await
        .unwrap();
    assert_eq!(
        r,
        Refresh::Updated {
            groups: vec!["household".into(), "kids".into()]
        }
    );
    assert_eq!(
        groups_of(&db, &s.user_id).await.unwrap(),
        ["household", "kids"]
    );
}

#[tokio::test]
async fn failed_refresh_keeps_last_membership() {
    let db = pf_db::temp_db();
    let dir = StaticDirectory::repo_fixture();
    let s = TestSession::sign_in(&db, &dir, "member-a").await.unwrap();
    dir.set_groups("member-a", &[]);
    dir.set_unreachable(true);
    assert_eq!(
        refresh_membership(&db, &dir, &s.user_id, now())
            .await
            .unwrap(),
        Refresh::KeptLast
    );
    assert_eq!(
        groups_of(&db, &s.user_id).await.unwrap(),
        ["adults", "household"]
    );
    assert!(session_user(&db, &s.token, now()).await.unwrap().is_some());
}

#[tokio::test]
async fn invalid_grant_fails_closed() {
    let db = pf_db::temp_db();
    let dir = StaticDirectory::repo_fixture();
    let s = TestSession::sign_in(&db, &dir, "member-b").await.unwrap();
    dir.set_invalid_grant("member-b", true);
    let r = refresh_membership(&db, &dir, &s.user_id, now()).await;
    assert!(matches!(
        r,
        Err(AuthError::Directory(DirectoryError::InvalidGrant))
    ));
    assert_eq!(
        session_user(&db, &s.token, now()).await.unwrap(),
        None,
        "session revoked"
    );
}
