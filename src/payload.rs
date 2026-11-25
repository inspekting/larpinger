//! Custom payloads: CLI decoding (text / hex / file), built-in presets,
//! A2S (Source/GoldSource, CS 1.6) response parsing, hex/ASCII dumps.

use std::fmt::Write as _;

pub const MAX_PAYLOAD: usize = 4096;
pub const MAX_RESPONSE: usize = 32768;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// A2S_INFO — server name/map/players (CS 1.6, CS2, Source, GoldSource).
    A2sInfo,
    /// A2S_PLAYER — player list (challenge flow).
    A2sPlayer,
    /// A2S_RULES — server cvars (challenge flow).
    A2sRules,
    /// Rust (Facepunch) — Steam A2S on the query port.
    Rust,
    /// FiveM GET /info.json (HTTP/TCP, templated Host).
    FiveM,
    /// FiveM GET /players.json (HTTP/TCP).
    FiveMPlayers,
    /// FiveM GET /dynamic.json (HTTP/TCP).
    FiveMDynamic,
    /// Minecraft Java status ping (TCP, varint handshake, templated).
    Minecraft,
    /// Minecraft Bedrock RakNet unconnected ping (UDP).
    Bedrock,
    /// SA-MP server info opcode 'i' (UDP, templated ip/port).
    Samp,
    /// TeamSpeak 3 Server Query / Basic Status (UDP).
    TeamSpeak3,
}

/// All preset names, for help text.
pub const PRESET_NAMES: &str = "a2s, a2s-player, a2s-rules, rust, fivem, fivem-players, fivem-dynamic, minecraft, bedrock, samp, ts3 (aliases: cs16, cs2, gmod, ark, dayz, valheim, mc, mc-bedrock, teamspeak)";

impl Preset {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "a2s" | "a2s-info" | "cs16" | "cs" | "cs2" | "csgo" | "css" | "tf2" | "gmod"
            | "garrysmod" | "ark" | "dayz" | "valheim" | "squad" | "source" | "goldsource" => {
                Ok(Self::A2sInfo)
            }
            "a2s-player" | "players" => Ok(Self::A2sPlayer),
            "a2s-rules" | "rules" => Ok(Self::A2sRules),
            "rust" => Ok(Self::Rust),
            "fivem" | "fivem-info" | "gta5" | "gta" => Ok(Self::FiveM),
            "fivem-players" => Ok(Self::FiveMPlayers),
            "fivem-dynamic" | "fivem-dyn" => Ok(Self::FiveMDynamic),
            "minecraft" | "mc" | "mc-java" => Ok(Self::Minecraft),
            "bedrock" | "mc-bedrock" | "mcpe" => Ok(Self::Bedrock),
            "samp" | "sa-mp" | "gta-samp" => Ok(Self::Samp),
            "ts3" | "teamspeak" | "teamspeak3" => Ok(Self::TeamSpeak3),
            other => Err(format!(
                "unknown preset '{other}' (expected {PRESET_NAMES})"
            )),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::A2sInfo => "A2S_INFO",
            Self::A2sPlayer => "A2S_PLAYER",
            Self::A2sRules => "A2S_RULES",
            Self::Rust => "RUST (A2S)",
            Self::FiveM => "FIVEM /info.json",
            Self::FiveMPlayers => "FIVEM /players.json",
            Self::FiveMDynamic => "FIVEM /dynamic.json",
            Self::Minecraft => "MINECRAFT",
            Self::Bedrock => "MC BEDROCK",
            Self::Samp => "SA-MP",
            Self::TeamSpeak3 => "TEAMSPEAK 3",
        }
    }
    /// (is_tcp, default port) used when the user kept CLI defaults.
    pub fn transport(self) -> (bool, u16) {
        match self {
            Self::A2sInfo | Self::A2sPlayer | Self::A2sRules => (false, 27015),
            Self::Rust => (false, 28016),
            Self::FiveM | Self::FiveMPlayers | Self::FiveMDynamic => (true, 30120),
            Self::Minecraft => (true, 25565),
            Self::Bedrock => (false, 19132),
            Self::Samp => (false, 7777),
            Self::TeamSpeak3 => (false, 9987),
        }
    }
    pub fn is_a2s(self) -> bool {
        matches!(
            self,
            Self::A2sInfo | Self::A2sPlayer | Self::A2sRules | Self::Rust
        )
    }
    fn http_path(self) -> Option<&'static str> {
        match self {
            Self::FiveM => Some("/info.json"),
            Self::FiveMPlayers => Some("/players.json"),
            Self::FiveMDynamic => Some("/dynamic.json"),
            _ => None,
        }
    }
    /// Static request bytes (templated presets use `request_for`).
    pub fn request(self) -> Vec<u8> {
        match self {
            Self::A2sInfo | Self::Rust => b"\xff\xff\xff\xffTSource Engine Query\x00".to_vec(),
            Self::A2sPlayer => b"\xff\xff\xff\xffU\xff\xff\xff\xff".to_vec(),
            Self::A2sRules => b"\xff\xff\xff\xffV\xff\xff\xff\xff".to_vec(),
            Self::Bedrock => bedrock_request(),
            Self::TeamSpeak3 => ts3_request(),
            _ => Vec::new(), // templated: built per-endpoint in request_for
        }
    }
    /// Per-endpoint request: fills Host / handshake / SAMP header templates.
    pub fn request_for(self, ip: std::net::IpAddr, port: u16, host: &str) -> Vec<u8> {
        if let Some(path) = self.http_path() {
            return format!(
                "GET {path} HTTP/1.1\r\nHost: {ip}:{port}\r\nUser-Agent: LARPinger/0.1\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
            )
            .into_bytes();
        }
        match self {
            Self::Minecraft => minecraft_request(host, port),
            Self::Samp => samp_request(ip, port),
            _ => self.request(),
        }
    }
    pub fn uses_challenge(self) -> bool {
        self.is_a2s()
    }
    /// Build the follow-up request after an A2S challenge reply.
    pub fn challenged_request(self, challenge: &[u8; 4]) -> Vec<u8> {
        match self {
            Self::A2sInfo | Self::Rust => {
                let mut r = self.request();
                r.extend_from_slice(challenge);
                r
            }
            Self::A2sPlayer | Self::A2sRules => {
                let mut r = self.request();
                let n = r.len();
                r[n - 4..].copy_from_slice(challenge);
                r
            }
            _ => self.request(),
        }
    }
}

