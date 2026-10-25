use crate::{PartHost, cache::Boot};
use pf_caps::Claims;
use pf_model::PfError;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(crate) async fn boot(s: &PartHost, cap: &str, c: &Claims) -> Result<Arc<String>, PfError> {
    if let Some(b) = s
        .inner
        .boot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(cap)
    {
        return Ok(b.json.clone());
    }
    let generation = s.inner.boot_generation.load(Ordering::Acquire);
    let owner =
        c.s.strip_prefix("user:")
            .filter(|_| c.g.is_none())
            .unwrap_or("")
            .to_string();
    let mut personal = serde_json::Map::new();
    if !owner.is_empty() {
        let p = c.p.clone();
        let o = owner.clone();
        personal = s.read(move |db| {
            let mut stmt = db.prepare("SELECT doc_id,body FROM data_docs WHERE part_id=?1 AND scope='personal' AND owner=?2 AND collection='__storage' ORDER BY doc_id")?;
            let rows = stmt.query_map([p,o], |r| Ok((r.get::<_,String>(0)?,r.get::<_,Vec<u8>>(1)?)))?;
            let mut m = serde_json::Map::new(); let mut size = 0;
            for row in rows { let (k,b) = row?; size += k.len()+b.len(); if size>262144 { break; } m.insert(k, serde_json::from_slice(&b).map_err(|_| PfError::internal("invalid personal document"))?); }
            Ok(m)
        }).await?;
    }
    let value = json!({"personal":personal,"scope":if owner.is_empty() {"none"} else {"person"},"events":c.s!="render"});
    let out = Arc::new(
        value
            .to_string()
            .replace('<', "\\u003c")
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029"),
    );
    let mut cache = s.inner.boot.lock().unwrap_or_else(|e| e.into_inner());
    // A personal write may have invalidated the cache while this read was in flight.
    if generation != s.inner.boot_generation.load(Ordering::Acquire) {
        return Ok(out);
    }
    if cache.len() >= 1024 {
        cache.clear();
    }
    cache.insert(
        cap.to_string(),
        Boot {
            part_id: c.p.clone(),
            owner,
            json: out.clone(),
        },
    );
    Ok(out)
}

pub(crate) fn html(s: &PartHost, source: &str, cap: &str, boot: &str) -> String {
    let blob = s.config().blob_origin();
    let prefix = format!("/c/{cap}/");
    let imports = json!({"imports":{"/":prefix,"/c/":"/c/"}})
        .to_string()
        .replace('<', "\\u003c");
    let mut tags = format!(
        "<script src=\"{blob}/b/{}\"></script><script type=\"importmap\">{imports}</script><script type=\"application/json\" id=\"pf-boot\">{boot}</script>",
        s.prelude_hash()
    );
    if let Some(hash) = &*s
        .inner
        .shell_css_hash
        .read()
        .unwrap_or_else(|e| e.into_inner())
    {
        tags.push_str(&format!(
            "<link rel=\"stylesheet\" href=\"{blob}/b/{hash}\">"
        ));
    }
    // Start the implicit head before *any* author bytes. Searching for a textual <head>
    // can match a comment or script string and let author scripts beat the storage shims.
    // The HTML parser merges later <html> attributes and ignores a second doctype/head tag.
    format!("<!doctype html>{tags}{source}")
}
