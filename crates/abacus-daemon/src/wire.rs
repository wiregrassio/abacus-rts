use abacus_core::error::ProtocolFault;

pub const LENGTH_PREFIX_BYTES: usize = 4;
pub const MAX_MESSAGE_SIZE: usize = 4096;
pub const PROTOCOL_VERSION: u8 = 1;

const TAG_CREATE_INTERLOCK: u8 = 0x01;
const TAG_ATTACH_INTERLOCK: u8 = 0x02;

const TAG_CREATED: u8 = 0x81;
const TAG_ATTACHED: u8 = 0x82;
const TAG_ERROR: u8 = 0x88;

const ERR_INTERLOCK_REAPED: u8 = 0x01;
const ERR_INTERLOCK_NOT_FOUND: u8 = 0x02;
const ERR_ALLOCATION_FAILED: u8 = 0x03;
const ERR_INVALID_REQUEST: u8 = 0x04;

// -- Request --

/// Wire-level request.
///
/// CreateInterlock carries tier (0=Interlock, 1=WaitCounter, 2=WaitTimer,
/// 3=WaitCron, 4=WaitBarrier).
/// WaitCounter: watched_name and watched_word (0=open_count, 1=closed_count).
/// WaitTimer: no extra fields.
/// WaitCron: interval_ns (u64).
/// WaitBarrier: conditions (Vec of (watched_name, watched_word, threshold)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    CreateInterlock {
        name: String,
        tier: u8,
        watched_name: Option<String>,
        watched_word: Option<u8>,
        /// WaitCron (tier 3): grid interval in nanoseconds.
        interval_ns: Option<u64>,
        /// WaitBarrier (tier 4): list of (watched_name, watched_word, threshold).
        conditions: Option<Vec<(String, u8, u64)>>,
    },
    AttachInterlock {
        name: String,
    },
}

// -- Response --

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Created { id: u64 },
    Attached { id: u64 },
    Error { code: u8, message: String },
}

impl Response {
    pub fn interlock_reaped() -> Self {
        Self::Error {
            code: ERR_INTERLOCK_REAPED,
            message: "InterlockReaped".into(),
        }
    }

    pub fn interlock_not_found(name: &str) -> Self {
        Self::Error {
            code: ERR_INTERLOCK_NOT_FOUND,
            message: format!("InterlockNotFound: {name}"),
        }
    }

    pub fn allocation_failed(msg: &str) -> Self {
        Self::Error {
            code: ERR_ALLOCATION_FAILED,
            message: msg.to_string(),
        }
    }

    pub fn invalid_request(msg: &str) -> Self {
        Self::Error {
            code: ERR_INVALID_REQUEST,
            message: msg.to_string(),
        }
    }
}

// Created and Attached carry 1 fd (the interlock). Error carries 0.
pub fn expected_fd_count(response: &Response) -> usize {
    match response {
        Response::Created { .. } | Response::Attached { .. } => 1,
        Response::Error { .. } => 0,
    }
}

// -- Encoding --