fn bedrock_request() -> Vec<u8> {
    // RakNet Unconnected Ping: 0x01 + timestamp + offline magic + client GUID.
    let mut v = vec![0x01u8];
    v.extend_from_slice(&0u64.to_be_bytes());
    v.extend_from_slice(&[
        0x00, 0xFF, 0xFF, 0x00, 0xFE, 0xFE, 0xFE, 0xFE, 0xFD, 0xFD, 0xFD, 0xFD, 0x12, 0x34, 0x56,
        0x78,
    ]);
    v.extend_from_slice(&0x4C415250u64.to_be_bytes()); // "LARP"
    v
}

fn samp_request(ip: std::net::IpAddr, port: u16) -> Vec<u8> {
    let mut v = b"SAMP".to_vec();
    match ip {
        std::net::IpAddr::V4(v4) => v.extend_from_slice(&v4.octets()),
        std::net::IpAddr::V6(_) => v.extend_from_slice(&[0, 0, 0, 0]),
    }
    v.extend_from_slice(&port.to_le_bytes());
    v.push(b'i');
    v
}

fn ts3_request() -> Vec<u8> {
    // TS3 standard UDP ping/status packet format (TS3 native TSDNS/ping header sequence)
    // Often starts with custom signature bytes or standard query payload string depending on the probe variant.
    // Standard basic TS3 status request packet format starts with specific magic bytes `TS3` or query formatting.
    b"\x54\x53\x33\x20\x00\x00\x00".to_vec()
}

fn varint(mut n: i32) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut b = (n & 0x7F) as u8;
        n = ((n as u32) >> 7) as i32;
        if n != 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
    out
}

