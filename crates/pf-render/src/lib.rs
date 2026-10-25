//! Optional browser checks. Every render gets a fresh context and a discarded storage overlay.
mod driver;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use pf_db::rusqlite::{OptionalExtension, params};
use pf_model::{PartService, PfError, Principal};
use pf_ops::CheckRequest;
use pf_parthost::PartHost;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{OnceCell, Semaphore};

#[derive(Clone, Debug)]
pub struct Config {
    pub enabled: bool,
    /// Chrome CDP origin, reachable only on the render network (loopback in the Mac harness).
    pub endpoint: String,
    /// The sidecar's on-demand process controller. None when Chrome is externally supervised.
    pub control_endpoint: Option<String>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://partifactory-render:9222".into(),
            control_endpoint: Some("http://partifactory-render:9223".into()),
        }
    }
}

type Job = Arc<OnceCell<Result<Value, PfError>>>;
struct Inner {
    parts: PartService,
    host: PartHost,
    cfg: Config,
    slots: Semaphore,
    jobs: Mutex<HashMap<String, Job>>,
    breaker: Mutex<(u32, Option<Instant>)>,
    rates: pf_serve::RateTable,
}
#[derive(Clone)]
pub struct RenderService(Arc<Inner>);
struct OverlayGuard {
    host: PartHost,
    id: String,
}
struct JobGuard {
    db: pf_db::Db,
    id: i64,
    purpose: String,
    finished: bool,
}
impl Drop for JobGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        tracing::warn!(event="render.job",purpose=%self.purpose,outcome="timeout");
        let db = self.db.clone();
        let id = self.id;
        tokio::spawn(async move {
            let _=db.write(move |tx| {
                tx.execute("UPDATE render_jobs SET status='failed',finished_at=?2 WHERE id=?1 AND status='running'",params![id,pf_db::ids::now_ms()])?;
                Ok::<_,PfError>(())
            }).await;
        });
    }
}
impl Drop for OverlayGuard {
    fn drop(&mut self) {
        self.host.end_render(&self.id);
    }
}

