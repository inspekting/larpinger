//! Target, subnet and port parsing.
//!
//! Targets: `host`, `1.2.3.4`, `10.0.0.0/24`, comma-separated mixes.
//! Ports:   `80`, `80,443`, `8000-8010`, `22,80,8000-8010`.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};

pub const MAX_HOSTS: u32 = 4096;

/// One resolved endpoint to probe. `port == None` for ICMP.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub display: String,
    pub ip: IpAddr,
    pub port: Option<u16>,
    pub protocol: Protocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    Icmp,
    Tcp,
    Udp,
}

impl Protocol {
    pub fn parse(s: &str) -> Result<Self, Error> {
        match s.to_ascii_lowercase().as_str() {
            "icmp" | "ping" => Ok(Self::Icmp),
            "tcp" => Ok(Self::Tcp),
            "udp" => Ok(Self::Udp),
            other => Err(Error::Msg(format!(
                "unknown protocol '{other}' (expected icmp|tcp|udp)"
            ))),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Icmp => "ICMP",
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug)]
pub enum Error {
    Msg(String),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Msg(m) => f.write_str(m),
        }
    }
}
impl std::error::Error for Error {}
fn err<T>(m: impl Into<String>) -> Result<T, Error> {
    Err(Error::Msg(m.into()))
}

/// Parse `80,443,8000-8010` (deduped, sorted). Empty input is an error.
pub fn parse_ports(input: &str) -> Result<Vec<u16>, Error> {
    let mut ports: Vec<u16> = Vec::new();
    for part in input.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((a, b)) = part.split_once('-') {
            let (a, b): (u16, u16) = (
                a.trim()
                    .parse()
                    .map_err(|_| Error::Msg(format!("invalid port '{a}'")))?,
                b.trim()
                    .parse()
                    .map_err(|_| Error::Msg(format!("invalid port '{b}'")))?,
            );
            if a == 0 || b == 0 {
                return err("port 0 is not valid");
            }
            if a > b {
                return err(format!("invalid port range '{part}' (start > end)"));
            }
            if (b - a) as usize > 1024 {
                return err(format!(
                    "port range '{part}' too large (max 1025 ports per range)"
                ));
            }
            ports.extend(a..=b);
        } else {
            let p: u16 = part
                .parse()
                .map_err(|_| Error::Msg(format!("invalid port '{part}'")))?;
            if p == 0 {
                return err("port 0 is not valid");
            }
            ports.push(p);
        }
    }
    if ports.is_empty() {
        return err("no ports given");
    }
    ports.sort_unstable();
    ports.dedup();
    Ok(ports)
}

/// Expand one target token into IPs. CIDR expands the subnet; hostnames resolve.
pub fn expand_target(raw: &str) -> Result<Vec<(String, IpAddr)>, Error> {
    let raw = raw.trim();
    if raw.is_empty() {
        return err("empty target");
    }
    if let Some((addr, prefix)) = raw.split_once('/') {
        return expand_cidr(addr.trim(), prefix.trim(), raw);
    }
    // Plain IP?
    if let Ok(ip) = raw.parse::<IpAddr>() {
        return Ok(vec![(raw.to_string(), ip)]);
    }
    // Hostname: resolve once.
    let resolved: Vec<IpAddr> = format!("{raw}:0")
        .to_socket_addrs()
        .map_err(|e| Error::Msg(format!("cannot resolve '{raw}': {e}")))?
        .map(|s| s.ip())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if resolved.is_empty() {
        return err(format!("cannot resolve '{raw}': no addresses"));
    }
    Ok(resolved
        .into_iter()
        .map(|ip| (raw.to_string(), ip))
        .collect())
}

