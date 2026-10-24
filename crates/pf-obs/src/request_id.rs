//! The request id: taken from a trusted ingress header or generated, echoed on the response, on
//! every log line and error page, and carried in a task-local so later code (API errors, the build
//! child, `/_rt`, backends: `X-PF-Request-Id`, §5d) can read it without re-plumbing.

use std::future::Future;

/// The header partifactory propagates to its own children and backends.
pub const PROPAGATION_HEADER: &str = "X-PF-Request-Id";

/// Longest request id accepted from an ingress header.
pub const MAX_LEN: usize = 128;

tokio::task_local! {
    static REQUEST_ID: String;
}

/// A fresh request id (32 lowercase hex characters).
pub fn generate() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Whether an ingress-supplied id is usable: 1 to 128 printable ASCII characters, no spaces.
pub fn is_acceptable(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_LEN && id.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// Run `fut` with `id` as the current request id.
pub async fn scope<F: Future>(id: String, fut: F) -> F::Output {
    REQUEST_ID.scope(id, fut).await
}

/// The current request id, inside a request's task (or a task spawned with [`propagate`]).
pub fn current() -> Option<String> {
    REQUEST_ID.try_with(Clone::clone).ok()
}

/// Wrap a future you are about to `tokio::spawn` so it keeps the current request id.
pub fn propagate<F: Future>(fut: F) -> impl Future<Output = F::Output> {
    let id = current();
    async move {
        match id {
            Some(id) => REQUEST_ID.scope(id, fut).await,
            None => fut.await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceptable_ids() {
        assert!(is_acceptable("abc-123"));
        assert!(is_acceptable(&"a".repeat(128)));
        assert!(!is_acceptable(&"a".repeat(129)));
        assert!(!is_acceptable(""));
        assert!(!is_acceptable("has space"));
        assert!(!is_acceptable("nl\n"));
        assert!(!is_acceptable("ünï"));
        assert_eq!(generate().len(), 32);
    }

    #[tokio::test]
    async fn request_id_task_local_propagates() {
        assert_eq!(current(), None);
        scope("rid-1".into(), async {
            assert_eq!(current().as_deref(), Some("rid-1"));
            let spawned = tokio::spawn(propagate(async { current() }));
            assert_eq!(spawned.await.unwrap().as_deref(), Some("rid-1"));
        })
        .await;
        assert_eq!(current(), None);
    }
}
