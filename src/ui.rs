use crate::config::AppConfig;
use crate::metrics::{MetricsCollector, MetricsSnapshot};
use crate::processes::{ProcessFilter, ProcessInfo, SortColumn};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Sparkline, Table, TableState,
};
use ratatui::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Cyan,
    Green,
    Amber,
    Magenta,
}

impl Theme {
    pub fn next(self) -> Self {
        match self {
            Theme::Cyan => Theme::Green,
            Theme::Green => Theme::Amber,
            Theme::Amber => Theme::Magenta,
            Theme::Magenta => Theme::Cyan,
        }
    }

    pub fn primary(self) -> Color {
        match self {
            Theme::Cyan => Color::Cyan,
            Theme::Green => Color::Green,
            Theme::Amber => Color::Yellow,
            Theme::Magenta => Color::Magenta,
        }
    }

    pub fn secondary(self) -> Color {
        match self {
            Theme::Cyan => Color::LightBlue,
            Theme::Green => Color::LightGreen,
            Theme::Amber => Color::LightRed,
            Theme::Magenta => Color::LightMagenta,
        }
    }

    pub fn accent(self) -> Color {
        match self {
            Theme::Cyan => Color::LightCyan,
            Theme::Green => Color::Cyan,
            Theme::Amber => Color::Cyan,
            Theme::Magenta => Color::Cyan,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Combined,
    Processes,
    Hardware,
}

impl ViewMode {
    pub fn next(self) -> Self {
        match self {
            ViewMode::Combined => ViewMode::Processes,
            ViewMode::Processes => ViewMode::Hardware,
            ViewMode::Hardware => ViewMode::Combined,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Combined => "Split",
            ViewMode::Processes => "Processes",
            ViewMode::Hardware => "Hardware",
        }
    }
}

pub const SETTINGS_COUNT: usize = 7;

pub struct UiState {
    pub view_mode: ViewMode,
    pub sort_col: SortColumn,
    pub sort_desc: bool,
    pub filter: ProcessFilter,
    pub search_query: String,
    pub search_mode: bool,
    pub table_state: TableState,
    pub confirm_kill: Option<(u32, String)>,
    pub status_msg: Option<String>,
    pub show_settings: bool,
    pub settings_selected: usize,
}

impl Default for UiState {
    fn default() -> Self {
        let mut ts = TableState::default();
        ts.select(Some(0));
        Self {
            view_mode: ViewMode::Combined,
            sort_col: SortColumn::Cpu,
            sort_desc: true,
            filter: ProcessFilter::All,
            search_query: String::new(),
            search_mode: false,
            table_state: ts,
            confirm_kill: None,
            status_msg: None,
            show_settings: false,
            settings_selected: 0,
        }
    }
}

pub fn render(
    f: &mut Frame,
    collector: &MetricsCollector,
    snap: &MetricsSnapshot,
    procs: &[&ProcessInfo],
    total_procs: usize,
    total_apps: usize,
    interval_ms: u64,
    paused: bool,
    theme: Theme,
    cfg: &AppConfig,
    ui: &mut UiState,
) {
    let area = f.area();

    match ui.view_mode {
        ViewMode::Combined => {
            if area.height >= 34 {
                let root = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // Header
                        Constraint::Length(8), // CPU & GPU/ANE
                        Constraint::Length(5), // Compact Power/Temp & Memory
                        Constraint::Min(10),   // Process Explorer
                        Constraint::Length(1), // Footer
                    ])
                    .split(area);

                render_header(f, root[0], collector, snap, interval_ms, paused, theme, ui.view_mode);
                render_top_row(f, root[1], collector, snap, theme);
                render_compact_power_mem(f, root[2], collector, snap, theme);
                render_process_table(f, root[3], procs, total_procs, total_apps, theme, ui);
                render_footer(f, root[4], theme, ui);
            } else {
                let root = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // Header
                        Constraint::Length(6), // Compact CPU/GPU/Power/RAM gauges
                        Constraint::Min(6),    // Process Explorer
                        Constraint::Length(1), // Footer
                    ])
                    .split(area);

                render_header(f, root[0], collector, snap, interval_ms, paused, theme, ui.view_mode);
                render_compact_split_gauges(f, root[1], snap, theme);
                render_process_table(f, root[2], procs, total_procs, total_apps, theme, ui);
                render_footer(f, root[3], theme, ui);
            }
        }
        ViewMode::Processes => {
            let root = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Header
                    Constraint::Length(1), // Single-line HW strip
                    Constraint::Min(10),   // Full Process Explorer
                    Constraint::Length(1), // Footer
                ])
                .split(area);

            render_header(f, root[0], collector, snap, interval_ms, paused, theme, ui.view_mode);
            render_mini_summary(f, root[1], snap, theme);
            render_process_table(f, root[2], procs, total_procs, total_apps, theme, ui);
            render_footer(f, root[3], theme, ui);
        }
        ViewMode::Hardware => {
            let root = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Header
                    Constraint::Length(9), // CPU & GPU/ANE
                    Constraint::Min(10),   // Per-core & Power/Thermals
                    Constraint::Length(5), // Memory & Swap
                    Constraint::Length(1), // Footer
                ])
                .split(area);

            render_header(f, root[0], collector, snap, interval_ms, paused, theme, ui.view_mode);
            render_top_row(f, root[1], collector, snap, theme);
            render_mid_row(f, root[2], collector, snap, theme);
            render_memory(f, root[3], snap, theme);
            render_footer(f, root[4], theme, ui);
        }
    }

    if ui.show_settings {
        let popup = centered_rect(area, 92, 18);
        f.render_widget(Clear, popup);
        render_settings(f, popup, cfg, theme, ui);
    }
}

