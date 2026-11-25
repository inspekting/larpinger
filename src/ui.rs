//! Ratatui dashboard: header, summary strip, live table, detail sparkline, key bar.
//!
//! Style: flat warm-dark, 1px borders, colour only for status. Tabular alignment,
//! no gradients, no chrome for its own sake.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::App;
use crate::payload;
use crate::stats::{fmt_ms, sparkline, Status};

const ACCENT: Color = Color::Cyan;
const OK: Color = Color::Green;
const BAD: Color = Color::Red;
const WARN: Color = Color::Yellow;
const DIM: Color = Color::DarkGray;
const FG: Color = Color::Gray;

fn status_style(s: Status) -> Style {
    match s {
        Status::Up => Style::default().fg(OK).add_modifier(Modifier::BOLD),
        Status::Open => Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        Status::Down => Style::default().fg(BAD).add_modifier(Modifier::BOLD),
        Status::Pending => Style::default().fg(DIM),
    }
}

fn dot(s: Status) -> &'static str {
    match s {
        Status::Up => "●",
        Status::Open => "◐",
        Status::Down => "●",
        Status::Pending => "○",
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let detail_h: u16 = if app.expanded {
        (area.height / 2).clamp(9, 22)
    } else {
        5
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(detail_h),
            Constraint::Length(1),
        ])
        .split(area);
    draw_header(f, app, rows[0]);
    draw_summary(f, app, rows[1]);
    draw_table(f, app, rows[2]);
    draw_detail(f, app, rows[3]);
    draw_footer(f, app, rows[4]);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let elapsed = app.started.elapsed().as_secs();
    let mode = if app.paused {
        Span::styled(
            " PAUSED ",
            Style::default()
                .fg(Color::Black)
                .bg(WARN)
                .add_modifier(Modifier::BOLD),
        )
    } else if app.finished {
        Span::styled(
            " DONE ",
            Style::default()
                .fg(Color::Black)
                .bg(OK)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " LIVE ",
            Style::default()
                .fg(Color::Black)
                .bg(OK)
                .add_modifier(Modifier::BOLD),
        )
    };
    let count_label = if app.count == 0 {
        "continuous".to_string()
    } else {
        format!("round {}/{}", app.round, app.count)
    };
    let line = Line::from(vec![
        Span::styled(
            " LARPinger ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("multi-protocol ping  ", Style::default().fg(DIM)),
        mode,
        Span::styled(
            format!(
                "   {} targets · {} · interval {}ms · timeout {}ms · {} · {:02}:{:02}",
                app.endpoints.len(),
                app.payload.label,
                app.interval.as_millis(),
                app.timeout.as_millis(),
                count_label,
                elapsed / 60,
                elapsed % 60,
            ),
            Style::default().fg(FG),
        ),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .title("");
    f.render_widget(Paragraph::new(line).block(block), area);
}

fn draw_summary(f: &mut Frame, app: &App, area: Rect) {
    let avg = app
        .global_avg_ms()
        .map(|v| format!("{v:.1}ms"))
        .unwrap_or_else(|| "—".into());
    let line = Line::from(vec![
        Span::styled("  ▲ up ", Style::default().fg(DIM)),
        Span::styled(
            format!("{}", app.up_count()),
            Style::default().fg(OK).add_modifier(Modifier::BOLD),
        ),
        Span::styled("   ▼ down ", Style::default().fg(DIM)),
        Span::styled(
            format!("{}", app.down_count()),
            Style::default().fg(BAD).add_modifier(Modifier::BOLD),
        ),
        Span::styled("   sent ", Style::default().fg(DIM)),
        Span::styled(format!("{}", app.total_sent()), Style::default().fg(FG)),
        Span::styled("  recv ", Style::default().fg(DIM)),
        Span::styled(format!("{}", app.total_recv()), Style::default().fg(FG)),
        Span::styled("  loss ", Style::default().fg(DIM)),
        Span::styled(
            format!("{:.1}%", app.global_loss()),
            Style::default().fg(if app.global_loss() > 20.0 {
                BAD
            } else if app.global_loss() > 0.0 {
                WARN
            } else {
                FG
            }),
        ),
        Span::styled("  avg ", Style::default().fg(DIM)),
        Span::styled(avg, Style::default().fg(FG)),
        Span::styled("   seq ", Style::default().fg(DIM)),
        Span::styled(format!("#{}", app.round), Style::default().fg(FG)),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM));
    f.render_widget(Paragraph::new(line).block(block), area);
}

fn draw_table(f: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(vec![
        Cell::from(""),
        Cell::from("TARGET"),
        Cell::from("PROTO"),
        Cell::from("PORT"),
        Cell::from("STATUS"),
        Cell::from("LAST"),
        Cell::from("MIN"),
        Cell::from("AVG"),
        Cell::from("MAX"),
        Cell::from("JIT"),
        Cell::from("LOSS"),
        Cell::from("RESP"),
        Cell::from("HISTORY"),
    ])
    .style(Style::default().fg(DIM).add_modifier(Modifier::BOLD))
    .height(1);

    // Viewport follows selection.
    let visible = area.height.saturating_sub(3) as usize;
    let start = if visible == 0 || app.selected < visible {
        0
    } else {
        app.selected + 1 - visible
    };

    let rows: Vec<Row> = app
        .endpoints
        .iter()
        .enumerate()
        .skip(start)
        .take(visible.max(1))
        .map(|(i, ep)| {
            let st = &app.stats[i];
            let sel = i == app.selected;
            let base = if sel {
                Style::default().bg(Color::Rgb(28, 32, 40)).fg(Color::White)
            } else {
                Style::default().fg(FG)
            };
            let jitter = if st.recv > 1 {
                format!("{:.1}ms", st.jitter_ms())
            } else {
                "—".into()
            };
            let loss = st.loss_pct();
            let loss_style = if loss >= 50.0 {
                Style::default().fg(BAD)
            } else if loss > 0.0 {
                Style::default().fg(WARN)
            } else {
                base
            };
            Row::new(vec![
                Cell::from(if sel { "▸" } else { " " }).style(Style::default().fg(ACCENT)),
                Cell::from(format!(
                    "{} {}",
                    ep.display,
                    if ep.display != ep.ip.to_string() {
                        format!("({})", ep.ip)
                    } else {
                        String::new()
                    }
                ))
                .style(base),
                Cell::from(ep.protocol.label()).style(base),
                Cell::from(ep.port.map(|p| p.to_string()).unwrap_or_else(|| "—".into()))
                    .style(base),
                Cell::from(format!("{} {}", dot(st.status), st.status.label()))
                    .style(status_style(st.status)),
                Cell::from(fmt_ms(st.last)).style(base),
                Cell::from(fmt_ms(st.min)).style(base),
                Cell::from(
                    st.avg_ms()
                        .map(|v| format!("{v:.1}ms"))
                        .unwrap_or_else(|| "—".into()),
                )
                .style(base),
                Cell::from(fmt_ms(st.max)).style(base),
                Cell::from(jitter).style(base),
                Cell::from(format!("{loss:.0}%")).style(loss_style),
                Cell::from(
                    st.last_response
                        .as_ref()
                        .map(|r| format!("{}B", r.len()))
                        .unwrap_or_else(|| "—".into()),
                )
                .style(Style::default().fg(ACCENT)),
            ])
            .style(base)
            .height(1)
        })
        .collect();

    let widths = [
        Constraint::Length(2),
        Constraint::Min(22),
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Length(20),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .title(Span::styled(" targets ", Style::default().fg(DIM)));
    f.render_widget(Table::new(rows, widths).header(header).block(block), area);
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let title = if app.expanded {
        " detail — response (enter/e to collapse) "
    } else {
        " detail (enter/e to expand response) "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if app.expanded { ACCENT } else { DIM }))
        .title(Span::styled(title, Style::default().fg(DIM)));
    let Some(ep) = app.endpoints.get(app.selected) else {
        f.render_widget(Paragraph::new("no targets").block(block), area);
        return;
    };
    let st = &app.stats[app.selected];
    let err = st.last_error.as_deref().unwrap_or("—");
    let spark = sparkline(&st.history, (area.width.saturating_sub(4)).min(72) as usize);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!(" {} ", ep.display),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "{} {}:{}  ",
                    ep.ip,
                    ep.protocol,
                    ep.port.map(|p| p.to_string()).unwrap_or_else(|| "-".into())
                ),
                Style::default().fg(DIM),
            ),
            Span::styled(
                format!("{} {}", dot(st.status), st.status.label()),
                status_style(st.status),
            ),
            Span::styled(
                format!(
                    "   sent {}  recv {}  loss {:.1}%   last {}  avg {}",
                    st.sent,
                    st.recv,
                    st.loss_pct(),
                    fmt_ms(st.last),
                    st.avg_ms()
                        .map(|v| format!("{v:.2}ms"))
                        .unwrap_or_else(|| "—".into()),
                ),
                Style::default().fg(FG),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!(" {spark}"), Style::default().fg(ACCENT)),
            Span::styled(
                format!("   err: {err}"),
                Style::default().fg(if st.last_error.is_some() { BAD } else { DIM }),
            ),
            Span::styled(
                match &st.last_response {
                    Some(r) => format!("   resp: {}B · {} replies", r.len(), st.responses),
                    None => "   resp: —".into(),
                },
                Style::default().fg(DIM),
            ),
        ]),
    ];
    if app.expanded {
        append_response(&mut lines, st.last_response.as_deref(), area);
    }
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// Parsed fields (A2S when recognised) + strings + hexdump, sized to the pane.
fn append_response(lines: &mut Vec<Line>, data: Option<&[u8]>, area: Rect) {
    let Some(data) = data else {
        lines.push(Line::from(Span::styled(
            " no response captured yet — payload probes (udp/tcp) record replies here",
            Style::default().fg(DIM),
        )));
        return;
    };
    // Rows left after the 2 header lines + border.
    let budget = (area.height as usize).saturating_sub(4);
    if let Some(fields) = payload::parse_response(data) {
        lines.push(Line::from(Span::styled(
            " ── parsed ─────────────────────────────",
            Style::default().fg(ACCENT),
        )));
        // Two fields per row keeps long values readable.
        for pair in fields.chunks(2) {
            let text = pair
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join("   ·   ");
            lines.push(Line::from(Span::styled(
                format!(" {text}"),
                Style::default().fg(FG),
            )));
        }
    } else {
        let strings = payload::printable_strings(data, 4);
        if !strings.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(" strings: ", Style::default().fg(ACCENT)),
                Span::styled(strings.join(" · "), Style::default().fg(FG)),
            ]));
        }
    }
    lines.push(Line::from(Span::styled(
        format!(" ── hex {}B ────────────────────────────", data.len()),
        Style::default().fg(ACCENT),
    )));
    let used = lines.len() - 2; // header rows already present
    let dump_rows = budget.saturating_sub(used);
    for row in payload::hexdump(data, dump_rows * 16) {
        lines.push(Line::from(Span::styled(
            format!(" {row}"),
            Style::default().fg(Color::White),
        )));
    }
}

fn draw_footer(f: &mut Frame, _app: &App, area: Rect) {
    let keys = Line::from(vec![
        Span::styled(
            " q",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" quit  ", Style::default().fg(DIM)),
        Span::styled(
            "space",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" pause  ", Style::default().fg(DIM)),
        Span::styled(
            "r",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" reset  ", Style::default().fg(DIM)),
        Span::styled(
            "↑↓/jk",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" select  ", Style::default().fg(DIM)),
        Span::styled(
            "enter",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" response  ", Style::default().fg(DIM)),
        Span::styled(
            "+/-",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" interval", Style::default().fg(DIM)),
    ]);
    f.render_widget(Paragraph::new(keys), area);
}
