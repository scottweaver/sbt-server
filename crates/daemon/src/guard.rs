//! Admission of HTTP clients. Binding to loopback does not keep browsers
//! out: a page on `attacker.example`, whose name is re-resolved to
//! 127.0.0.1 (DNS rebinding), fetches `http://attacker.example:8077/exec`
//! and the browser delivers it here with that name as `Host` and an
//! `Origin` header. `sbtctl` sends a loopback `Host` and never an `Origin`,
//! so every request is held to both.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use thiserror::Error;

use crate::server::ApiError;

/// A `Host` header value that names this machine's loopback interface:
/// a loopback IP literal (the same set `LoopbackAddr` accepts for `--bind`)
/// or `localhost`, with an optional port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopbackHost {
    Ip(IpAddr),
    Localhost,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HostError {
    #[error("request has no Host header")]
    Missing,
    #[error("Host {0:?} is malformed")]
    Malformed(String),
    #[error("Host {0:?} is not loopback")]
    NotLoopback(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Refusal {
    #[error("{0}; sbtd answers only to Host 127.0.0.1, [::1] or localhost")]
    Host(#[from] HostError),
    #[error("request carries an Origin header; sbtd does not serve browsers")]
    Origin,
}

/// Parse a `Host` header value (`host[:port]`, IPv6 in brackets) into proof
/// that it addresses loopback.
///
/// # Errors
///
/// The value is not `host[:port]` with a numeric port, or the host is
/// neither a loopback IP literal nor `localhost`.
pub fn parse_loopback_host(text: &str) -> Result<LoopbackHost, HostError> {
    let malformed = || HostError::Malformed(text.to_owned());
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(malformed());
    }
    let (host, port) = match text.strip_prefix('[') {
        Some(rest) => {
            let (ip, after) = rest.split_once(']').ok_or_else(malformed)?;
            let ip: Ipv6Addr = ip.parse().map_err(|_| malformed())?;
            let port = match after {
                "" => None,
                _ => Some(after.strip_prefix(':').ok_or_else(malformed)?),
            };
            (HostName::Ip(IpAddr::V6(ip)), port)
        }
        None => match text.rsplit_once(':') {
            Some((host, _)) if host.is_empty() || host.contains(':') => {
                return Err(malformed());
            }
            Some((host, port)) => (HostName::parse(host), Some(port)),
            None => (HostName::parse(text), None),
        },
    };
    if let Some(port) = port {
        port.parse::<u16>().map_err(|_| malformed())?;
    }
    match host {
        HostName::Ip(ip) if ip.is_loopback() => Ok(LoopbackHost::Ip(ip)),
        HostName::Localhost => Ok(LoopbackHost::Localhost),
        HostName::Ip(_) | HostName::Other => Err(HostError::NotLoopback(text.to_owned())),
    }
}

enum HostName {
    Ip(IpAddr),
    Localhost,
    Other,
}

impl HostName {
    fn parse(host: &str) -> Self {
        match host.parse::<Ipv4Addr>() {
            Ok(ip) => Self::Ip(IpAddr::V4(ip)),
            Err(_) if host.eq_ignore_ascii_case("localhost") => Self::Localhost,
            Err(_) => Self::Other,
        }
    }
}

/// Decide whether a request may reach a handler, from its headers alone.
///
/// # Errors
///
/// The `Host` header is missing, malformed or not loopback, or an `Origin`
/// header is present.
pub fn admit(headers: &HeaderMap) -> Result<LoopbackHost, Refusal> {
    let host = headers.get(header::HOST).ok_or(HostError::Missing)?;
    let host = parse_loopback_host(header_text(host).as_ref())?;
    if headers.contains_key(header::ORIGIN) {
        return Err(Refusal::Origin);
    }
    Ok(host)
}

fn header_text(value: &HeaderValue) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(value.as_bytes())
}

/// The middleware every route sits behind: a refused request is answered
/// with 403 in sbtd's own error shape and never reaches a handler.
pub async fn require_loopback_client(request: Request, next: Next) -> Response {
    match admit(request.headers()) {
        Ok(_) => next.run(request).await,
        Err(refusal) => {
            tracing::warn!(
                %refusal,
                method = %request.method(),
                path = request.uri().path(),
                "request refused"
            );
            ApiError::forbidden(&refusal).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_literals_and_localhost_are_accepted_with_or_without_a_port() {
        let v4 = LoopbackHost::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let v6 = LoopbackHost::Ip(IpAddr::V6(Ipv6Addr::LOCALHOST));
        for (text, expected) in [
            ("127.0.0.1", v4),
            ("127.0.0.1:8077", v4),
            ("127.0.0.1:0", v4),
            ("[::1]", v6),
            ("[::1]:8077", v6),
            ("localhost", LoopbackHost::Localhost),
            ("localhost:8077", LoopbackHost::Localhost),
            ("LocalHost:8077", LoopbackHost::Localhost),
            (
                "127.0.0.2:1",
                LoopbackHost::Ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2))),
            ),
        ] {
            assert_eq!(parse_loopback_host(text), Ok(expected), "{text}");
        }
    }

    #[test]
    fn any_other_host_is_not_loopback() {
        for text in [
            "attacker.example",
            "attacker.example:8077",
            "0.0.0.0:8077",
            "192.168.1.2:8077",
            "[::]:8077",
            "[fe80::1]:8077",
            "localhost.attacker.example",
            "notlocalhost:8077",
            "127.0.0.1.attacker.example",
        ] {
            assert_eq!(
                parse_loopback_host(text),
                Err(HostError::NotLoopback(text.to_owned())),
                "{text}"
            );
        }
    }

    #[test]
    fn a_host_that_is_not_host_port_is_malformed() {
        for text in [
            "",
            "localhost:",
            "localhost:abc",
            "localhost:65536",
            "[::1",
            "[::1]x",
            "[::1]:",
            "[not-an-ip]:8077",
            "::1:8077",
        ] {
            assert_eq!(
                parse_loopback_host(text),
                Err(HostError::Malformed(text.to_owned())),
                "{text}"
            );
        }
    }

    fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| (name.clone(), HeaderValue::from_str(value).unwrap()))
            .collect()
    }

    #[test]
    fn admission_needs_a_loopback_host_and_no_origin() {
        assert_eq!(
            admit(&headers(&[(header::HOST, "127.0.0.1:8077")])),
            Ok(LoopbackHost::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)))
        );
        assert_eq!(admit(&headers(&[])), Err(Refusal::Host(HostError::Missing)));
        assert_eq!(
            admit(&headers(&[(header::HOST, "attacker.example:8077")])),
            Err(Refusal::Host(HostError::NotLoopback(
                "attacker.example:8077".to_owned()
            )))
        );
        for origin in ["http://attacker.example", "http://127.0.0.1:8077", "null"] {
            assert_eq!(
                admit(&headers(&[
                    (header::HOST, "127.0.0.1:8077"),
                    (header::ORIGIN, origin)
                ])),
                Err(Refusal::Origin),
                "{origin}"
            );
        }
    }

    #[test]
    fn a_host_that_is_not_text_is_malformed_not_a_panic() {
        let mut map = HeaderMap::new();
        map.insert(header::HOST, HeaderValue::from_bytes(b"\xff\xfe").unwrap());
        assert!(matches!(
            admit(&map),
            Err(Refusal::Host(HostError::Malformed(_)))
        ));
    }
}
