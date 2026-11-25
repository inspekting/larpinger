//! Probe engines: native ICMP echo (raw socket, `ping(8)` fallback), TCP connect, UDP exchange.

use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use crate::payload::{a2s_challenge, PayloadCfg, MAX_RESPONSE};
use crate::stats::Status;
use crate::target::{Endpoint, Protocol};

#[derive(Debug)]
pub enum Outcome {
    Ok {
        status: Status,
        rtt: Duration,
        response: Option<Vec<u8>>,
    },
    Fail(String),
}

pub fn probe(ep: &Endpoint, timeout: Duration, seq: u16, cfg: &PayloadCfg) -> Outcome {
    match ep.protocol {
        Protocol::Icmp => icmp(ep.ip, timeout, seq),
        Protocol::Tcp => tcp(ep, timeout, cfg),
        Protocol::Udp => udp(ep, timeout, cfg),
    }
}

// ---------------------------------------------------------------- TCP

fn tcp(ep: &Endpoint, timeout: Duration, cfg: &PayloadCfg) -> Outcome {
    let addr = SocketAddr::new(ep.ip, ep.port.unwrap_or(0));
    let start = Instant::now();
    let mut stream = match TcpStream::connect_timeout(&addr, timeout) {
        Ok(s) => s,
        Err(e) => return Outcome::Fail(friendly_err(&e)),
    };
    if cfg.preset.is_none() && cfg.label == "default" {
        let rtt = start.elapsed();
        let _ = stream.shutdown(std::net::Shutdown::Both);
        return Outcome::Ok {
            status: Status::Up,
            rtt,
            response: None,
        };
    }
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    if let Err(e) = stream.write_all(&cfg.request_for(ep)) {
        return Outcome::Fail(friendly_err(&e));
    }
    // Read until EOF (HTTP Connection: close), cap, or timeout-after-data.
    let mut acc: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let first = acc.is_empty();
                acc.extend_from_slice(&chunk[..n]);
                if acc.len() >= MAX_RESPONSE {
                    acc.truncate(MAX_RESPONSE);
                    break;
                }
                if first {
                    // Response started: don't wait the full timeout for a
                    // peer that keeps the socket open (e.g. Minecraft).
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                break
            }
            Err(e) if acc.is_empty() => return Outcome::Fail(friendly_err(&e)),
            Err(_) => break,
        }
    }
    Outcome::Ok {
        status: Status::Up,
        rtt: start.elapsed(),
        response: if acc.is_empty() { None } else { Some(acc) },
    }
}

// ---------------------------------------------------------------- UDP

fn udp(ep: &Endpoint, timeout: Duration, cfg: &PayloadCfg) -> Outcome {
    let bind: SocketAddr = if ep.ip.is_ipv6() {
        "[::]:0".parse().unwrap()
    } else {
        "0.0.0.0:0".parse().unwrap()
    };
    let sock = match UdpSocket::bind(bind) {
        Ok(s) => s,
        Err(e) => return Outcome::Fail(friendly_err(&e)),
    };
    let target = SocketAddr::new(ep.ip, ep.port.unwrap_or(0));
    if let Err(e) = sock.connect(target) {
        return Outcome::Fail(friendly_err(&e));
    }
    let _ = sock.set_read_timeout(Some(timeout));
    let _ = sock.set_write_timeout(Some(timeout));
    let start = Instant::now();
    match udp_exchange(&sock, cfg, ep) {
        Ok(resp) => Outcome::Ok {
            status: Status::Up,
            rtt: start.elapsed(),
            response: Some(resp),
        },
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
            Outcome::Fail("port unreachable".into())
        }
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Outcome::Ok {
                status: Status::Open,
                rtt: start.elapsed(),
                response: None,
            }
        }
        Err(e) => Outcome::Fail(friendly_err(&e)),
    }
}

/// One UDP request/response, answering a single A2S challenge if the preset asks.
fn udp_exchange(sock: &UdpSocket, cfg: &PayloadCfg, ep: &Endpoint) -> io::Result<Vec<u8>> {
    sock.send(&cfg.request_for(ep))?;
    let first = udp_recv(sock)?;
    if let Some(preset) = cfg.preset {
        if preset.uses_challenge() {
            if let Some(challenge) = a2s_challenge(&first) {
                sock.send(&preset.challenged_request(&challenge))?;
                return udp_recv(sock);
            }
        }
    }
    Ok(first)
}

