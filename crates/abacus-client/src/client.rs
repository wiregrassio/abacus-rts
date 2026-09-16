//! AbacusClient: connection to the Abacus daemon over UDS.
//!
//! NOTE: This crate depends on abacus-daemon for the wire protocol types
//! (encode_request, decode_response, Request, Response). This is a known
//! coupling. The wire protocol should be extracted to a shared crate
//! (abacus-proto or into abacus-core) in a future cleanup pass.
//!
//! The client-side transport (connect, send, recv_with_fds) is implemented
//! here rather than using the daemon's Connection struct, which lacks a
//! send_request method and has a private stream field.

use std::io::Write;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;

use abacus_core::error::{IoOperation, ProtocolFault, TransportError};
use abacus_core::interlock::{interlock_open, InterlockHandle};
use abacus_daemon::wire::{
    decode_response, encode_request, expected_fd_count, Request, Response,
    LENGTH_PREFIX_BYTES, MAX_MESSAGE_SIZE,
};

use crate::interlock::{AttachedInterlock, AttachedWaitCounter, ClockHandle, Interlock};
use crate::process_clock::ProcessClock;
use crate::types::WatchedWord;
use crate::wait_barrier::WaitBarrier;
use crate::wait_counter::WaitCounter;
use crate::wait_cron::WaitCron;
use crate::wait_timer::WaitTimer;

// -- SDK error type --

/// SDK error type. Named variants for daemon error codes and SDK-side faults.
#[derive(Debug)]
pub enum SdkError {
    /// The interlock was reaped (TTL lapsed or name claimed). Daemon code 0x01.
    InterlockReaped,
    /// Attach to a name not in the registry. Daemon code 0x02.
    InterlockNotFound { name: String },
    /// Daemon could not allocate the interlock. Daemon code 0x03.
    AllocationFailed { message: String },
    /// Transport-level error (connection, framing, protocol).
    Transport(TransportError),
    /// mmap of the received fd failed (SDK-side).
    MmapFailed { message: String },
    /// The request was invalid (bad tier, missing fields, reserved name). Daemon code 0x04.
    InvalidRequest { message: String },
    /// Unexpected response type from the daemon.
    UnexpectedResponse { message: String },
}

impl std::fmt::Display for SdkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InterlockReaped => write!(f, "interlock reaped"),
            Self::InterlockNotFound { name } => write!(f, "interlock not found: {name}"),
            Self::AllocationFailed { message } => write!(f, "allocation failed: {message}"),
            Self::Transport(e) => write!(f, "transport: {e}"),
            Self::MmapFailed { message } => write!(f, "mmap failed: {message}"),
            Self::InvalidRequest { message } => write!(f, "invalid request: {message}"),
            Self::UnexpectedResponse { message } => write!(f, "unexpected response: {message}"),
        }
    }
}

impl std::error::Error for SdkError {}

impl From<TransportError> for SdkError {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

pub type Result<T> = std::result::Result<T, SdkError>;

// -- Client connection transport --

/// Client-side UDS connection with SCM_RIGHTS fd receiving.
struct ClientConn {
    stream: UnixStream,
}

const MAX_FDS: usize = 1;

impl ClientConn {
    fn connect(path: &Path) -> std::result::Result<Self, TransportError> {
        let stream = UnixStream::connect(path).map_err(|e| TransportError::Io {
            operation: IoOperation::Connect,
            errno: e.raw_os_error().unwrap_or(0),
        })?;
        Ok(Self { stream })
    }

    fn send_frame(&mut self, frame: &[u8]) -> std::result::Result<(), TransportError> {
        let mut written = 0usize;
        while written < frame.len() {
            match self.stream.write(&frame[written..]) {
                Ok(0) => {
                    return Err(TransportError::Io {
                        operation: IoOperation::Write,
                        errno: libc::EPIPE,
                    });
                }
                Ok(n) => written += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    return Err(TransportError::Io {
                        operation: IoOperation::Write,
                        errno: e.raw_os_error().unwrap_or(0),
                    });
                }
            }
        }
        Ok(())
    }

    fn recv_response(
        &mut self,
    ) -> std::result::Result<(Response, Vec<OwnedFd>), TransportError> {
        // Receive the length prefix with any ancillary fds via recvmsg.
        let (prefix_buf, fds) = self.recvmsg_prefix()?;
        let payload_len = u32::from_le_bytes(prefix_buf) as usize;
        if payload_len > MAX_MESSAGE_SIZE {
            return Err(TransportError::Protocol {
                fault: ProtocolFault::FrameTooLarge {
                    len: payload_len,
                    max: MAX_MESSAGE_SIZE,
                },
            });
        }

        // Read the payload.
        let mut payload = vec![0u8; payload_len];
        self.read_exact(&mut payload, false)?;

        let response =
            decode_response(&payload).map_err(|fault| TransportError::Protocol { fault })?;

        let needed = expected_fd_count(&response);
        if fds.len() != needed {
            return Err(TransportError::Protocol {
                fault: ProtocolFault::Truncated {
                    needed,
                    have: fds.len(),
                },
            });
        }

        Ok((response, fds))
    }