fn unavailable() -> PfError {
    PfError::new(
        "renderer_unavailable",
        "The renderer is disabled or unavailable.",
    )
    .with("hint", "enable the render profile and check sidecar health")
}
impl RenderService {
    pub fn new(parts: PartService, host: PartHost, cfg: Config) -> Self {
        pf_obs::metrics::metrics().render_up.set(0);
        let rates = pf_serve::RateTable::new(true);
        rates.register(pf_serve::RateRule::per_principal(
            "part_check",
            "api",
            30,
            Duration::from_secs(3600),
        ));
        let service = Self(Arc::new(Inner {
            parts,
            host,
            cfg,
            slots: Semaphore::new(1),
            jobs: Mutex::new(HashMap::new()),
            breaker: Mutex::new((0, None)),
            rates,
        }));
        if service.enabled() {
            let weak = Arc::downgrade(&service.0);
            std::thread::Builder::new()
                .name("render-health".into())
                .spawn(move || {
                    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    else {
                        return;
                    };
                    runtime.block_on(async move {
                        let client = reqwest::Client::new();
                        while let Some(inner) = weak.upgrade() {
                            let endpoint = inner
                                .cfg
                                .control_endpoint
                                .as_ref()
                                .map(|c| format!("{}/health", c.trim_end_matches('/')));
                            let up = if let Some(endpoint) = endpoint {
                                client
                                    .get(endpoint)
                                    .timeout(Duration::from_secs(2))
                                    .send()
                                    .await
                                    .ok()
                                    .filter(|r| r.status().is_success())
                                    .is_some()
                            } else {
                                client
                                    .get(format!("{}/json/version", inner.cfg.endpoint))
                                    .header(reqwest::header::HOST, "localhost")
                                    .timeout(Duration::from_secs(2))
                                    .send()
                                    .await
                                    .is_ok_and(|r| r.status().is_success())
                            };
                            pf_obs::metrics::metrics().render_up.set(i64::from(up));
                            drop(inner);
                            tokio::time::sleep(Duration::from_secs(15)).await;
                        }
                    });
                })
                .expect("start renderer health monitor");
        }
        service
    }
    pub fn enabled(&self) -> bool {
        self.0.cfg.enabled
    }
    pub async fn check(
        &self,
        principal: &Principal,
        request: CheckRequest,
        data_read: bool,
    ) -> Result<Value, PfError> {
        let started = tokio::time::Instant::now();
        request.validate()?;
        if !self.enabled() {
            return Err(unavailable());
        }
        if let Err(wait) = self.0.rates.check("part_check", &principal.key()) {
            tracing::warn!(
                event = "ratelimit.trip",
                route = "part_check",
                route_class = "api",
                outcome = "rate_limited"
            );
            return Err(PfError::new("rate_limited", "At most 30 checks per hour.")
                .with("retry_after", wait.as_secs().saturating_add(1)));
        }
        let mut part = self
            .0
            .parts
            .get(
                principal,
                pf_model::GetRequest {
                    name: request.name.clone(),
                    version: request.version.clone(),
                    ..Default::default()
                },
            )
            .await?;
        let n = part["version"].as_i64().unwrap_or(0);
        let deadline = started + Duration::from_secs(14);
        while matches!(part["build"]["status"].as_str(), Some("queued" | "running"))
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(40)).await;
            part = self
                .0
                .parts
                .get(
                    principal,
                    pf_model::GetRequest {
                        name: request.name.clone(),
                        version: Some(json!(n)),
                        ..Default::default()
                    },
                )
                .await?;
        }
        if part["build"]["status"] != "ok" {
            return Ok(
                json!({"name":request.name,"url":"/","version":n,"head":part["head"],"live":part["live"],"build":part["build"],"screenshot":null,"blank":null,"height":null,"errors":[],"console":[],"failed_requests":[],"actions":[],"timing":{"ready_ms":null,"total_ms":0},"passed":false}),
            );
        }
        let version = self.version_id(&part["id"], n).await?;
        let mut result = tokio::time::timeout_at(
            started + Duration::from_secs(15),
            self.join_render(
                &part,
                &version,
                &request,
                data_read,
                "check",
                &principal.key(),
            ),
        )
        .await
        .map_err(|_| unavailable())??;
        self.authorize_screenshot(principal, &result).await?;
        if request.include_base64
            && let Some(id) = result["screenshot"]["id"].as_str()
        {
            result["screenshot"]["base64"] =
                json!(STANDARD.encode(self.screenshot(principal, id).await?));
        }
        Ok(result)
    }

    async fn version_id(&self, part: &Value, n: i64) -> Result<String, PfError> {
        let part = part.as_str().unwrap_or("").to_string();
        self.0
            .parts
            .db
            .read(move |c| {
                c.query_row(
                    "SELECT id FROM versions WHERE part_id=?1 AND n=?2",
                    params![part, n],
                    |r| r.get(0),
                )
                .map_err(PfError::from)
            })
            .await
    }
    async fn join_render(
        &self,
        part: &Value,
        version: &str,
        request: &CheckRequest,
        data_read: bool,
        purpose: &str,
        actor: &str,
    ) -> Result<Value, PfError> {
        let key = format!(
            "{}:{}:{}:{}:{}",
            part["id"],
            version,
            request.viewport.as_deref().unwrap_or("mobile"),
            data_read,
            pf_model::idem::args_hash(
                "render",
                &json!({"actions":request.actions,"wait_ms":request.wait_ms.unwrap_or(8000)})
            )
        );
        let cell = self
            .0
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key.clone())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();
        let result = cell
            .get_or_init(|| self.render(part, version, request, data_read, purpose, actor))
            .await
            .clone();
        let service = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let mut jobs = service.0.jobs.lock().unwrap_or_else(|e| e.into_inner());
            if jobs.get(&key).is_some_and(|j| Arc::ptr_eq(j, &cell)) {
                jobs.remove(&key);
            }
        });
        result
    }
    async fn render(
        &self,
        part: &Value,
        version: &str,
        request: &CheckRequest,
        data_read: bool,
        purpose: &str,
        actor: &str,
    ) -> Result<Value, PfError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        {
            let breaker = self.0.breaker.lock().unwrap_or_else(|e| e.into_inner());
            if breaker.1.is_some_and(|until| Instant::now() < until) {
                return Err(unavailable());
            }
        }
        let _permit = tokio::time::timeout_at(deadline, self.0.slots.acquire())
            .await
            .map_err(|_| unavailable())?
            .map_err(|_| unavailable())?;
        let (part_id, version_id, viewport, purpose_owned, actor_owned, request_json, now) = (
            part["id"].as_str().unwrap_or("").to_string(),
            version.to_string(),
            request.viewport.clone().unwrap_or_else(|| "mobile".into()),
            purpose.to_string(),
            actor.to_string(),
            serde_json::to_string(request).map_err(|e| PfError::internal(e.to_string()))?,
            self.0.parts.now(),
        );
        let id=self.0.parts.db.write(move|tx| {tx.execute("INSERT INTO render_jobs(part_id,version_id,viewport,purpose,requested_by,request,status,queued_at) VALUES(?1,?2,?3,?4,?5,?6,'running',?7)",params![part_id,version_id,viewport,purpose_owned,actor_owned,request_json,now])?;Ok::<_,PfError>(tx.last_insert_rowid())}).await?;
        let mut job = JobGuard {
            db: self.0.parts.db.clone(),
            id,
            purpose: purpose.into(),
            finished: false,
        };
        let minter = format!("render:{id}");
        self.0
            .host
            .begin_render(&minter, part["id"].as_str().unwrap_or(""), data_read)
            .await?;
        let _overlay = OverlayGuard {
            host: self.0.host.clone(),
            id: minter.clone(),
        };
        let grant = pf_caps::Grant {
            part_id: part["id"].as_str().unwrap_or("").into(),
            host: request.name.clone(),
            subject: "render".into(),
            role: 0,
            read_only: true,
            data_write: false,
            guest: None,
            minter,
            version: Some(version[..32.min(version.len())].into()),
            draft: None,
        };
        let cap = pf_caps::mint(
            &self.0.host.deps().keys,
            &self.0.host.deps().epochs,
            &grant,
            self.0.host.now(),
        )
        .map_err(|_| PfError::internal("cannot mint render capability"))?;
        tracing::info!(event = "cap.mint", subject = "render");
        let url = format!(
            "{}/c/{cap}/",
            self.0.host.config().part_origin(&request.name)
        );
        let mut outcome = Err(unavailable());
        for _ in 0..2 {
            if let Some(control) = &self.0.cfg.control_endpoint {
                let started = tokio::time::timeout_at(
                    deadline,
                    reqwest::Client::new()
                        .post(format!("{}/start", control.trim_end_matches('/')))
                        .timeout(Duration::from_secs(3))
                        .send(),
                )
                .await;
                if !started
                    .as_ref()
                    .is_ok_and(|r| r.as_ref().is_ok_and(|r| r.status().is_success()))
                {
                    outcome = Err(unavailable());
                    continue;
                }
            }
            outcome =
                tokio::time::timeout_at(deadline, driver::run(&self.0.cfg.endpoint, &url, request))
                    .await
                    .map_err(|_| unavailable())
                    .and_then(|r| r);
            if outcome.is_ok() {
                break;
            }
        }
        let output = match outcome {
            Ok(output) => {
                pf_obs::metrics::metrics().render_up.set(1);
                *self.0.breaker.lock().unwrap_or_else(|e| e.into_inner()) = (0, None);
                output
            }
            Err(error) => {
                pf_obs::metrics::metrics().render_up.set(0);
                {
                    let mut breaker = self.0.breaker.lock().unwrap_or_else(|e| e.into_inner());
                    breaker.0 += 1;
                    if breaker.0 >= 3 {
                        breaker.1 = Some(Instant::now() + Duration::from_secs(30));
                    }
                }
                self.0
                    .parts
                    .db
                    .write(move |tx| {
                        tx.execute(
                            "UPDATE render_jobs SET status='failed',finished_at=?2 WHERE id=?1",
                            params![id, pf_db::ids::now_ms()],
                        )?;
                        Ok::<_, PfError>(())
                    })
                    .await?;
                tracing::warn!(event = "render.job", purpose, outcome = "unavailable");
                job.finished = true;
                return Err(error);
            }
        };
        let blob = self.0.parts.blobs.put(&output.png)?;
        let hash = blob.hash.clone();
        let bytes = blob.size;
        let mut result = output.result;
        result["name"] = json!(request.name);
        result["url"] = json!("/");
        result["version"] = part["version"].clone();
        result["head"] = part["head"].clone();
        result["live"] = part["live"].clone();
        result["build"] = part["build"].clone();
        let width = result
            .as_object_mut()
            .expect("result")
            .remove("width")
            .unwrap_or(json!(780));
        let height = result
            .as_object_mut()
            .expect("result")
            .remove("height_pixels")
            .unwrap_or(json!(1688));
        result["screenshot"] = json!({"id":id.to_string(),"url":format!("{}/api/v1/screenshots/{id}",self.0.host.config().workspace_origin()),"expires_at":pf_model::read::iso(self.0.parts.now()+600000),"width":width,"height":height});
        let (stored, pid, vid, passed, now) = (
            result.clone(),
            part["id"].as_str().unwrap_or("").to_string(),
            version.to_string(),
            result["passed"] == true,
            self.0.parts.now(),
        );
        self.0.parts.db.write(move|tx| {
            pf_model::version::record_blob(tx,&hash,bytes,now)?;
            let mut stored=stored;stored["screenshot_hash"]=json!(hash);
            tx.execute("UPDATE render_jobs SET status=?2,result=?3,finished_at=?4 WHERE id=?1",params![id,if passed{"ok"}else{"failed"},stored.to_string(),now])?;
            if passed {tx.execute("INSERT INTO part_previews(part_id,version_id,source,poster_hash,state,updated_at) SELECT ?1,?2,'render',?3,'ready',?4 WHERE EXISTS(SELECT 1 FROM parts WHERE id=?1 AND live_version_id=?2) ON CONFLICT(part_id) DO UPDATE SET version_id=excluded.version_id,source=excluded.source,poster_hash=excluded.poster_hash,state=excluded.state,updated_at=excluded.updated_at",params![pid,vid,hash,now])?;}
            Ok::<_,PfError>(())
        }).await?;
        tracing::info!(
            event = "render.job",
            purpose,
            outcome = if passed { "passed" } else { "failed" }
        );
        job.finished = true;
        Ok(result)
    }
    /// Called after the build queue has switched live; the gate's render predates that switch.
    pub async fn promote_preview(&self, part: String) -> Result<(), PfError> {
        let now = self.0.parts.now();
        self.0.parts.db.write(move |tx| {
            let candidate:Option<(String,String)>=tx.query_row(
                "SELECT r.version_id,r.result FROM render_jobs r JOIN parts p ON p.id=r.part_id AND p.live_version_id=r.version_id WHERE p.id=?1 AND r.status='ok' ORDER BY r.finished_at DESC LIMIT 1",
                [&part],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((version,result))=candidate {
                let result:Value=serde_json::from_str(&result).map_err(|e|PfError::internal(e.to_string()))?;
                if let Some(hash)=result["screenshot_hash"].as_str() {
                    tx.execute("INSERT INTO part_previews(part_id,version_id,source,poster_hash,state,updated_at) VALUES(?1,?2,'render',?3,'ready',?4) ON CONFLICT(part_id) DO UPDATE SET version_id=excluded.version_id,source=excluded.source,poster_hash=excluded.poster_hash,state=excluded.state,updated_at=excluded.updated_at",params![part,version,hash,now])?;
                }
            }
            Ok(())
        }).await
    }
    async fn authorize_screenshot(&self, p: &Principal, result: &Value) -> Result<(), PfError> {
        let Some(id) = result["screenshot"]["id"].as_str() else {
            return Ok(());
        };
        let key = format!("screenshot:{id}:{}", p.key());
        self.0
            .parts
            .db
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO kv(key,value) VALUES(?1,'1') ON CONFLICT(key) DO NOTHING",
                    [key],
                )?;
                Ok::<_, PfError>(())
            })
            .await
    }
    pub async fn screenshot(&self, principal: &Principal, id: &str) -> Result<Vec<u8>, PfError> {
        let id = id
            .parse::<i64>()
            .map_err(|_| PfError::not_found("screenshot not found"))?;
        let p = principal.key();
        let now = self.0.parts.now();
        let hash=self.0.parts.db.read(move|c| {
            let result:Option<String>=c.query_row("SELECT result FROM render_jobs WHERE id=?1 AND finished_at>?2 AND (requested_by=?3 OR EXISTS(SELECT 1 FROM kv WHERE key=?4))",params![id,now-600000,p,format!("screenshot:{id}:{p}")],|r|r.get(0)).optional()?.flatten();
            let result:Value=serde_json::from_str(&result.ok_or_else(||PfError::not_found("screenshot not found"))?).map_err(|e|PfError::internal(e.to_string()))?;
            result["screenshot_hash"].as_str().map(str::to_string).ok_or_else(||PfError::not_found("screenshot not found"))
        }).await?;
        self.0.parts.blobs.get(&hash).map_err(PfError::from)
    }
}
impl pf_ops::Extension for RenderService {
    fn run<'a>(
        &'a self,
        ctx: &'a pf_ops::Ctx,
        name: &'a str,
        input: Value,
    ) -> pf_ops::OpFuture<'a> {
        Box::pin(async move {
            if name != "part_check" {
                return Err(PfError::not_found("unknown feature operation"));
            }
            let request = serde_json::from_value(input)
                .map_err(|e| PfError::invalid("body", e.to_string()))?;
            self.check(
                &ctx.caller.principal,
                request,
                ctx.scopes.contains("data:read"),
            )
            .await
        })
    }
}
impl pf_build::queue::CheckGate for RenderService {
    fn check(
        &self,
        part: String,
        build: i64,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<pf_build::queue::Gate, PfError>> + Send>,
    > {
        let service = self.clone();
        Box::pin(async move {
            if !service.enabled() {
                return Ok(pf_build::queue::Gate::Skipped);
            }
            let (name,version,n,owner)=service.0.parts.db.read(move|c|c.query_row("SELECT p.name,v.id,v.n,p.owner_user_id FROM builds b JOIN versions v ON v.id=b.version_id JOIN parts p ON p.id=v.part_id WHERE b.id=?1 AND p.id=?2",params![build,part],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?))).map_err(PfError::from)).await?;
            let principal = Principal::person(owner);
            let part = service
                .0
                .parts
                .get(
                    &principal,
                    pf_model::GetRequest {
                        name: name.clone(),
                        version: Some(json!(n)),
                        ..Default::default()
                    },
                )
                .await?;
            let request = CheckRequest {
                name,
                version: Some(json!(n)),
                viewport: None,
                wait_ms: None,
                actions: None,
                include_base64: false,
            };
            match service
                .join_render(&part, &version, &request, true, "gate", "gate")
                .await
            {
                Ok(result) => Ok(if result["passed"] == true {
                    pf_build::queue::Gate::Passed
                } else {
                    pf_build::queue::Gate::Failed
                }),
                Err(_) => Ok(pf_build::queue::Gate::Skipped),
            }
        })
    }
}
