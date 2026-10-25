//! Re-executed build workers. Pipe I/O happens off the caller so even a blocked write
//! is covered by the deadline. A malformed response is killed before reaping.
use crate::{BuildInput, BuildOutput};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

pub const MEM_CAP: u64 = 1 << 30;
pub const DEADLINE: Duration = Duration::from_secs(10);
pub const MAX_FRAME: usize = 64 << 20;

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    Ping,
    Build(BuildInput),
}
#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Pong,
    Built(BuildOutput),
    Failed { code: String, message: String },
}

pub fn write_frame<T: Serialize>(out: &mut impl Write, value: &T) -> io::Result<()> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).map_err(io::Error::other)?;
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::other("build frame exceeds 64 MiB"));
    }
    out.write_all(&(bytes.len() as u32).to_be_bytes())?;
    out.write_all(&bytes)?;
    out.flush()
}
pub fn read_frame<T: for<'de> Deserialize<'de>>(input: &mut impl Read) -> io::Result<Option<T>> {
    let mut size = [0u8; 4];
    if input.read(&mut size[..1])? == 0 {
        return Ok(None);
    }
    input.read_exact(&mut size[1..])?;
    let size = u32::from_be_bytes(size) as usize;
    if size > MAX_FRAME {
        return Err(io::Error::other("build frame exceeds 64 MiB"));
    }
    let mut bytes = vec![0; size];
    input.read_exact(&mut bytes)?;
    ciborium::from_reader(bytes.as_slice())
        .map(Some)
        .map_err(io::Error::other)
}

/// Call before processing any bytes. Linux filters all socket creation/connect syscalls;
/// macOS workers are spawned through sandbox-exec's deny-network profile.
pub fn apply_limits() -> io::Result<()> {
    let memory_limit = rlimit::Resource::AS.set(MEM_CAP, MEM_CAP);
    // macOS may reject lowering RLIMIT_AS below its reserved virtual address space.
    // The binary's counting allocator enforces the same cap there.
    #[cfg(not(target_os = "macos"))]
    memory_limit.map_err(io::Error::other)?;
    #[cfg(target_os = "macos")]
    let _ = memory_limit;
    rlimit::Resource::CORE.set(0, 0).map_err(io::Error::other)?;
    #[cfg(target_os = "linux")]
    {
        use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, TargetArch};
        let rules = [
            libc::SYS_socket,
            libc::SYS_socketpair,
            libc::SYS_connect,
            libc::SYS_accept,
            libc::SYS_accept4,
            libc::SYS_bind,
            libc::SYS_listen,
            libc::SYS_sendto,
            libc::SYS_sendmsg,
            libc::SYS_sendmmsg,
        ]
        .into_iter()
        .map(|n| (n, Vec::new()))
        .collect();
        let filter = SeccompFilter::new(
            rules,
            SeccompAction::Allow,
            SeccompAction::Errno(libc::EPERM as u32),
            TargetArch::try_from(std::env::consts::ARCH).map_err(io::Error::other)?,
        )
        .map_err(io::Error::other)?;
        let program: BpfProgram = filter.try_into().map_err(io::Error::other)?;
        seccompiler::apply_filter(&program).map_err(io::Error::other)?;
    }
    Ok(())
}

pub fn worker_main() -> io::Result<()> {
    apply_limits()?;
    let (mut input, mut output) = (io::stdin().lock(), io::stdout().lock());
    while let Some(request) = read_frame::<Request>(&mut input)? {
        let response = match request {
            Request::Ping => Response::Pong,
            Request::Build(job) => {
                eprintln!(
                    "{}",
                    serde_json::json!({"event":"build.worker","request_id":job.request_id,"outcome":"started"})
                );
                #[cfg(feature = "dev")]
                if let Some(fault) = job.files.get(".pf/fault") {
                    match fault.bytes.as_slice() {
                        b"panic" => panic!("injected build panic"),
                        b"hang" => loop {
                            thread::sleep(Duration::from_secs(3600));
                        },
                        b"oom" => {
                            let mut allocations = Vec::new();
                            loop {
                                allocations.push(vec![1u8; 64 << 20]);
                                std::hint::black_box(&allocations);
                            }
                        }
                        _ => {}
                    }
                }
                match crate::render::render(job) {
                    Ok(out) => Response::Built(out),
                    Err(message) => Response::Failed {
                        code: "build_failed".into(),
                        message,
                    },
                }
            }
        };
        write_frame(&mut output, &response)?;
    }
    Ok(())
}

pub struct Worker {
    child: Child,
    requests: mpsc::Sender<Request>,
    responses: mpsc::Receiver<io::Result<Response>>,
}

