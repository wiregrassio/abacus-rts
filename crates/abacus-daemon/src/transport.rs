use std::io::{Read, Write};
use std::mem;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use abacus_core::error::{IoOperation, ProtocolFault, TransportError};
use crate::wire::{
    decode_request, decode_response, encode_response, expected_fd_count, Request, Response,
    LENGTH_PREFIX_BYTES, MAX_MESSAGE_SIZE,
};

const LISTEN_BACKLOG: i32 = 64;
const MAX_FDS_PER_MESSAGE: usize = 1;

// -- Server --

pub struct Server {
    listener: UnixListener,
    path: PathBuf,
}

impl Server {
    pub fn create(path: &Path) -> std::result::Result<Self, TransportError> {
        // Remove stale socket if present.
        if path.exists() {
            if is_socket_file(path)? {
                if probe_connect_succeeds(path) {
                    return Err(TransportError::SocketPathOccupied {
                        path: path.to_string_lossy().into_owned(),
                        live_daemon: true,
                    });
                }
                let _ = std::fs::remove_file(path);
            } else {
                return Err(TransportError::SocketPathOccupied {
                    path: path.to_string_lossy().into_owned(),
                    live_daemon: false,
                });
            }
        }

        let listener = UnixListener::bind(path).map_err(|e| TransportError::Io {
            operation: IoOperation::Bind,
            errno: e.raw_os_error().unwrap_or(0),
        })?;

        listener
            .set_nonblocking(true)
            .map_err(|e| TransportError::Io {
                operation: IoOperation::SetNonBlocking,
                errno: e.raw_os_error().unwrap_or(0),
            })?;

        let rc = unsafe { libc::listen(listener.as_raw_fd(), LISTEN_BACKLOG) };
        if rc != 0 {
            let _ = std::fs::remove_file(path);
            return Err(TransportError::Io {
                operation: IoOperation::Listen,
                errno: last_errno(),
            });
        }

        Ok(Self {
            listener,
            path: path.to_path_buf(),
        })
    }

