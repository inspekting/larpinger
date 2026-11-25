//! LARPinger — multi-protocol ping (ICMP / TCP / UDP), subnets, custom ports,
//! continuous by default, live TUI with a plain log mode for pipes.

mod app;
mod payload;
mod probe;
mod stats;
mod target;
mod ui;

use std::io::{self, IsTerminal};
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::App;
use probe::Outcome;
use stats::fmt_ms;
use target::{build_endpoints, parse_ports, Protocol};

#[derive(Parser, Debug)]
#[command(
    name = "larpinger",
    version,
    about = "Multi-protocol ping — ICMP/TCP/UDP, subnets, custom ports, continuous live TUI"
)]
struct Cli {
    /// Targets: IP, hostname, CIDR subnet (10.0.0.0/24), or comma-separated mixes. Repeatable.
    #[arg(required = true, value_name = "TARGET")]
    targets: Vec<String>,

    /// Protocol(s): icmp, tcp, udp — repeat flag or comma-separate (-p tcp -p udp / -p tcp,udp)
    #[arg(short, long, value_delimiter = ',', default_values_t = vec!["icmp".to_string()])]
    protocol: Vec<String>,

    /// Port(s) for tcp/udp: 80 · 80,443 · 8000-8010 (ignored for icmp)
    #[arg(short = 'P', long, default_value = "80", value_name = "PORTS")]
    port: String,

    /// Milliseconds between probe rounds
    #[arg(short, long, default_value_t = 1000, value_name = "MS")]
    interval: u64,

    /// Probe timeout in milliseconds
    #[arg(short, long, default_value_t = 1000, value_name = "MS")]
    timeout: u64,

    /// Stop after N rounds (0 = continuous, the default)
    #[arg(short, long, default_value_t = 0, value_name = "N")]
    count: u64,

    /// Plain log output instead of the TUI (auto-enabled when stdout is not a TTY)
    #[arg(long)]
    no_tui: bool,

    /// Custom payload as text with escapes (\xFF \n \r \t \0 \\). For tcp/udp.
    #[arg(long, value_name = "TEXT", conflicts_with_all = ["payload_hex", "payload_file", "preset"])]
    payload: Option<String>,

    /// Custom payload as hex: "ff ff 54..." or ff,ff,54 (spaces/commas optional)
    #[arg(long, value_name = "HEX", conflicts_with_all = ["payload_file", "preset"])]
    payload_hex: Option<String>,

    /// Read payload bytes from a file (max 4096 bytes)
    #[arg(long, value_name = "FILE", conflicts_with = "preset")]
    payload_file: Option<std::path::PathBuf>,

    /// Built-in game query preset: a2s, a2s-player, a2s-rules, rust, fivem, fivem-players, fivem-dynamic, minecraft, bedrock, samp (+ aliases cs16/cs2/gmod/ark/dayz/valheim/mc). Auto-selects protocol+port when -p/-P are left at defaults
    #[arg(long, value_name = "NAME")]
    preset: Option<String>,

    /// In plain mode, print each response (parsed + hexdump) under its log line
    #[arg(long)]
    show_response: bool,
}

fn resolve_payload(cli: &Cli) -> Result<payload::PayloadCfg, String> {
    use payload::{decode_hex_payload, decode_text_payload, PayloadCfg, Preset};
    if let Some(name) = &cli.preset {
        let preset = Preset::parse(name)?;
        return Ok(PayloadCfg {
            bytes: preset.request(),
            preset: Some(preset),
            label: preset.label().into(),
        });
    }
    if let Some(hex) = &cli.payload_hex {
        let bytes = decode_hex_payload(hex)?;
        return Ok(PayloadCfg {
            label: format!("hex {}B", bytes.len()),
            bytes,
            preset: None,
        });
    }
    if let Some(text) = &cli.payload {
        let bytes = decode_text_payload(text)?;
        return Ok(PayloadCfg {
            label: format!("text {}B", bytes.len()),
            bytes,
            preset: None,
        });
    }
    if let Some(path) = &cli.payload_file {
        let bytes =
            std::fs::read(path).map_err(|e| format!("--payload-file {}: {e}", path.display()))?;
        if bytes.is_empty() || bytes.len() > payload::MAX_PAYLOAD {
            return Err(format!(
                "--payload-file: need 1..={} bytes",
                payload::MAX_PAYLOAD
            ));
        }
        return Ok(PayloadCfg {
            label: format!("file {} {}B", path.display(), bytes.len()),
            bytes,
            preset: None,
        });
    }
    Ok(PayloadCfg::default())
}

