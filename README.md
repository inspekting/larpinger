# LARPinger

Multi-protocol ping for the terminal — **ICMP, TCP and UDP** in one tool, with
custom ports, subnet expansion, and a live TUI. Continuous by default.

```
┌──────────────────────────────────────────────────────────────────────┐
│ LARPinger  multi-protocol ping   LIVE   4 targets · interval 1000ms │
├──────────────────────────────────────────────────────────────────────┤
│  ▲ up 3   ▼ down 1   sent 128  recv 121  loss 5.5%  avg 12.4ms       │
├──────────────────────────────────────────────────────────────────────┤
│  TARGET           PROTO PORT STATUS   LAST    MIN   AVG   MAX  LOSS  │
│▸ 192.168.1.1      ICMP  —    ● UP     1.20ms  0.9   1.3   4.1  0%   │
│  192.168.1.10:443 TCP   443  ● UP     0.84ms  0.7   0.9   2.0  0%   │
│  10.0.0.5:53      UDP   53   ◐ OPEN   —       —     —     —    —    │
├──────────────────────────────────────────────────────────────────────┤
│ detail  192.168.1.1  ICMP  ● UP   ▁▂▁▃▂▁▁▂▅▂▁▂   err: —             │
└──────────────────────────────────────────────────────────────────────┘
 q quit   space pause   r reset   ↑↓/jk select   +/- interval
```

## Install

```sh
cargo build --release
sudo install -m755 target/release/larpinger /usr/local/bin/

# ICMP raw sockets need privilege — pick one:
sudo larpinger 1.1.1.1                                    # run as root, or
sudo setcap cap_net_raw+ep /usr/local/bin/larpinger       # grant the capability once
```

Without raw-socket permission LARPinger automatically falls back to the system
`ping` binary for ICMP.

## Usage

```sh
larpinger 1.1.1.1                                  # ICMP, continuous
larpinger -p tcp -P 443 1.1.1.1                    # TCP ping to 443
larpinger -p tcp -P 22,80,443 example.com          # several ports
larpinger -p tcp -P 8000-8010 192.168.1.10         # port range
larpinger -p udp -P 53 8.8.8.8                     # UDP probe
larpinger -p icmp,tcp -P 80 192.168.1.0/24         # subnet × protocols × ports
larpinger -c 10 -i 500 10.0.0.0/30                 # 10 rounds, 500 ms apart
larpinger --no-tui -c 3 -p tcp -P 443 1.1.1.1      # plain log (auto when piped)
```

## Custom payloads & response inspection

Send your own bytes on TCP/UDP probes and read what comes back. Four ways to
specify a payload (mutually exclusive), plus built-in game-server presets:

```sh
# CS 1.6 / Source server — A2S_INFO preset (name, map, players, VAC, version)
larpinger -p udp -P 27015 --preset a2s 217.156.22.85
larpinger -p udp -P 27015 --preset a2s-player 217.156.22.85   # live player list
larpinger -p udp -P 27015 --preset a2s-rules 217.156.22.85    # server cvars

# Popular games — preset auto-selects protocol + port if -p/-P stay default
larpinger --preset rust 203.0.113.10            # Rust (A2S, udp/28016 query port)
larpinger --preset fivem 203.0.113.20           # FiveM /info.json (tcp/30120)
larpinger --preset fivem-players 203.0.113.20   # FiveM live player list
larpinger --preset fivem-dynamic 203.0.113.20   # FiveM hostname/clients/map
larpinger --preset minecraft mc.hypixel.net     # Minecraft Java status (tcp/25565)
larpinger --preset bedrock geo.hivebedrock.network  # Bedrock RakNet (udp/19132)
larpinger --preset samp 203.0.113.30            # SA-MP info (udp/7777)
larpinger --preset cs2 203.0.113.40             # aliases: cs16 gmod ark dayz valheim squad tf2…

# Same query hand-rolled as hex, and as escaped text
larpinger -p udp -P 27015 --payload-hex "ffffffff54536f7572636520456e67696e6520517565727900" 217.156.22.85
larpinger -p udp -P 27015 --payload '\xff\xff\xff\xffTSource Engine Query\0' 217.156.22.85
larpinger -p tcp -P 80 --payload 'GET / HTTP/1.0\r\n\r\n' example.com
larpinger -p udp -P 9999 --payload-file probe.bin 10.0.0.5
```