    pub fn try_accept(&self) -> std::result::Result<Option<Connection>, TransportError> {
        match self.listener.accept() {
            Ok((stream, _)) => Ok(Some(Connection { stream })),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(TransportError::Io {
                operation: IoOperation::Accept,
                errno: e.raw_os_error().unwrap_or(0),
            }),
        }
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.listener.as_raw_fd()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// -- Connection --

pub struct Connection {
    stream: UnixStream,
}

impl Connection {
    pub fn connect(path: &Path) -> std::result::Result<Self, TransportError> {
        let stream = UnixStream::connect(path).map_err(|e| TransportError::Io {
            operation: IoOperation::Connect,
            errno: e.raw_os_error().unwrap_or(0),
        })?;
        Ok(Self { stream })
    }

    /// Set read and write timeouts on the underlying stream.
    pub fn set_timeouts(
        &self,
        duration: std::time::Duration,
    ) -> std::result::Result<(), TransportError> {
        let timeout = Some(duration);
        self.stream
            .set_read_timeout(timeout)
            .map_err(|e| TransportError::Io {
                operation: IoOperation::Read,
                errno: e.raw_os_error().unwrap_or(0),
            })?;
        self.stream
            .set_write_timeout(timeout)
            .map_err(|e| TransportError::Io {
                operation: IoOperation::Write,
                errno: e.raw_os_error().unwrap_or(0),
            })?;
        Ok(())
    }

    /// Set the underlying stream to non-blocking mode.
    pub fn set_nonblocking(&self, nonblocking: bool) -> std::result::Result<(), TransportError> {
        self.stream
            .set_nonblocking(nonblocking)
            .map_err(|e| TransportError::Io {
                operation: IoOperation::SetNonBlocking,
                errno: e.raw_os_error().unwrap_or(0),
            })
    }

    /// Return the raw file descriptor of the underlying stream.
    pub fn as_raw_fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }

    pub fn recv_request(&mut self) -> std::result::Result<Request, TransportError> {
        let payload_len = read_frame_prefix(&mut self.stream)?;
        let payload = read_frame_payload(&mut self.stream, payload_len)?;
        decode_request(&payload).map_err(|fault| TransportError::Protocol { fault })
    }

    pub fn send_response(&mut self, resp: &Response) -> std::result::Result<(), TransportError> {
        let frame = encode_response(resp).map_err(|fault| TransportError::Protocol { fault })?;
        write_all_retrying(&mut self.stream, &frame)
    }

    pub fn send_response_with_fds(
        &mut self,
        resp: &Response,
        fds: &[BorrowedFd<'_>],
    ) -> std::result::Result<(), TransportError> {
        let frame = encode_response(resp).map_err(|fault| TransportError::Protocol { fault })?;
        sendmsg_with_fds(&mut self.stream, &frame, fds)
    }

    pub fn recv_response(
        &mut self,
    ) -> std::result::Result<(Response, Vec<OwnedFd>), TransportError> {
        let (prefix_buf, fds) = recvmsg_prefix_with_fds(&mut self.stream)?;
        let payload_len = u32::from_le_bytes(prefix_buf) as usize;
        if payload_len > MAX_MESSAGE_SIZE {
            return Err(TransportError::Protocol {
                fault: ProtocolFault::FrameTooLarge {
                    len: payload_len,
                    max: MAX_MESSAGE_SIZE,
                },
            });
        }
        let payload = read_frame_payload(&mut self.stream, payload_len)?;
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
}

// -- I/O helpers --

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn read_exact_retrying(
    stream: &mut UnixStream,
    buf: &mut [u8],
    is_frame_start: bool,
) -> std::result::Result<(), TransportError> {
    let total = buf.len();
    let mut filled: usize = 0;
    while filled < total {
        match stream.read(&mut buf[filled..]) {
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
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if filled == 0 && is_frame_start {
                    return Err(TransportError::WouldBlock);
                }
                // Mid-frame WouldBlock: data in flight, retry.
                continue;
            }
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

fn write_all_retrying(
    stream: &mut UnixStream,
    buf: &[u8],
) -> std::result::Result<(), TransportError> {
    let total = buf.len();
    let mut written: usize = 0;
    while written < total {
        match stream.write(&buf[written..]) {
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

fn read_frame_prefix(
    stream: &mut UnixStream,
) -> std::result::Result<usize, TransportError> {
    let mut prefix = [0u8; LENGTH_PREFIX_BYTES];
    read_exact_retrying(stream, &mut prefix, true)?;
    let len = u32::from_le_bytes(prefix) as usize;
    if len > MAX_MESSAGE_SIZE {
        return Err(TransportError::Protocol {
            fault: ProtocolFault::FrameTooLarge {
                len,
                max: MAX_MESSAGE_SIZE,
            },
        });
    }
    Ok(len)
}

fn read_frame_payload(
    stream: &mut UnixStream,
    len: usize,
) -> std::result::Result<Vec<u8>, TransportError> {
    let mut payload = vec![0u8; len];
    read_exact_retrying(stream, &mut payload, false)?;
    Ok(payload)
}

// -- SCM_RIGHTS --

const fn cmsg_space_for_fds(n: usize) -> usize {
    let data_len = n * mem::size_of::<RawFd>();
    let hdr_align =
        (mem::size_of::<libc::cmsghdr>() + mem::size_of::<usize>() - 1) & !(mem::size_of::<usize>() - 1);
    let data_align = (data_len + mem::size_of::<usize>() - 1) & !(mem::size_of::<usize>() - 1);
    hdr_align + data_align
}

#[repr(C)]
struct CmsgBuf {
    _align: [libc::cmsghdr; 0],
    buf: [u8; cmsg_space_for_fds(MAX_FDS_PER_MESSAGE)],
}

impl CmsgBuf {
    fn new() -> Self {
        Self {
            _align: [],
            buf: [0u8; cmsg_space_for_fds(MAX_FDS_PER_MESSAGE)],
        }
    }
    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.buf.as_mut_ptr()
    }
    fn len(&self) -> usize {
        self.buf.len()
    }
}

fn sendmsg_with_fds(
    stream: &mut UnixStream,
    frame: &[u8],
    fds: &[BorrowedFd<'_>],
) -> std::result::Result<(), TransportError> {
    if fds.len() > MAX_FDS_PER_MESSAGE {
        return Err(TransportError::Protocol {
            fault: ProtocolFault::FrameTooLarge {
                len: fds.len(),
                max: MAX_FDS_PER_MESSAGE,
            },
        });
    }

    let raw_fds: Vec<RawFd> = fds.iter().map(|fd| fd.as_raw_fd()).collect();
    let fd_bytes = raw_fds.len() * mem::size_of::<RawFd>();

    let mut cmsg_buf = CmsgBuf::new();
    let cmsg_len = unsafe { libc::CMSG_SPACE(fd_bytes as libc::c_uint) } as usize;

    let mut iov = libc::iovec {
        iov_base: frame.as_ptr() as *mut libc::c_void,
        iov_len: frame.len(),
    };

    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = cmsg_len;

    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() {
            return Err(TransportError::Io {
                operation: IoOperation::SendMsg,
                errno: libc::EINVAL,
            });
        }
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(fd_bytes as libc::c_uint) as usize;
        std::ptr::copy_nonoverlapping(
            raw_fds.as_ptr() as *const u8,
            libc::CMSG_DATA(cmsg),
            fd_bytes,
        );
    }

    let sent = sendmsg_retry(stream.as_raw_fd(), &msg)?;
    if sent < frame.len() {
        write_all_retrying(stream, &frame[sent..])?;
    }
    Ok(())
}

fn sendmsg_retry(fd: RawFd, msg: &libc::msghdr) -> std::result::Result<usize, TransportError> {
    loop {
        let rc = unsafe { libc::sendmsg(fd, msg as *const libc::msghdr, 0) };
        if rc < 0 {
            let errno = last_errno();
            if errno == libc::EINTR {
                continue;
            }
            return Err(TransportError::Io {
                operation: IoOperation::SendMsg,
                errno,
            });
        }
        return Ok(rc as usize);
    }
}

fn recvmsg_prefix_with_fds(
    stream: &mut UnixStream,
) -> std::result::Result<([u8; LENGTH_PREFIX_BYTES], Vec<OwnedFd>), TransportError> {
    let mut prefix_buf = [0u8; LENGTH_PREFIX_BYTES];
    let mut cmsg_buf = CmsgBuf::new();

    let mut iov = libc::iovec {
        iov_base: prefix_buf.as_mut_ptr() as *mut libc::c_void,
        iov_len: LENGTH_PREFIX_BYTES,
    };

    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = cmsg_buf.len();

    let received = recvmsg_retry(stream.as_raw_fd(), &mut msg)?;
    if received == 0 {
        return Err(TransportError::ConnectionClosed);
    }

    let fds = extract_fds_from_cmsg(&msg);

    if msg.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(TransportError::Protocol {
            fault: ProtocolFault::Truncated {
                needed: MAX_FDS_PER_MESSAGE,
                have: fds.len(),
            },
        });
    }

    if received < LENGTH_PREFIX_BYTES {
        read_exact_retrying(stream, &mut prefix_buf[received..], false)?;
    }

    Ok((prefix_buf, fds))
}

fn recvmsg_retry(
    fd: RawFd,
    msg: &mut libc::msghdr,
) -> std::result::Result<usize, TransportError> {
    loop {
        let rc = unsafe { libc::recvmsg(fd, msg, libc::MSG_CMSG_CLOEXEC) };
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

fn extract_fds_from_cmsg(msg: &libc::msghdr) -> Vec<OwnedFd> {
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

fn is_socket_file(path: &Path) -> std::result::Result<bool, TransportError> {
    use std::os::unix::fs::FileTypeExt;
    let meta = std::fs::metadata(path).map_err(|e| TransportError::Io {
        operation: IoOperation::Stat,
        errno: e.raw_os_error().unwrap_or(0),
    })?;
    Ok(meta.file_type().is_socket())
}

fn probe_connect_succeeds(path: &Path) -> bool {
    UnixStream::connect(path).is_ok()
}