fn minecraft_request(host: &str, port: u16) -> Vec<u8> {
    // Handshake (protocol 754, next-state=status) + empty Status Request.
    let mut hs = vec![0x00]; // packet id
    hs.extend_from_slice(&varint(754));
    hs.extend_from_slice(&varint(host.len() as i32));
    hs.extend_from_slice(host.as_bytes());
    hs.extend_from_slice(&port.to_be_bytes());
    hs.extend_from_slice(&varint(1));
    let mut out = varint(hs.len() as i32);
    out.extend_from_slice(&hs);
    out.extend_from_slice(&[0x01, 0x00]); // status request packet
    out
}

/// Resolved probe payload configuration shared by every endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadCfg {
    pub bytes: Vec<u8>,
    pub preset: Option<Preset>,
    /// Human label for header / logs.
    pub label: String,
}

impl PayloadCfg {
    /// Bytes to send to one endpoint (preset templates filled per-target).
    pub fn request_for(&self, ep: &crate::target::Endpoint) -> Vec<u8> {
        match self.preset {
            Some(p) => p.request_for(ep.ip, ep.port.unwrap_or(0), &ep.display),
            None => self.bytes.clone(),
        }
    }
}

impl Default for PayloadCfg {
    fn default() -> Self {
        Self {
            bytes: b"larpinger".to_vec(),
            preset: None,
            label: "default".into(),
        }
    }
}

/// Decode `--payload` text with `\xNN`, `\n`, `\r`, `\t`, `\0`, `\\` escapes.
pub fn decode_text_payload(input: &str) -> Result<Vec<u8>, String> {
    let b = input.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                b'n' => {
                    out.push(b'\n');
                    i += 2;
                }
                b'r' => {
                    out.push(b'\r');
                    i += 2;
                }
                b't' => {
                    out.push(b'\t');
                    i += 2;
                }
                b'0' => {
                    out.push(0);
                    i += 2;
                }
                b'\\' => {
                    out.push(b'\\');
                    i += 2;
                }
                b'x' => {
                    if i + 3 >= b.len() {
                        return Err("--payload: incomplete \\xNN escape".into());
                    }
                    let hex = std::str::from_utf8(&b[i + 2..i + 4]).unwrap_or("");
                    let v = u8::from_str_radix(hex, 16)
                        .map_err(|_| format!("--payload: invalid \\x{hex} escape"))?;
                    out.push(v);
                    i += 4;
                }
                other => {
                    return Err(format!("--payload: unknown escape \\{}", other as char));
                }
            }
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    check_len(out)
}

/// Decode `--payload-hex`: `ff ff 54 ...`, `ffff54...`, `0xff,0xff` all accepted.
pub fn decode_hex_payload(input: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',' && *c != '_')
        .collect::<String>()
        .replace("0x", "")
        .replace("0X", "");
    if cleaned.is_empty() || cleaned.len() % 2 != 0 {
        return Err("--payload-hex: need a non-empty even number of hex digits".into());
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    let bytes = cleaned.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let s = std::str::from_utf8(pair).unwrap();
        out.push(
            u8::from_str_radix(s, 16).map_err(|_| format!("--payload-hex: invalid hex '{s}'"))?,
        );
    }
    check_len(out)
}

fn check_len(out: Vec<u8>) -> Result<Vec<u8>, String> {
    if out.is_empty() {
        return Err("payload is empty".into());
    }
    if out.len() > MAX_PAYLOAD {
        return Err(format!(
            "payload too large ({} > {MAX_PAYLOAD} bytes)",
            out.len()
        ));
    }
    Ok(out)
}

/// If `data` is an A2S challenge reply (FF FF FF FF 41 <4 bytes>), return the challenge.
pub fn a2s_challenge(data: &[u8]) -> Option<[u8; 4]> {
    if data.len() >= 9 && data[..4] == [0xFF, 0xFF, 0xFF, 0xFF] && data[4] == 0x41 {
        Some([data[5], data[6], data[7], data[8]])
    } else {
        None
    }
}

