//! `AppState`: what every handler can reach (`State<AppState>`, or the request extension the
//! access layer inserts). Later beads hang their services on it as typed extensions:
//!
//! ```ignore
//! let mut b = ServerBuilder::new(cfg);
//! b.ext(db.clone());                       // any Clone + Send + Sync + 'static
//! // in a handler:
//! async fn h(State(s): State<AppState>) { let db = s.ext::<Db>().expect("db"); }
//! ```

use crate::config::ServeConfig;
use crate::health::ReadyRegistry;
use crate::ratelimit::RateTable;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    config: ServeConfig,
    ready: ReadyRegistry,
    rates: RateTable,
    ext: http::Extensions,
    log: Option<pf_obs::LogHandle>,
}

impl AppState {
    pub(crate) fn new(
        config: ServeConfig,
        ready: ReadyRegistry,
        rates: RateTable,
        ext: http::Extensions,
        log: Option<pf_obs::LogHandle>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                ready,
                rates,
                ext,
                log,
            }),
        }
    }

    pub fn config(&self) -> &ServeConfig {
        &self.inner.config
    }

    pub fn ready(&self) -> &ReadyRegistry {
        &self.inner.ready
    }

    pub fn rates(&self) -> &RateTable {
        &self.inner.rates
    }

    /// A typed extension added with `ServerBuilder::ext`.
    pub fn ext<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.inner.ext.get::<T>()
    }

    /// The runtime log-level handle (`None` when logging was set up elsewhere, as in tests).
    pub fn log_handle(&self) -> Option<&pf_obs::LogHandle> {
        self.inner.log.as_ref()
    }
}