pub fn encode_request(req: &Request) -> std::result::Result<Vec<u8>, ProtocolFault> {
    let mut payload = vec![PROTOCOL_VERSION];
    match req {
        Request::CreateInterlock {
            name,
            tier,
            watched_name,
            watched_word,
            interval_ns,
            conditions,
        } => {
            payload.push(TAG_CREATE_INTERLOCK);
            encode_string_into(&mut payload, name)?;
            payload.push(*tier);
            match *tier {
                1 => {
                    // WaitCounter: encode watched_name and watched_word.
                    // watched_word is required for tier 1; reject None.
                    let ww = watched_word.ok_or(ProtocolFault::Truncated {
                        needed: 1,
                        have: 0,
                    })?;
                    if let Some(ref wn) = watched_name {
                        encode_string_into(&mut payload, wn)?;
                    }
                    payload.push(ww);
                }
                3 => {
                    // WaitCron: encode interval_ns as u64 LE.
                    if let Some(ival) = interval_ns {
                        payload.extend_from_slice(&ival.to_le_bytes());
                    }
                }
                4 => {
                    // WaitBarrier: encode condition_count (u16) then each
                    // (watched_name, watched_word, threshold).
                    if let Some(ref conds) = conditions {
                        let count = conds.len() as u16;
                        payload.extend_from_slice(&count.to_le_bytes());
                        for (wn, ww, threshold) in conds {
                            encode_string_into(&mut payload, wn)?;
                            payload.push(*ww);
                            payload.extend_from_slice(&threshold.to_le_bytes());
                        }
                    }
                }
                _ => {
                    // Tier 0 (Interlock) and tier 2 (WaitTimer): no extra fields.
                }
            }
        }
        Request::AttachInterlock { name } => {
            payload.push(TAG_ATTACH_INTERLOCK);
            encode_string_into(&mut payload, name)?;
        }
    }
    if payload.len() > MAX_MESSAGE_SIZE {
        return Err(ProtocolFault::FrameTooLarge {
            len: payload.len(),
            max: MAX_MESSAGE_SIZE,
        });
    }
    let len = payload.len() as u32;
    let mut frame = len.to_le_bytes().to_vec();
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn encode_response(resp: &Response) -> std::result::Result<Vec<u8>, ProtocolFault> {
    let mut payload = vec![PROTOCOL_VERSION];
    match resp {
        Response::Created { id } => {
            payload.push(TAG_CREATED);
            payload.extend_from_slice(&id.to_le_bytes());
        }
        Response::Attached { id } => {
            payload.push(TAG_ATTACHED);
            payload.extend_from_slice(&id.to_le_bytes());
        }
        Response::Error { code, message } => {
            payload.push(TAG_ERROR);
            payload.push(*code);
            encode_string_into(&mut payload, message)?;
        }
    }
    if payload.len() > MAX_MESSAGE_SIZE {
        return Err(ProtocolFault::FrameTooLarge {
            len: payload.len(),
            max: MAX_MESSAGE_SIZE,
        });
    }
    let len = payload.len() as u32;
    let mut frame = len.to_le_bytes().to_vec();
    frame.extend_from_slice(&payload);
    Ok(frame)
}

fn encode_string_into(buf: &mut Vec<u8>, s: &str) -> std::result::Result<(), ProtocolFault> {
    let bytes = s.as_bytes();
    if bytes.len() > u16::MAX as usize {
        return Err(ProtocolFault::FrameTooLarge {
            len: bytes.len(),
            max: u16::MAX as usize,
        });
    }
    buf.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(bytes);
    Ok(())
}

// -- Decoding --

pub fn decode_request(data: &[u8]) -> std::result::Result<Request, ProtocolFault> {
    if data.len() < 2 {
        return Err(ProtocolFault::Truncated {
            needed: 2,
            have: data.len(),
        });
    }
    let version = data[0];
    if version != PROTOCOL_VERSION {
        return Err(ProtocolFault::UnsupportedVersion { version });
    }
    let tag = data[1];
    let rest = &data[2..];
    match tag {
        TAG_CREATE_INTERLOCK => {
            let name = decode_string(rest)?;
            let mut pos = 2 + name.len();
            if pos >= rest.len() {
                return Err(ProtocolFault::Truncated {
                    needed: data.len() + 1,
                    have: data.len(),
                });
            }
            let tier = rest[pos];
            pos += 1;
            let mut watched_name = None;
            let mut watched_word = None;
            let mut interval_ns = None;
            let mut conditions = None;
            match tier {
                1 => {
                    // WaitCounter: decode watched_name and watched_word.
                    let wn = decode_string(&rest[pos..])?;
                    pos += 2 + wn.len();
                    if pos >= rest.len() {
                        return Err(ProtocolFault::Truncated {
                            needed: data.len() + 1,
                            have: data.len(),
                        });
                    }
                    let ww = rest[pos];
                    watched_name = Some(wn);
                    watched_word = Some(ww);
                }
                3 => {
                    // WaitCron: decode interval_ns (u64 LE).
                    if rest.len() < pos + 8 {
                        return Err(ProtocolFault::Truncated {
                            needed: pos + 8 + 2, // approximate
                            have: rest.len(),
                        });
                    }
                    interval_ns = Some(u64::from_le_bytes(
                        rest[pos..pos + 8].try_into().unwrap(),
                    ));
                }
                4 => {
                    // WaitBarrier: decode condition_count (u16), then each condition.
                    if rest.len() < pos + 2 {
                        return Err(ProtocolFault::Truncated {
                            needed: pos + 2,
                            have: rest.len(),
                        });
                    }
                    let count = u16::from_le_bytes([rest[pos], rest[pos + 1]]) as usize;
                    pos += 2;
                    let mut conds = Vec::with_capacity(count);
                    for _ in 0..count {
                        let wn = decode_string(&rest[pos..])?;
                        pos += 2 + wn.len();
                        if pos >= rest.len() {
                            return Err(ProtocolFault::Truncated {
                                needed: pos + 1,
                                have: rest.len(),
                            });
                        }
                        let ww = rest[pos];
                        pos += 1;
                        if rest.len() < pos + 8 {
                            return Err(ProtocolFault::Truncated {
                                needed: pos + 8,
                                have: rest.len(),
                            });
                        }
                        let threshold = u64::from_le_bytes(
                            rest[pos..pos + 8].try_into().unwrap(),
                        );
                        pos += 8;
                        conds.push((wn, ww, threshold));
                    }
                    conditions = Some(conds);
                }
                _ => {
                    // Tier 0 (Interlock) and tier 2 (WaitTimer): no extra fields.
                }
            }
            Ok(Request::CreateInterlock {
                name,
                tier,
                watched_name,
                watched_word,
                interval_ns,
                conditions,
            })
        }
        TAG_ATTACH_INTERLOCK => {
            let name = decode_string(rest)?;
            Ok(Request::AttachInterlock { name })
        }
        _ => Err(ProtocolFault::UnknownTag { tag }),
    }
}

pub fn decode_response(data: &[u8]) -> std::result::Result<Response, ProtocolFault> {
    if data.len() < 2 {
        return Err(ProtocolFault::Truncated {
            needed: 2,
            have: data.len(),
        });
    }
    let version = data[0];
    if version != PROTOCOL_VERSION {
        return Err(ProtocolFault::UnsupportedVersion { version });
    }
    let tag = data[1];
    let rest = &data[2..];
    match tag {
        TAG_CREATED => {
            if rest.len() < 8 {
                return Err(ProtocolFault::Truncated {
                    needed: 10,
                    have: data.len(),
                });
            }
            let id = u64::from_le_bytes(rest[..8].try_into().unwrap());
            Ok(Response::Created { id })
        }
        TAG_ATTACHED => {
            if rest.len() < 8 {
                return Err(ProtocolFault::Truncated {
                    needed: 10,
                    have: data.len(),
                });
            }
            let id = u64::from_le_bytes(rest[..8].try_into().unwrap());
            Ok(Response::Attached { id })
        }
        TAG_ERROR => {
            if rest.is_empty() {
                return Err(ProtocolFault::Truncated {
                    needed: 3,
                    have: data.len(),
                });
            }
            let code = rest[0];
            let message = if rest.len() > 1 {
                decode_string(&rest[1..])?
            } else {
                String::new()
            };
            Ok(Response::Error { code, message })
        }
        _ => Err(ProtocolFault::UnknownTag { tag }),
    }
}

fn decode_string(data: &[u8]) -> std::result::Result<String, ProtocolFault> {
    if data.len() < 2 {
        return Err(ProtocolFault::Truncated {
            needed: 2,
            have: data.len(),
        });
    }
    let len = u16::from_le_bytes([data[0], data[1]]) as usize;
    if data.len() < 2 + len {
        return Err(ProtocolFault::Truncated {
            needed: 2 + len,
            have: data.len(),
        });
    }
    String::from_utf8(data[2..2 + len].to_vec()).map_err(|_| ProtocolFault::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_interlock_round_trip() {
        let req = Request::CreateInterlock {
            name: "cam0".into(),
            tier: 0,
            watched_name: None,
            watched_word: None,
            interval_ns: None,
            conditions: None,
        };
        let frame = encode_request(&req).unwrap();
        let decoded = decode_request(&frame[4..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn create_wait_counter_round_trip() {
        let req = Request::CreateInterlock {
            name: "frame_done".into(),
            tier: 1,
            watched_name: Some("cam0".into()),
            watched_word: Some(1),
            interval_ns: None,
            conditions: None,
        };
        let frame = encode_request(&req).unwrap();
        let decoded = decode_request(&frame[4..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn create_wait_counter_open_count_round_trip() {
        let req = Request::CreateInterlock {
            name: "startup_wait".into(),
            tier: 1,
            watched_name: Some("pipeline".into()),
            watched_word: Some(0),
            interval_ns: None,
            conditions: None,
        };
        let frame = encode_request(&req).unwrap();
        let decoded = decode_request(&frame[4..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn create_wait_timer_round_trip() {
        let req = Request::CreateInterlock {
            name: "timeout_5ms".into(),
            tier: 2,
            watched_name: None,
            watched_word: None,
            interval_ns: None,
            conditions: None,
        };
        let frame = encode_request(&req).unwrap();
        let decoded = decode_request(&frame[4..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn create_wait_cron_round_trip() {
        let req = Request::CreateInterlock {
            name: "cron_33ms".into(),
            tier: 3,
            watched_name: None,
            watched_word: None,
            interval_ns: Some(33_000_000),
            conditions: None,
        };
        let frame = encode_request(&req).unwrap();
        let decoded = decode_request(&frame[4..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn create_wait_barrier_round_trip() {
        let req = Request::CreateInterlock {
            name: "barrier_all".into(),
            tier: 4,
            watched_name: None,
            watched_word: None,
            interval_ns: None,
            conditions: Some(vec![
                ("cam0".into(), 1, 100),
                ("cam1".into(), 0, 50),
            ]),
        };
        let frame = encode_request(&req).unwrap();
        let decoded = decode_request(&frame[4..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn attach_round_trip() {
        let req = Request::AttachInterlock {
            name: "sensor".into(),
        };
        let frame = encode_request(&req).unwrap();
        let decoded = decode_request(&frame[4..]).unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn response_created_round_trip() {
        let resp = Response::Created { id: 42 };
        let frame = encode_response(&resp).unwrap();
        let decoded = decode_response(&frame[4..]).unwrap();
        assert_eq!(decoded, resp);
    }

    #[test]
    fn response_error_round_trip() {
        let resp = Response::interlock_not_found("missing");
        let frame = encode_response(&resp).unwrap();
        let decoded = decode_response(&frame[4..]).unwrap();
        assert_eq!(decoded, resp);
    }

    #[test]
    fn unknown_version_rejected() {
        let data = [99, TAG_CREATE_INTERLOCK, 0];
        assert!(matches!(
            decode_request(&data),
            Err(ProtocolFault::UnsupportedVersion { version: 99 })
        ));
    }

    #[test]
    fn expected_fd_count_created() {
        assert_eq!(expected_fd_count(&Response::Created { id: 1 }), 1);
    }

    #[test]
    fn expected_fd_count_attached() {
        assert_eq!(expected_fd_count(&Response::Attached { id: 1 }), 1);
    }

    #[test]
    fn expected_fd_count_error() {
        assert_eq!(expected_fd_count(&Response::allocation_failed("x")), 0);
    }
}