fn expand_cidr(addr: &str, prefix_s: &str, raw: &str) -> Result<Vec<(String, IpAddr)>, Error> {
    let ip: Ipv4Addr = addr.parse().map_err(|_| {
        Error::Msg(format!(
            "invalid subnet address in '{raw}' (IPv4 CIDR only)"
        ))
    })?;
    let prefix: u32 = prefix_s
        .parse()
        .map_err(|_| Error::Msg(format!("invalid prefix in '{raw}'")))?;
    if prefix > 32 {
        return err(format!("invalid prefix /{prefix} in '{raw}'"));
    }
    let host_bits = 32 - prefix;
    let count: u32 = if host_bits >= 32 {
        u32::MAX
    } else {
        1u32 << host_bits
    };
    if count > MAX_HOSTS {
        return err(format!(
            "subnet '{raw}' expands to {count} hosts (limit {MAX_HOSTS}) — use a smaller subnet"
        ));
    }
    let base = u32::from(ip);
    let mask: u32 = if prefix == 0 {
        0
    } else {
        u32::MAX << host_bits
    };
    let network = base & mask;
    // /32 = the host itself, /31 = both link hosts, else skip network+broadcast.
    let (start, end) = if prefix >= 31 {
        (network, network + count - 1)
    } else {
        (network + 1, network + count - 2)
    };
    let mut out = Vec::with_capacity((end - start + 1) as usize);
    let mut cur = start;
    loop {
        out.push((
            Ipv4Addr::from(cur).to_string(),
            IpAddr::V4(Ipv4Addr::from(cur)),
        ));
        if cur == end {
            break;
        }
        cur += 1;
    }
    Ok(out)
}

/// Build the full endpoint matrix: targets × protocols × ports.
pub fn build_endpoints(
    targets: &[String],
    protocols: &[Protocol],
    ports: &[u16],
) -> Result<Vec<Endpoint>, Error> {
    let mut hosts: Vec<(String, IpAddr)> = Vec::new();
    for t in targets {
        for part in t.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            hosts.extend(expand_target(part)?);
        }
    }
    if hosts.is_empty() {
        return err("no targets");
    }
    let mut out = Vec::new();
    for proto in protocols {
        for (display, ip) in &hosts {
            match proto {
                Protocol::Icmp => out.push(Endpoint {
                    display: display.clone(),
                    ip: *ip,
                    port: None,
                    protocol: *proto,
                }),
                Protocol::Tcp | Protocol::Udp => {
                    for p in ports {
                        out.push(Endpoint {
                            display: display.clone(),
                            ip: *ip,
                            port: Some(*p),
                            protocol: *proto,
                        });
                    }
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_single_list_range() {
        assert_eq!(parse_ports("80").unwrap(), vec![80]);
        assert_eq!(parse_ports("443,80,80").unwrap(), vec![80, 443]);
        assert_eq!(
            parse_ports("8000-8002,22").unwrap(),
            vec![22, 8000, 8001, 8002]
        );
    }

    #[test]
    fn ports_reject_bad() {
        assert!(parse_ports("0").is_err());
        assert!(parse_ports("90-80").is_err());
        assert!(parse_ports("nope").is_err());
        assert!(parse_ports("").is_err());
    }

    #[test]
    fn cidr_expansion() {
        let v = expand_target("192.168.1.0/30").unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].1, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(v[1].1, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2)));
        assert_eq!(expand_target("10.0.0.5/32").unwrap().len(), 1);
        assert_eq!(expand_target("10.0.0.0/31").unwrap().len(), 2);
        assert_eq!(expand_target("10.0.0.0/24").unwrap().len(), 254);
    }

    #[test]
    fn cidr_rejects_huge() {
        assert!(expand_target("10.0.0.0/8").is_err());
        assert!(expand_target("10.0.0.0/33").is_err());
    }

    #[test]
    fn matrix_icmp_ignores_ports_tcp_uses_them() {
        let eps = build_endpoints(
            &["127.0.0.1".into()],
            &[Protocol::Icmp, Protocol::Tcp],
            &[80, 443],
        )
        .unwrap();
        assert_eq!(eps.len(), 3);
        assert_eq!(eps.iter().filter(|e| e.port.is_none()).count(), 1);
    }
}