// ---------------------------------------------------------------- A2S parsing

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    fn u16le(&mut self) -> Option<u16> {
        let lo = self.u8()?;
        let hi = self.u8()?;
        Some(u16::from_le_bytes([lo, hi]))
    }
    fn i32le(&mut self) -> Option<i32> {
        let mut b = [0u8; 4];
        for x in &mut b {
            *x = self.u8()?;
        }
        Some(i32::from_le_bytes(b))
    }
    fn f32le(&mut self) -> Option<f32> {
        let mut b = [0u8; 4];
        for x in &mut b {
            *x = self.u8()?;
        }
        Some(f32::from_le_bytes(b))
    }
    fn cstr(&mut self) -> Option<String> {
        let start = self.pos;
        while self.pos < self.b.len() && self.b[self.pos] != 0 {
            self.pos += 1;
        }
        if self.pos >= self.b.len() {
            return None;
        }
        let s = String::from_utf8_lossy(&self.b[start..self.pos]).into_owned();
        self.pos += 1; // NUL
        Some(s)
    }
}

/// Parse an A2S response into (key, value) lines. `None` if it isn't one.
pub fn parse_a2s(data: &[u8]) -> Option<Vec<(String, String)>> {
    if data.len() < 5 || data[..4] != [0xFF, 0xFF, 0xFF, 0xFF] {
        return None;
    }
    let mut r = Reader { b: data, pos: 4 };
    match r.u8()? {
        0x49 => parse_a2s_info(&mut r),
        0x6D => parse_goldsource_info(&mut r),
        0x44 => parse_a2s_player(&mut r),
        0x45 => parse_a2s_rules(&mut r),
        0x41 => Some(vec![("type".into(), "challenge (retry required)".into())]),
        _ => None,
    }
}

fn parse_a2s_info(r: &mut Reader) -> Option<Vec<(String, String)>> {
    let protocol = r.u8()?;
    let name = r.cstr()?;
    let map = r.cstr()?;
    let folder = r.cstr()?;
    let game = r.cstr()?;
    let app_id = r.u16le()?;
    let players = r.u8()?;
    let max_players = r.u8()?;
    let bots = r.u8()?;
    let server_type = r.u8()? as char;
    let env = r.u8()? as char;
    let visibility = r.u8()?;
    let vac = r.u8()?;
    let mut out = vec![
        ("server".into(), name),
        ("map".into(), map),
        ("game".into(), format!("{game} ({folder})")),
        (
            "players".into(),
            format!("{players}/{max_players} (bots {bots})"),
        ),
        (
            "type".into(),
            format!(
                "{server_type}/{env} · app {} · protocol {}",
                app_id, protocol
            ),
        ),
        (
            "visibility".into(),
            if visibility == 0 {
                "public".into()
            } else {
                "passworded".into()
            },
        ),
        (
            "vac".into(),
            if vac == 1 {
                "secured".into()
            } else {
                "unsecured".into()
            },
        ),
    ];
    if let Some(version) = r.cstr() {
        if !version.is_empty() {
            out.push(("version".into(), version));
        }
    }
    Some(out)
}

fn parse_goldsource_info(r: &mut Reader) -> Option<Vec<(String, String)>> {
    let _address = r.cstr()?;
    let name = r.cstr()?;
    let map = r.cstr()?;
    let folder = r.cstr()?;
    let players = r.u8()?;
    let max_players = r.u8()?;
    Some(vec![
        ("server".into(), name),
        ("map".into(), map),
        ("folder".into(), folder),
        ("players".into(), format!("{players}/{max_players}")),
    ])
}

fn parse_a2s_player(r: &mut Reader) -> Option<Vec<(String, String)>> {
    let count = r.u8()?;
    let mut out = vec![("player-count".into(), count.to_string())];
    for _ in 0..count {
        let _idx = r.u8()?;
        let name = r.cstr().unwrap_or_default();
        let frags = r.i32le().unwrap_or(0);
        let time = r.f32le().unwrap_or(0.0);
        out.push((
            "player".into(),
            format!("{name} · frags {frags} · {:.0}s", time),
        ));
        if out.len() > 64 {
            break;
        }
    }
    Some(out)
}

fn parse_a2s_rules(r: &mut Reader) -> Option<Vec<(String, String)>> {
    let count = r.u16le()?;
    let mut out = vec![("rule-count".into(), count.to_string())];
    for _ in 0..count {
        let k = r.cstr().unwrap_or_default();
        let v = r.cstr().unwrap_or_default();
        if !k.is_empty() {
            out.push((k, v));
        }
        if out.len() > 64 {
            out.push(("…".into(), "truncated".into()));
            break;
        }
    }
    Some(out)
}

