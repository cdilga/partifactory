//! Schema v1 against the plan (§5e): the DDL is the plan's verbatim, every object exists (golden
//! snapshot), and the constraints reject what they must. One helper per DDL block.

use pf_db::rusqlite::{self, Connection};
use pf_db::{INSIGHTS, MAIN, migrate};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn main_db() -> Connection {
    let mut c = Connection::open_in_memory().unwrap();
    c.pragma_update(None, "foreign_keys", "ON").unwrap();
    migrate(&mut c, MAIN).unwrap();
    seed(&c);
    c
}

fn insights_db() -> Connection {
    let mut c = Connection::open_in_memory().unwrap();
    c.pragma_update(None, "foreign_keys", "ON").unwrap();
    migrate(&mut c, INSIGHTS).unwrap();
    c
}

/// Extract the n-th ```sql block under "## 5e. Data model" in PLAN.md.
fn plan_sql_block(n: usize) -> String {
    let plan = std::fs::read_to_string(repo().join("PLAN.md")).unwrap();
    let start = plan.find("## 5e. Data model").expect("§5e");
    let end = plan[start..].find("## 5f.").map(|i| start + i).unwrap();
    let sec = &plan[start..end];
    let mut blocks = Vec::new();
    let mut rest = sec;
    while let Some(i) = rest.find("```sql\n") {
        let body = &rest[i + 7..];
        let j = body.find("```").unwrap();
        blocks.push(body[..j].to_string());
        rest = &body[j + 3..];
    }
    blocks[n].clone()
}

#[test]
fn ddl_matches_plan_snapshot() {
    assert_eq!(
        MAIN[0].sql,
        plan_sql_block(0),
        "0001_schema_v1.sql must be the §5e main DDL verbatim"
    );
    assert_eq!(
        INSIGHTS[0].sql,
        plan_sql_block(1),
        "insights 0001 must be the §5e insights DDL verbatim"
    );
}

fn schema_dump(c: &Connection) -> String {
    let mut s = c
        .prepare(
            "SELECT type, name, tbl_name, COALESCE(sql, '') FROM sqlite_schema \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .unwrap();
    let rows: Vec<(String, String, String, String)> = s
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut out = String::new();
    for (t, n, tbl, sql) in rows {
        out.push_str(&format!("-- {t} {n} on {tbl}\n{sql};\n\n"));
    }
    out
}

fn golden(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("PF_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden {}; run with PF_UPDATE_GOLDEN=1",
            path.display()
        )
    });
    assert_eq!(actual, want, "schema drifted from {}", path.display());
}

#[test]
fn schema_golden_snapshot() {
    let mut c = Connection::open_in_memory().unwrap();
    migrate(&mut c, MAIN).unwrap();
    golden("schema_v3.sql", &schema_dump(&c));
    let mut v1 = Connection::open_in_memory().unwrap();
    migrate(&mut v1, &MAIN[..1]).unwrap();
    golden("schema_v1.sql", &schema_dump(&v1));
    golden("insights_v1.sql", &schema_dump(&insights_db()));
}

