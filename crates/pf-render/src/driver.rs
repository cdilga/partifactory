use chromiumoxide::page::ScreenshotParams;
use chromiumoxide::{
    Browser, Page,
    cdp::browser_protocol::{
        emulation::SetDeviceMetricsOverrideParams,
        input::{DispatchMouseEventParams, DispatchMouseEventType, MouseButton},
        network::{EventLoadingFailed, EventRequestWillBeSent, EventResponseReceived},
        page::CaptureScreenshotFormat,
    },
    cdp::js_protocol::runtime::TerminateExecutionParams,
};
use futures_util::StreamExt;
use pf_model::PfError;
use pf_ops::{Action, CheckRequest};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::Cursor,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct Session {
    browser: Option<Browser>,
    handler: Option<tokio::task::JoinHandle<()>>,
    listeners: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Session {
    fn drop(&mut self) {
        for task in self.listeners.drain(..) {
            task.abort();
        }
        if let (Some(mut browser), Some(handler)) = (self.browser.take(), self.handler.take()) {
            tokio::spawn(async move {
                let _ =
                    tokio::time::timeout(Duration::from_secs(1), browser.quit_incognito_context())
                        .await;
                handler.abort();
            });
        }
    }
}

pub struct Output {
    pub result: Value,
    pub png: Vec<u8>,
}
fn unavailable(_: impl std::fmt::Display) -> PfError {
    PfError::new("renderer_unavailable", "The render browser is unavailable.")
        .with("hint", "check the render sidecar and retry")
}

pub fn strip_cap(value: &str) -> String {
    // Outputs use paths only; never expose a capability, including nested stack URLs.
    let mut text = value.to_string();
    while let Some(start) = text.find("/c/") {
        let tail = &text[start + 3..];
        let end = tail
            .find(|ch: char| {
                ch == '/' || ch.is_whitespace() || matches!(ch, '?' | '#' | '\"' | '\'' | ')')
            })
            .map(|n| start + 3 + n)
            .unwrap_or(text.len());
        text.replace_range(start..end, "");
    }
    text
}
pub fn safe_url(value: &str) -> String {
    let value = strip_cap(value);
    url::Url::parse(&value)
        .ok()
        .map(|u| {
            format!(
                "{}{}",
                u.path(),
                u.query().map(|q| format!("?{q}")).unwrap_or_default()
            )
        })
        .unwrap_or(value)
}
fn sanitize(value: &mut Value) {
    match value {
        Value::String(s) => *s = strip_cap(s),
        Value::Array(a) => {
            for v in a {
                sanitize(v)
            }
        }
        Value::Object(o) => {
            for (key, v) in o {
                if key == "url" {
                    if let Some(s) = v.as_str() {
                        *v = json!(safe_url(s));
                    }
                } else {
                    sanitize(v)
                }
            }
        }
        _ => {}
    }
}

const COLLECT: &str = r#"(()=>{
const state={errors:[],console:[],failed_requests:[]};
for(const key of ['errors','failed_requests']) {const list=state[key];list.push=function(...items){if(this.length<200)return Array.prototype.push.apply(this,items.slice(0,200-this.length));return this.length;};}
Object.defineProperty(window,'__pfRender',{value:state});
const clean=s=>String(s||'').slice(0,8192);
addEventListener('error',e=>{
  const t=e.target;
  if(t&&t!==window&&(t.src||t.href)) {const url=t.src||t.href;state.errors.push({kind:'blocked_load',message:'Resource failed',url});state.failed_requests.push({url,reason:'blocked_load'});}
  else state.errors.push({kind:'error',message:clean(e.message),stack:clean(e.error&&e.error.stack),url:clean(e.filename)});
},true);
addEventListener('unhandledrejection',e=>state.errors.push({kind:'unhandledrejection',message:clean(e.reason&&e.reason.message||e.reason),stack:clean(e.reason&&e.reason.stack)}));
addEventListener('securitypolicyviolation',e=>{state.errors.push({kind:'csp',message:clean(e.effectiveDirective),url:clean(e.blockedURI)});state.failed_requests.push({url:clean(e.blockedURI),reason:'csp'});});
for(const level of ['error','warn']) {const native=console[level];console[level]=function(...args){if(state.console.length<50)state.console.push({level,text:args.map(clean).join(' ').slice(0,4096)});return native.apply(this,args);};}
})()"#;