// ---------------------------------------------------------------- game parsers

/// Unified response parser: A2S, HTTP/JSON (FiveM), Minecraft, Bedrock, SA-MP, TeamSpeak 3.
pub fn parse_response(data: &[u8]) -> Option<Vec<(String, String)>> {
    parse_a2s(data)
        .or_else(|| parse_http_json(data))
        .or_else(|| parse_minecraft(data))
        .or_else(|| parse_bedrock(data))
        .or_else(|| parse_samp(data))
        .or_else(|| parse_ts3(data))
}

fn json_at(data: &[u8]) -> Option<serde_json::Value> {
    let start = data.iter().position(|&b| b == b'{' || b == b'[')?;
    let text = std::str::from_utf8(&data[start..]).ok()?;
    serde_json::Deserializer::from_str(text)
        .into_iter::<serde_json::Value>()
        .next()?
        .ok()
}

fn jstr(v: &serde_json::Value, keys: &[&str]) -> Option<String> {
    let mut cur = v;
    for k in keys {
        cur = cur.get(k)?;
    }
    match cur {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn parse_http_json(data: &[u8]) -> Option<Vec<(String, String)>> {
    if !data.starts_with(b"HTTP/") {
        return None;
    }
    let status = String::from_utf8_lossy(data)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let v = json_at(data)?;
    let mut out = vec![("http".into(), status)];
    if let Some(arr) = v.as_array() {
        // FiveM /players.json
        out.push(("players-online".into(), arr.len().to_string()));
        for p in arr.iter().take(24) {
            let name = jstr(p, &["name"]).unwrap_or_else(|| "?".into());
            let ping = jstr(p, &["ping"]).unwrap_or_default();
            out.push(("player".into(), format!("{name} · ping {ping}")));
        }
        return Some(out);
    }
    // FiveM /dynamic.json
    if v.get("hostname").is_some() || v.get("clients").is_some() {
        for (label, keys) in [
            ("server", &["hostname"][..]),
            ("clients", &["clients"][..]),
            ("max-clients", &["sv_maxclients"][..]),
            ("map", &["mapname"][..]),
            ("gametype", &["gametype"][..]),
        ] {
            if let Some(x) = jstr(&v, keys) {
                out.push((label.into(), x));
            }
        }
        return Some(out);
    }
    // FiveM /info.json
    if let Some(vars) = v.get("vars") {
        for (label, key) in [
            ("server", "sv_hostname"),
            ("max-clients", "sv_maxClients"),
            ("game", "sv_enforceGameBuild"),
            ("onesync", "onesync_enabled"),
            ("txadmin", "txAdmin-version"),
        ] {
            if let Some(x) = jstr(vars, &[key]) {
                out.push((label.into(), x));
            }
        }
    }
    if let Some(x) = jstr(&v, &["server"]) {
        out.push(("server-sw".into(), x));
    }
    if let Some(x) = jstr(&v, &["version"]) {
        out.push(("version".into(), x));
    }
    if let Some(res) = v.get("resources").and_then(|r| r.as_array()) {
        out.push(("resources".into(), res.len().to_string()));
    }
    if out.len() > 1 {
        Some(out)
    } else {
        None
    }
}

fn mc_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(o) => {
            let mut s = o
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(extra) = o.get("extra").and_then(|e| e.as_array()) {
                for e in extra {
                    s.push_str(&mc_text(e));
                }
            }
            s
        }
        _ => String::new(),
    }
}

fn parse_minecraft(data: &[u8]) -> Option<Vec<(String, String)>> {
    if data.starts_with(b"HTTP/") || data.len() < 3 {
        return None;
    }
    let v = json_at(data)?;
    let version = jstr(&v, &["version", "name"])?;
    let mut out = vec![("version".into(), version)];
    if let Some(proto) = jstr(&v, &["version", "protocol"]) {
        out.push(("protocol".into(), proto));
    }
    if let (Some(on), Some(max)) = (
        jstr(&v, &["players", "online"]),
        jstr(&v, &["players", "max"]),
    ) {
        out.push(("players".into(), format!("{on}/{max}")));
    }
    if let Some(desc) = v.get("description") {
        let motd = mc_text(desc);
        if !motd.is_empty() {
            out.push(("motd".into(), motd.chars().take(90).collect()));
        }
    }
    if let Some(sample) = v
        .get("players")
        .and_then(|p| p.get("sample"))
        .and_then(|s| s.as_array())
    {
        for p in sample.iter().take(16) {
            if let Some(name) = jstr(p, &["name"]) {
                out.push(("player".into(), name));
            }
        }
    }
    Some(out)
}