Preset catalogue — all replies are parsed in the expanded view (Enter):

| Preset | Transport | Default port | Returns |
|---|---|---|---|
| `a2s` (+ `cs16/cs2/gmod/ark/dayz/valheim/…`) | UDP | 27015 | server, map, game, players, VAC, version |
| `a2s-player` / `a2s-rules` | UDP | 27015 | player list w/ frags+time / cvars (challenge flow) |
| `rust` | UDP | 28016 | Rust server via Steam A2S (name, map, players) |
| `fivem` / `fivem-players` / `fivem-dynamic` | TCP (HTTP) | 30120 | `/info.json` / player list / hostname+clients |
| `minecraft` (`mc`) | TCP | 25565 | version, protocol, players, MOTD, sample |
| `bedrock` (`mc-bedrock`) | UDP | 19132 | MOTD, version, players, world, gamemode |
| `samp` | UDP | 7777 | hostname, players, gamemode, map, password flag |

In the TUI every row shows the last reply size (`RESP 108B`); select a row and
press **Enter** (or `e`) to expand the response pane: parsed fields first (A2S
replies are decoded — server, map, players…), then printable strings and a full
hex/ASCII dump. In plain mode add `--show-response` for the same output inline:

```text
19:16:18Z UDP 217.156.22.85:27015  UP  32.3ms resp=108B seq=1
    server: CS.ELITECSTRIKE.RO # Old's Gamers SKINS|VIP
    map: de_dust2
    players: 30/32 (bots 0)
    0000  ff ff ff ff 49 30 43 53 …  |····I0CS.ELITECS|
```

A2S presets handle the challenge/response handshake automatically. Payloads
are capped at 4096 bytes, responses are captured up to 32768 bytes. With no
payload, TCP is a bare connect ping and UDP sends a small default probe.
Templated presets (FiveM `Host:`, Minecraft handshake, SA-MP header) are
filled in per-target, so subnets and multi-port scans work per endpoint.

| Flag | Default | Meaning |
|---|---|---|
| `-p, --protocol` | `icmp` | `icmp` / `tcp` / `udp`, comma-separated or repeated |
| `-P, --port` | `80` | Port(s) for TCP/UDP: `80`, `80,443`, `8000-8010` |
| `-i, --interval` | `1000` | ms between probe rounds |
| `-t, --timeout` | `1000` | probe timeout in ms |
| `-c, --count` | `0` | rounds to run; `0` = continuous (default) |
| `--payload` / `--payload-hex` / `--payload-file` | — | custom payload (text w/ `\xNN` escapes, hex, or file) |
| `--preset` | — | game query preset — see catalogue above; auto-selects protocol+port at CLI defaults |
| `--show-response` | off | plain mode: print parsed response + hexdump per reply |
| `--no-tui` | off | plain log output |

Targets accept IPs, hostnames, CIDR subnets (`10.0.0.0/24`, up to 4096 hosts),
comma-separated mixes, and multiple arguments. Subnets skip network/broadcast
except `/31` and `/32`.

## TUI keys

`q` / `Esc` / `Ctrl+C` quit · `space` pause · `r` reset stats · `↑↓` / `j` `k`
select · `Enter`/`e` expand response · `PgUp`/`PgDn` jump · `+`/`-` interval.

## Protocol semantics

- **ICMP** — native Echo Request over a raw socket; replies matched by id/seq.
- **TCP** — full connect; latency is connect time, refusal = `DOWN`.
- **UDP** — datagram exchange; a reply = `UP`, ICMP port-unreachable = `DOWN`,
  silence = `OPEN` (open\|filtered, standard UDP-scan semantics).

Per-target stats: sent/recv, loss %, last/min/avg/max, RFC 3550 jitter, and a
48-probe latency history sparkline.

## Development

```sh
cargo test            # unit tests: parsing, subnets, checksum, stats
cargo build --release
```

Layout: `src/target.rs` parsing/expansion · `src/payload.rs` payloads, A2S
parsing, hexdump · `src/probe.rs` ICMP/TCP/UDP engines · `src/stats.rs` rolling
stats + sparkline · `src/app.rs` scheduling/state · `src/ui.rs` Ratatui
dashboard · `src/main.rs` CLI, TUI & plain loops.