fn udp_recv(sock: &UdpSocket) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; MAX_RESPONSE];
    let n = sock.recv(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

// ---------------------------------------------------------------- ICMP

fn icmp(ip: IpAddr, timeout: Duration, seq: u16) -> Outcome {
    match icmp_native(ip, timeout, seq) {
        Ok(rtt) => Outcome::Ok { status: Status::Up, rtt, response: None },
        Err(e) if is_permission(&e) => match icmp_via_ping(ip, timeout) {
            Ok(rtt) => Outcome::Ok { status: Status::Up, rtt, response: None },
            Err(pe) => Outcome::Fail(format!(
                "raw ICMP denied ({e}); ping fallback failed: {pe} — try sudo or `setcap cap_net_raw+ep`"
            )),
        },
        Err(e) => Outcome::Fail(friendly_io(&e)),
    }
}

fn is_permission(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(1)
}

fn icmp_native(ip: IpAddr, timeout: Duration, seq: u16) -> io::Result<Duration> {
    use socket2::{Domain, Protocol as SockProto, Socket, Type};
    let ip4 = match ip {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(_) => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "ICMPv6 not supported yet — use tcp/udp for IPv6 targets",
            ))
        }
    };
    // SOCK_RAW = 3 on Linux; socket2 0.5 exposes it via Type::from(c_int).
    let sock = Socket::new(Domain::IPV4, Type::from(3), Some(SockProto::ICMPV4))?;
    sock.set_read_timeout(Some(timeout))?;
    sock.set_write_timeout(Some(timeout))?;
    let id: u16 = (std::process::id() & 0xFFFF) as u16;
    let packet = echo_request(id, seq);
    let dest: socket2::SockAddr = SocketAddr::new(IpAddr::V4(ip4), 0).into();
    let start = Instant::now();
    sock.send_to(&packet, &dest)?;

    let mut buf = [std::mem::MaybeUninit::<u8>::uninit(); 1500];
    loop {
        if start.elapsed() > timeout {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timeout"));
        }
        let n = sock.recv(&mut buf)?;
        // SAFETY: recv initialized the first `n` bytes.
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, n) };
        if parse_echo_reply(bytes, id, seq) {
            return Ok(start.elapsed());
        }
        // Not our reply (other traffic on the raw socket): keep waiting.
    }
}

/// Build an ICMP Echo Request (type 8) with a 32-byte payload.
fn echo_request(id: u16, seq: u16) -> [u8; 40] {
    let mut pkt = [0u8; 40];
    pkt[0] = 8; // type: echo request
    pkt[1] = 0; // code
    pkt[4..6].copy_from_slice(&id.to_be_bytes());
    pkt[6..8].copy_from_slice(&seq.to_be_bytes());
    for (i, b) in pkt[8..].iter_mut().enumerate() {
        *b = b'a' + (i % 26) as u8;
    }
    let sum = checksum(&pkt);
    pkt[2..4].copy_from_slice(&sum.to_be_bytes());
    pkt
}

/// Validate an IPv4+ICMP Echo Reply for `id`/`seq`.
fn parse_echo_reply(buf: &[u8], id: u16, seq: u16) -> bool {
    if buf.len() < 20 {
        return false;
    }
    let ihl = ((buf[0] & 0x0F) as usize) * 4;
    if buf.len() < ihl + 8 || buf[9] != 1 {
        return false; // not ICMP
    }
    let icmp = &buf[ihl..];
    icmp[0] == 0 // type: echo reply
        && u16::from_be_bytes([icmp[4], icmp[5]]) == id
        && u16::from_be_bytes([icmp[6], icmp[7]]) == seq
}

pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = data.chunks_exact(2);
    for c in &mut chunks {
        sum += u16::from_be_bytes([c[0], c[1]]) as u32;
    }
    if let Some(&last) = chunks.remainder().first() {
        sum += (last as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Fallback: system `ping`. Used only when raw sockets are denied.
fn icmp_via_ping(ip: IpAddr, timeout: Duration) -> Result<Duration, String> {
    let secs = timeout.as_secs().max(1).to_string();
    let out = std::process::Command::new("ping")
        .args(["-c", "1", "-W", &secs, &ip.to_string()])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("ping exited {}", out.status));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // `time=12.3 ms` (Linux) / `time=12ms`
    for tok in text.split_whitespace() {
        if let Some(v) = tok
            .strip_prefix("time=")
            .or_else(|| tok.strip_prefix("time<"))
        {
            if let Ok(ms) = v.trim_end_matches("ms").parse::<f64>() {
                return Ok(Duration::from_secs_f64(ms / 1000.0));
            }
        }
    }
    Err("could not parse ping output".into())
}

fn friendly_err(e: &io::Error) -> String {
    friendly_io(e)
}

fn friendly_io(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::ConnectionRefused => "connection refused".into(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => "timeout".into(),
        io::ErrorKind::PermissionDenied => "permission denied".into(),
        _ => {
            let s = e.to_string();
            // Trim the noisy OS suffix, keep it short for the table cell.
            s.chars().take(48).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_known_vector() {
        // RFC 1071 example header.
        let data = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(checksum(&data), 0x220d);
    }

    #[test]
    fn echo_reply_parsing() {
        let id = 0x1234u16;
        let seq = 7u16;
        let mut ip = [0u8; 20];
        ip[0] = 0x45;
        ip[9] = 1;
        let mut pkt = Vec::new();
        pkt.extend_from_slice(&ip);
        pkt.extend_from_slice(&[0, 0, 0, 0]);
        pkt.extend_from_slice(&id.to_be_bytes());
        pkt.extend_from_slice(&seq.to_be_bytes());
        assert!(parse_echo_reply(&pkt, id, seq));
        assert!(!parse_echo_reply(&pkt, id, seq + 1));
    }

    #[test]
    fn loopback_tcp_and_icmp_paths() {
        let ep = Endpoint {
            display: "127.0.0.1".into(),
            ip: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: Some(9),
            protocol: crate::target::Protocol::Tcp,
        };
        // Discard port is closed on virtually every host: must fail fast, not hang.
        assert!(matches!(
            probe(
                &ep,
                Duration::from_millis(500),
                1,
                &crate::payload::PayloadCfg::default()
            ),
            Outcome::Fail(_)
        ));
    }
}
