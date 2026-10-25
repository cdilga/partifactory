use crate::{PartHost, content};
use axum::{
    extract::Request,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use serde_json::Value;
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub struct RtEvent {
    pub kind: String,
    pub data: Value,
}
impl RtEvent {
    pub fn version() -> Self {
        Self {
            kind: "pf:version".into(),
            data: serde_json::json!({}),
        }
    }
}

#[derive(Clone, Default)]
pub struct RtEvents(Arc<Mutex<HashMap<String, broadcast::Sender<RtEvent>>>>);
impl RtEvents {
    pub fn subscribe(&self, part: &str) -> broadcast::Receiver<RtEvent> {
        let mut channels = self.0.lock().unwrap_or_else(|e| e.into_inner());
        channels.retain(|_, tx| tx.receiver_count() > 0);
        channels
            .entry(part.into())
            .or_insert_with(|| broadcast::channel(256).0)
            .subscribe()
    }
    pub fn publish(&self, part: &str, event: RtEvent) {
        let mut channels = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = channels.get(part)
            && tx.send(event).is_err()
        {
            channels.remove(part);
        }
    }
}

pub async fn handle(s: PartHost, req: Request) -> Response {
    let (_, c) = match content::authorize(&s, &req) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let live = match s.live(&c.h, c.v.as_deref()).await {
        Ok(Some(l)) if l.part_id == c.p => l,
        _ => return http::StatusCode::NOT_FOUND.into_response(),
    };
    if !crate::rt::origin_allowed(&s, &req, &c, live.unsandboxed) {
        return http::StatusCode::FORBIDDEN.into_response();
    }
    let rx = s.events().subscribe(&c.p);
    let stream = futures_util::stream::unfold((rx, c, s), |(mut rx, c, s)| async move {
        if s.now() / 1000 >= c.w * 60
            || c.e < s.deps().epochs.part_epoch(&c.p)
            || c.me < s.deps().epochs.get(&pf_caps::minter_key(&c.m))
            || c.b < s.deps().epochs.boot_floor()
        {
            return None;
        }
        tokio::select! {
            result=rx.recv()=>match result {
                Ok(e)=>Some((Ok::<_,Infallible>(Event::default().event(e.kind).data(e.data.to_string())),(rx,c,s))),
                Err(broadcast::error::RecvError::Lagged(_))=>Some((Ok(Event::default().event("resync").data("{}")),(rx,c,s))),
                Err(_)=>None,
            },
            _=tokio::time::sleep(Duration::from_secs(15))=>Some((Ok(Event::default().comment("keepalive")),(rx,c,s)))
        }
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}
