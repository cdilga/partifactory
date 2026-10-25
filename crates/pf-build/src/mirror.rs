//! Parent-only mirroring. The cache holds fetched bytes (SRI is against those bytes);
//! only recursively rewritten output is classified as a shared blob.
use crate::{
    Diagnostic, SourceFile,
    egress::{Egress, FetchError},
    scan,
};
use pf_db::rusqlite::{OptionalExtension, params};
use pf_model::PfError;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mirrored {
    pub url: String,
    pub resolved: String,
    pub hash: String,
    pub content_type: String,
}
#[derive(Default)]
pub struct Resolution {
    pub mapping: BTreeMap<String, String>,
    pub assets: Vec<Mirrored>,
    pub warnings: Vec<Diagnostic>,
}
pub struct Mirror {
    db: pf_db::Db,
    blobs: pf_blobs::BlobStore,
    guard: Egress,
    blob_origin: String,
    active: BTreeSet<String>,
    resolved: BTreeMap<String, String>,
    assets: Vec<Mirrored>,
    warnings: Vec<Diagnostic>,
    total: usize,
}
impl Mirror {
    pub fn new(
        db: pf_db::Db,
        blobs: pf_blobs::BlobStore,
        guard: Egress,
        blob_origin: String,
    ) -> Self {
        Self {
            db,
            blobs,
            guard,
            blob_origin,
            active: BTreeSet::new(),
            resolved: BTreeMap::new(),
            assets: vec![],
            warnings: vec![],
            total: 0,
        }
    }
    pub async fn resolve(
        mut self,
        files: &BTreeMap<String, SourceFile>,
    ) -> Result<Resolution, PfError> {
        let work = async {
            for source in files.values() {
                let Ok(text) = std::str::from_utf8(&source.bytes) else {
                    continue;
                };
                let markdown;
                let (text, typ) = if source.content_type.starts_with("text/markdown") {
                    markdown = crate::render::markdown(text);
                    (markdown.as_str(), "text/html")
                } else {
                    (text, source.content_type.as_str())
                };
                for reference in references(text, typ)? {
                    if absolute(&reference.url) {
                        let original = self.asset(reference.url.clone(), 0).await?;
                        if let Some(integrity) = reference.integrity {
                            let raw = self.cached_bytes(&reference.url).await?;
                            if !integrity_matches(&integrity, &raw) {
                                return Err(failure(&reference.url, "integrity_mismatch"));
                            }
                        }
                        self.resolved.insert(reference.url, original);
                    }
                }
            }
            Ok(())
        };
        tokio::time::timeout(Duration::from_secs(30), work)
            .await
            .map_err(|_| failure("", "timeout"))??;
        Ok(Resolution {
            mapping: self.resolved,
            assets: self.assets,
            warnings: self.warnings,
        })
    }
    async fn cached_bytes(&self, url: &str) -> Result<Vec<u8>, PfError> {
        let u = url.to_string();
        let hash: String = self
            .db
            .read(move |db| {
                db.query_row("SELECT hash FROM mirror_cache WHERE url=?1", [u], |r| {
                    r.get(0)
                })
                .map_err(PfError::from)
            })
            .await?;
        self.blobs.get(&hash).map_err(PfError::from)
    }
    fn asset<'a>(
        &'a mut self,
        raw: String,
        depth: usize,
    ) -> Pin<Box<dyn Future<Output = Result<String, PfError>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(value) = self.resolved.get(&raw) {
                return Ok(value.clone());
            }
            if depth > 8 {
                return Err(failure(&raw, "depth_limit"));
            }
            if !self.active.insert(raw.clone()) {
                return Err(failure(&raw, "dependency_cycle"));
            }
            if self.active.len() + self.assets.len() > 200 {
                return Err(failure(&raw, "asset_limit"));
            }
            let now = pf_db::ids::now_ms();
            let key = raw.clone();
            let cache=self.db.read(move|db|{
                db.query_row("SELECT resolved_url,hash,content_type,pinned,fetched_at FROM mirror_cache WHERE url=?1",[key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,bool>(3)?,r.get::<_,i64>(4)?))).optional().map_err(PfError::from)
            }).await?;
            // Re-check policy even for cached entries after the admin changes the allowlist.
            if !Url::parse(&raw).is_ok_and(|url| self.guard.allowed(&url)) {
                return Err(failure(&raw, "not_allowlisted"));
            }
            let (resolved, bytes, typ) = if let Some((resolved, hash, typ, pinned, at)) =
                cache.filter(|row| row.3 || now - row.4 < 86400000)
            {
                let _ = (pinned, at);
                if !Url::parse(&resolved).is_ok_and(|url| self.guard.allowed(&url)) {
                    return Err(failure(&raw, "not_allowlisted"));
                }
                (resolved, self.blobs.get(&hash)?, typ)
            } else {
                let fetched = self
                    .guard
                    .fetch(&raw)
                    .await
                    .map_err(|FetchError { reason }| failure(&raw, &reason))?;
                let put = self.blobs.put(&fetched.bytes)?;
                let key = raw.clone();
                let target = fetched.resolved.clone();
                let typ = fetched.content_type.clone();
                let pinned = exact_url(&target) && exact_url(&key);
                self.db.write(move|db|->Result<(),PfError>{
                    pf_model::version::record_blob(db,&put.hash,put.size,now)?;
                    db.execute("INSERT INTO mirror_cache(url,resolved_url,hash,content_type,pinned,fetched_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(url) DO UPDATE SET resolved_url=excluded.resolved_url,hash=excluded.hash,content_type=excluded.content_type,pinned=excluded.pinned,fetched_at=excluded.fetched_at",params![key,target,put.hash,typ,pinned,now])?;Ok(())
                }).await?;
                (fetched.resolved, fetched.bytes, fetched.content_type)
            };
            self.total += bytes.len();
            if self.total > 50 << 20 {
                return Err(failure(&raw, "total_size"));
            }
            if !exact_url(&raw) {
                self.warnings.push(Diagnostic {
                    code: "cdn_unpinned".into(),
                    file: None,
                    message: format!(
                        "A floating CDN dependency was pinned: {}",
                        redacted_url(&raw)
                    ),
                });
            }
            let mut mapping = BTreeMap::new();
            let output = if let Ok(text) = std::str::from_utf8(&bytes) {
                let references = references(text, &typ)?;
                let base = Url::parse(&resolved).map_err(|_| failure(&raw, "url"))?;
                for reference in references {
                    if reference.url.starts_with("data:")
                        || reference.url.starts_with("blob:")
                        || reference.url.starts_with('#')
                    {
                        continue;
                    }
                    let dependency = base
                        .join(&reference.url)
                        .map_err(|_| failure(&raw, "url"))?
                        .to_string();
                    let local = self.asset(dependency.clone(), depth + 1).await?;
                    if let Some(integrity) = reference.integrity
                        && !integrity_matches(&integrity, &self.cached_bytes(&dependency).await?)
                    {
                        return Err(failure(&dependency, "integrity_mismatch"));
                    }
                    mapping.insert(reference.url, local);
                }
                if typ.contains("javascript") {
                    scan::rewrite_js(text, &mut |u| mapping.get(u).cloned())
                        .0
                        .into_bytes()
                } else if typ.starts_with("text/css") {
                    scan::rewrite_css(text, &mut |u, _| mapping.get(u).cloned()).into_bytes()
                } else {
                    bytes
                }
            } else {
                bytes
            };
            let put = self.blobs.put(&output)?;
            let hash = put.hash.clone();
            let label = redacted_url(&resolved);
            let content_type = typ.clone();
            self.db.write(move|db|->Result<(),PfError>{pf_model::version::record_blob(db,&put.hash,put.size,now)?;db.execute("INSERT INTO shared_blobs(hash,source,label,added_at,content_type) VALUES(?1,'mirror',?2,?3,?4) ON CONFLICT(hash) DO UPDATE SET content_type=excluded.content_type",params![put.hash,label,now,content_type])?;Ok(())}).await?;
            let target = format!("{}/b/{hash}", self.blob_origin);
            self.assets.push(Mirrored {
                url: raw.clone(),
                resolved,
                hash,
                content_type: typ,
            });
            self.active.remove(&raw);
            self.resolved.insert(raw, target.clone());
            Ok(target)
        })
    }
}