fn names_of(c: &Connection, kind: &str) -> BTreeSet<String> {
    let mut s = c
        .prepare("SELECT name FROM sqlite_schema WHERE type = ?1 AND name NOT LIKE 'sqlite_%'")
        .unwrap();
    s.query_map([kind], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Every CREATE TABLE / INDEX / TRIGGER named in a DDL block, by a plain scan of the text.
fn declared(sql: &str, kw: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in sql.lines() {
        let l = line.trim_start();
        for prefix in [
            format!("CREATE {kw} "),
            format!("CREATE UNIQUE {kw} "),
            format!("CREATE VIRTUAL {kw} "),
        ] {
            if let Some(rest) = l.strip_prefix(&prefix) {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                out.insert(name);
            }
        }
    }
    out
}

fn assert_all_declared_exist(c: &Connection, sql: &str) {
    for (kw, kind) in [
        ("TABLE", "table"),
        ("INDEX", "index"),
        ("TRIGGER", "trigger"),
    ] {
        let want = declared(sql, kw);
        let have = names_of(c, kind);
        assert!(!want.is_empty() || kw == "TRIGGER");
        for w in &want {
            assert!(have.contains(w), "{kind} {w} declared in §5e but missing");
        }
    }
}

#[test]
fn every_declared_object_exists() {
    let c = main_db();
    assert_all_declared_exist(&c, MAIN[0].sql);
    assert_eq!(
        declared(MAIN[0].sql, "TRIGGER"),
        BTreeSet::from(["runtime_errors_keep1000".to_string()])
    );
    let i = insights_db();
    assert_all_declared_exist(&i, INSIGHTS[0].sql);
}

#[test]
fn partial_indexes_are_partial() {
    let c = main_db();
    for idx in [
        "builds_queue",
        "builds_cache",
        "blobs_unref",
        "grants_expiry",
        "public_links_part",
        "access_asks_open",
        "invites_open",
    ] {
        let sql: String = c
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='index' AND name=?1",
                [idx],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| panic!("index {idx} missing"));
        assert!(
            sql.contains(" WHERE "),
            "{idx} must be a partial index: {sql}"
        );
    }
}

// ------------------------------------------------------------------ row helpers

type Row<'a> = Vec<(&'a str, String)>;

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn insert_sql(table: &str, base: &Row, over: &[(&str, &str)]) -> String {
    let mut cols = Vec::new();
    let mut vals = Vec::new();
    for (c, v) in base {
        cols.push(c.to_string());
        vals.push(
            over.iter()
                .find(|(k, _)| k == c)
                .map(|(_, v)| v.to_string())
                .unwrap_or(v.clone()),
        );
    }
    for (k, v) in over {
        if !base.iter().any(|(c, _)| c == k) {
            cols.push(k.to_string());
            vals.push(v.to_string());
        }
    }
    format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        cols.join(", "),
        vals.join(", ")
    )
}

fn ok(c: &Connection, table: &str, base: &Row, over: &[(&str, &str)]) {
    let sql = insert_sql(table, base, over);
    c.execute_batch(&format!("SAVEPOINT t; {sql}; ROLLBACK TO t; RELEASE t;"))
        .unwrap_or_else(|e| panic!("expected ok: {sql}: {e}"));
}

fn fails(c: &Connection, table: &str, base: &Row, over: &[(&str, &str)], what: &str) {
    let sql = insert_sql(table, base, over);
    match c.execute(&sql, []) {
        Ok(_) => panic!("expected {what} rejection: {sql}"),
        Err(e) => {
            let m = e.to_string();
            assert!(m.contains(what), "expected {what}, got {m}: {sql}");
        }
    }
}

fn check(c: &Connection, table: &str, base: &Row, over: &[(&str, &str)]) {
    // The valid base row must pass, so a rejection is the override's fault.
    ok(c, table, base, &[]);
    fails(c, table, base, over, "CHECK constraint failed");
}

fn fk(c: &Connection, table: &str, base: &Row, over: &[(&str, &str)]) {
    ok(c, table, base, &[]);
    fails(c, table, base, over, "FOREIGN KEY constraint failed");
}

