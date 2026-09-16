use std::os::fd::{AsFd, AsRawFd};
use std::path::Path;
use std::sync::atomic::Ordering;

use abacus_core::clock::{futex_wake, monotonic_now_nanos};
use abacus_core::error::{IoOperation, StartupError, TransportError};
use abacus_core::interlock::interlock_dup_fd;
use crate::registry::Registry;
use crate::transport::{Connection, Server};
use crate::wire::{Request, Response};

const NANOS_PER_MS: u64 = 1_000_000;

/// Per-client state in the connection table.
struct ClientState {
    conn: Connection,
}

pub fn daemon_run(socket_path: &Path) -> std::result::Result<(), StartupError> {
    let server = Server::create(socket_path)?;
    let mut registry = Registry::new()?;

    let anchor = monotonic_now_nanos();
    let clock = registry.clock().clone();

    eprintln!("abacus: daemon running on {}", socket_path.display());

    let mut cycle: u64 = 0;
    let mut clients: Vec<ClientState> = Vec::new();

    loop {
        // Compute timeout until next ms boundary.
        cycle += 1;
        let next_boundary = anchor + cycle * NANOS_PER_MS;
        let now = monotonic_now_nanos();
        let timeout_ms = if next_boundary > now {
            ((next_boundary - now + NANOS_PER_MS - 1) / NANOS_PER_MS) as i32
        } else {
            0
        };

        // Build pollfd array: [listener, client0, client1, ...]
        let mut pollfds: Vec<libc::pollfd> = Vec::with_capacity(1 + clients.len());
        pollfds.push(libc::pollfd {
            fd: server.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
        for client in &clients {
            pollfds.push(libc::pollfd {
                fd: client.conn.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }

        // Poll: wait for I/O or timeout.
        let poll_ret = unsafe {
            libc::poll(
                pollfds.as_mut_ptr(),
                pollfds.len() as libc::nfds_t,
                timeout_ms,
            )
        };
        if poll_ret < 0 {
            let errno = std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(0);
            if errno == libc::EINTR {
                // Interrupted by signal, retry.
            } else {
                return Err(StartupError::EventLoopFailed { errno });
            }
        }

        // Handle new connections.
        if pollfds[0].revents & libc::POLLIN != 0 {
            loop {
                match server.try_accept() {
                    Ok(Some(conn)) => {
                        if let Err(e) = conn.set_nonblocking(true) {
                            eprintln!("abacus: set_nonblocking error: {e}");
                            continue;
                        }
                        clients.push(ClientState { conn });
                    }
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("abacus: accept error: {e}");
                        break;
                    }
                }
            }
        }

        // Handle client requests and disconnects.
        let mut to_remove: Vec<usize> = Vec::new();
        for (i, pfd) in pollfds[1..].iter().enumerate() {
            if pfd.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
                // Client disconnected. Interlocks die by TTL only.
                to_remove.push(i);
            } else if pfd.revents & libc::POLLIN != 0 {
                match handle_client_request(&mut clients[i], &mut registry) {
                    Ok(()) => {}
                    Err(e) => {
                        if is_connection_error(&e) {
                            to_remove.push(i);
                        } else {
                            eprintln!("abacus: request error on client {i}: {e}");
                        }
                    }
                }
            }
        }
        // Remove disconnected clients (reverse order to preserve indices).
        to_remove.sort_unstable();
        to_remove.dedup();
        for i in to_remove.into_iter().rev() {
            clients.remove(i);
        }

        // Advance clock: open_count = current monotonic ms.
        let current_ms = monotonic_now_nanos() / NANOS_PER_MS;
        clock.words().open_count.store(current_ms, Ordering::Release);

        // Evaluate all interlocks: reap expired, wake waiters.
        registry.evaluate_all(current_ms);

        // Wake clock watchers.
        futex_wake(&clock.words().open_count);

        // Refresh clock expiration so it is never reaped.
        registry.refresh_clock_expiration();
    }
}

fn handle_client_request(
    client: &mut ClientState,
    registry: &mut Registry,
) -> std::result::Result<(), TransportError> {
    let request = match client.conn.recv_request() {
        Ok(req) => req,
        Err(TransportError::WouldBlock) => return Ok(()),
        Err(TransportError::Protocol { ref fault }) => {
            eprintln!("abacus: protocol fault from client: {fault}");
            let resp = Response::invalid_request(&format!("{fault}"));
            let _ = client.conn.send_response(&resp);
            return Ok(());
        }
        Err(e) => return Err(e),
    };

    match request {
        Request::CreateInterlock {
            name,
            tier,
            watched_name,
            watched_word,
            interval_ns,
            conditions,
        } => {
            let tier_enum = match tier {
                0 => crate::registry::Tier::Interlock,
                1 => crate::registry::Tier::WaitCounter,
                2 => crate::registry::Tier::WaitTimer,
                3 => crate::registry::Tier::WaitCron,
                4 => crate::registry::Tier::WaitBarrier,
                _ => {
                    let resp = Response::invalid_request("invalid tier");
                    client.conn.send_response(&resp)?;
                    return Ok(());
                }
            };
            match registry.create(
                name,
                tier_enum,
                watched_name,
                watched_word,
                interval_ns,
                conditions,
            ) {
                Ok((id, handle)) => {
                    let interlock_fd = match interlock_dup_fd(&handle) {
                        Ok(fd) => fd,
                        Err(e) => {
                            let resp = Response::allocation_failed(
                                &format!("fd dup failed: {e}"),
                            );
                            let _ = client.conn.send_response(&resp);
                            return Ok(());
                        }
                    };
                    let resp = Response::Created { id };
                    client.conn.send_response_with_fds(&resp, &[interlock_fd.as_fd()])?;
                }
                Err(e) => {
                    let resp = map_condition_to_response(&e);
                    client.conn.send_response(&resp)?;
                }
            }
        }
        Request::AttachInterlock { name } => {
            match registry.attach(&name) {
                Ok((id, handle)) => {
                    let interlock_fd = match interlock_dup_fd(&handle) {
                        Ok(fd) => fd,
                        Err(e) => {
                            let resp = Response::allocation_failed(
                                &format!("fd dup failed: {e}"),
                            );
                            let _ = client.conn.send_response(&resp);
                            return Ok(());
                        }
                    };
                    let resp = Response::Attached { id };
                    client.conn.send_response_with_fds(&resp, &[interlock_fd.as_fd()])?;
                }
                Err(e) => {
                    let resp = map_condition_to_response(&e);
                    client.conn.send_response(&resp)?;
                }
            }
        }
    }

    Ok(())
}

/// Classify whether a TransportError indicates the connection is dead.
/// Connection errors: ConnectionClosed, or I/O errors on read/write/send/recv
/// with EPIPE or ECONNRESET. Everything else is a request-level error.
fn is_connection_error(e: &TransportError) -> bool {
    match e {
        TransportError::ConnectionClosed => true,
        TransportError::Io { operation, errno } => {
            matches!(
                operation,
                IoOperation::Read
                    | IoOperation::Write
                    | IoOperation::SendMsg
                    | IoOperation::RecvMsg
            ) && (*errno == libc::EPIPE || *errno == libc::ECONNRESET)
        }
        _ => false,
    }
}

/// Map a Condition to the correct wire Response error variant.
fn map_condition_to_response(e: &abacus_core::error::Condition) -> Response {
    match e {
        abacus_core::error::Condition::InterlockNotFound { .. } => {
            Response::interlock_not_found(&e.to_string())
        }
        abacus_core::error::Condition::InterlockReaped => {
            Response::interlock_reaped()
        }
        abacus_core::error::Condition::AllocationFailed { .. } => {
            Response::allocation_failed(&e.to_string())
        }
        abacus_core::error::Condition::InvalidRequest { .. } => {
            Response::invalid_request(&e.to_string())
        }
    }
}