fn parse_bedrock(data: &[u8]) -> Option<Vec<(String, String)>> {
    if data.first() != Some(&0x1C) || data.len() < 35 {
        return None;
    }
    let text = String::from_utf8_lossy(&data[35..]).into_owned();
    let f: Vec<&str> = text.split(';').collect();
    if f.len() < 8 || f[0] != "MCPE" {
        return None;
    }
    Some(vec![
        ("server".into(), f[1].to_string()),
        ("protocol".into(), f[2].to_string()),
        ("version".into(), f[3].to_string()),
        ("players".into(), format!("{}/{}", f[4], f[5])),
        ("world".into(), f.get(7).unwrap_or(&"").to_string()),
        ("gamemode".into(), f.get(8).unwrap_or(&"").to_string()),
    ])
}

fn parse_samp(data: &[u8]) -> Option<Vec<(String, String)>> {
    if data.len() < 11 || &data[..4] != b"SAMP" || data[10] != b'i' {
        return None;
    }
    let mut r = Reader { b: data, pos: 11 };
    let password = r.u8()?;
    let players = r.u16le()?;
    let max_players = r.u16le()?;
    let read_str = |r: &mut Reader| -> Option<String> {
        let len = r.i32le()? as usize;
        if r.pos + len > data.len() {
            return None;
        }
        let s = String::from_utf8_lossy(&data[r.pos..r.pos + len]).into_owned();
        r.pos += len;
        Some(s)
    };
    let hostname = read_str(&mut r).unwrap_or_default();
    let gamemode = read_str(&mut r).unwrap_or_default();
    let map = read_str(&mut r).unwrap_or_default();
    Some(vec![
        ("server".into(), hostname),
        ("players".into(), format!("{players}/{max_players}")),
        ("gamemode".into(), gamemode),
        ("map".into(), map),
        (
            "password".into(),
            if password == 1 {
                "yes".into()
            } else {
                "no".into()
            },
        ),
    ])
}

fn parse_ts3(data: &[u8]) -> Option<Vec<(String, String)>> {
    // Basic TS3 UDP status packet heuristic check
    let text = String::from_utf8_lossy(data);
    if text.contains("virtualserver_") || text.contains("TS3") || data.len() > 10 {
        let mut out = vec![("protocol".into(), "TeamSpeak 3".into())];
        // Parse basic key-value items if returned as key=value format
        for line in text.split(&['\n', '\r', ' ']) {
            if let Some((k, v)) = line.split_once('=') {
                if !k.is_empty() && !v.is_empty() {
                    out.push((k.to_string(), v.to_string()));
                }
            }
        }
        if out.len() > 1 {
            return Some(out);
        }
    }
    None
}

// ---------------------------------------------------------------- dumps

/// Classic 16-byte hexdump with ASCII gutter.
pub fn hexdump(data: &[u8], max_bytes: usize) -> Vec<String> {
    let shown = &data[..data.len().min(max_bytes)];
    let mut lines = Vec::new();
    for (off, chunk) in shown.chunks(16).enumerate() {
        let offset = off * 16;
        let mut hex = String::new();
        for (i, b) in chunk.iter().enumerate() {
            if i == 8 {
                hex.push(' ');
            }
            let _ = write!(hex, "{b:02x} ");
        }
        for i in chunk.len()..16 {
            if i == 8 {
                hex.push(' ');
            }
            hex.push_str("   ");
        }
        let ascii: String = chunk
            .iter()
            .map(|&b| {
                if (0x20..0x7F).contains(&b) {
                    b as char
                } else {
                    '·'
                }
            })
            .collect();
        lines.push(format!("{offset:04x}  {hex} |{ascii}|"));
    }
    if data.len() > shown.len() {
        lines.push(format!("… {} more bytes", data.len() - shown.len()));
    }
    lines
}