async fn evaluate(page: &Page, expr: &str) -> Result<Value, PfError> {
    page.evaluate(expr)
        .await
        .map_err(unavailable)?
        .into_value::<Value>()
        .map_err(unavailable)
}
async fn action(page: &Page, action: &Action) -> Result<Value, PfError> {
    match action {
        Action::Click {
            selector: Some(selector),
            ..
        } => {
            page.find_element(selector)
                .await
                .map_err(unavailable)?
                .click()
                .await
                .map_err(unavailable)?;
            Ok(Value::Null)
        }
        Action::Click {
            x: Some(x),
            y: Some(y),
            ..
        } => {
            for kind in [
                DispatchMouseEventType::MousePressed,
                DispatchMouseEventType::MouseReleased,
            ] {
                page.execute(
                    DispatchMouseEventParams::builder()
                        .r#type(kind)
                        .x(*x)
                        .y(*y)
                        .button(MouseButton::Left)
                        .click_count(1)
                        .build()
                        .map_err(unavailable)?,
                )
                .await
                .map_err(unavailable)?;
            }
            Ok(Value::Null)
        }
        Action::Click { .. } => Err(PfError::invalid("actions", "invalid click")),
        Action::Type { selector, text } => {
            let el = page.find_element(selector).await.map_err(unavailable)?;
            el.click().await.map_err(unavailable)?;
            el.type_str(text).await.map_err(unavailable)?;
            Ok(Value::Null)
        }
        Action::WaitMs { ms } => {
            tokio::time::sleep(Duration::from_millis(*ms)).await;
            Ok(Value::Null)
        }
        Action::EvalReturn { expr } => {
            let value = evaluate(page, expr).await?;
            if value.to_string().len() > 4096 {
                return Err(PfError::invalid(
                    "actions",
                    "eval_return value exceeds 4 KiB",
                ));
            }
            Ok(value)
        }
    }
}