fn parse_protocols(raw: &[String]) -> Result<Vec<Protocol>, String> {
    let mut out: Vec<Protocol> = Vec::new();
    for r in raw {
        let p = Protocol::parse(r).map_err(|e| e.to_string())?;
        if !out.contains(&p) {
            out.push(p);
        }
    }
    if out.is_empty() {
        return Err("no protocol given".into());
    }
    Ok(out)
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("larpinger: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let payload = resolve_payload(&cli)?;
    // Preset convenience: with CLI defaults untouched, adopt the game's
    // transport + port (e.g. --preset fivem => tcp/30120, minecraft => tcp/25565).
    let mut protocol_raw = cli.protocol.clone();
    let mut port_raw = cli.port.clone();
    if let Some(preset) = payload.preset {
        let (is_tcp, def_port) = preset.transport();
        if protocol_raw == vec!["icmp".to_string()] {
            protocol_raw = vec![if is_tcp { "tcp".into() } else { "udp".into() }];
        }
        if port_raw == "80" {
            port_raw = def_port.to_string();
        }
    }
    let protocols = parse_protocols(&protocol_raw)?;
    let needs_ports = protocols.iter().any(|p| *p != Protocol::Icmp);
    let ports = if needs_ports {
        parse_ports(&port_raw).map_err(|e| e.to_string())?
    } else {
        vec![]
    };
    let endpoints = build_endpoints(&cli.targets, &protocols, &ports).map_err(|e| e.to_string())?;
    if endpoints.is_empty() {
        return Err("no endpoints to probe".into());
    }
    if cli.interval == 0 {
        return Err("--interval must be >= 1ms".into());
    }
    if payload.preset.is_some() || payload.label != "default" {
        // Payloads only make sense on stream/datagram probes.
        if protocols.iter().all(|p| *p == Protocol::Icmp) {
            eprintln!("larpinger: note: payload/preset ignored for icmp (use -p tcp or udp)");
        }
    }
    let show_response = cli.show_response;
    let mut app = App::new(
        endpoints,
        Duration::from_millis(cli.interval),
        Duration::from_millis(cli.timeout.max(1)),
        cli.count,
        payload,
    );

    if cli.no_tui || !io::stdout().is_terminal() {
        run_plain(&mut app, show_response)
    } else {
        run_tui(&mut app)
    }
}

// ---------------------------------------------------------------- plain mode

fn run_plain(app: &mut App, show_response: bool) -> Result<(), String> {
    println!(
        "LARPinger · {} targets · interval {}ms · timeout {}ms · payload {} · {}",
        app.endpoints.len(),
        app.interval.as_millis(),
        app.timeout.as_millis(),
        app.payload.label,
        if app.count == 0 {
            "continuous (Ctrl+C to stop)".into()
        } else {
            format!("count {}", app.count)
        },
    );
    app.launch_round();
    loop {
        let fresh = app.drain();
        for (idx, out) in &fresh {
            let ep = &app.endpoints[*idx];
            let dest = match ep.port {
                Some(p) => format!("{}:{}", ep.display, p),
                None => ep.display.clone(),
            };
            match out {
                Outcome::Ok {
                    status,
                    rtt,
                    response,
                } => {
                    let resp_note = match response {
                        Some(r) => format!(" resp={}B", r.len()),
                        None => String::new(),
                    };
                    println!(
                        "{} {:<4} {:<24} {:<5} {}{} seq={}",
                        ts(),
                        ep.protocol,
                        dest,
                        status.label(),
                        fmt_ms(Some(*rtt)),
                        resp_note,
                        app.stats[*idx].seq
                    );
                    if show_response {
                        if let Some(r) = response {
                            print_response(r);
                        }
                    }
                }
                Outcome::Fail(e) => {
                    println!(
                        "{} {:<4} {:<24} DOWN  {e} seq={}",
                        ts(),
                        ep.protocol,
                        dest,
                        app.stats[*idx].seq
                    )
                }
            }
        }
        if app.finished {
            break;
        }
        if app.due() {
            app.launch_round();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    print_summary(app);
    Ok(())
}

/// Expanded response for plain mode: parsed fields first, then hexdump.
fn print_response(data: &[u8]) {
    if let Some(fields) = payload::parse_response(data) {
        for (k, v) in &fields {
            println!("    {k}: {v}");
        }
    } else {
        let strings = payload::printable_strings(data, 6);
        if !strings.is_empty() {
            println!("    strings: {}", strings.join(" · "));
        }
    }
    for line in payload::hexdump(data, 256) {
        println!("    {line}");
    }
}

fn print_summary(app: &App) {
    println!(
        "\n--- summary · {} sent · {} recv · {:.1}% loss · avg {} ---",
        app.total_sent(),
        app.total_recv(),
        app.global_loss(),
        app.global_avg_ms()
            .map(|v| format!("{v:.1}ms"))
            .unwrap_or_else(|| "—".into()),
    );
    for (ep, st) in app.endpoints.iter().zip(&app.stats) {
        let dest = match ep.port {
            Some(p) => format!("{}:{}", ep.display, p),
            None => ep.display.clone(),
        };
        println!(
            "  {:<4} {:<24} {:<5} sent={} recv={} loss={:.0}% min={} avg={} max={}",
            ep.protocol,
            dest,
            st.status.label(),
            st.sent,
            st.recv,
            st.loss_pct(),
            fmt_ms(st.min),
            st.avg_ms()
                .map(|v| format!("{v:.1}ms"))
                .unwrap_or_else(|| "—".into()),
            fmt_ms(st.max),
        );
    }
}

fn ts() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}Z")
}

// ---------------------------------------------------------------- TUI mode

fn run_tui(app: &mut App) -> Result<(), String> {
    enable_raw_mode().map_err(|e| e.to_string())?;
    let mut stdout = io::stdout();
    stdout
        .execute(EnterAlternateScreen)
        .map_err(|e| e.to_string())?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;
    let result = tui_loop(app, &mut terminal);
    disable_raw_mode().ok();
    terminal.backend_mut().execute(LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    drop(terminal);
    if result.is_ok() {
        print_summary(app);
    }
    result
}

fn tui_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<(), String> {
    app.launch_round();
    let mut last_draw = Instant::now();
    loop {
        let _ = app.drain();
        if app.due() {
            app.launch_round();
        }
        // Redraw at ~30fps max, and immediately after probe activity.
        if last_draw.elapsed() >= Duration::from_millis(33) {
            terminal
                .draw(|f| ui::draw(f, app))
                .map_err(|e| e.to_string())?;
            last_draw = Instant::now();
        }
        if app.finished && app.pending == 0 {
            // One final frame, then leave on keypress or after a beat.
            terminal
                .draw(|f| ui::draw(f, app))
                .map_err(|e| e.to_string())?;
            let deadline = Instant::now() + Duration::from_secs(3600);
            while Instant::now() < deadline {
                if event::poll(Duration::from_millis(100)).map_err(|e| e.to_string())? {
                    break;
                }
            }
            return Ok(());
        }
        if event::poll(Duration::from_millis(5)).map_err(|e| e.to_string())? {
            if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
                if key.kind == KeyEventKind::Release {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return finish(terminal, app),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return finish(terminal, app)
                    }
                    KeyCode::Char(' ') => app.paused = !app.paused,
                    KeyCode::Enter | KeyCode::Char('e') => app.expanded = !app.expanded,
                    KeyCode::Char('r') => app.reset_stats(),
                    KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
                    KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
                    KeyCode::PageDown => app.move_selection(10),
                    KeyCode::PageUp => app.move_selection(-10),
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        app.interval =
                            (app.interval + Duration::from_millis(100)).min(Duration::from_secs(60))
                    }
                    KeyCode::Char('-') => {
                        app.interval = app
                            .interval
                            .saturating_sub(Duration::from_millis(100))
                            .max(Duration::from_millis(100))
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Leave the TUI and print the plain summary to the real terminal.
fn finish(
    _terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    _app: &mut App,
) -> Result<(), String> {
    Ok(())
}