/// Printable strings (≥4 chars) found in a response — `strings(1)` lite.
pub fn printable_strings(data: &[u8], limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for &b in data {
        if (0x20..0x7F).contains(&b) {
            cur.push(b as char);
        } else {
            if cur.len() >= 4 {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
            if out.len() >= limit {
                return out;
            }
        }
    }
    if cur.len() >= 4 && out.len() < limit {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_payload_escapes() {
        assert_eq!(
            decode_text_payload(r"\xff\xffT\0").unwrap(),
            vec![0xFF, 0xFF, b'T', 0]
        );
        assert_eq!(decode_text_payload("a\nb").unwrap(), b"a\nb");
        assert!(decode_text_payload(r"\x1").is_err());
        assert!(decode_text_payload(r"\q").is_err());
    }

    #[test]
    fn hex_payload_forms() {
        assert_eq!(
            decode_hex_payload("ff ff 54").unwrap(),
            vec![0xFF, 0xFF, 0x54]
        );
        assert_eq!(decode_hex_payload("0xff,0x00").unwrap(), vec![0xFF, 0]);
        assert!(decode_hex_payload("abc").is_err());
    }

    #[test]
    fn preset_bytes_are_a2s_info() {
        let req = Preset::A2sInfo.request();
        assert_eq!(&req[..5], b"\xff\xff\xff\xffT");
        assert!(req.ends_with(b"Source Engine Query\0"));
    }

    #[test]
    fn parse_cs16_style_info() {
        let mut d = vec![0xFF, 0xFF, 0xFF, 0xFF, 0x49, 48];
        for s in ["Test Server", "de_dust2", "cstrike", "Counter-Strike"] {
            d.extend_from_slice(s.as_bytes());
            d.push(0);
        }
        d.extend_from_slice(&10u16.to_le_bytes());
        d.extend_from_slice(&[31, 32, 0, b'd', b'l', 0, 1]);
        d.extend_from_slice(b"1.1.2.7/Stdio\0");
        let parsed = parse_a2s(&d).unwrap();
        let map: std::collections::HashMap<_, _> = parsed.into_iter().collect();
        assert_eq!(map["map"], "de_dust2");
        assert_eq!(map["players"], "31/32 (bots 0)");
        assert_eq!(map["server"], "Test Server");
    }

    #[test]
    fn hexdump_and_strings() {
        let lines = hexdump(b"\xff\x00hello world\x00", 64);
        assert!(lines[0].contains("hello world"));
        assert_eq!(
            printable_strings(b"\x00abcd\x00xy\x00long string", 4),
            vec!["abcd", "long string"]
        );
    }

    #[test]
    fn challenge_detect() {
        assert_eq!(
            a2s_challenge(&[0xFF, 0xFF, 0xFF, 0xFF, 0x41, 1, 2, 3, 4]),
            Some([1, 2, 3, 4])
        );
        assert_eq!(a2s_challenge(b"nope"), None);
    }

    #[test]
    fn game_preset_aliases_and_templates() {
        assert_eq!(Preset::parse("rust").unwrap(), Preset::Rust);
        assert_eq!(Preset::parse("fivem").unwrap(), Preset::FiveM);
        assert_eq!(Preset::parse("mc").unwrap(), Preset::Minecraft);
        assert_eq!(Preset::parse("cs2").unwrap(), Preset::A2sInfo);
        assert_eq!(Preset::parse("ts3").unwrap(), Preset::TeamSpeak3);
        let http = Preset::FiveM.request_for("1.2.3.4".parse().unwrap(), 30120, "1.2.3.4");
        assert!(String::from_utf8(http)
            .unwrap()
            .starts_with("GET /info.json "));
        let mc = Preset::Minecraft.request_for("1.2.3.4".parse().unwrap(), 25565, "example.org");
        assert!(mc.ends_with(&[0x01, 0x00]));
        assert_eq!(Preset::FiveM.transport(), (true, 30120));
        assert_eq!(Preset::Bedrock.transport(), (false, 19132));
        assert_eq!(Preset::TeamSpeak3.transport(), (false, 9987));
    }
}