#[derive(Debug, Clone)]
pub struct Reference {
    pub url: String,
    pub integrity: Option<String>,
}
fn absolute(url: &str) -> bool {
    url.starts_with("https:") || url.starts_with("http:") || url.starts_with("//")
}
pub fn references(source: &str, typ: &str) -> Result<Vec<Reference>, PfError> {
    let refs = Arc::new(Mutex::new(Vec::new()));
    if typ.starts_with("text/html") {
        use lol_html::{RewriteStrSettings, element, rewrite_str, text};
        let a = refs.clone();
        let b = refs.clone();
        let c = refs.clone();
        let _ = rewrite_str(
            source,
            RewriteStrSettings::new()
                .append_element_content_handler(element!(
                    "script[src], link[href], img, source, video, audio",
                    move |el| {
                        let mut refs = a.lock().unwrap_or_else(|e| e.into_inner());
                        for attr in ["src", "href", "poster"] {
                            if let Some(url) = el.get_attribute(attr) {
                                refs.push(Reference {
                                    url,
                                    integrity: el.get_attribute("integrity"),
                                });
                            }
                        }
                        if let Some(srcset) = el.get_attribute("srcset") {
                            refs.extend(scan::srcset_urls(&srcset).into_iter().map(|(_, url)| {
                                Reference {
                                    url,
                                    integrity: None,
                                }
                            }));
                        }
                        Ok(())
                    }
                ))
                .append_element_content_handler(element!("[style]", move |el| {
                    if let Some(value) = el.get_attribute("style") {
                        b.lock().unwrap_or_else(|e| e.into_inner()).extend(
                            scan::scan_css(&value).refs.into_iter().map(|r| Reference {
                                url: r.url,
                                integrity: None,
                            }),
                        );
                    }
                    Ok(())
                }))
                .append_element_content_handler(text!("style, script:not([src])", move |chunk| {
                    // Collect the node first, since a token can cross a streaming chunk boundary.
                    c.lock().unwrap_or_else(|e| e.into_inner()).push(Reference {
                        url: format!("\0{}", chunk.as_str()),
                        integrity: None,
                    });
                    Ok(())
                })),
        )
        .map_err(|_| PfError::invalid("html", "cannot scan HTML"))?;
        let mut values = refs.lock().unwrap_or_else(|e| e.into_inner());
        let inline = values
            .iter()
            .filter_map(|r| r.url.strip_prefix('\0'))
            .collect::<Vec<_>>()
            .join("\n");
        values.retain(|r| !r.url.starts_with('\0'));
        values.extend(scan::scan_css(&inline).refs.into_iter().map(|r| Reference {
            url: r.url,
            integrity: None,
        }));
        values.extend(
            scan::scan_js(&inline)
                .specifiers
                .into_iter()
                .map(|r| Reference {
                    url: r.spec,
                    integrity: None,
                }),
        );
    } else if typ.starts_with("text/css") {
        refs.lock()
            .unwrap()
            .extend(scan::scan_css(source).refs.into_iter().map(|r| Reference {
                url: r.url,
                integrity: None,
            }));
    } else if typ.contains("javascript") {
        refs.lock()
            .unwrap()
            .extend(
                scan::scan_js(source)
                    .specifiers
                    .into_iter()
                    .map(|r| Reference {
                        url: r.spec,
                        integrity: None,
                    }),
            );
    }
    Ok(refs.lock().unwrap_or_else(|e| e.into_inner()).clone())
}
pub fn failure(url: &str, reason: &str) -> PfError {
    PfError::new("mirror_failed", "could not mirror a dependency")
        .with("url", url)
        .with("reason", reason)
}
pub fn redacted_url(raw: &str) -> String {
    Url::parse(raw)
        .map(|mut u| {
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        })
        .unwrap_or_default()
}
pub fn exact_url(raw: &str) -> bool {
    cdn_url_to_npm(raw).is_some_and(|(_, v)| exact_version(&v))
}
fn exact_version(v: &str) -> bool {
    let core = v.split(['-', '+']).next().unwrap_or("");
    core.split('.').count() == 3
        && core
            .split('.')
            .all(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
}
pub fn cdn_url_to_npm(raw: &str) -> Option<(String, String)> {
    let url = Url::parse(raw).ok()?;
    let host = url.host_str()?;
    let path = url.path().trim_start_matches('/');
    if host == "cdnjs.cloudflare.com" {
        let mut s = path.strip_prefix("ajax/libs/")?.split('/');
        return Some((s.next()?.into(), s.next()?.into()));
    }
    let path = match host {
        "cdn.jsdelivr.net" => path.strip_prefix("npm/")?,
        "unpkg.com" | "esm.sh" => path,
        _ => return None,
    };
    let end = if path.starts_with('@') {
        let slash = path.find('/')?;
        path[slash + 1..]
            .find('/')
            .map(|n| slash + 1 + n)
            .unwrap_or(path.len())
    } else {
        path.find('/').unwrap_or(path.len())
    };
    let package = &path[..end];
    let (name, version) = package.rsplit_once('@')?;
    if name.is_empty() {
        return None;
    }
    Some((name.into(), version.into()))
}
pub fn integrity_matches(integrity: &str, bytes: &[u8]) -> bool {
    use base64::Engine;
    use sha2::{Digest, Sha256, Sha384, Sha512};
    // SRI chooses the strongest recognized algorithm, not any matching weaker token.
    for algorithm in ["sha512", "sha384", "sha256"] {
        let tokens: Vec<_> = integrity
            .split_ascii_whitespace()
            .filter_map(|token| token.strip_prefix(&format!("{algorithm}-")))
            .collect();
        if tokens.is_empty() {
            continue;
        }
        let digest = match algorithm {
            "sha512" => Sha512::digest(bytes).to_vec(),
            "sha384" => Sha384::digest(bytes).to_vec(),
            _ => Sha256::digest(bytes).to_vec(),
        };
        return tokens.into_iter().any(|value| {
            base64::engine::general_purpose::STANDARD
                .decode(value.split('?').next().unwrap_or(""))
                .is_ok_and(|v| v == digest)
        });
    }
    false
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_urls() {
        assert_eq!(
            cdn_url_to_npm("https://cdn.jsdelivr.net/npm/@scope/pkg@1.2.3/a.js"),
            Some(("@scope/pkg".into(), "1.2.3".into()))
        );
        assert_eq!(
            cdn_url_to_npm(
                "https://cdnjs.cloudflare.com/ajax/libs/react/18.3.1/umd/react.production.min.js"
            ),
            Some(("react".into(), "18.3.1".into()))
        );
        assert!(!exact_url("https://esm.sh/react@18"));
        assert!(exact_url("https://esm.sh/react@18.3.1"));
    }
    #[test]
    fn sri_uses_strongest_algorithm() {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let good = format!(
            "sha256-{}",
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(b"hello"))
        );
        assert!(integrity_matches(&good, b"hello"));
        assert!(!integrity_matches(&format!("{good} sha512-AAAA"), b"hello"));
    }
}
