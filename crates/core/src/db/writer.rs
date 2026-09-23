//! The single database writer. Background jobs send write operations over a channel; the
//! writer commits them in batches (up to `max_batch` operations or `max_wait`, whichever
//! comes first). Each operation runs in its own savepoint, so one failure doesn't roll
//! back the rest of the batch.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rusqlite::Connection;

use crate::error::{AppError, AppResult, ErrorCode};

pub type WriteOp = Box<dyn FnOnce(&Connection) -> AppResult<()> + Send>;

struct Job {
    op: WriteOp,
    done: Option<tokio::sync::oneshot::Sender<AppResult<()>>>,
}

#[derive(Clone)]
pub struct Writer {
    tx: mpsc::Sender<Job>,
    commits: Arc<AtomicUsize>,
}

impl Writer {
    /// Starts the writer thread. It owns `conn` and stops when every `Writer` handle is dropped.
    pub fn spawn(
        mut conn: Connection,
        max_batch: usize,
        max_wait: Duration,
    ) -> (Writer, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel::<Job>();
        let commits = Arc::new(AtomicUsize::new(0));
        let counter = commits.clone();
        let handle = std::thread::spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut batch = vec![first];
                let deadline = Instant::now() + max_wait;
                while batch.len() < max_batch {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    match rx.recv_timeout(deadline - now) {
                        Ok(job) => batch.push(job),
                        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
                    }
                }
                commit_batch(&mut conn, batch);
                counter.fetch_add(1, Ordering::Relaxed);
            }
        });
        (Writer { tx, commits }, handle)
    }

    /// Queues a write and waits until it has been committed.
    pub async fn write(&self, op: WriteOp) -> AppResult<()> {
        let (done, wait) = tokio::sync::oneshot::channel();
        self.tx
            .send(Job {
                op,
                done: Some(done),
            })
            .map_err(|_| closed())?;
        wait.await.map_err(|_| closed())?
    }

    /// Queues a write without waiting (used for high-volume answer inserts).
    pub fn send(&self, op: WriteOp) -> AppResult<()> {
        self.tx.send(Job { op, done: None }).map_err(|_| closed())
    }

    /// Number of transactions committed so far.
    pub fn commits(&self) -> usize {
        self.commits.load(Ordering::Relaxed)
    }
}

fn closed() -> AppError {
    AppError::new(ErrorCode::Internal, "database writer has stopped")
}

fn commit_batch(conn: &mut Connection, batch: Vec<Job>) {
    let mut results = Vec::with_capacity(batch.len());
    let outcome = (|| -> rusqlite::Result<()> {
        let mut tx = conn.transaction()?;
        for job in batch {
            let mut sp = tx.savepoint()?;
            let res = (job.op)(&sp);
            if res.is_ok() {
                sp.commit()?;
            } else {
                sp.rollback()?;
            }
            results.push((job.done, res));
        }
        tx.commit()
    })();
    for (done, res) in results {
        let res = match (&outcome, res) {
            (Err(e), _) => Err(AppError::from(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("batch commit failed: {e}")),
            ))),
            (Ok(()), r) => r,
        };
        if let Some(done) = done {
            let _ = done.send(res);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        let conn = crate::db::open(&path).unwrap();
        conn.execute("INSERT INTO projects(title) VALUES ('p')", [])
            .unwrap();
        (dir, path)
    }

    #[tokio::test]
    async fn batches_many_small_writes() {
        let (_dir, path) = setup();
        let (writer, handle) = Writer::spawn(
            crate::db::open(&path).unwrap(),
            50,
            Duration::from_millis(250),
        );
        for i in 0..499 {
            writer
                .send(Box::new(move |c| {
                    c.execute(
                        "INSERT INTO settings(key, value) VALUES (?1, 'x')",
                        [format!("k{i}")],
                    )?;
                    Ok(())
                }))
                .unwrap();
        }
        writer
            .write(Box::new(|c| {
                c.execute("INSERT INTO settings(key, value) VALUES ('last', 'x')", [])?;
                Ok(())
            }))
            .await
            .unwrap();
        let commits = writer.commits();
        assert!(commits <= 11, "500 writes took {commits} commits");
        drop(writer);
        handle.join().unwrap();
        let n: i64 = crate::db::open(&path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 500);
    }

    #[tokio::test]
    async fn one_failing_write_does_not_undo_the_batch() {
        let (_dir, path) = setup();
        let (writer, _handle) = Writer::spawn(
            crate::db::open(&path).unwrap(),
            50,
            Duration::from_millis(100),
        );
        let ok = writer.write(Box::new(|c| {
            c.execute("INSERT INTO settings(key, value) VALUES ('a', '1')", [])?;
            Ok(())
        }));
        let bad = writer.write(Box::new(|c| {
            c.execute("INSERT INTO settings(key, value) VALUES ('a', '2')", [])?; // duplicate key
            Ok(())
        }));
        let (ok, bad) = tokio::join!(ok, bad);
        ok.unwrap();
        assert_eq!(bad.unwrap_err().code, ErrorCode::Database);
        let v: String = crate::db::open(&path)
            .unwrap()
            .query_row("SELECT value FROM settings WHERE key='a'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "1");
    }
}
