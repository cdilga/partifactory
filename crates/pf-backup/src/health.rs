//! The admin view and readiness share the same registered probes.
use crate::BackupService;
use axum::{
    Form, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use pf_db::{DbError, rusqlite::params};
use pf_serve::{AppState, CheckKind, ServerBuilder};
use serde::Deserialize;
use std::sync::{Arc, atomic::Ordering};

pub fn install(builder: &mut ServerBuilder, service: BackupService, auth: pf_auth::AuthService) {
    *service.auth.write().unwrap_or_else(|e| e.into_inner()) = Some(auth.clone());
    let db = service.db.clone();
    builder.ready_check("db",CheckKind::Required,move||{let db=db.clone();async move{db.write(|tx|{tx.execute("INSERT INTO kv(key,value) VALUES('health.probe','ok') ON CONFLICT(key) DO UPDATE SET value='ok'",[])?;Ok::<_,DbError>(())}).await.map_err(|e|e.to_string())}});
    let root = service.blobs.root().to_path_buf();
    builder.ready_check("blobs", CheckKind::Required, move || {
        let root = root.clone();
        async move {
            tokio::task::spawn_blocking(move || {
                // A tiny fixed probe bypasses allocation policy: reaching the budget must not evict reads.
                let blobs = pf_blobs::BlobStore::open(root).map_err(|e| e.to_string())?;
                let bytes = b"partifactory readiness probe v1";
                let put = blobs.put(bytes).map_err(|e| e.to_string())?;
                if !blobs.has(&put.hash)
                    || blobs.get(&put.hash).map_err(|e| e.to_string())? != bytes
                {
                    return Err("Blob roundtrip failed".into());
                }
                Ok(())
            })
            .await
            .map_err(|e| e.to_string())?
        }
    });
    let idp = service.idp_ok.clone();
    builder.ready_check("idp", CheckKind::Degraded, move || {
        let idp = idp.clone();
        async move {
            if idp.load(Ordering::Relaxed) {
                Ok(())
            } else {
                Err("Identity provider unreachable; existing sessions continue.".into())
            }
        }
    });
    let db = service.db.clone();
    builder.ready_check("directory", CheckKind::Degraded, move || {
        let db = db.clone();
        async move {
            let stale = db
                .read(|conn| {
                    conn.query_row(
                        "SELECT COUNT(*) FROM kv WHERE key LIKE 'directory.stale:%'",
                        [],
                        |r| r.get::<_, i64>(0),
                    )
                    .map_err(DbError::from)
                })
                .await
                .map_err(|e| e.to_string())?;
            if stale == 0 {
                Ok(())
            } else {
                Err(format!(
                    "{stale} directory memberships await a successful refresh"
                ))
            }
        }
    });
    let backup = service.clone();
    builder.ready_check("backup", CheckKind::Degraded, move || {
        let backup = backup.clone();
        async move {
            tokio::task::spawn_blocking(move || {
                let now = pf_db::ids::now_ms();
                if backup
                    .value("backup.copy_error")
                    .map_err(|e| e.to_string())?
                    .is_some_and(|error| !error.is_empty())
                {
                    return Err(
                        "The last consistent database copy failed; the previous copy is preserved."
                            .into(),
                    );
                }
                let copy = backup
                    .value("backup.copy_at")
                    .map_err(|e| e.to_string())?
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0);
                let export = backup
                    .value("backup.export_at")
                    .map_err(|e| e.to_string())?
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0);
                if now.saturating_sub(copy) > 7_200_000 || now.saturating_sub(export) > 172_800_000
                {
                    Err("Consistent copy or scheduled export is stale".into())
                } else {
                    Ok(())
                }
            })
            .await
            .map_err(|e| e.to_string())?
        }
    });
    builder
        .readiness()
        .set_importing(service.data_dir.join(".importing").exists());
    builder.ext(service).ext(auth).workspace(
        Router::new()
            .route("/admin/health", get(page))
            .route("/admin/health/log-level", post(log_level))
            .route("/admin/health/run-now", post(run_now)),
    );
}
fn escape(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn row(html: &mut String, name: &str, state: &str, detail: &str) {
    let icon = match state {
        "ok" => "✓",
        "red" => "!",
        _ => "⚠",
    };
    html.push_str(&format!(
        "<tr data-health-row=\"{}\"><th>{}</th><td class=\"{}\">{} {}</td><td>{}</td></tr>",
        escape(name),
        escape(name),
        escape(state),
        icon,
        escape(state),
        escape(detail)
    ));
}
async fn page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let auth = state.ext::<pf_auth::AuthService>().expect("auth");
    if let Err(error) = auth.require_admin_session(&headers, false).await {
        return fail(error);
    }
    let service = state.ext::<BackupService>().expect("backup");
    let checks = state.ready().run().await;
    let mut html = String::from(
        "<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>Health · Partifactory</title><style>body{font:16px system-ui;margin:2rem auto;padding:0 1rem;max-width:72rem;color:#172330;background:#f4f6f8}table{border-collapse:collapse;width:100%;background:white}th,td{text-align:left;padding:.8rem;border-bottom:1px solid #dce1e6}th{width:24%}.ok{color:#146039}.amber{color:#815000}.red{color:#a51d32}input,button,select{font:inherit;padding:.5rem}h2{margin-top:2rem}</style><nav><a href='/'>Partifactory</a></nav><h1>Health</h1><h2>Readiness</h2><table>",
    );
    row(
        &mut html,
        "Ready",
        if checks.ready() { "ok" } else { "red" },
        checks.status,
    );
    for check in &checks.checks {
        row(
            &mut html,
            check.name,
            if check.ok {
                "ok"
            } else if check.kind == CheckKind::Required {
                "red"
            } else {
                "amber"
            },
            check.detail.as_deref().unwrap_or("Healthy"),
        );
    }
    html.push_str("</table><h2>Backup layers</h2><table>");
    let snapshots = format!("{:?}", service.config.host_snapshots);
    row(
        &mut html,
        "Host snapshots",
        if snapshots == "On" { "ok" } else { "amber" },
        &snapshots,
    );
    for (label, key) in [
        ("Hourly database copy", "backup.copy_at"),
        ("Newest export", "backup.export_at"),
        ("Export bytes", "backup.export_bytes"),
        ("Same disk", "backup.export_same_disk"),
    ] {
        let value = service.value(key).ok().flatten();
        let detail = if key.ends_with("_at") {
            value
                .as_ref()
                .and_then(|v| v.parse::<i64>().ok())
                .map(|at| {
                    format!(
                        "{} seconds ago",
                        pf_db::ids::now_ms().saturating_sub(at) / 1000
                    )
                })
                .unwrap_or_else(|| "Not yet recorded".into())
        } else {
            value.clone().unwrap_or_else(|| "Not yet recorded".into())
        };
        row(
            &mut html,
            label,
            if value.is_none()
                || (key == "backup.export_same_disk" && value.as_deref() == Some("true"))
            {
                "amber"
            } else {
                "ok"
            },
            &detail,
        );
    }
    match service
        .value("drill.last_result")
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str::<crate::DrillReport>(&value).ok())
    {
        Some(report) => {
            let age = pf_db::ids::now_ms().saturating_sub(report.at).max(0) / 1000;
            row(
                &mut html,
                "Last drill",
                if !report.ok {
                    "red"
                } else if age > 30 * 86_400 {
                    "amber"
                } else {
                    "ok"
                },
                &format!(
                    "{}; recovery {:.1} seconds; {} hours ago",
                    if report.ok { "Passed" } else { "Failed" },
                    report.rto_s,
                    age / 3600
                ),
            );
        }
        None => row(
            &mut html,
            "Last drill",
            "amber",
            "No recovery drill has been recorded",
        ),
    }
    html.push_str("</table><h2>Disk budget and stores</h2><table>");
    if let Some(budget) = state.ext::<Arc<pf_janitor::Budget>>() {
        let (used, cap) = budget.bytes();
        row(
            &mut html,
            "Disk budget",
            match budget.state() {
                "full" => "red",
                "warn" => "amber",
                _ => "ok",
            },
            &format!("{used} / {cap} bytes"),
        );
    }
    let rows = service
        .db
        .read(|conn| {
            let mut rows = vec![];
            for store in pf_janitor::stores::STORES {
                rows.push((
                    store.name.to_owned(),
                    store.size(conn)?,
                    store.cap(conn)?,
                    store.retention.to_owned(),
                ));
            }
            Ok::<_, DbError>(rows)
        })
        .await;
    match rows {
        Ok(rows) => {
            for (name, size, cap, retention) in rows {
                row(
                    &mut html,
                    &name,
                    if size >= cap {
                        "red"
                    } else if size as u128 * 100 >= cap as u128 * 80 {
                        "amber"
                    } else {
                        "ok"
                    },
                    &format!("{size} / {cap} bytes; {retention}"),
                );
            }
        }
        Err(_) => row(&mut html, "Stores", "red", "Store measurements unavailable"),
    }
    let exports = pf_janitor::budget::tree_size(&service.export_dir()).unwrap_or(0);
    row(
        &mut html,
        "Scheduled exports",
        "ok",
        &format!(
            "{exports} bytes; retain {} complete exports",
            service.config.export_keep.max(1)
        ),
    );
    html.push_str("</table><h2>Safe defaults</h2><table>");
    if let Some(site) = &service.site {
        for protection in pf_config::PROTECTIONS {
            if (protection.is_off)(site)
                || site
                    .unsafe_
                    .get(protection.name)
                    .is_some_and(|v| v == "off")
            {
                row(&mut html, protection.name, "amber", protection.key);
            }
        }
    }
    html.push_str("</table><h2>Activity and disk use</h2><table>");
    let activity=service.db.read(|conn|{let mut rows=vec![];for(label,sql)in[("Quota hits (30 days)","SELECT COUNT(*) FROM quota_hits"),("Break-glass uses","SELECT COUNT(*) FROM audit WHERE actor='break-glass' OR action LIKE '%break_glass%'"),("Crash loops","SELECT COUNT(*) FROM kv WHERE key LIKE 'crash_loop:%'")]{rows.push((label.to_owned(),conn.query_row(sql,[],|r|r.get::<_,i64>(0))?.to_string()));}let mut q=conn.prepare("SELECT p.title,COALESCE(sum(b.size),0) FROM parts p LEFT JOIN versions v ON v.part_id=p.id LEFT JOIN version_files f ON f.version_id=v.id LEFT JOIN blobs b ON b.hash=f.blob_hash GROUP BY p.id ORDER BY 2 DESC LIMIT 5")?;for row in q.query_map([],|r|Ok((format!("Part: {}",r.get::<_,String>(0)?),format!("{} bytes",r.get::<_,i64>(1)?))))?{rows.push(row?);}let mut q=conn.prepare("SELECT s.slug,COALESCE(sum(b.size),0) FROM spaces s LEFT JOIN parts p ON p.space_id=s.id LEFT JOIN versions v ON v.part_id=p.id LEFT JOIN version_files f ON f.version_id=v.id LEFT JOIN blobs b ON b.hash=f.blob_hash GROUP BY s.id ORDER BY 2 DESC LIMIT 5")?;for row in q.query_map([],|r|Ok((format!("Space: {}",r.get::<_,String>(0)?),format!("{} bytes",r.get::<_,i64>(1)?))))?{rows.push(row?);}Ok::<_,DbError>(rows)}).await;
    if let Ok(rows) = activity {
        for (name, value) in rows {
            row(&mut html, &name, "ok", &value);
        }
    }
    let failures = pf_obs::metrics::metrics()
        .registry
        .gather()
        .iter()
        .filter(|family| family.name() == "pf_http_requests_total")
        .flat_map(|family| family.get_metric())
        .filter(|metric| {
            metric
                .get_label()
                .iter()
                .any(|label| label.name() == "status_class" && label.value().starts_with('5'))
        })
        .map(|metric| metric.get_counter().value())
        .sum::<f64>();
    row(
        &mut html,
        "5xx responses",
        if failures > 0.0 { "amber" } else { "ok" },
        &failures.to_string(),
    );
    row(&mut html, "Version", "ok", env!("CARGO_PKG_VERSION"));
    html.push_str("</table><h2>Maintenance</h2><form method=post action='/admin/health/run-now'><label>Store <input name=store placeholder='All stores'></label> <button>Run janitor now</button></form><h2>Runtime log level</h2>");
    if let Some(handle) = state.log_handle() {
        html.push_str(&format!("<form method=post action='/admin/health/log-level'><label>Filter <input name=filter value='{}' maxlength=256></label> <button>Apply log filter</button></form>",escape(handle.current())));
    } else {
        html.push_str("<p>Logging is managed by the process host.</p>");
    }
    html.push_str(
        "<p>Changing maintenance settings requires a fresh admin passkey confirmation.</p></html>",
    );
    ([(header::CACHE_CONTROL,"no-store"),(header::CONTENT_SECURITY_POLICY,"default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'")],Html(html)).into_response()
}
#[derive(Deserialize)]
struct LogForm {
    filter: String,
}
async fn log_level(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LogForm>,
) -> Response {
    let auth = state.ext::<pf_auth::AuthService>().expect("auth");
    if let Err(error) = auth.check_origin(&headers) {
        return fail(error);
    }
    let actor = match auth.require_admin_session(&headers, true).await {
        Ok(actor) => actor,
        Err(error) => return fail(error),
    };
    if form.filter.len() > 256 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(handle) = state.log_handle() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let previous = handle.current();
    if handle.set(&form.filter).is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let service = state.ext::<BackupService>().expect("backup");
    let value = serde_json::json!({"filter":form.filter}).to_string();
    let before = serde_json::json!({"filter":previous}).to_string();
    let result=service.db.write(move|tx|{tx.execute("INSERT INTO audit(at,user_id,action,target_kind,target_id,before,after) VALUES(?1,?2,'health.log_level','system','logging',?3,?4)",params![pf_db::ids::now_ms(),actor.user_id,before,value])?;Ok::<_,DbError>(())}).await;
    if result.is_err() {
        let _ = handle.set(&previous);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    axum::response::Redirect::to("/admin/health").into_response()
}
#[derive(Deserialize)]
struct RunForm {
    store: Option<String>,
}
async fn run_now(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<RunForm>,
) -> Response {
    let auth = state.ext::<pf_auth::AuthService>().expect("auth");
    if let Err(e) = auth.check_origin(&headers) {
        return fail(e);
    }
    if let Err(e) = auth.require_admin_session(&headers, true).await {
        return fail(e);
    }
    let Some(janitor) = state.ext::<pf_janitor::Janitor>() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let store = form.store.as_deref().filter(|s| !s.is_empty());
    if store.is_some_and(|s| s != "exports" && pf_janitor::stores::by_name(s).is_none()) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match janitor.run_now(store, pf_db::ids::now_ms()).await {
        Ok(_) => axum::response::Redirect::to("/admin/health").into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn fail(error: pf_model::error::PfError) -> Response {
    (
        StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(error.to_json()),
    )
        .into_response()
}
