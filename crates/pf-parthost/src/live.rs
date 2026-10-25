use crate::PartHost;
use pf_db::rusqlite::{OptionalExtension, params};
use pf_model::PfError;
use std::{collections::HashMap, sync::Arc};

#[derive(Debug, Clone)]
pub struct FileRef {
    pub hash: String,
    pub br_hash: Option<String>,
    pub content_type: String,
}

#[derive(Debug, Clone)]
pub struct Live {
    pub part_id: String,
    pub version_id: String,
    pub build_id: i64,
    pub entry: String,
    pub unsandboxed: bool,
    pub files: HashMap<String, FileRef>,
}

pub(crate) struct Cached {
    pub live: Option<Arc<Live>>,
    pub at: i64,
}

impl PartHost {
    pub async fn live(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Arc<Live>>, PfError> {
        let key = format!("{name}:{}", version.unwrap_or_default());
        if let Some(c) = self
            .inner
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            && (c.live.is_some() || self.now() - c.at < 5000)
        {
            return Ok(c.live.clone());
        }
        let n = name.to_string();
        let v = version.map(str::to_string);
        let result = self.read(move |db| {
            let row: Option<(String, String, i64, String, String)> = if let Some(v) = v {
                db.query_row("SELECT p.id,v.id,b.id,v.entry,p.isolation FROM parts p JOIN versions v ON v.part_id=p.id JOIN builds b ON b.version_id=v.id AND b.status='ok' WHERE p.name=?1 AND p.deleted_at IS NULL AND substr(v.id,1,length(?2))=?2 ORDER BY b.id DESC LIMIT 1", params![n,v], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?
            } else {
                db.query_row("SELECT p.id,v.id,p.live_build_id,v.entry,p.isolation FROM parts p JOIN versions v ON v.id=p.live_version_id WHERE p.name=?1 AND p.deleted_at IS NULL AND p.live_build_id IS NOT NULL", [n], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?
            };
            let Some((part_id, version_id, build_id, entry, isolation)) = row else { return Ok(None) };
            let mut stmt = db.prepare("SELECT path,blob_hash,br_hash,content_type FROM build_files WHERE build_id=?1")?;
            let files = stmt.query_map([build_id], |r| Ok((r.get::<_,String>(0)?, FileRef { hash:r.get(1)?, br_hash:r.get(2)?, content_type:r.get(3)? })))?.collect::<Result<HashMap<_,_>,_>>()?;
            Ok(Some(Arc::new(Live { part_id, version_id, build_id, entry, unsandboxed:isolation=="unsandboxed", files })))
        }).await?;
        self.inner
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                key,
                Cached {
                    live: result.clone(),
                    at: self.now(),
                },
            );
        Ok(result)
    }

    pub(crate) fn bytes(&self, hash: &str) -> Result<bytes::Bytes, PfError> {
        if let Some(b) = self
            .inner
            .bytes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(hash)
        {
            return Ok(b);
        }
        let b = bytes::Bytes::from(self.deps().blobs.get(hash)?);
        self.inner
            .bytes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .put(hash.into(), b.clone());
        Ok(b)
    }
}