    fn recvmsg_prefix(
        &mut self,
    ) -> std::result::Result<([u8; LENGTH_PREFIX_BYTES], Vec<OwnedFd>), TransportError> {
        let mut prefix_buf = [0u8; LENGTH_PREFIX_BYTES];

        // cmsg buffer for SCM_RIGHTS
        let cmsg_space = unsafe { libc::CMSG_SPACE((MAX_FDS * mem::size_of::<RawFd>()) as u32) }
            as usize;
        let mut cmsg_buf = vec![0u8; cmsg_space];

        let mut iov = libc::iovec {
            iov_base: prefix_buf.as_mut_ptr() as *mut libc::c_void,
            iov_len: LENGTH_PREFIX_BYTES,
        };

        let mut msg: libc::msghdr = unsafe { mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = cmsg_space;

        let received = self.recvmsg_retry(&mut msg)?;
        if received == 0 {
            return Err(TransportError::ConnectionClosed);
        }

        let fds = extract_fds(&msg);

        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(TransportError::Protocol {
                fault: ProtocolFault::Truncated {
                    needed: MAX_FDS,
                    have: fds.len(),
                },
            });
        }

        // If we got a short read on the prefix, finish reading.
        if received < LENGTH_PREFIX_BYTES {
            self.read_exact(&mut prefix_buf[received..], false)?;
        }

        Ok((prefix_buf, fds))
    }

    fn recvmsg_retry(
        &self,
        msg: &mut libc::msghdr,
    ) -> std::result::Result<usize, TransportError> {
        loop {
            let rc = unsafe {
                libc::recvmsg(self.stream.as_raw_fd(), msg, libc::MSG_CMSG_CLOEXEC)
            };
            if rc < 0 {
                let errno = last_errno();
                if errno == libc::EINTR {
                    continue;
                }
                return Err(TransportError::Io {
                    operation: IoOperation::RecvMsg,
                    errno,
                });
            }
            return Ok(rc as usize);
        }
    }

    fn read_exact(
        &mut self,
        buf: &mut [u8],
        is_frame_start: bool,
    ) -> std::result::Result<(), TransportError> {
        use std::io::Read;
        let total = buf.len();
        let mut filled = 0usize;
        while filled < total {
            match self.stream.read(&mut buf[filled..]) {
                Ok(0) => {
                    if filled == 0 && is_frame_start {
                        return Err(TransportError::ConnectionClosed);
                    }
                    return Err(TransportError::Protocol {
                        fault: ProtocolFault::Truncated {
                            needed: total,
                            have: filled,
                        },
                    });
                }
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    return Err(TransportError::Io {
                        operation: IoOperation::Read,
                        errno: e.raw_os_error().unwrap_or(0),
                    });
                }
            }
        }
        Ok(())
    }
}

fn extract_fds(msg: &libc::msghdr) -> Vec<OwnedFd> {
    let mut fds = Vec::new();
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data_ptr = libc::CMSG_DATA(cmsg);
                let data_len = (*cmsg).cmsg_len - libc::CMSG_LEN(0) as usize;
                let n_fds = data_len / mem::size_of::<RawFd>();
                for i in 0..n_fds {
                    let raw: RawFd = std::ptr::read_unaligned(
                        data_ptr.add(i * mem::size_of::<RawFd>()) as *const RawFd,
                    );
                    fds.push(OwnedFd::from_raw_fd(raw));
                }
            }
            cmsg = libc::CMSG_NXTHDR(msg, cmsg);
        }
    }
    fds
}

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

// -- AbacusClient --

/// A client connection to the Abacus daemon.
pub struct AbacusClient {
    conn: ClientConn,
    /// The system clock, attached once on connect. Read-only.
    clock: ClockHandle,
}

impl AbacusClient {
    /// Connect to the daemon at `socket_path`. Immediately attaches to the
    /// "clock" interlock for use by WaitTimer.
    pub fn connect(socket_path: &Path) -> Result<Self> {
        let mut conn = ClientConn::connect(socket_path)?;

        // Attach to the system clock and wrap in a read-only ClockHandle.
        let clock_handle = Self::do_attach(&mut conn, "clock")?;
        let clock = ClockHandle::new(clock_handle);

        Ok(Self { conn, clock })
    }