fn seed(c: &Connection) {
    c.execute_batch(
        "INSERT INTO users (id, auth_source, idp_issuer, idp_sub, username, display_name, created_at, synced_at)
           VALUES ('u_1', 'oidc', 'https://idp.pf.test', 'sub-1', 'admin', 'The admin', 0, 0);
         INSERT INTO spaces (id, slug, kind, title, owner_user_id, created_at) VALUES ('sp_1', '~admin', 'personal', 'Mine', 'u_1', 0);
         INSERT INTO parts (id, name, space_id, title, kind, owner_user_id, created_by_user, created_at, updated_at)
           VALUES ('p_1', 'alpha', 'sp_1', 'Alpha', 'html', 'u_1', 'u_1', 0, 0);
         INSERT INTO blobs (hash, size, created_at) VALUES ('h1', 1, 0);
         INSERT INTO versions (id, part_id, n, manifest_hash, entry, author_user, via, created_at)
           VALUES ('v1', 'p_1', 1, 'v1', 'index.html', 'u_1', 'api', 0);
         INSERT INTO builds (id, version_id, cache_key, toolchain, method, status, queued_at)
           VALUES (1, 'v1', 'k', 't', 'server_build', 'ok', 0);
         INSERT INTO public_links (id, token_hash, part_id, created_by, created_at) VALUES ('l_1', x'01', 'p_1', 'u_1', 0);
         INSERT INTO agent_connections (id, kind, user_id, agent_name, scopes, spaces, expires_at, created_at)
           VALUES ('c_1', 'pat', 'u_1', 'Agent', '[]', '[]', 9, 0);
         INSERT INTO events (at, type, payload) VALUES (0, 'part.created', '{}');
         INSERT INTO container_sets (id, part_id, version_id, state) VALUES (1, 'p_1', 'v1', 'healthy');",
    )
    .unwrap();
}

fn r<'a>(pairs: &[(&'a str, &str)]) -> Row<'a> {
    pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
}

// ------------------------------------------------------------------ identity, spaces, parts, names, tags, notes

#[test]
fn checks_identity_and_parts() {
    let c = main_db();
    let user = r(&[
        ("id", "'u_2'"),
        ("auth_source", "'oidc'"),
        ("idp_issuer", "'https://idp.pf.test'"),
        ("idp_sub", "'sub-2'"),
        ("username", "'member'"),
        ("display_name", "'A member'"),
        ("created_at", "0"),
        ("synced_at", "0"),
    ]);
    check(&c, "users", &user, &[("idp_sub", "NULL")]);
    check(&c, "users", &user, &[("auth_source", "'local'")]); // local with an issuer
    check(&c, "users", &user, &[("status", "'gone'")]);

    let group = r(&[
        ("name", "'pf-family'"),
        ("source", "'local'"),
        ("managed", "1"),
        ("synced_at", "0"),
    ]);
    check(&c, "groups", &group, &[("name", "'family'")]); // managed, not pf-*
    check(&c, "groups", &group, &[("managed", "0")]); // local but unmanaged

    let space = r(&[
        ("id", "'sp_2'"),
        ("slug", "'~member'"),
        ("kind", "'personal'"),
        ("title", "'T'"),
        ("owner_user_id", "'u_1'"),
        ("created_at", "0"),
    ]);
    // u_1 already has a personal space: use a shared space as the valid base.
    let shared = r(&[
        ("id", "'sp_2'"),
        ("slug", "'family'"),
        ("kind", "'shared'"),
        ("title", "'T'"),
        ("created_at", "0"),
    ]);
    check(&c, "spaces", &shared, &[("kind", "'personal'")]); // personal without owner
    fails(&c, "spaces", &space, &[], "UNIQUE constraint failed"); // one personal space per person

    let part = r(&[
        ("id", "'p_2'"),
        ("name", "'beta'"),
        ("space_id", "'sp_1'"),
        ("title", "'B'"),
        ("kind", "'html'"),
        ("owner_user_id", "'u_1'"),
        ("created_by_user", "'u_1'"),
        ("created_at", "0"),
        ("updated_at", "0"),
    ]);
    let n59 = q(&"a".repeat(59));
    for bad in ["'-a'", "'a-'", "'a--b'", "'A'", n59.as_str(), "'a_b'", "''"] {
        check(&c, "parts", &part, &[("name", bad)]);
    }
    ok(&c, "parts", &part, &[("name", &q(&"a".repeat(58)))]);
    ok(&c, "parts", &part, &[("name", "'a-b-9'")]);
    check(&c, "parts", &part, &[("kind", "'pdf'")]);
    check(&c, "parts", &part, &[("preview", "'{not json'")]);
    ok(&c, "parts", &part, &[("preview", "'{\"video\":null}'")]);
    check(&c, "parts", &part, &[("api_prefix", "'api'")]);

    let name = r(&[
        ("name", "'alpha'"),
        ("part_id", "'p_1'"),
        ("state", "'active'"),
        ("created_at", "0"),
    ]);
    check(&c, "names", &name, &[("state", "'retired'")]);

    ok(
        &c,
        "part_tags",
        &r(&[("part_id", "'p_1'"), ("tag", "'x'")]),
        &[],
    );
    let note = r(&[
        ("part_id", "'p_1'"),
        ("n", "1"),
        ("body", "'hi'"),
        ("author_user", "'u_1'"),
        ("created_at", "0"),
    ]);
    check(&c, "part_notes", &note, &[("body", &q(&"x".repeat(16385)))]);
    ok(&c, "part_notes", &note, &[("body", &q(&"x".repeat(16384)))]);
}

