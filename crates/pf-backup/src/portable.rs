//! Portable parts carry content/history and author labels, never identity or active access rows.
use crate::{Result, invalid};
use pf_db::rusqlite::{
    Connection,
    types::{Value as SqlValue, ValueRef},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};
const TABLES: &[&str] = &[
    "blobs",
    "parts",
    "versions",
    "version_files",
    "builds",
    "build_files",
    "names",
    "part_tags",
    "part_notes",
];
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
fn rows(conn: &Connection, sql: &str) -> Result<Vec<Value>> {
    let mut statement = conn.prepare(sql)?;
    let names = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut cursor = statement.query([])?;
    let mut output = vec![];
    while let Some(row) = cursor.next()? {
        let mut value = serde_json::Map::new();
        for (index, name) in names.iter().enumerate() {
            value.insert(
                name.clone(),
                match row.get_ref(index)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(value) => json!(value),
                    ValueRef::Real(value) => json!(value),
                    ValueRef::Text(value) => json!(
                        std::str::from_utf8(value)
                            .map_err(|_| invalid("Non-UTF8 portable metadata."))?
                    ),
                    ValueRef::Blob(_) => {
                        return Err(invalid(
                            "Binary identity metadata cannot enter a parts-only export.",
                        ));
                    }
                },
            );
        }
        output.push(Value::Object(value));
    }
    Ok(output)
}
pub(crate) fn export(conn: &Connection) -> Result<Value> {
    let mut tables = BTreeMap::new();
    tables.insert("blobs",rows(conn,"SELECT * FROM blobs WHERE hash IN (SELECT blob_hash FROM version_files UNION SELECT output_hash FROM builds UNION SELECT log_hash FROM builds UNION SELECT sbom_hash FROM builds UNION SELECT blob_hash FROM build_files UNION SELECT br_hash FROM build_files UNION SELECT manifest_hash FROM versions)")?);
    tables.insert("parts",rows(conn,"SELECT p.id,p.name,s.slug AS space_slug,p.title,p.description,p.kind,p.head_version_id,p.live_version_id,p.live_build_id,p.head_n,p.shared_data_writers,p.isolation,p.created_at,p.updated_at,p.deleted_at FROM parts p JOIN spaces s ON s.id=p.space_id ORDER BY p.id")?);
    tables.insert("versions",rows(conn,"SELECT v.id,v.part_id,v.n,v.parent_id,v.manifest_hash,v.entry,u.display_name AS author_label,v.via,v.message,v.rebased_from,v.restored_from,v.live_gate,v.created_at FROM versions v JOIN users u ON u.id=v.author_user ORDER BY v.part_id,v.n")?);
    tables.insert("part_notes",rows(conn,"SELECT n.part_id,n.n,n.body,u.display_name AS author_label,n.created_at FROM part_notes n JOIN users u ON u.id=n.author_user ORDER BY n.part_id,n.n")?);
    for table in [
        "version_files",
        "builds",
        "build_files",
        "names",
        "part_tags",
    ] {
        tables.insert(table, rows(conn, &format!("SELECT * FROM {table}"))?);
    }
    let mut audience_labels = vec![];
    let mut statement=conn.prepare("SELECT DISTINCT p.id,g.subject,u.username,u.email FROM parts p JOIN grants g ON (g.target_kind='part' AND g.target_id=p.id) OR (g.target_kind='space' AND g.target_id=p.space_id) LEFT JOIN users u ON g.subject='user:'||u.id ORDER BY p.id,g.subject")?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })? {
        let (part, subject, username, email) = row?;
        let (kind, label) = if let Some(group) = subject.strip_prefix("group:") {
            ("group", group.to_owned())
        } else if subject == "everyone-signed-in" {
            ("everyone", subject)
        } else {
            ("person", email.or(username).unwrap_or_default())
        };
        if !label.is_empty() {
            audience_labels.push(json!({"part_id":part,"kind":kind,"label":label}));
        }
    }
    Ok(json!({"format_version":1,"tables":tables,"audience_labels":audience_labels}))
}
fn placeholder(conn: &Connection, label: &str) -> Result<String> {
    let digest = pf_db::sha256_hex(label.as_bytes());
    let id = format!("u_import_{}", &digest[..24]);
    let username = format!("imported-{}", &digest[..16]);
    conn.execute("INSERT OR IGNORE INTO users(id,auth_source,username,display_name,status,in_audience,created_at,synced_at) VALUES(?1,'local',?2,?3,'disabled',0,0,0)",pf_db::rusqlite::params![id,username,label])?;
    Ok(id)
}
fn insert(conn: &Connection, table: &str, record: &serde_json::Map<String, Value>) -> Result<()> {
    let columns = record
        .keys()
        .map(|key| quoted(key))
        .collect::<Vec<_>>()
        .join(",");
    let placeholders = std::iter::repeat_n("?", record.len())
        .collect::<Vec<_>>()
        .join(",");
    let values = record
        .values()
        .map(|value| {
            Ok(match value {
                Value::Null => SqlValue::Null,
                Value::String(value) => SqlValue::Text(value.clone()),
                Value::Number(value) => SqlValue::Integer(
                    value
                        .as_i64()
                        .ok_or_else(|| invalid("Non-integer portable metadata."))?,
                ),
                Value::Bool(value) => SqlValue::Integer(i64::from(*value)),
                _ => return Err(invalid("Nested portable table metadata.")),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    conn.execute(
        &format!(
            "INSERT INTO {} ({columns}) VALUES ({placeholders})",
            quoted(table)
        ),
        pf_db::rusqlite::params_from_iter(values),
    )?;
    Ok(())
}
pub(crate) fn import(path: &Path, destination: &Path) -> Result<()> {
    if std::fs::metadata(path)?.len() > 128 * 1024 * 1024 {
        return Err(invalid(
            "Portable parts metadata exceeds 128 MiB; use a full export.",
        ));
    }
    let document: Value = serde_json::from_reader(std::fs::File::open(path)?)?;
    if document["format_version"] != 1 {
        return Err(invalid("Unsupported portable-parts format."));
    }
    let tables = document["tables"]
        .as_object()
        .ok_or_else(|| invalid("Portable parts have no table map."))?;
    if tables.keys().any(|name| !TABLES.contains(&name.as_str())) {
        return Err(invalid(
            "Portable parts contain identity or access records.",
        ));
    }
    let mut conn = Connection::open(destination)?;
    pf_db::migrate(&mut conn, pf_db::MAIN)?;
    let tx = conn.transaction()?;
    tx.execute_batch("PRAGMA defer_foreign_keys=ON")?;
    let owner = placeholder(&tx, "Imported owner")?;
    for table in TABLES {
        let Some(records) = tables.get(*table) else {
            continue;
        };
        for value in records
            .as_array()
            .ok_or_else(|| invalid("Portable table is not an array."))?
        {
            let mut record = value
                .as_object()
                .ok_or_else(|| invalid("Invalid portable row."))?
                .clone();
            if *table == "parts" {
                let slug = record
                    .remove("space_slug")
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .ok_or_else(|| invalid("Portable part has no space slug."))?;
                let digest = pf_db::sha256_hex(slug.as_bytes());
                let space = format!("sp_import_{}", &digest[..24]);
                tx.execute("INSERT OR IGNORE INTO spaces(id,slug,kind,title,created_at) VALUES(?1,?2,'shared',?2,0)",pf_db::rusqlite::params![space,slug])?;
                record.insert("space_id".into(), json!(space));
                record.insert("owner_user_id".into(), json!(owner));
                record.insert("created_by_user".into(), json!(owner));
                for forbidden in ["created_by_conn", "steward_conn"] {
                    record.remove(forbidden);
                }
            }
            if matches!(*table, "versions" | "part_notes") {
                let label = record
                    .remove("author_label")
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "Imported author".into());
                record.insert("author_user".into(), json!(placeholder(&tx, &label)?));
                record.remove("actor_conn");
            }
            insert(&tx, table, &record)?;
        }
    }
    let labels = document
        .get("audience_labels")
        .cloned()
        .unwrap_or_else(|| json!([]));
    tx.execute(
        "INSERT INTO kv(key,value) VALUES('import.audience_labels',?1)",
        [serde_json::to_string(&labels)?],
    )?;
    tx.commit()?;
    drop(conn);
    crate::archive::integrity(destination)?;
    Ok(())
}