pub async fn run(endpoint: &str, url: &str, request: &CheckRequest) -> Result<Output, PfError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
        .map_err(unavailable)?;
    let version: Value = client
        .get(format!("{}/json/version", endpoint.trim_end_matches('/')))
        .header(reqwest::header::HOST, "localhost")
        .send()
        .await
        .map_err(unavailable)?
        .error_for_status()
        .map_err(unavailable)?
        .json()
        .await
        .map_err(unavailable)?;
    let mut websocket = url::Url::parse(
        version["webSocketDebuggerUrl"]
            .as_str()
            .ok_or_else(|| unavailable("no websocket"))?,
    )
    .map_err(unavailable)?;
    let origin = url::Url::parse(endpoint).map_err(unavailable)?;
    websocket.set_host(origin.host_str()).map_err(unavailable)?;
    websocket
        .set_port(origin.port())
        .map_err(|_| unavailable("invalid port"))?;
    let (browser, mut handler) = Browser::connect(websocket.to_string())
        .await
        .map_err(unavailable)?;
    let handler_task = tokio::spawn(async move {
        while let Some(result) = handler.next().await {
            if result.is_err() {
                break;
            }
        }
    });
    let mut session = Session {
        browser: Some(browser),
        handler: Some(handler_task),
        listeners: Vec::new(),
    };
    let browser = session.browser.as_mut().expect("browser session");
    browser
        .start_incognito_context()
        .await
        .map_err(unavailable)?;
    let page = browser.new_page("about:blank").await.map_err(unavailable)?;
    let mut requests = page
        .event_listener::<EventRequestWillBeSent>()
        .await
        .map_err(unavailable)?;
    let mut failures = page
        .event_listener::<EventLoadingFailed>()
        .await
        .map_err(unavailable)?;
    let mut responses = page
        .event_listener::<EventResponseReceived>()
        .await
        .map_err(unavailable)?;
    let failed = Arc::new(Mutex::new(Vec::<Value>::new()));
    let output = failed.clone();
    session.listeners.push(tokio::spawn(async move {
        let mut urls=HashMap::new();
        loop {tokio::select! {
            event=requests.next()=>match event {
                Some(event)=>{if urls.len()<2000 {urls.insert(event.request_id.clone(),event.request.url.chars().take(2048).collect::<String>());}},
                None=>break,
            },
            event=failures.next()=>match event {
                Some(event)=>{let mut output=output.lock().unwrap_or_else(|e|e.into_inner());if output.len()<200 {output.push(json!({"url":urls.get(&event.request_id).cloned().unwrap_or_else(||"/".into()),"reason":event.error_text.chars().take(512).collect::<String>()}));}},
                None=>break,
            },
            event=responses.next()=>match event {
                Some(event)=>{if event.response.status>=400 {let mut output=output.lock().unwrap_or_else(|e|e.into_inner());if output.len()<200 {output.push(json!({"url":event.response.url.chars().take(2048).collect::<String>(),"reason":format!("HTTP {}",event.response.status)}));}}},
                None=>break,
            },
        }}
    }));
    let mobile = request.viewport.as_deref().unwrap_or("mobile") == "mobile";
    let start = Instant::now();
    let mut actions = Vec::new();
    let result=tokio::time::timeout(Duration::from_secs(12),async {
        page.execute(SetDeviceMetricsOverrideParams::builder().width(if mobile {390}else{1280}).height(if mobile {844}else{800}).device_scale_factor(if mobile {2.0}else{1.0}).mobile(mobile).build().map_err(unavailable)?).await.map_err(unavailable)?;
        page.evaluate_on_new_document(COLLECT).await.map_err(unavailable)?;
        page.goto(url).await.map_err(unavailable)?;
        let wait=Duration::from_millis(request.wait_ms.unwrap_or(8000));
        while start.elapsed()<wait {
            let state=evaluate(&page,"({ready:window.__pfReadyAt||null,idle:performance.now()-Math.max(0,...performance.getEntriesByType('resource').map(e=>e.responseEnd))>=500})").await?;
            if !state["ready"].is_null()||state["idle"]==true {break;}
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        if let Some(steps)=&request.actions {
            for (i,step) in steps.iter().enumerate() {
                let name=match step {Action::Click{..}=>"click",Action::Type{..}=>"type",Action::WaitMs{..}=>"wait_ms",Action::EvalReturn{..}=>"eval_return"};
                match action(&page,step).await {
                    Ok(value)=>{let mut row=json!({"i":i,"do":name,"ok":true});if matches!(step,Action::EvalReturn{..}){row["value"]=value;}actions.push(row);},
                    Err(error)=>{actions.push(json!({"i":i,"do":name,"ok":false,"error":strip_cap(&error.message)}));break;}
                }
            }
        }
        let mut result=evaluate(&page,"({...window.__pfRender,height:document.documentElement.scrollHeight,body_empty:!document.body||!document.body.innerText.trim(),ready_ms:window.__pfReadyAt?Math.max(0,window.__pfReadyAt-performance.timeOrigin):null})").await?;
        let png=page.screenshot(ScreenshotParams::builder().format(CaptureScreenshotFormat::Png).full_page(false).build()).await.map_err(unavailable)?;
        let blank=result["body_empty"]==true&&uniform(&png)?;
        result.as_object_mut().expect("render state").remove("body_empty");
        let ready=result.as_object_mut().expect("render state").remove("ready_ms").unwrap_or(Value::Null);
        let errors=result["errors"].as_array().is_none_or(|a|a.iter().all(|e|e["kind"]!="error"&&e["kind"]!="unhandledrejection"));
        result["passed"]=json!(errors&&!blank&&actions.iter().all(|a|a["ok"]==true));result["blank"]=json!(blank);result["actions"]=json!(actions);result["timing"]=json!({"ready_ms":ready,"total_ms":start.elapsed().as_millis() as u64});
        result["width"]=json!(if mobile {780}else{1280});result["height_pixels"]=json!(if mobile {1688}else{800});
        if let Some(requests)=result["failed_requests"].as_array_mut() {requests.extend(failed.lock().unwrap_or_else(|e|e.into_inner()).iter().cloned());}
        sanitize(&mut result);
        Ok(Output{result,png})
    }).await;
    match result {
        Ok(result) => result,
        Err(_) => {
            let _ = tokio::time::timeout(
                Duration::from_millis(500),
                page.execute(TerminateExecutionParams::default()),
            )
            .await;
            let mut result = tokio::time::timeout(
                Duration::from_millis(500),
                evaluate(
                    &page,
                    "window.__pfRender||({errors:[],console:[],failed_requests:[]})",
                ),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(json!({"errors":[],"console":[],"failed_requests":[]}));
            result["errors"]
                .as_array_mut()
                .expect("errors")
                .push(json!({"kind":"error","message":"Render reached its hard time limit"}));
            result["passed"] = json!(false);
            result["blank"] = json!(false);
            result["height"] = Value::Null;
            result["actions"] = json!(actions);
            result["timing"] =
                json!({"ready_ms":null,"total_ms":start.elapsed().as_millis() as u64});
            result["width"] = json!(if mobile { 780 } else { 1280 });
            result["height_pixels"] = json!(if mobile { 1688 } else { 800 });
            let png = tokio::time::timeout(
                Duration::from_millis(750),
                page.screenshot(
                    ScreenshotParams::builder()
                        .format(CaptureScreenshotFormat::Png)
                        .full_page(false)
                        .build(),
                ),
            )
            .await
            .map_err(unavailable)?
            .map_err(unavailable)?;
            if let Some(requests) = result["failed_requests"].as_array_mut() {
                requests.extend(
                    failed
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .iter()
                        .cloned(),
                );
            }
            sanitize(&mut result);
            Ok(Output { result, png })
        }
    }
}

pub fn uniform(bytes: &[u8]) -> Result<bool, PfError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(unavailable)?;
    let size = reader
        .output_buffer_size()
        .filter(|n| *n <= 64 * 1024 * 1024)
        .ok_or_else(|| unavailable("screenshot too large"))?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(unavailable)?;
    let channels = info.color_type.samples();
    let pixels = buf[..info.buffer_size()].chunks_exact(channels);
    let total = pixels.len();
    let mut counts: HashMap<Vec<u8>, usize> = HashMap::new();
    let mut most = 0;
    for p in pixels {
        let n = counts.entry(p.to_vec()).or_default();
        *n += 1;
        most = most.max(*n);
    }
    Ok(total > 0 && most * 100 >= total * 98)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn action_limits_reject_excess_before_rendering() {
        for extra in [
            json!({"actions":vec![json!({"do":"click","selector":"button"});21]}),
            json!({"actions":[{"do":"type","selector":"input","text":"é".repeat(1001)}]}),
            json!({"actions":[{"do":"wait_ms","ms":2001}]}),
            json!({"wait_ms":8001}),
            json!({"actions":[{"do":"click","x":1}]}),
        ] {
            let mut input = json!({"name":"board"});
            input
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let request: CheckRequest = serde_json::from_value(input).unwrap();
            assert_eq!(request.validate().unwrap_err().code, "invalid_input");
        }
        let request:CheckRequest=serde_json::from_value(json!({"name":"board","wait_ms":8000,"actions":[{"do":"type","selector":"input","text":"é".repeat(1000)},{"do":"wait_ms","ms":2000}]})).unwrap();
        request.validate().unwrap();
    }
    #[test]
    fn never_returns_cap_urls() {
        assert_eq!(
            safe_url("https://board-part.pf.test/c/v1.secret/path?q=1"),
            "/path?q=1"
        );
        assert!(
            !strip_cap("at https://board-part.pf.test/c/secret/index.html:2:3").contains("secret")
        );
    }
    #[test]
    fn blank_threshold() {
        for (white, expected) in [(98, true), (97, false)] {
            let mut bytes = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut bytes, 10, 10);
                encoder.set_color(png::ColorType::Rgb);
                encoder.set_depth(png::BitDepth::Eight);
                let mut writer = encoder.write_header().unwrap();
                let mut image = vec![0u8; 300];
                for p in image.chunks_exact_mut(3).take(white) {
                    p.fill(255);
                }
                writer.write_image_data(&image).unwrap();
            }
            assert_eq!(uniform(&bytes).unwrap(), expected);
        }
    }
}