impl Worker {
    pub fn spawn(exe: &Path) -> io::Result<Self> {
        #[cfg(target_os = "macos")]
        let mut cmd = {
            let mut c = Command::new("/usr/bin/sandbox-exec");
            c.args(["-p", "(version 1)(allow default)(deny network*)"])
                .arg(exe);
            c
        };
        #[cfg(not(target_os = "macos"))]
        let mut cmd = Command::new(exe);
        let mut child = cmd
            .arg("build-worker")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("missing child input"))?;
        let mut output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing child output"))?;
        let (requests, rx) = mpsc::channel();
        let (tx, responses) = mpsc::channel();
        thread::Builder::new()
            .name("pf-build-pipe".into())
            .spawn(move || {
                while let Ok(request) = rx.recv() {
                    let result = write_frame(&mut input, &request).and_then(|()| {
                        read_frame::<Response>(&mut output)?
                            .ok_or_else(|| io::Error::other("build child EOF"))
                    });
                    let failed = result.is_err();
                    if tx.send(result).is_err() || failed {
                        break;
                    }
                }
            })?;
        Ok(Self {
            child,
            requests,
            responses,
        })
    }
    pub fn call(&mut self, request: Request, deadline: Duration) -> Result<Response, ChildError> {
        if self.requests.send(request).is_err() {
            return Err(self.crashed());
        }
        match self.responses.recv_timeout(deadline) {
            Ok(Ok(response)) => Ok(response),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.stop();
                Err(ChildError::Timeout)
            }
            _ => Err(self.crashed()),
        }
    }
    fn crashed(&mut self) -> ChildError {
        // On an invalid frame a worker may still be alive, so never wait before killing.
        let _ = self.child.kill();
        let status = self
            .child
            .wait()
            .map(|s| s.to_string())
            .unwrap_or_else(|e| e.to_string());
        ChildError::Crashed(status)
    }
    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}
#[derive(Debug, thiserror::Error)]
pub enum ChildError {
    #[error("{message}")]
    Failed { code: String, message: String },
    #[error("build child exceeded its deadline")]
    Timeout,
    #[error("build child crashed: {0}")]
    Crashed(String),
    #[error("could not spawn build child: {0}")]
    Spawn(#[from] io::Error),
}

pub fn pool_size_for(cores: usize) -> usize {
    (cores / 2).clamp(1, 2)
}
pub fn pool_size() -> usize {
    pool_size_for(
        thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
    )
}

/// A worker belongs to one queue lane. Dead workers are replaced on the next request,
/// so spawn failure is a failed build rather than a panic in the server.
pub struct Pool {
    exe: PathBuf,
    workers: Vec<Option<Worker>>,
    next: usize,
    pub deadline: Duration,
}
impl Pool {
    pub fn new(exe: &Path, size: usize) -> io::Result<Self> {
        if size == 0 {
            return Err(io::Error::other("worker pool cannot be empty"));
        }
        let workers = (0..size)
            .map(|_| Worker::spawn(exe).map(Some))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            exe: exe.into(),
            workers,
            next: 0,
            deadline: DEADLINE,
        })
    }
    pub fn build(&mut self, input: BuildInput) -> Result<BuildOutput, ChildError> {
        let index = self.next % self.workers.len();
        self.next = self.next.wrapping_add(1);
        if self.workers[index].is_none() {
            self.workers[index] = Some(Worker::spawn(&self.exe)?);
        }
        let result = self.workers[index]
            .as_mut()
            .expect("worker exists")
            .call(Request::Build(input), self.deadline);
        let outcome = match &result {
            Ok(Response::Built(_)) => "ok",
            Ok(_) => "failed",
            Err(ChildError::Timeout) => "timeout",
            Err(_) => "builder_crashed",
        };
        tracing::info!(event = "build.child", outcome);
        match result {
            Ok(Response::Built(out)) => Ok(out),
            Ok(Response::Failed { code, message }) => Err(ChildError::Failed { code, message }),
            Ok(Response::Pong) => {
                self.workers[index] = None;
                Err(ChildError::Crashed("unexpected pong".into()))
            }
            Err(error) => {
                self.workers[index] = None;
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_reject_oversize_and_partial_headers() {
        assert!(read_frame::<Request>(&mut &(u32::MAX.to_be_bytes())[..]).is_err());
        assert!(read_frame::<Request>(&mut &[0, 0][..]).is_err());
        assert!(read_frame::<Request>(&mut &[][..]).unwrap().is_none());
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &Request::Ping).unwrap();
        assert!(matches!(
            read_frame::<Request>(&mut bytes.as_slice()).unwrap(),
            Some(Request::Ping)
        ));
    }
}