    /// Create a bare interlock (tier 0). Returns a mapped Interlock handle
    /// with an auto-started touch thread.
    pub fn create_interlock(&mut self, name: &str) -> Result<Interlock> {
        let handle = self.do_create(name, 0, None, None)?;
        Ok(Interlock::new(handle))
    }

    /// Attach to an existing interlock by name. Returns an AttachedInterlock
    /// handle. Attacher: open_count r/w, closed_count r/w, expiration r/o.
    ///
    /// The name "clock" is reserved. The system clock is attached once on
    /// connect and accessed via `client.clock()`. Passing "clock" here returns
    /// `Err(InvalidRequest)`.
    pub fn attach_interlock(&mut self, name: &str) -> Result<AttachedInterlock> {
        if name == "clock" {
            return Err(SdkError::InvalidRequest {
                message: "use client.clock() to access the system clock".to_string(),
            });
        }
        let handle = Self::do_attach(&mut self.conn, name)?;
        Ok(AttachedInterlock::new(handle))
    }

    /// Attach to a WaitCounter by name. Returns a read-only AttachedWaitCounter.
    ///
    /// Note: the wire protocol does not carry tier information on attach. The returned
    /// AttachedWaitCounter is read-only by construction, but the caller must ensure the
    /// name refers to a WaitCounter. Attaching a bare interlock or WaitTimer name returns
    /// a valid handle with the wrong semantic contract.
    pub fn attach_wait_counter(&mut self, name: &str) -> Result<AttachedWaitCounter> {
        let handle = Self::do_attach(&mut self.conn, name)?;
        Ok(AttachedWaitCounter::new(handle))
    }

    /// Create a WaitCounter (tier 1) that watches `watched_name`.
    pub fn create_wait_counter(
        &mut self,
        name: &str,
        watched_name: &str,
        watched_word: WatchedWord,
    ) -> Result<WaitCounter> {
        let handle = self.do_create(name, 1, Some(watched_name), Some(watched_word.to_u8()))?;
        Ok(WaitCounter::new(handle))
    }

    /// Create a WaitTimer (tier 2). Auto-watches clock.open_count.
    pub fn create_wait_timer(&mut self, name: &str) -> Result<WaitTimer> {
        let handle = self.do_create(name, 2, None, None)?;
        Ok(WaitTimer::new(handle, self.clock.handle().clone()))
    }

    /// Create a WaitCron (tier 3). Grid-aligned recurring timer at
    /// `interval_ms` spacing, aligned to the monotonic epoch.
    pub fn create_wait_cron(&mut self, name: &str, interval_ms: u64) -> Result<WaitCron> {
        let req = Request::CreateInterlock {
            name: name.to_string(),
            tier: 3,
            watched_name: None,
            watched_word: None,
            interval_ns: Some(interval_ms.saturating_mul(1_000_000)),
            conditions: None,
        };
        let (resp, fds) = self.send_recv(&req)?;
        match resp {
            Response::Created { .. } => {
                let fd = extract_single_fd(fds)?;
                let handle = interlock_open(fd).map_err(|e| {
                    SdkError::MmapFailed {
                        message: format!("{e}"),
                    }
                })?;
                Ok(WaitCron::new(handle))
            }
            Response::Error { code, message } => Err(parse_daemon_error(code, message)),
            _ => Err(SdkError::UnexpectedResponse {
                message: "unexpected response type for create".to_string(),
            }),
        }
    }

    /// Create a WaitBarrier (tier 4). Watches N conditions; fires when all
    /// are met. Each condition is (watched_name, watched_word, threshold).
    pub fn create_wait_barrier(
        &mut self,
        name: &str,
        conditions: Vec<(String, WatchedWord, u64)>,
    ) -> Result<WaitBarrier> {
        let wire_conditions: Vec<(String, u8, u64)> = conditions
            .into_iter()
            .map(|(n, w, t)| (n, w.to_u8(), t))
            .collect();
        let req = Request::CreateInterlock {
            name: name.to_string(),
            tier: 4,
            watched_name: None,
            watched_word: None,
            interval_ns: None,
            conditions: Some(wire_conditions),
        };
        let (resp, fds) = self.send_recv(&req)?;
        match resp {
            Response::Created { .. } => {
                let fd = extract_single_fd(fds)?;
                let handle = interlock_open(fd).map_err(|e| {
                    SdkError::MmapFailed {
                        message: format!("{e}"),
                    }
                })?;
                Ok(WaitBarrier::new(handle))
            }
            Response::Error { code, message } => Err(parse_daemon_error(code, message)),
            _ => Err(SdkError::UnexpectedResponse {
                message: "unexpected response type for create".to_string(),
            }),
        }
    }

