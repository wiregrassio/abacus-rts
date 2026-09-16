use std::fmt;

// -- Daemon-domain conditions --

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    InterlockReaped,
    InterlockNotFound { name: String },
    AllocationFailed { step: AllocationStep, errno: i32 },
    InvalidRequest { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocationStep {
    MemfdCreate,
    Ftruncate,
    Mmap,
}

impl fmt::Display for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InterlockReaped => write!(f, "InterlockReaped"),
            Self::InterlockNotFound { name } => write!(f, "InterlockNotFound: {name}"),
            Self::AllocationFailed { step, errno } => {
                write!(f, "AllocationFailed at {step:?}, errno={errno}")
            }
            Self::InvalidRequest { message } => {
                write!(f, "InvalidRequest: {message}")
            }
        }
    }
}

impl std::error::Error for Condition {}

pub type Result<T> = std::result::Result<T, Condition>;

// -- Transport errors --

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoOperation {
    Bind,
    Listen,
    Accept,
    Connect,
    SendMsg,
    RecvMsg,
    Read,
    Write,
    Stat,
    Unlink,
    SetNonBlocking,
    Poll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolFault {
    Truncated { needed: usize, have: usize },
    FrameTooLarge { len: usize, max: usize },
    UnsupportedVersion { version: u8 },
    UnknownTag { tag: u8 },
    InvalidUtf8,
}

impl fmt::Display for ProtocolFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { needed, have } => {
                write!(f, "truncated: needed {needed}, have {have}")
            }
            Self::FrameTooLarge { len, max } => {
                write!(f, "frame too large: {len} > {max}")
            }
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported version: {version}")
            }
            Self::UnknownTag { tag } => write!(f, "unknown tag: 0x{tag:02x}"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Protocol { fault: ProtocolFault },
    Io { operation: IoOperation, errno: i32 },
    ConnectionClosed,
    WouldBlock,
    SocketPathOccupied { path: String, live_daemon: bool },
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol { fault } => write!(f, "protocol fault: {fault}"),
            Self::Io { operation, errno } => {
                write!(f, "I/O error in {operation:?}: errno {errno}")
            }
            Self::ConnectionClosed => write!(f, "connection closed by peer"),
            Self::WouldBlock => write!(f, "would block (no data available)"),
            Self::SocketPathOccupied { path, live_daemon } => {
                if *live_daemon {
                    write!(f, "socket path occupied by a live daemon: {path}")
                } else {
                    write!(f, "socket path occupied by a non-socket object: {path}")
                }
            }
        }
    }
}

impl std::error::Error for TransportError {}

// -- Daemon startup errors --

#[derive(Debug)]
pub enum StartupError {
    Transport(TransportError),
    Allocation(Condition),
    EventLoopFailed { errno: i32 },
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "transport: {e}"),
            Self::Allocation(e) => write!(f, "allocation: {e}"),
            Self::EventLoopFailed { errno } => {
                write!(f, "event loop: poll failed permanently, errno={errno}")
            }
        }
    }
}

impl std::error::Error for StartupError {}

impl From<TransportError> for StartupError {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

impl From<Condition> for StartupError {
    fn from(e: Condition) -> Self {
        Self::Allocation(e)
    }
}