fn centered_rect(area: Rect, max_w: u16, max_h: u16) -> Rect {
    let w = area.width.saturating_sub(4).min(max_w).max(area.width.min(40));
    let h = area.height.saturating_sub(2).min(max_h).max(area.height.min(12));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn render_header(
    f: &mut Frame,
    area: Rect,
    collector: &MetricsCollector,
    snap: &MetricsSnapshot,
    interval_ms: u64,
    paused: bool,
    theme: Theme,
    view_mode: ViewMode,
) {
    let chip = &collector.chip;
    let ram_gb = chip.mem_total_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    let thermal_color = match snap.thermal_pressure.as_str() {
        "Nominal" => Color::Green,
        "Moderate" => Color::Yellow,
        _ => Color::Red,
    };

    let mut spans = vec![
        Span::styled(
            " MTOP ",
            Style::default()
                .fg(Color::Black)
                .bg(theme.primary())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            &chip.chip_name,
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                " ({}P+{}E • {}G • {:.0}GB) macOS {}",
                chip.p_cores, chip.e_cores, chip.gpu_cores, ram_gb, chip.os_version
            ),
            Style::default().fg(Color::Gray),
        ),
        Span::raw("  Thermal: "),
        Span::styled(
            &snap.thermal_pressure,
            Style::default().fg(thermal_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}ms  ", interval_ms),
            Style::default().fg(Color::DarkGray),
        ),
    ];

    for tab in [ViewMode::Combined, ViewMode::Processes, ViewMode::Hardware] {
        if tab == view_mode {
            spans.push(Span::styled(
                format!(" {} ", tab.label()),
                Style::default()
                    .fg(Color::Black)
                    .bg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {} ", tab.label()),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }

    if paused {
        spans.push(Span::styled(
            "  [PAUSED]",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    }

    let header = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.primary())),
    );
    f.render_widget(header, area);
}

fn render_mini_summary(f: &mut Frame, area: Rect, snap: &MetricsSnapshot, theme: Theme) {
    let gb = 1024.0 * 1024.0 * 1024.0;
    let sys_w = snap
        .system_power_w
        .map(|w| format!("{:.1}W", w))
        .unwrap_or_else(|| "N/A".to_string());
    let gpu_w = snap
        .gpu_power_w
        .map(|w| format!("{:.2}W", w))
        .unwrap_or_else(|| "N/A".to_string());
    let temp = snap
        .soc_temp_avg_c
        .map(|c| format!("{:.1}°C", c))
        .unwrap_or_else(|| "N/A".to_string());

    let line = Line::from(vec![
        Span::styled(" CPU: ", Style::default().fg(theme.primary()).add_modifier(Modifier::BOLD)),
        Span::raw(format!(
            "{:.1}% (E {:.0}%@{:.0}MHz | P {:.0}%@{:.0}MHz)",
            snap.cpu_total_util_pct,
            snap.ecpu_util_pct,
            snap.ecpu_freq_mhz,
            snap.pcpu_util_pct,
            snap.pcpu_freq_mhz
        )),
        Span::styled("   GPU: ", Style::default().fg(theme.accent()).add_modifier(Modifier::BOLD)),
        Span::raw(format!(
            "{:.1}%@{:.0}MHz ({})",
            snap.gpu_util_pct, snap.gpu_freq_mhz, gpu_w
        )),
        Span::styled("   Pwr: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::raw(format!("{} ({})", sys_w, temp)),
        Span::styled("   RAM: ", Style::default().fg(theme.secondary()).add_modifier(Modifier::BOLD)),
        Span::raw(format!(
            "{:.1}/{:.0}GB",
            snap.mem_used_bytes as f64 / gb,
            snap.mem_total_bytes as f64 / gb
        )),
    ]);

    let bar = Paragraph::new(line);
    f.render_widget(bar, area);
}

fn render_compact_split_gauges(f: &mut Frame, area: Rect, snap: &MetricsSnapshot, theme: Theme) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Left: E-CPU, P-CPU, GPU gauges
    let left_block = Block::default()
        .title(Span::styled(
            format!(" Compute ({:.1}% CPU) ", snap.cpu_total_util_pct),
            Style::default().fg(theme.primary()).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let left_inner = left_block.inner(cols[0]);
    f.render_widget(left_block, cols[0]);

    let left_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(left_inner);

    f.render_widget(
        Gauge::default()
            .gauge_style(Style::default().fg(theme.primary()).bg(Color::DarkGray))
            .ratio((snap.ecpu_util_pct / 100.0).clamp(0.0, 1.0))
            .label(format!("E-CPU {:4.1}% @ {:4.0} MHz", snap.ecpu_util_pct, snap.ecpu_freq_mhz)),
        left_rows[0],
    );
    f.render_widget(
        Gauge::default()
            .gauge_style(Style::default().fg(theme.secondary()).bg(Color::DarkGray))
            .ratio((snap.pcpu_util_pct / 100.0).clamp(0.0, 1.0))
            .label(format!("P-CPU {:4.1}% @ {:4.0} MHz", snap.pcpu_util_pct, snap.pcpu_freq_mhz)),
        left_rows[1],
    );
    let gpu_w = snap
        .gpu_power_w
        .map(|w| format!("{:.2}W", w))
        .unwrap_or_else(|| "N/A".to_string());
    f.render_widget(
        Gauge::default()
            .gauge_style(Style::default().fg(theme.accent()).bg(Color::DarkGray))
            .ratio((snap.gpu_util_pct / 100.0).clamp(0.0, 1.0))
            .label(format!("GPU   {:4.1}% @ {:4.0} MHz | {}", snap.gpu_util_pct, snap.gpu_freq_mhz, gpu_w)),
        left_rows[2],
    );
    let ane = snap
        .ane_power_w
        .map(|w| format!("ANE: {:.2}W", w))
        .unwrap_or_else(|| "ANE: N/A (sudo)".to_string());
    f.render_widget(
        Paragraph::new(Span::styled(ane, Style::default().fg(Color::DarkGray))),
        left_rows[3],
    );

    // Right: Memory gauge + Power & Temps
    let right_block = Block::default()
        .title(Span::styled(
            " Power, Thermals & Memory ",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let right_inner = right_block.inner(cols[1]);
    f.render_widget(right_block, cols[1]);

    let right_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(right_inner);

    let gb = 1024.0 * 1024.0 * 1024.0;
    let mem_ratio = if snap.mem_total_bytes > 0 {
        (snap.mem_used_bytes as f64 / snap.mem_total_bytes as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    f.render_widget(
        Gauge::default()
            .gauge_style(Style::default().fg(theme.primary()).bg(Color::DarkGray))
            .ratio(mem_ratio)
            .label(format!(
                "RAM: {:.1}/{:.0} GB ({:.0}%)",
                snap.mem_used_bytes as f64 / gb,
                snap.mem_total_bytes as f64 / gb,
                mem_ratio * 100.0
            )),
        right_rows[0],
    );

    let fmt_w = |o: Option<f64>| o.map(|v| format!("{:.2}W", v)).unwrap_or_else(|| "N/A".to_string());
    let fmt_c = |o: Option<f64>| o.map(|v| format!("{:.1}°C", v)).unwrap_or_else(|| "N/A".to_string());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Sys: ", Style::default().fg(Color::Gray)),
            Span::styled(fmt_w(snap.system_power_w), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("  CPU: ", Style::default().fg(Color::Gray)),
            Span::styled(fmt_w(snap.cpu_power_w), Style::default().fg(theme.primary())),
            Span::styled("  GPU: ", Style::default().fg(Color::Gray)),
            Span::styled(fmt_w(snap.gpu_power_w), Style::default().fg(theme.accent())),
        ])),
        right_rows[1],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("SoC Temp: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{} (max {})", fmt_c(snap.soc_temp_avg_c), fmt_c(snap.soc_temp_max_c)),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("  GPU: ", Style::default().fg(Color::Gray)),
            Span::styled(fmt_c(snap.gpu_temp_avg_c), Style::default().fg(Color::Yellow)),
        ])),
        right_rows[2],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Swap: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!(
                    "{:.2}/{:.1}G  (Wired {:.1}G, Comp {:.1}G)",
                    snap.swap_used_bytes as f64 / gb,
                    snap.swap_total_bytes as f64 / gb,
                    snap.mem_wired_bytes as f64 / gb,
                    snap.mem_compressed_bytes as f64 / gb,
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ])),
        right_rows[3],
    );
}

fn render_top_row(
    f: &mut Frame,
    area: Rect,
    collector: &MetricsCollector,
    snap: &MetricsSnapshot,
    theme: Theme,
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Left: CPU E-Cluster & P-Cluster
    let cpu_block = Block::default()
        .title(Span::styled(
            format!(" CPU ({:.1}% total) ", snap.cpu_total_util_pct),
            Style::default().fg(theme.primary()).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let cpu_inner = cpu_block.inner(cols[0]);
    f.render_widget(cpu_block, cols[0]);

    let cpu_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(cpu_inner);

    let e_gauge = Gauge::default()
        .gauge_style(Style::default().fg(theme.primary()).bg(Color::DarkGray))
        .ratio((snap.ecpu_util_pct / 100.0).clamp(0.0, 1.0))
        .label(format!(
            "E-CPU: {:5.1}% @ {:4.0} MHz",
            snap.ecpu_util_pct, snap.ecpu_freq_mhz
        ));
    f.render_widget(e_gauge, cpu_rows[0]);

    let e_hist: Vec<u64> = collector.ecpu_hist.iter().copied().collect();
    let e_spark = Sparkline::default()
        .data(&e_hist)
        .max(100)
        .style(Style::default().fg(theme.primary()));
    f.render_widget(e_spark, cpu_rows[1]);

    let p_gauge = Gauge::default()
        .gauge_style(Style::default().fg(theme.secondary()).bg(Color::DarkGray))
        .ratio((snap.pcpu_util_pct / 100.0).clamp(0.0, 1.0))
        .label(format!(
            "P-CPU: {:5.1}% @ {:4.0} MHz",
            snap.pcpu_util_pct, snap.pcpu_freq_mhz
        ));
    f.render_widget(p_gauge, cpu_rows[2]);

    let p_hist: Vec<u64> = collector.pcpu_hist.iter().copied().collect();
    let p_spark = Sparkline::default()
        .data(&p_hist)
        .max(100)
        .style(Style::default().fg(theme.secondary()));
    f.render_widget(p_spark, cpu_rows[3]);

    // Right: GPU & ANE
    let gpu_block = Block::default()
        .title(Span::styled(
            " GPU & ANE ",
            Style::default().fg(theme.accent()).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let gpu_inner = gpu_block.inner(cols[1]);
    f.render_widget(gpu_block, cols[1]);

    let gpu_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(2),
            Constraint::Length(1),
        ])
        .split(gpu_inner);

    let gpu_pwr_str = snap
        .gpu_power_w
        .map(|w| format!("{:.2} W", w))
        .unwrap_or_else(|| "N/A".to_string());
    let gpu_gauge = Gauge::default()
        .gauge_style(Style::default().fg(theme.accent()).bg(Color::DarkGray))
        .ratio((snap.gpu_util_pct / 100.0).clamp(0.0, 1.0))
        .label(format!(
            "GPU: {:5.1}% @ {:4.0} MHz  |  {}",
            snap.gpu_util_pct, snap.gpu_freq_mhz, gpu_pwr_str
        ));
    f.render_widget(gpu_gauge, gpu_rows[0]);

    let g_hist: Vec<u64> = collector.gpu_hist.iter().copied().collect();
    let g_spark = Sparkline::default()
        .data(&g_hist)
        .max(100)
        .style(Style::default().fg(theme.accent()));
    f.render_widget(g_spark, gpu_rows[1]);

    let ane_line = match snap.ane_power_w {
        Some(w) => Line::from(vec![
            Span::styled("ANE Power: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{:.2} W", w),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
        ]),
        None => Line::from(vec![
            Span::styled("ANE Power: ", Style::default().fg(Color::Gray)),
            Span::styled(
                if collector.has_powermetrics() {
                    "0.00 W (idle)"
                } else {
                    "N/A (run with sudo for ANE/CPU power)"
                },
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    };
    f.render_widget(Paragraph::new(ane_line), gpu_rows[2]);
}

fn render_compact_power_mem(
    f: &mut Frame,
    area: Rect,
    collector: &MetricsCollector,
    snap: &MetricsSnapshot,
    theme: Theme,
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let fmt_w = |opt: Option<f64>| -> String {
        opt.map(|v| format!("{:.2}W", v)).unwrap_or_else(|| "N/A".to_string())
    };
    let fmt_c = |opt: Option<f64>| -> String {
        opt.map(|v| format!("{:.1}°C", v)).unwrap_or_else(|| "N/A".to_string())
    };

    // Left: Power & Thermals
    let pwr_block = Block::default()
        .title(Span::styled(
            " Power & Thermals ",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let pwr_inner = pwr_block.inner(cols[0]);
    f.render_widget(pwr_block, cols[0]);

    let pwr_lines = vec![
        Line::from(vec![
            Span::styled("Sys: ", Style::default().fg(Color::Gray)),
            Span::styled(
                fmt_w(snap.system_power_w),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" (Avg {:.1}W / Peak {:.1}W)", collector.avg_system_power_w, collector.peak_system_power_w),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            Span::styled("GPU: ", Style::default().fg(Color::Gray)),
            Span::styled(fmt_w(snap.gpu_power_w), Style::default().fg(theme.accent())),
            Span::styled("  CPU: ", Style::default().fg(Color::Gray)),
            Span::styled(fmt_w(snap.cpu_power_w), Style::default().fg(theme.primary())),
            Span::styled("  ANE: ", Style::default().fg(Color::Gray)),
            Span::styled(fmt_w(snap.ane_power_w), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("SoC Temp: ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{} (max {})", fmt_c(snap.soc_temp_avg_c), fmt_c(snap.soc_temp_max_c)),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("  GPU: ", Style::default().fg(Color::Gray)),
            Span::styled(fmt_c(snap.gpu_temp_avg_c), Style::default().fg(Color::Yellow)),
        ]),
    ];
    f.render_widget(Paragraph::new(pwr_lines), pwr_inner);

    // Right: Memory & Swap
    render_memory(f, cols[1], snap, theme);
}

fn render_mid_row(
    f: &mut Frame,
    area: Rect,
    collector: &MetricsCollector,
    snap: &MetricsSnapshot,
    theme: Theme,
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Left: Per-Core Utilization
    let cores_block = Block::default()
        .title(Span::styled(
            " Per-Core Utilization & Frequency ",
            Style::default().fg(theme.primary()).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let cores_inner = cores_block.inner(cols[0]);
    f.render_widget(cores_block, cols[0]);

    let mut lines = Vec::new();
    let bar_w = cores_inner.width.saturating_sub(24).clamp(6, 24) as usize;
    for core in snap.ecpu_cores.iter().chain(snap.pcpu_cores.iter()) {
        let is_p = core.name.starts_with('P');
        let color = if is_p { theme.secondary() } else { theme.primary() };
        let filled = ((core.util_pct / 100.0) * bar_w as f64).round() as usize;
        let filled = filled.min(bar_w);
        let empty = bar_w - filled;
        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<3} ", core.name),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(bar, Style::default().fg(color)),
            Span::styled(
                format!(" {:5.1}% {:4.0} MHz", core.util_pct, core.freq_mhz),
                Style::default().fg(Color::White),
            ),
        ]));
    }
    f.render_widget(Paragraph::new(lines), cores_inner);

    // Right: Power & Thermals
    let pwr_block = Block::default()
        .title(Span::styled(
            " Power & Thermals ",
            Style::default().fg(theme.primary()).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let pwr_inner = pwr_block.inner(cols[1]);
    f.render_widget(pwr_block, cols[1]);

    let pwr_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(pwr_inner);

    let fmt_w = |opt: Option<f64>| -> String {
        opt.map(|v| format!("{:.2} W", v)).unwrap_or_else(|| "N/A".to_string())
    };
    let fmt_c = |opt: Option<f64>| -> String {
        opt.map(|v| format!("{:.1}°C", v)).unwrap_or_else(|| "N/A".to_string())
    };

    let pwr_lines = vec![
        Line::from(vec![
            Span::styled("System Power: ", Style::default().fg(Color::Gray)),
            Span::styled(
                fmt_w(snap.system_power_w),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "   (Avg: {:.2} W  Peak: {:.2} W)",
                    collector.avg_system_power_w, collector.peak_system_power_w
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            Span::styled("GPU Power:    ", Style::default().fg(Color::Gray)),
            Span::styled(fmt_w(snap.gpu_power_w), Style::default().fg(theme.accent())),
            Span::styled("   CPU Power: ", Style::default().fg(Color::Gray)),
            Span::styled(fmt_w(snap.cpu_power_w), Style::default().fg(theme.primary())),
        ]),
        Line::from(vec![
            Span::styled("DRAM Power:   ", Style::default().fg(Color::Gray)),
            Span::styled(fmt_w(snap.dram_power_w), Style::default().fg(Color::White)),
            Span::styled("   ANE Power: ", Style::default().fg(Color::Gray)),
            Span::styled(fmt_w(snap.ane_power_w), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("SoC Temp:     ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{} (max {})", fmt_c(snap.soc_temp_avg_c), fmt_c(snap.soc_temp_max_c)),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("   GPU Temp: ", Style::default().fg(Color::Gray)),
            Span::styled(fmt_c(snap.gpu_temp_avg_c), Style::default().fg(Color::Yellow)),
        ]),
    ];
    f.render_widget(Paragraph::new(pwr_lines), pwr_rows[0]);

    let p_hist: Vec<u64> = collector.power_hist.iter().copied().collect();
    let max_p = (collector.peak_system_power_w * 10.0).ceil().max(100.0) as u64;
    let p_spark = Sparkline::default()
        .data(&p_hist)
        .max(max_p)
        .style(Style::default().fg(Color::Yellow));
    f.render_widget(p_spark, pwr_rows[1]);
}

fn render_memory(f: &mut Frame, area: Rect, snap: &MetricsSnapshot, theme: Theme) {
    let block = Block::default()
        .title(Span::styled(
            " Memory & Swap ",
            Style::default().fg(theme.primary()).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    let gb = 1024.0 * 1024.0 * 1024.0;
    let total_gb = snap.mem_total_bytes as f64 / gb;
    let used_gb = snap.mem_used_bytes as f64 / gb;
    let ratio = if snap.mem_total_bytes > 0 {
        (snap.mem_used_bytes as f64 / snap.mem_total_bytes as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let mem_gauge = Gauge::default()
        .gauge_style(Style::default().fg(theme.primary()).bg(Color::DarkGray))
        .ratio(ratio)
        .label(format!(
            "RAM: {:.2} / {:.1} GB ({:.1}%)",
            used_gb,
            total_gb,
            ratio * 100.0
        ));
    f.render_widget(mem_gauge, rows[0]);

    let detail = Line::from(vec![
        Span::styled("Active: ", Style::default().fg(Color::Gray)),
        Span::styled(
            format!("{:.1}G", snap.mem_active_bytes as f64 / gb),
            Style::default().fg(Color::White),
        ),
        Span::styled(" • Wired: ", Style::default().fg(Color::Gray)),
        Span::styled(
            format!("{:.1}G", snap.mem_wired_bytes as f64 / gb),
            Style::default().fg(Color::White),
        ),
        Span::styled(" • Comp: ", Style::default().fg(Color::Gray)),
        Span::styled(
            format!("{:.1}G", snap.mem_compressed_bytes as f64 / gb),
            Style::default().fg(Color::White),
        ),
        Span::styled(" • Swap: ", Style::default().fg(Color::Gray)),
        Span::styled(
            format!(
                "{:.2}/{:.1}G",
                snap.swap_used_bytes as f64 / gb,
                snap.swap_total_bytes as f64 / gb
            ),
            Style::default().fg(Color::White),
        ),
    ]);
    f.render_widget(Paragraph::new(detail), rows[1]);
}

fn render_process_table(
    f: &mut Frame,
    area: Rect,
    procs: &[&ProcessInfo],
    total_procs: usize,
    total_apps: usize,
    theme: Theme,
    ui: &mut UiState,
) {
    let arrow = if ui.sort_desc { "▼" } else { "▲" };
    let search_badge = if ui.search_mode {
        format!("  [Search: {}_]", ui.search_query)
    } else if !ui.search_query.is_empty() {
        format!("  [Search: {}]", ui.search_query)
    } else {
        String::new()
    };

    let title = Line::from(vec![
        Span::styled(
            format!(
                " Process Explorer ({}/{} shown, {} apps) ",
                procs.len(),
                total_procs,
                total_apps
            ),
            Style::default().fg(theme.primary()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" [Filter: {}] ", ui.filter.label()),
            Style::default().fg(theme.accent()),
        ),
        Span::styled(
            format!(" [Sort: {} {}] ", ui.sort_col.label(), arrow),
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(search_badge, Style::default().fg(Color::Green)),
    ]);

    let col_header = |col: SortColumn, label: &str, right_align: bool| -> Cell<'static> {
        let active = ui.sort_col == col;
        let text = if active {
            if right_align {
                format!("{arrow} {label}")
            } else {
                format!("{label} {arrow}")
            }
        } else {
            label.to_string()
        };
        let style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(theme.primary())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme.primary())
                .add_modifier(Modifier::BOLD)
        };
        let line = if right_align {
            Line::from(text).alignment(ratatui::layout::Alignment::Right)
        } else {
            Line::from(text)
        };
        Cell::from(line).style(style)
    };

    let header = Row::new([
        col_header(SortColumn::Pid, "PID", true),
        col_header(SortColumn::App, "App / Process", false),
        col_header(SortColumn::Cpu, "CPU%", true),
        col_header(SortColumn::Memory, "Memory", true),
        col_header(SortColumn::NetDown, "Net ↓/s", true),
        col_header(SortColumn::NetUp, "Net ↑/s", true),
        col_header(SortColumn::Disk, "Disk/s", true),
        col_header(SortColumn::Threads, "Thr", true),
    ])
    .height(1)
    .style(Style::default().bg(Color::Rgb(24, 28, 36)));

    let rows: Vec<Row> = procs
        .iter()
        .map(|p| {
            let cpu_color = if p.cpu_pct >= 80.0 {
                Color::Red
            } else if p.cpu_pct >= 20.0 {
                Color::Yellow
            } else if p.cpu_pct >= 2.0 {
                theme.primary()
            } else {
                Color::Gray
            };

            let name_line = if p.is_app {
                let app_label = p.app_name.as_deref().unwrap_or(&p.name);
                if app_label != p.name {
                    Line::from(vec![
                        Span::styled("[App] ", Style::default().fg(theme.accent()).add_modifier(Modifier::BOLD)),
                        Span::styled(app_label.to_string(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                        Span::styled(format!(" ({})", p.name), Style::default().fg(Color::DarkGray)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("[App] ", Style::default().fg(theme.accent()).add_modifier(Modifier::BOLD)),
                        Span::styled(app_label.to_string(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                    ])
                }
            } else if let Some(ref parent_app) = p.app_name {
                Line::from(vec![
                    Span::styled(p.name.clone(), Style::default().fg(Color::Gray)),
                    Span::styled(format!(" [{parent_app}]"), Style::default().fg(Color::DarkGray)),
                ])
            } else {
                Line::from(Span::styled(p.name.clone(), Style::default().fg(Color::Gray)))
            };

            let net_rx_style = if p.net_rx_bps > 1024.0 {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let net_tx_style = if p.net_tx_bps > 1024.0 {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let disk_style = if p.disk_bps() > 1024.0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            Row::new([
                Cell::from(Line::from(p.pid.to_string()).alignment(ratatui::layout::Alignment::Right))
                    .style(Style::default().fg(Color::DarkGray)),
                Cell::from(name_line),
                Cell::from(
                    Line::from(format!("{:5.1}%", p.cpu_pct))
                        .alignment(ratatui::layout::Alignment::Right),
                )
                .style(Style::default().fg(cpu_color)),
                Cell::from(
                    Line::from(fmt_bytes(p.mem_bytes))
                        .alignment(ratatui::layout::Alignment::Right),
                )
                .style(Style::default().fg(Color::White)),
                Cell::from(
                    Line::from(fmt_rate(p.net_rx_bps))
                        .alignment(ratatui::layout::Alignment::Right),
                )
                .style(net_rx_style),
                Cell::from(
                    Line::from(fmt_rate(p.net_tx_bps))
                        .alignment(ratatui::layout::Alignment::Right),
                )
                .style(net_tx_style),
                Cell::from(
                    Line::from(fmt_rate(p.disk_bps()))
                        .alignment(ratatui::layout::Alignment::Right),
                )
                .style(disk_style),
                Cell::from(
                    Line::from(p.threads.to_string())
                        .alignment(ratatui::layout::Alignment::Right),
                )
                .style(Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(7),
        Constraint::Min(24),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(5),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .row_highlight_style(
            Style::default()
                .bg(Color::Rgb(42, 52, 68))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    f.render_stateful_widget(table, area, &mut ui.table_state);
}

fn render_footer(f: &mut Frame, area: Rect, theme: Theme, ui: &UiState) {
    if let Some((pid, ref name)) = ui.confirm_kill {
        let prompt = Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" Terminate '{name}' (PID {pid})? "),
                Style::default().fg(Color::Black).bg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Press ", Style::default().fg(Color::White)),
            Span::styled("y", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled(" to confirm, ", Style::default().fg(Color::White)),
            Span::styled("n/Esc", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(" to cancel.", Style::default().fg(Color::White)),
        ]));
        f.render_widget(prompt, area);
        return;
    }

    if ui.search_mode {
        let bar = Paragraph::new(Line::from(vec![
            Span::styled(
                " Search: ",
                Style::default().fg(Color::Black).bg(theme.primary()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {}_", ui.search_query), Style::default().fg(Color::White)),
            Span::styled(
                "   (Enter: keep filter • Esc: clear • Backspace: delete)",
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        f.render_widget(bar, area);
        return;
    }

    if let Some(ref msg) = ui.status_msg {
        let bar = Paragraph::new(Line::from(vec![
            Span::styled(format!(" {msg} "), Style::default().fg(Color::Yellow)),
        ]));
        f.render_widget(bar, area);
        return;
    }

    let key = |k: &str| {
        Span::styled(
            k.to_string(),
            Style::default()
                .fg(theme.primary())
                .add_modifier(Modifier::BOLD),
        )
    };
    let spans = if ui.show_settings {
        vec![
            Span::raw(" "),
            key("↑↓"),
            Span::raw(":Select  "),
            key("Enter/←→"),
            Span::raw(":Change  "),
            key("R"),
            Span::raw(":Reset  "),
            key(",/Esc"),
            Span::raw(":Close  "),
            key("q"),
            Span::raw(":Quit"),
        ]
    } else {
        match ui.view_mode {
            ViewMode::Combined | ViewMode::Processes => vec![
                Span::raw(" "),
                key("Tab"),
                Span::raw(":View  "),
                key("←→/c/m/n/u/a/d"),
                Span::raw(":Sort  "),
                key("r"),
                Span::raw(":Rev  "),
                key("f"),
                Span::raw(":Filter  "),
                key("/"),
                Span::raw(":Search  "),
                key("↑↓"),
                Span::raw(":Sel  "),
                key("x"),
                Span::raw(":Kill  "),
                key("p"),
                Span::raw(":Pause  "),
                key("+/-"),
                Span::raw(":Rate  "),
                key(","),
                Span::raw(":Settings  "),
                key("q"),
                Span::raw(":Quit"),
            ],
            ViewMode::Hardware => vec![
                Span::raw(" "),
                key("Tab"),
                Span::raw(":View  "),
                key("p"),
                Span::raw(":Pause  "),
                key("+/-"),
                Span::raw(":Rate  "),
                key(","),
                Span::raw(":Settings  "),
                key("q"),
                Span::raw(":Quit"),
            ],
        }
    };
    let footer = Paragraph::new(Line::from(spans)).style(Style::default().fg(Color::Gray));
    f.render_widget(footer, area);
}

fn render_settings(
    f: &mut Frame,
    area: Rect,
    cfg: &AppConfig,
    theme: Theme,
    ui: &UiState,
) {
    let outer = Block::default()
        .title(Span::styled(
            " Settings ",
            Style::default()
                .fg(theme.primary())
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.primary()));
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(9), Constraint::Length(5)])
        .split(inner);

    let theme_label = match cfg.theme {
        crate::config::SavedTheme::Cyan => "Cyan",
        crate::config::SavedTheme::Green => "Green",
        crate::config::SavedTheme::Amber => "Amber",
        crate::config::SavedTheme::Magenta => "Magenta",
    };

    let sort_col: SortColumn = cfg.sort_col.into();
    let order_label = if cfg.sort_desc {
        "Descending (▼)"
    } else {
        "Ascending (▲)"
    };
    let filter_label = if cfg.apps_only { "Apps Only" } else { "All" };

    let items: [(&str, String, &str); SETTINGS_COUNT] = [
        (
            "Startup Mode",
            if cfg.remember_last_state {
                "[x] Remember last state".to_string()
            } else {
                "[ ] Use default screen".to_string()
            },
            "Restore last view, sort, filter, theme, and interval on startup",
        ),
        (
            "Default Screen",
            format!("< {} >", cfg.default_screen.label()),
            if cfg.remember_last_state {
                "Screen to open when 'Remember last state' is off (currently overridden by last state)"
            } else {
                "Initial screen opened on startup"
            },
        ),
        (
            "Color Theme",
            format!("< {} >", theme_label),
            "Active UI accent palette",
        ),
        (
            "Sampling Interval",
            format!("< {} ms >", cfg.interval_ms),
            "Hardware and process polling period (250ms - 10000ms)",
        ),
        (
            "Default Sort Column",
            format!("< {} >", sort_col.label()),
            "Process Explorer column sorted on launch",
        ),
        (
            "Default Sort Order",
            format!("< {} >", order_label),
            "Sort direction applied on launch",
        ),
        (
            "Default Process Filter",
            format!("< {} >", filter_label),
            "Show all processes or .app bundles only on launch",
        ),
    ];

    let rows: Vec<Row> = items
        .iter()
        .enumerate()
        .map(|(idx, (name, val, desc))| {
            let selected = idx == ui.settings_selected;
            let name_style = if selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let val_style = if idx == 1 && cfg.remember_last_state {
                Style::default().fg(Color::DarkGray)
            } else if selected {
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.primary())
            };

            let mut row = Row::new([
                Cell::from(format!("  {}", name)).style(name_style),
                Cell::from(val.clone()).style(val_style),
                Cell::from((*desc).to_string()).style(Style::default().fg(Color::DarkGray)),
            ]);
            if selected {
                row = row.style(Style::default().bg(Color::Rgb(42, 52, 68)));
            }
            row
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(24),
            Constraint::Length(26),
            Constraint::Min(20),
        ],
    )
    .header(
        Row::new([
            Cell::from("  Option"),
            Cell::from("Value"),
            Cell::from("Description"),
        ])
        .style(
            Style::default()
                .fg(theme.primary())
                .add_modifier(Modifier::BOLD),
        ),
    );
    f.render_widget(table, sections[0]);

    let summary = if cfg.remember_last_state {
        format!(
            "Next launch will restore your last screen ({}) and current preferences.",
            cfg.last_screen.label()
        )
    } else {
        format!(
            "Next launch will open on the default screen ({}).",
            cfg.default_screen.label()
        )
    };
    let path_str = AppConfig::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(unavailable)".to_string());

    let info_block = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(" Startup: ", Style::default().fg(Color::Gray)),
            Span::styled(summary, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled(" Config:  ", Style::default().fg(Color::Gray)),
            Span::styled(path_str, Style::default().fg(theme.accent())),
            Span::styled("  (saved automatically)", Style::default().fg(Color::DarkGray)),
        ]),
    ])
    .block(
        Block::default()
            .title(Span::styled(" Persistence ", Style::default().fg(Color::Yellow)))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(info_block, sections[1]);
}

pub fn fmt_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

pub fn fmt_rate(bps: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    if bps < 1.0 {
        "0 B/s".to_string()
    } else if bps >= GB {
        format!("{:.2} GB/s", bps / GB)
    } else if bps >= MB {
        format!("{:.1} MB/s", bps / MB)
    } else if bps >= KB {
        format!("{:.1} KB/s", bps / KB)
    } else {
        format!("{:.0} B/s", bps)
    }
}