    /// Create a ProcessClock (tier 0 bare interlock with a custom touch thread
    /// that updates open_count to the current clock time on each tick).
    /// Used as a liveness and uptime beacon for other processes to observe.
    pub fn create_process_clock(&mut self, name: &str) -> Result<ProcessClock> {
        let handle = self.do_create(name, 0, None, None)?;
        Ok(ProcessClock::new(handle, self.clock.handle().clone()))
    }

    /// Access the system clock (read-only).
    pub fn clock(&self) -> &ClockHandle {
        &self.clock
    }

    /// Check if the daemon connection is still alive. Uses MSG_PEEK|MSG_DONTWAIT
    /// to probe the socket without consuming data. Returns false if the peer
    /// has closed (recv returns 0 = EOF). Returns true if the connection is
    /// alive (recv returns EAGAIN = no data available, or positive = data
    /// waiting).
    pub fn is_connected(&self) -> bool {
        let fd = self.conn.stream.as_raw_fd();
        let mut buf = [0u8; 1];
        let ret = unsafe {
            libc::recv(
                fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                1,
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            )
        };
        if ret == 0 {
            // EOF: peer closed.
            return false;
        }
        if ret < 0 {
            let errno = std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(0);
            if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK {
                // No data available but connection alive.
                return true;
            }
            // Other error (ECONNRESET, EBADF, etc): connection dead.
            return false;
        }
        // ret > 0: data waiting, connection alive.
        true
    }

    // -- internal helpers --

    fn do_create(
        &mut self,
        name: &str,
        tier: u8,
        watched_name: Option<&str>,
        watched_word: Option<u8>,
    ) -> Result<InterlockHandle> {
        let req = Request::CreateInterlock {
            name: name.to_string(),
            tier,
            watched_name: watched_name.map(|s| s.to_string()),
            watched_word,
            interval_ns: None,
            conditions: None,
        };
        let (resp, fds) = self.send_recv(&req)?;
        match resp {
            Response::Created { .. } => {
                let fd = extract_single_fd(fds)?;
                let handle = interlock_open(fd).map_err(|e| {
                    SdkError::MmapFailed {
                        message: format!("{e}"),
                    }
                })?;
                Ok(handle)
            }
            Response::Error { code, message } => Err(parse_daemon_error(code, message)),
            _ => Err(SdkError::UnexpectedResponse {
                message: "unexpected response type for create".to_string(),
            }),
        }
    }

    fn do_attach(conn: &mut ClientConn, name: &str) -> Result<InterlockHandle> {
        let req = Request::AttachInterlock {
            name: name.to_string(),
        };
        let frame = encode_request(&req).map_err(|fault| {
            SdkError::Transport(TransportError::Protocol { fault })
        })?;
        conn.send_frame(&frame)?;
        let (resp, fds) = conn.recv_response()?;
        match resp {
            Response::Attached { .. } => {
                let fd = extract_single_fd(fds)?;
                let handle = interlock_open(fd).map_err(|e| {
                    SdkError::MmapFailed {
                        message: format!("{e}"),
                    }
                })?;
                Ok(handle)
            }
            Response::Error { code, message } => Err(parse_daemon_error(code, message)),
            _ => Err(SdkError::UnexpectedResponse {
                message: "unexpected response type for attach".to_string(),
            }),
        }
    }

    fn send_recv(
        &mut self,
        req: &Request,
    ) -> Result<(Response, Vec<OwnedFd>)> {
        let frame = encode_request(req).map_err(|fault| {
            SdkError::Transport(TransportError::Protocol { fault })
        })?;
        self.conn.send_frame(&frame)?;
        let result = self.conn.recv_response()?;
        Ok(result)
    }
}

/// Parse daemon error codes into named SdkError variants.
/// Wire protocol error codes per CONTRACTS.md:
///   0x01 = InterlockReaped
///   0x02 = InterlockNotFound
///   0x03 = AllocationFailed
///   0x04 = InvalidRequest
fn parse_daemon_error(code: u8, message: String) -> SdkError {
    match code {
        0x01 => SdkError::InterlockReaped,
        0x02 => SdkError::InterlockNotFound { name: message },
        0x03 => SdkError::AllocationFailed { message },
        0x04 => SdkError::InvalidRequest { message },
        _ => SdkError::UnexpectedResponse {
            message: format!("unknown daemon error 0x{code:02x}: {message}"),
        },
    }
}

fn extract_single_fd(mut fds: Vec<OwnedFd>) -> Result<OwnedFd> {
    if fds.len() != 1 {
        return Err(SdkError::Transport(TransportError::Protocol {
            fault: ProtocolFault::Truncated {
                needed: 1,
                have: fds.len(),
            },
        }));
    }
    Ok(fds.remove(0))
}
