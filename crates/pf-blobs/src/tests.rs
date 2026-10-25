use super::*;
use std::sync::{Arc, Mutex};

/// Capture JSON log lines emitted while `f` runs (thread-local subscriber).
pub(crate) fn capture_logs<T>(f: impl FnOnce() -> T) -> (T, String) {
    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let buf = Buf::default();
    let w = buf.clone();
    let sub = tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_writer(move || w.clone())
        .finish();
    let out = tracing::subscriber::with_default(sub, || {
        tracing::callsite::rebuild_interest_cache();
        f()
    });
    let s = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    (out, s)
}

#[test]
fn put_has_get_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let s = BlobStore::open(dir.path()).unwrap();
    let p = s.put(b"hello").unwrap();
    assert!(p.created);
    assert_eq!(
        p.hash,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    assert!(s.has(&p.hash));
    assert_eq!(s.get(&p.hash).unwrap(), b"hello");
    assert!(
        s.path(&p.hash)
            .ends_with("2c/f2/2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
    );
    let again = s.put(b"hello").unwrap();
    assert!(!again.created);
    assert!(matches!(
        s.get(&sha256_hex(b"nope")),
        Err(BlobError::NotFound(_))
    ));
    assert!(matches!(s.get("../etc"), Err(BlobError::BadHash(_))));
}

#[test]
fn concurrent_same_hash_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let s = BlobStore::open(dir.path()).unwrap();
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let data = Arc::new(data);
    let handles: Vec<_> = (0..16)
        .map(|_| {
            let s = s.clone();
            let d = data.clone();
            std::thread::spawn(move || s.put(&d).unwrap())
        })
        .collect();
    let puts: Vec<Put> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(puts.iter().all(|p| p.hash == puts[0].hash));
    assert!(puts.iter().filter(|p| p.created).count() >= 1);
    assert_eq!(s.get(&puts[0].hash).unwrap(), *data);
    // No temp file is left behind.
    assert_eq!(fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
}

#[test]
fn bitflip_detected_on_read() {
    let dir = tempfile::tempdir().unwrap();
    let s = BlobStore::open(dir.path()).unwrap();
    let p = s.put(b"precious bytes").unwrap();
    let path = s.path(&p.hash);
    let mut b = fs::read(&path).unwrap();
    b[3] ^= 0x01;
    fs::write(&path, b).unwrap();
    let (r, logs) = capture_logs(|| s.get(&p.hash));
    assert!(matches!(r, Err(BlobError::Corrupt { .. })), "{r:?}");
    let line = logs
        .lines()
        .find(|l| l.contains("\"event\":\"blob.corrupt\""))
        .expect(&logs);
    assert!(line.contains(&format!("\"hash\":\"{}\"", p.hash)), "{line}");
}

struct Full;
impl DiskBudget for Full {
    fn is_full(&self, _incoming: u64) -> bool {
        true
    }
}

#[test]
fn storage_full_from_budget_hook() {
    let dir = tempfile::tempdir().unwrap();
    let s = BlobStore::with_budget(dir.path(), Arc::new(Full)).unwrap();
    assert!(matches!(s.put(b"x"), Err(BlobError::StorageFull)));
    assert!(!s.has(&sha256_hex(b"x")));
}

#[test]
fn stale_temp_swept() {
    let dir = tempfile::tempdir().unwrap();
    let s = BlobStore::open(dir.path()).unwrap();
    let stale = dir.path().join("tmp").join("abc.123.tmp");
    fs::write(&stale, b"half a blob").unwrap();
    // A fresh temp file survives a sweep with a long threshold.
    assert_eq!(s.sweep_stale_temps(Duration::from_secs(3600)).unwrap(), 0);
    assert!(stale.exists());
    assert_eq!(s.sweep_stale_temps(Duration::ZERO).unwrap(), 1);
    assert!(!stale.exists());
}

#[test]
fn recent_dedup_renews_grace_before_database_reference_commit() {
    let dir = tempfile::tempdir().unwrap();
    let s = BlobStore::open(dir.path()).unwrap();
    let put = s.put(b"pending upload").unwrap();
    File::open(s.path(&put.hash))
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
        .unwrap();
    assert!(!s.put(b"pending upload").unwrap().created);
    let cutoff = SystemTime::now() - Duration::from_secs(7 * 86400);
    assert_eq!(s.remove_if_older_than(&put.hash, cutoff).unwrap(), None);
    assert_eq!(s.get(&put.hash).unwrap(), b"pending upload");
}

#[test]
fn paused_gc_and_dedup_coordinate_clones_and_independently_opened_stores() {
    use std::sync::mpsc;
    for reopen in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let s = BlobStore::open(dir.path()).unwrap();
        let put = s.put(b"racing upload").unwrap();
        File::open(s.path(&put.hash))
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
            .unwrap();
        let gc = s.clone();
        let hash = put.hash.clone();
        let (paused_tx, paused_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let collector = std::thread::spawn(move || {
            gc.remove_guarded_with(&hash, Some(SystemTime::now()), || {
                paused_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            })
            .unwrap()
        });
        paused_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let uploader = if reopen {
            BlobStore::open(dir.path()).unwrap()
        } else {
            s.clone()
        };
        let (done_tx, done_rx) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            done_tx.send(uploader.put(b"racing upload")).unwrap();
        });
        assert!(
            matches!(
                done_rx.recv_timeout(Duration::from_millis(50)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "put must wait for GC's atomic mtime/delete section"
        );
        resume_tx.send(()).unwrap();
        assert_eq!(collector.join().unwrap(), Some(13));
        assert!(
            done_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap()
                .created,
            "a put after sweep must recreate bytes, never return a stale dedup result"
        );
        writer.join().unwrap();
        assert_eq!(s.get(&put.hash).unwrap(), b"racing upload");
    }
}