// ------------------------------------------------------------------ versions, builds, blobs, grants, links, invites

#[test]
fn checks_builds_and_access() {
    let c = main_db();
    let version = r(&[
        ("id", "'v2'"),
        ("part_id", "'p_1'"),
        ("n", "2"),
        ("parent_id", "'v1'"),
        ("manifest_hash", "'v2'"),
        ("entry", "'index.html'"),
        ("author_user", "'u_1'"),
        ("via", "'api'"),
        ("created_at", "0"),
    ]);
    check(&c, "versions", &version, &[("via", "'email'")]);
    check(&c, "versions", &version, &[("live_gate", "'never'")]);
    fails(
        &c,
        "versions",
        &version,
        &[("n", "1")],
        "UNIQUE constraint failed",
    );

    let build = r(&[
        ("id", "2"),
        ("version_id", "'v1'"),
        ("cache_key", "'k2'"),
        ("toolchain", "'t'"),
        ("method", "'server_build'"),
        ("status", "'queued'"),
        ("queued_at", "0"),
    ]);
    check(&c, "builds", &build, &[("status", "'done'")]);
    check(&c, "builds", &build, &[("method", "'magic'")]);
    check(&c, "builds", &build, &[("target", "'both'")]);
    check(&c, "builds", &build, &[("gate_check", "'maybe'")]);
    check(&c, "builds", &build, &[("warnings", "'['")]);
    check(&c, "builds", &build, &[("errors", "'{'")]);
    ok(&c, "builds", &build, &[("sbom_hash", "'h1'")]);

    let draft = r(&[
        ("user_id", "'u_1'"),
        ("part_id", "'p_1'"),
        ("base_version_id", "'v1'"),
        ("files", "'{}'"),
        ("updated_at", "0"),
    ]);
    check(&c, "drafts", &draft, &[("files", "'nope'")]);

    let grant = r(&[
        ("target_kind", "'part'"),
        ("target_id", "'p_1'"),
        ("subject", "'user:u_1'"),
        ("role", "1"),
        ("created_by", "'u_1'"),
        ("created_at", "0"),
    ]);
    check(&c, "grants", &grant, &[("subject", "'link:x'")]);
    check(&c, "grants", &grant, &[("role", "4")]);
    check(&c, "grants", &grant, &[("target_kind", "'folder'")]);
    ok(&c, "grants", &grant, &[("subject", "'everyone-signed-in'")]);
    ok(&c, "grants", &grant, &[("subject", "'group:family'")]);

    let link = r(&[
        ("id", "'l_2'"),
        ("token_hash", "x'02'"),
        ("part_id", "'p_1'"),
        ("created_by", "'u_1'"),
        ("created_at", "0"),
    ]);
    check(&c, "public_links", &link, &[("role", "2")]);
    check(&c, "public_links", &link, &[("mode", "'edit'")]);
    check(&c, "public_links", &link, &[("preview", "'gif'")]);
    let default_preview: String = c
        .query_row("SELECT preview FROM public_links WHERE id='l_1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(default_preview, "video");

    let invite = r(&[
        ("id", "'i_1'"),
        ("kind", "'single'"),
        ("status", "'sent'"),
        ("created_at", "0"),
        ("expires_at", "9"),
    ]);
    check(&c, "invites", &invite, &[("uses", "2")]);
    check(&c, "invites", &invite, &[("groups", "'[x'")]);
    check(&c, "invites", &invite, &[("grant_role", "4")]);

    let ask = r(&[
        ("part_id", "'p_1'"),
        ("user_id", "'u_1'"),
        ("note", "'please'"),
        ("created_at", "0"),
    ]);
    check(&c, "access_asks", &ask, &[("note", &q(&"x".repeat(281)))]);
    check(&c, "access_asks", &ask, &[("closed_as", "'ignored'")]);

    let audit = r(&[
        ("at", "0"),
        ("action", "'grant.set'"),
        ("target_kind", "'part'"),
        ("target_id", "'p_1'"),
    ]);
    check(&c, "audit", &audit, &[("before", "'{'")]);
    check(&c, "audit", &audit, &[("after", "'x'")]);
}

#[test]
fn foreign_keys() {
    let c = main_db();
    let vf = r(&[
        ("version_id", "'v1'"),
        ("path", "'index.html'"),
        ("blob_hash", "'h1'"),
        ("size", "1"),
        ("mime", "'text/html'"),
    ]);
    fk(&c, "version_files", &vf, &[("blob_hash", "'missing'")]);
    let build = r(&[
        ("id", "3"),
        ("version_id", "'v1'"),
        ("cache_key", "'k3'"),
        ("toolchain", "'t'"),
        ("method", "'server_build'"),
        ("status", "'queued'"),
        ("queued_at", "0"),
    ]);
    fk(&c, "builds", &build, &[("version_id", "'nope'")]);
    let part = r(&[
        ("id", "'p_3'"),
        ("name", "'gamma'"),
        ("space_id", "'sp_1'"),
        ("title", "'G'"),
        ("kind", "'html'"),
        ("owner_user_id", "'u_1'"),
        ("created_by_user", "'u_1'"),
        ("created_at", "0"),
        ("updated_at", "0"),
    ]);
    fk(&c, "parts", &part, &[("forked_from", "'no-such-version'")]);
    fk(&c, "parts", &part, &[("space_id", "'sp_nope'")]);
}

#[test]
fn build_deps_and_advisories() {
    let c = main_db();
    let idx = names_of(&c, "index");
    assert!(idx.contains("build_deps_build") && idx.contains("build_deps_pkg"));
    let dep = r(&[
        ("build_id", "1"),
        ("ecosystem", "'npm'"),
        ("name", "'react'"),
        ("version", "'18.3.1'"),
    ]);
    ok(&c, "build_deps", &dep, &[]);
    c.execute(&insert_sql("build_deps", &dep, &[]), []).unwrap();
    let adv = r(&[
        ("part_id", "'p_1'"),
        ("build_id", "1"),
        ("ecosystem", "'npm'"),
        ("package", "'react'"),
        ("version", "'18.3.1'"),
        ("osv_id", "'GHSA-1'"),
        ("severity", "'high'"),
        ("first_seen", "0"),
    ]);
    check(&c, "advisories", &adv, &[("state", "'ignored'")]);
    check(
        &c,
        "advisories",
        &adv,
        &[("state", "'ignored'"), ("ignored_by", "'user:u_1'")],
    );
    ok(
        &c,
        "advisories",
        &adv,
        &[
            ("state", "'ignored'"),
            ("ignored_by", "'user:u_1'"),
            ("reason", "'not used'"),
        ],
    );
    check(
        &c,
        "advisories",
        &adv,
        &[("ignored_by", "'user:u_1'"), ("reason", "'r'")],
    ); // open, yet ignored
    check(&c, "advisories", &adv, &[("severity", "'meh'")]);
    c.execute(&insert_sql("advisories", &adv, &[]), []).unwrap();
    fails(&c, "advisories", &adv, &[], "UNIQUE constraint failed");
    // Both cascade with their build.
    c.execute("DELETE FROM builds WHERE id = 1", []).unwrap();
    let n: i64 = c
        .query_row(
            "SELECT (SELECT COUNT(*) FROM build_deps) + (SELECT COUNT(*) FROM advisories)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0);
}

// ------------------------------------------------------------------ agents, data, comments, assets, runtime errors

#[test]
fn checks_agents_and_data() {
    let c = main_db();
    let conn = r(&[
        ("id", "'c_2'"),
        ("kind", "'pat'"),
        ("user_id", "'u_1'"),
        ("agent_name", "'A'"),
        ("scopes", "'[]'"),
        ("spaces", "'[]'"),
        ("expires_at", "9"),
        ("created_at", "0"),
    ]);
    check(&c, "agent_connections", &conn, &[("user_id", "NULL")]); // person without user
    check(
        &c,
        "agent_connections",
        &conn,
        &[("principal_kind", "'service'")],
    ); // service with a user
    check(&c, "agent_connections", &conn, &[("scopes", "'x'")]);
    let tok = r(&[
        ("hash", "x'aa'"),
        ("kind", "'pat'"),
        ("prefix", "'pf_pat_'"),
        ("connection_id", "'c_1'"),
        ("resource", "'https://parts.pf.test/'"),
        ("issued_at", "0"),
        ("expires_at", "9"),
    ]);
    check(&c, "oauth_tokens", &tok, &[("kind", "'id'")]);
    let doc = r(&[
        ("part_id", "'p_1'"),
        ("scope", "'shared'"),
        ("collection", "'__storage'"),
        ("doc_id", "'k'"),
        ("body", "x'7b7d'"),
        ("rev", "1"),
        ("updated_at", "0"),
        ("updated_by", "'user:u_1'"),
    ]);
    check(&c, "data_docs", &doc, &[("owner", "'u_1'")]); // shared doc with an owner
    check(&c, "data_docs", &doc, &[("scope", "'personal'")]); // personal without owner
    check(&c, "data_docs", &doc, &[("body", "zeroblob(262145)")]);
    ok(&c, "data_docs", &doc, &[("body", "zeroblob(262144)")]);
    let snap = r(&[
        ("part_id", "'p_1'"),
        ("taken_at", "0"),
        ("blob_hash", "'h1'"),
        ("docs", "0"),
        ("bytes", "0"),
    ]);
    check(&c, "data_snapshots", &snap, &[("reason", "'manual'")]);
    let pol = r(&[
        ("part_id", "'p_1'"),
        ("set_by", "'user:u_1'"),
        ("set_at", "0"),
    ]);
    check(&c, "data_snapshot_policies", &pol, &[("mode", "'weekly'")]);
    let asset = r(&[
        ("id", "'a_1'"),
        ("part_id", "'p_1'"),
        ("blob_hash", "'h1'"),
        ("mime", "'image/png'"),
        ("size", "26214400"),
        ("name", "'x.png'"),
        ("uploaded_by", "'user:u_1'"),
        ("created_at", "0"),
    ]);
    check(&c, "assets", &asset, &[("size", "26214401")]);
    let comment = r(&[
        ("id", "'cm_1'"),
        ("part_id", "'p_1'"),
        ("thread_id", "'cm_1'"),
        ("body", "'hi'"),
        ("provenance", "'person'"),
        ("created_at", "0"),
    ]);
    check(&c, "comments", &comment, &[("provenance", "'guest'")]);
    check(&c, "comments", &comment, &[("anchor", "'{'")]);
    check(&c, "comments", &comment, &[("agent_state", "'closed'")]);
    let lease = r(&[
        ("id", "'ls_1'"),
        ("part_id", "'p_1'"),
        ("holder_label", "'Agent'"),
        ("expires_at", "9"),
        ("created_at", "0"),
    ]);
    check(&c, "leases", &lease, &[("scope", "'dir'")]);
}

#[test]
fn runtime_errors_keep_1000_per_part() {
    let c = main_db();
    c.execute_batch("BEGIN").unwrap();
    {
        let mut s = c
            .prepare(
                "INSERT INTO runtime_errors (part_id, version_id, fingerprint, kind, message, first_at, last_at)
                 VALUES ('p_1', 'v1', ?1, 'error', 'boom', ?2, ?2)",
            )
            .unwrap();
        for i in 0..1001i64 {
            // last_at grows with i, so i = 0 is the oldest.
            s.execute(rusqlite::params![format!("f{i}"), 1000 + i])
                .unwrap();
        }
    }
    c.execute_batch("COMMIT").unwrap();
    let (n, min): (i64, i64) = c
        .query_row(
            "SELECT COUNT(*), MIN(last_at) FROM runtime_errors WHERE part_id='p_1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(n, 1000);
    assert_eq!(min, 1001, "the oldest by last_at was dropped");
    let base = r(&[
        ("part_id", "'p_1'"),
        ("version_id", "'v1'"),
        ("fingerprint", "'zz'"),
        ("kind", "'error'"),
        ("message", "'m'"),
        ("first_at", "0"),
        ("last_at", "0"),
    ]);
    check(&c, "runtime_errors", &base, &[("kind", "'warning'")]);
    check(&c, "runtime_errors", &base, &[("source", "'server'")]);
}

// ------------------------------------------------------------------ backends, events, webhooks, render jobs, parts_fts

#[test]
fn checks_backends_and_the_rest() {
    let c = main_db();
    let secret = r(&[
        ("part_id", "'p_1'"),
        ("name", "'API_KEY'"),
        ("value_enc", "x'00'"),
        ("set_by_user", "'u_1'"),
        ("set_at", "0"),
    ]);
    check(&c, "part_secrets", &secret, &[("name", "'lower'")]);
    check(
        &c,
        "part_secrets",
        &secret,
        &[("name", &q(&"A".repeat(65)))],
    );
    ok(
        &c,
        "part_secrets",
        &secret,
        &[("name", &q(&"A".repeat(64)))],
    );
    check(&c, "part_secrets", &secret, &[("inject_as", "'stdin'")]);
    let container = r(&[
        ("id", "'ct_1'"),
        ("set_id", "1"),
        ("service", "'web'"),
        ("role", "'main'"),
        ("image_digest", "'sha256:x'"),
        ("state", "'running'"),
    ]);
    check(&c, "containers", &container, &[("role", "'sidecar'")]);
    let render = r(&[
        ("part_id", "'p_1'"),
        ("version_id", "'v1'"),
        ("viewport", "'mobile'"),
        ("purpose", "'check'"),
        ("status", "'queued'"),
        ("queued_at", "0"),
    ]);
    check(&c, "render_jobs", &render, &[("purpose", "'screenshot'")]);
    ok(&c, "render_jobs", &render, &[("purpose", "'preview'")]);
    check(&c, "render_jobs", &render, &[("result", "'{'")]);
    let hook = r(&[
        ("target", "'t'"),
        ("event_seq", "1"),
        ("next_at", "0"),
        ("status", "'pending'"),
    ]);
    check(&c, "webhook_deliveries", &hook, &[("status", "'failed'")]);
    fk(&c, "webhook_deliveries", &hook, &[("event_seq", "999")]);
    let event = r(&[("at", "0"), ("type", "'part.created'"), ("payload", "'{}'")]);
    check(&c, "events", &event, &[("payload", "'not json'")]);
    let image = r(&[
        ("part_id", "'p_1'"),
        ("digest", "'sha256:x'"),
        ("source", "'server_build'"),
        ("size", "1"),
        ("created_at", "0"),
    ]);
    check(&c, "backend_images", &image, &[("source", "'pulled'")]);
    let cs = r(&[
        ("part_id", "'p_1'"),
        ("version_id", "'v1'"),
        ("state", "'starting'"),
    ]);
    fails(&c, "container_sets", &cs, &[], "UNIQUE constraint failed");
    c.execute("DELETE FROM container_sets", []).unwrap();
    check(&c, "container_sets", &cs, &[("state", "'paused'")]);
}

#[test]
fn previews_in_schema_v1() {
    let c = main_db();
    let prev = r(&[
        ("part_id", "'p_1'"),
        ("version_id", "'v1'"),
        ("source", "'render'"),
        ("state", "'ready'"),
        ("updated_at", "0"),
    ]);
    check(&c, "part_previews", &prev, &[("source", "'magic'")]);
    check(&c, "part_previews", &prev, &[("state", "'gone'")]);
    fk(&c, "part_previews", &prev, &[("version_id", "'nope'")]);
}

#[test]
fn parts_fts_is_fts5_without_triggers() {
    let c = main_db();
    let sql: String = c
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name = 'parts_fts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(sql.contains("USING fts5"), "{sql}");
    let mut s = c
        .prepare("SELECT name FROM pragma_table_info('parts_fts') ORDER BY cid")
        .unwrap();
    let cols: Vec<String> = s
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        cols,
        [
            "part_id",
            "title",
            "description",
            "tags",
            "body",
            "notes",
            "comments"
        ]
    );
    let n: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'trigger' AND sql LIKE '%parts_fts%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0);
    c.execute(
        "INSERT INTO parts_fts (part_id, title) VALUES ('p_1', 'Sleep chart')",
        [],
    )
    .unwrap();
    let hit: String = c
        .query_row(
            "SELECT part_id FROM parts_fts WHERE parts_fts MATCH 'sle*'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hit, "p_1");
}

// ------------------------------------------------------------------ insights

#[test]
fn insights_checks() {
    let c = insights_db();
    let ev = r(&[("at", "0"), ("kind", "'view'")]);
    check(&c, "events_raw", &ev, &[("surface", "'email'")]);
    check(&c, "events_raw", &ev, &[("attrs", "'{'")]);
    let tr = r(&[
        ("trace_id", "x'01'"),
        ("request_id", "'r'"),
        ("started_at", "0"),
        ("duration_us", "1"),
        ("root", "'/'"),
        ("status", "'ok'"),
        ("kept_because", "'error'"),
    ]);
    check(&c, "traces", &tr, &[("kept_because", "'whim'")]);
    let seg = r(&[
        ("path", "'app/x'"),
        ("stream", "'app'"),
        ("first_at", "0"),
        ("last_at", "0"),
        ("bytes", "0"),
        ("lines", "0"),
    ]);
    check(&c, "log_segments", &seg, &[("stream", "'backend'")]); // backend needs part_id
    check(&c, "log_segments", &seg, &[("part_id", "'p_1'")]); // app must not have one
    check(&c, "log_segments", &seg, &[("max_level", "'fatal'")]);
    let jr = r(&[
        ("at", "0"),
        ("store", "'blobs'"),
        ("action", "'gc'"),
        ("items", "0"),
        ("freed_bytes", "0"),
        ("size_after", "0"),
    ]);
    check(&c, "janitor_runs", &jr, &[("action", "'burn'")]);
    c.execute_batch("INSERT INTO traces (trace_id, request_id, started_at, duration_us, root, status, kept_because) VALUES (x'01','r',0,1,'/','ok','error')").unwrap();
    let sp = r(&[
        ("trace_id", "x'01'"),
        ("span_id", "1"),
        ("name", "'x'"),
        ("start_us", "0"),
        ("duration_us", "1"),
    ]);
    check(&c, "spans", &sp, &[("fields", "'{'")]);
}

#[test]
fn insights_has_no_content_columns() {
    // Supports the no-content-in-logs rule: no column that could hold an IP address or part
    // content. `rollups_daily.value` is an INTEGER count, which is allowed; a text or blob
    // `value` column would not be.
    let c = insights_db();
    let tables = names_of(&c, "table");
    for t in tables.iter().filter(|t| !t.starts_with('_')) {
        let mut s = c
            .prepare(&format!("SELECT name, type FROM pragma_table_info('{t}')"))
            .unwrap();
        let cols: Vec<(String, String)> = s
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for (name, ty) in cols {
            let n = name.to_lowercase();
            let parts: Vec<&str> = n.split('_').collect();
            assert!(!parts.contains(&"ip"), "{t}.{name} looks like an IP column");
            assert!(
                !parts.contains(&"body") && !parts.contains(&"content"),
                "{t}.{name} looks like content"
            );
            if parts.contains(&"value") {
                assert_eq!(ty, "INTEGER", "{t}.{name} may only be a number");
            }
        }
    }
}
