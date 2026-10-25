use crate::{PartHost, content};
use axum::{
    extract::Request,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
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
    if c.s == "render" {
        return http::StatusCode::FORBIDDEN.into_response();
    }
    let unsandboxed = match s.live(&c.h, c.v.as_deref()).await {
        Ok(Some(l)) if l.part_id == c.p => l.unsandboxed,
        Ok(None) if c.v.is_none() => {
            let part = c.p.clone();
            let name = c.h.clone();
            match s.read(move|db| {
                use pf_db::rusqlite::OptionalExtension;
                Ok(db.query_row("SELECT isolation='unsandboxed' FROM parts WHERE id=?1 AND name=?2 AND deleted_at IS NULL",pf_db::rusqlite::params![part,name],|r|r.get::<_,bool>(0)).optional()?)
            }).await {
                Ok(Some(unsandboxed))=>unsandboxed,
                _=>return http::StatusCode::NOT_FOUND.into_response(),
            }
        }
        _ => return http::StatusCode::NOT_FOUND.into_response(),
    };
    if !crate::rt::origin_allowed(&s, &req, &c, unsandboxed) {
        return http::StatusCode::FORBIDDEN.into_response();
    }
    let rx = s.events().subscribe(&c.p);
    let data_rx = s.data_events().subscribe(&c.p);
    let part = c.p.clone();
    let now = s.now();
    let _=s.write(move|tx| {tx.execute("INSERT INTO kv(key,value) VALUES(?1,CAST(?2 AS TEXT)) ON CONFLICT(key) DO UPDATE SET value=excluded.value",pf_db::rusqlite::params![format!("data.opened:{part}"),now])?; Ok(())}).await;
    let stream = futures_util::stream::unfold(
        (rx, data_rx, c, s),
        |(mut rx, mut data_rx, c, s)| async move {
            if s.now() / 1000 >= c.w * 60
                || c.expires_at.is_some_and(|expiry| s.now() >= expiry)
                || c.e < s.deps().epochs.part_epoch(&c.p)
                || c.me < s.deps().epochs.get(&pf_caps::minter_key(&c.m))
                || c.b < s.deps().epochs.boot_floor()
            {
                return None;
            }
            tokio::select! {
                result=rx.recv()=>match result {
                    Ok(mut e)=>{
                        if e.kind=="pf:version" {
                            let part=c.p.clone();
                            e.data=s.read(move|db| {
                                let (n,live):(i64,Option<i64>)=db.query_row("SELECT head_n,(SELECT n FROM versions WHERE id=parts.live_version_id) FROM parts WHERE id=?1",[part],|r|Ok((r.get(0)?,r.get(1)?)))?;
                                Ok(serde_json::json!({"n":n,"live":live}))
                            }).await.unwrap_or_else(|_|serde_json::json!({}));
                        }
                        Some((Ok::<_,Infallible>(Event::default().event(e.kind).data(e.data.to_string())),(rx,data_rx,c,s)))
                    },
                    Err(broadcast::error::RecvError::Lagged(_))=>Some((Ok(Event::default().event("pf:resync").data("{}")),(rx,data_rx,c,s))),
                    Err(_)=>None,
                },
                result=data_rx.recv()=>match result {
                    Ok(e) if e.ty=="data.changed"=>{
                        tracing::info!(event="sse.deliver",r#type=%e.ty,part_id=%c.p);
                        Some((Ok(Event::default().id(e.seq.to_string()).event(e.ty).data(e.payload.to_string())),(rx,data_rx,c,s)))
                    },
                    Ok(_)=>Some((Ok(Event::default().comment("keepalive")),(rx,data_rx,c,s))),
                    Err(broadcast::error::RecvError::Lagged(_))=>Some((Ok(Event::default().event("pf:resync").data("{}")),(rx,data_rx,c,s))),
                    Err(_)=>None,
                },
                _=tokio::time::sleep(Duration::from_secs(25))=>Some((Ok(Event::default().comment("keepalive")),(rx,data_rx,c,s)))
            }
        },
    );
    Sse::new(stream).into_response()
}
