mod config;
mod ioreport;
mod metrics;
mod powermetrics;
mod processes;
mod smc;
mod sysinfo;
mod ui;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use config::{AppConfig, SavedScreen};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use metrics::MetricsCollector;
use processes::{
    sort_processes, terminate_process, ProcessFilter, ProcessInfo, ProcessMonitor, SortColumn,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use serde::Serialize;
use std::io;
use std::time::{Duration, Instant};
use ui::{fmt_bytes, fmt_rate, Theme, UiState};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ColorArg {
    Cyan,
    Green,
    Amber,
    Magenta,
}

impl From<ColorArg> for Theme {
    fn from(c: ColorArg) -> Self {
        match c {
            ColorArg::Cyan => Theme::Cyan,
            ColorArg::Green => Theme::Green,
            ColorArg::Amber => Theme::Amber,
            ColorArg::Magenta => Theme::Magenta,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "mtop",
    version,
    about = "Apple Silicon performance monitor & process explorer for macOS 27+"
)]
struct Cli {
    /// Sampling interval in milliseconds (overrides saved setting)
    #[arg(short, long)]
    interval: Option<u64>,

    /// Color theme (overrides saved setting)
    #[arg(short, long, value_enum)]
    color: Option<ColorArg>,

    /// Initial sort column for process explorer (overrides saved setting)
    #[arg(short, long, value_enum)]
    sort: Option<SortColumn>,

    /// Show only .app bundle processes initially
    #[arg(long)]
    apps: bool,

    /// Print a single sample and exit
    #[arg(long)]
    once: bool,

    /// Number of top processes to print in --once mode
    #[arg(long, default_value_t = 15)]
    top: usize,

    /// Output metrics and processes as JSON
    #[arg(long)]
    json: bool,

    /// Enable powermetrics subprocess (requires root/sudo for CPU/ANE/DRAM power)
    #[arg(long)]
    powermetrics: bool,
}

#[derive(Serialize)]
struct JsonReport<'a> {
    hardware: &'a metrics::MetricsSnapshot,
    processes: &'a [ProcessInfo],
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut cfg = AppConfig::load();
    let mut interval_ms = cli.interval.unwrap_or(cfg.interval_ms).clamp(100, 10_000);
    let mut theme: Theme = cli.color.map(Into::into).unwrap_or_else(|| cfg.theme.into());
    let sort_col = cli.sort.unwrap_or_else(|| cfg.sort_col.into());

    let mut collector = MetricsCollector::new(interval_ms, cli.powermetrics)?;
    let mut proc_mon = ProcessMonitor::new();

    // Wait one warmup interval so IOReport, CPU%, and nettop have real delta baselines
    std::thread::sleep(Duration::from_millis(interval_ms.clamp(550, 1000)));

    if cli.once || cli.json {
        loop {
            let snap = collector.collect()?;
            let mut procs = proc_mon.sample();
            if cli.apps || (cli.sort.is_none() && cfg.apps_only) {
                procs.retain(|p| p.is_app);
            }
            sort_processes(&mut procs, sort_col, sort_col.default_descending());
            if cli.json {
                let report = JsonReport {
                    hardware: &snap,
                    processes: &procs[..procs.len().min(cli.top.max(1))],
                };
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_text_summary(&collector, &snap, &procs[..procs.len().min(cli.top)]);
            }
            if cli.once {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(interval_ms));
        }
    }

    // TUI mode
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        orig_hook(info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut ui_state = UiState {
        view_mode: cfg.initial_view_mode(),
        sort_col,
        sort_desc: if let Some(s) = cli.sort {
            s.default_descending()
        } else if cfg.remember_last_state {
            cfg.sort_desc
        } else {
            sort_col.default_descending()
        },
        filter: if cli.apps {
            ProcessFilter::AppsOnly
        } else {
            cfg.initial_filter()
        },
        ..Default::default()
    };

    let res = run_tui(
        &mut terminal,
        &mut collector,
        &mut proc_mon,
        &mut interval_ms,
        &mut theme,
        &mut cfg,
        &mut ui_state,
    );

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res
}

fn filter_and_sort<'a>(
    raw: &'a mut [ProcessInfo],
    ui: &UiState,
) -> (Vec<&'a ProcessInfo>, usize, usize) {
    sort_processes(raw, ui.sort_col, ui.sort_desc);
    let total_procs = raw.len();
    let total_apps = raw.iter().filter(|p| p.is_app).count();
    let q = ui.search_query.to_ascii_lowercase();

    let filtered: Vec<&ProcessInfo> = raw
        .iter()
        .filter(|p| match ui.filter {
            ProcessFilter::All => true,
            ProcessFilter::AppsOnly => p.is_app,
        })
        .filter(|p| {
            if q.is_empty() {
                true
            } else {
                p.name.to_ascii_lowercase().contains(&q)
                    || p.app_name
                        .as_deref()
                        .map(|a| a.to_ascii_lowercase().contains(&q))
                        .unwrap_or(false)
                    || p.pid.to_string().contains(&q)
            }
        })
        .collect();

    (filtered, total_procs, total_apps)
}

fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    collector: &mut MetricsCollector,
    proc_mon: &mut ProcessMonitor,
    interval_ms: &mut u64,
    theme: &mut Theme,
    cfg: &mut AppConfig,
    ui: &mut UiState,
) -> Result<()> {
    let mut paused = false;
    let mut snap = collector.collect()?;
    let mut raw_procs = proc_mon.sample();
    let mut last_tick = Instant::now();

    loop {
        let (filtered, total_procs, total_apps) = filter_and_sort(&mut raw_procs, ui);
        if filtered.is_empty() {
            ui.table_state.select(None);
        } else {
            let sel = ui.table_state.selected().unwrap_or(0).min(filtered.len() - 1);
            ui.table_state.select(Some(sel));
        }

        let selected_proc = ui
            .table_state
            .selected()
            .and_then(|i| filtered.get(i))
            .map(|p| (p.pid, p.display_name().to_string()));

        terminal.draw(|f| {
            ui::render(
                f,
                collector,
                &snap,
                &filtered,
                total_procs,
                total_apps,
                *interval_ms,
                paused,
                *theme,
                cfg,
                ui,
            );
        })?;

        let filtered_len = filtered.len();
        let timeout = Duration::from_millis(50);
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                ui.status_msg = None;

                if let Some((pid, ref name)) = ui.confirm_kill.clone() {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            match terminate_process(pid) {
                                Ok(()) => {
                                    ui.status_msg = Some(format!("Sent SIGTERM to {name} ({pid})"));
                                }
                                Err(e) => {
                                    ui.status_msg =
                                        Some(format!("Failed to terminate {name} ({pid}): {e}"));
                                }
                            }
                            ui.confirm_kill = None;
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                            ui.confirm_kill = None;
                        }
                        _ => {}
                    }
                    continue;
                }

                if ui.search_mode {
                    match key.code {
                        KeyCode::Esc => {
                            ui.search_query.clear();
                            ui.search_mode = false;
                        }
                        KeyCode::Enter => {
                            ui.search_mode = false;
                        }
                        KeyCode::Backspace => {
                            ui.search_query.pop();
                            ui.table_state.select(Some(0));
                        }
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            ui.search_query.push(c);
                            ui.table_state.select(Some(0));
                        }
                        _ => {}
                    }
                    continue;
                }

                let has_procs = matches!(ui.view_mode, ui::ViewMode::Combined | ui::ViewMode::Processes);
                let in_settings = ui.view_mode == ui::ViewMode::Settings;

                match key.code {
                    KeyCode::Char('q') => {
                        save_last_state(cfg, ui, *theme, *interval_ms);
                        break;
                    }
                    KeyCode::Esc => {
                        if has_procs && !ui.search_query.is_empty() {
                            ui.search_query.clear();
                        } else {
                            save_last_state(cfg, ui, *theme, *interval_ms);
                            break;
                        }
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        save_last_state(cfg, ui, *theme, *interval_ms);
                        break;
                    }
                    KeyCode::Tab => {
                        ui.view_mode = ui.view_mode.next();
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Char('p') => paused = !paused,
                    KeyCode::Char(' ') if !in_settings => paused = !paused,
                    KeyCode::Char('t') => {
                        *theme = theme.next();
                        cfg.theme = (*theme).into();
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        *interval_ms = (*interval_ms + 250).min(10_000);
                        cfg.interval_ms = *interval_ms;
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Char('-') => {
                        *interval_ms = interval_ms.saturating_sub(250).max(250);
                        cfg.interval_ms = *interval_ms;
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    // Settings tab controls
                    KeyCode::Down | KeyCode::Char('j') if in_settings => {
                        ui.settings_selected = (ui.settings_selected + 1) % ui::SETTINGS_COUNT;
                    }
                    KeyCode::Up | KeyCode::Char('k') if in_settings => {
                        ui.settings_selected = (ui.settings_selected + ui::SETTINGS_COUNT - 1) % ui::SETTINGS_COUNT;
                    }
                    KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right if in_settings => {
                        apply_setting_change(ui.settings_selected, true, cfg, ui, theme, interval_ms);
                    }
                    KeyCode::Left if in_settings => {
                        apply_setting_change(ui.settings_selected, false, cfg, ui, theme, interval_ms);
                    }
                    KeyCode::Char('R') if in_settings => {
                        *cfg = AppConfig::default();
                        *theme = cfg.theme.into();
                        *interval_ms = cfg.interval_ms;
                        ui.sort_col = cfg.sort_col.into();
                        ui.sort_desc = cfg.sort_desc;
                        ui.filter = cfg.initial_filter();
                        cfg.save();
                        ui.status_msg = Some("Settings reset to defaults".to_string());
                    }
                    // Process Explorer controls
                    KeyCode::Char('/') if has_procs => {
                        ui.search_mode = true;
                    }
                    KeyCode::Char('f') if has_procs => {
                        ui.filter = ui.filter.next();
                        ui.table_state.select(Some(0));
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Char('r') if has_procs => {
                        ui.sort_desc = !ui.sort_desc;
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Char('s') | KeyCode::Right if has_procs => {
                        ui.sort_col = ui.sort_col.next();
                        ui.sort_desc = ui.sort_col.default_descending();
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Left if has_procs => {
                        ui.sort_col = ui.sort_col.prev();
                        ui.sort_desc = ui.sort_col.default_descending();
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Char('c') if has_procs => {
                        set_sort(ui, SortColumn::Cpu);
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Char('m') if has_procs => {
                        set_sort(ui, SortColumn::Memory);
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Char('n') if has_procs => {
                        set_sort(ui, SortColumn::NetDown);
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Char('u') if has_procs => {
                        set_sort(ui, SortColumn::NetUp);
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Char('a') if has_procs => {
                        set_sort(ui, SortColumn::App);
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Char('d') if has_procs => {
                        set_sort(ui, SortColumn::Disk);
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Char('i') if has_procs => {
                        set_sort(ui, SortColumn::Pid);
                        save_last_state(cfg, ui, *theme, *interval_ms);
                    }
                    KeyCode::Down | KeyCode::Char('j') if has_procs => {
                        move_selection(ui, filtered_len, 1);
                    }
                    KeyCode::Up | KeyCode::Char('k') if has_procs => {
                        move_selection(ui, filtered_len, -1);
                    }
                    KeyCode::PageDown if has_procs => {
                        move_selection(ui, filtered_len, 10);
                    }
                    KeyCode::PageUp if has_procs => {
                        move_selection(ui, filtered_len, -10);
                    }
                    KeyCode::Home | KeyCode::Char('g') if has_procs => {
                        if filtered_len > 0 {
                            ui.table_state.select(Some(0));
                        }
                    }
                    KeyCode::End | KeyCode::Char('G') if has_procs => {
                        if filtered_len > 0 {
                            ui.table_state.select(Some(filtered_len - 1));
                        }
                    }
                    KeyCode::Char('x') | KeyCode::Delete if has_procs => {
                        if let Some((pid, name)) = selected_proc {
                            ui.confirm_kill = Some((pid, name));
                        }
                    }
                    _ => {}
                }
            }
        }

        if !paused && last_tick.elapsed() >= Duration::from_millis(*interval_ms) {
            snap = collector.collect()?;
            raw_procs = proc_mon.sample();
            last_tick = Instant::now();
        }
    }

    Ok(())
}

fn save_last_state(cfg: &mut AppConfig, ui: &UiState, theme: Theme, interval_ms: u64) {
    if !cfg.remember_last_state {
        return;
    }
    if let Some(screen) = SavedScreen::from_view_mode(ui.view_mode) {
        cfg.last_screen = screen;
    }
    cfg.theme = theme.into();
    cfg.interval_ms = interval_ms;
    cfg.sort_col = ui.sort_col.into();
    cfg.sort_desc = ui.sort_desc;
    cfg.apps_only = ui.filter == ProcessFilter::AppsOnly;
    cfg.save();
}

fn apply_setting_change(
    idx: usize,
    forward: bool,
    cfg: &mut AppConfig,
    ui: &mut UiState,
    theme: &mut Theme,
    interval_ms: &mut u64,
) {
    match idx {
        0 => {
            cfg.remember_last_state = !cfg.remember_last_state;
        }
        1 => {
            cfg.default_screen = if forward {
                cfg.default_screen.next()
            } else {
                cfg.default_screen.prev()
            };
        }
        2 => {
            *theme = if forward {
                theme.next()
            } else {
                theme.next().next().next()
            };
            cfg.theme = (*theme).into();
        }
        3 => {
            *interval_ms = if forward {
                (*interval_ms + 250).min(10_000)
            } else {
                interval_ms.saturating_sub(250).max(250)
            };
            cfg.interval_ms = *interval_ms;
        }
        4 => {
            let cur: SortColumn = cfg.sort_col.into();
            let next = if forward { cur.next() } else { cur.prev() };
            cfg.sort_col = next.into();
            cfg.sort_desc = next.default_descending();
            ui.sort_col = next;
            ui.sort_desc = cfg.sort_desc;
        }
        5 => {
            cfg.sort_desc = !cfg.sort_desc;
            ui.sort_desc = cfg.sort_desc;
        }
        6 => {
            cfg.apps_only = !cfg.apps_only;
            ui.filter = cfg.initial_filter();
            ui.table_state.select(Some(0));
        }
        _ => {}
    }
    cfg.save();
}

fn set_sort(ui: &mut UiState, col: SortColumn) {
    if ui.sort_col == col {
        ui.sort_desc = !ui.sort_desc;
    } else {
        ui.sort_col = col;
        ui.sort_desc = col.default_descending();
    }
}

fn move_selection(ui: &mut UiState, len: usize, delta: isize) {
    if len == 0 {
        ui.table_state.select(None);
        return;
    }
    let cur = ui.table_state.selected().unwrap_or(0) as isize;
    let next = (cur + delta).clamp(0, len as isize - 1) as usize;
    ui.table_state.select(Some(next));
}

fn print_text_summary(
    collector: &MetricsCollector,
    snap: &metrics::MetricsSnapshot,
    procs: &[ProcessInfo],
) {
    let gb = 1024.0 * 1024.0 * 1024.0;
    println!(
        "{} ({}P+{}E CPU, {}G GPU, {:.0} GB) | macOS {} | Thermal: {}",
        collector.chip.chip_name,
        collector.chip.p_cores,
        collector.chip.e_cores,
        collector.chip.gpu_cores,
        collector.chip.mem_total_bytes as f64 / gb,
        collector.chip.os_version,
        snap.thermal_pressure
    );
    println!(
        "CPU:   {:.1}% total | E-CPU: {:.1}% @ {:.0} MHz | P-CPU: {:.1}% @ {:.0} MHz",
        snap.cpu_total_util_pct,
        snap.ecpu_util_pct,
        snap.ecpu_freq_mhz,
        snap.pcpu_util_pct,
        snap.pcpu_freq_mhz
    );
    println!(
        "GPU:   {:.1}% @ {:.0} MHz | Power: {}",
        snap.gpu_util_pct,
        snap.gpu_freq_mhz,
        snap.gpu_power_w
            .map(|w| format!("{:.2} W", w))
            .unwrap_or_else(|| "N/A".to_string())
    );
    println!(
        "Power: System={} | CPU={} | ANE={} | DRAM={}",
        fmt_w(snap.system_power_w),
        fmt_w(snap.cpu_power_w),
        fmt_w(snap.ane_power_w),
        fmt_w(snap.dram_power_w)
    );
    println!(
        "Temp:  SoC avg={} max={} | GPU={}",
        fmt_c(snap.soc_temp_avg_c),
        fmt_c(snap.soc_temp_max_c),
        fmt_c(snap.gpu_temp_avg_c)
    );
    println!(
        "RAM:   {:.2} / {:.1} GB (Active {:.2}, Wired {:.2}, Compressed {:.2}) | Swap {:.2}/{:.2} GB",
        snap.mem_used_bytes as f64 / gb,
        snap.mem_total_bytes as f64 / gb,
        snap.mem_active_bytes as f64 / gb,
        snap.mem_wired_bytes as f64 / gb,
        snap.mem_compressed_bytes as f64 / gb,
        snap.swap_used_bytes as f64 / gb,
        snap.swap_total_bytes as f64 / gb
    );
    println!(
        "\n{:<7} {:<30} {:>7} {:>10} {:>11} {:>11} {:>11} {:>4}",
        "PID", "APP / PROCESS", "CPU%", "MEMORY", "NET DOWN", "NET UP", "DISK/S", "THR"
    );
    for p in procs {
        let label = if p.is_app {
            format!("[App] {}", p.display_name())
        } else {
            p.display_name().to_string()
        };
        let trunc: String = label.chars().take(29).collect();
        println!(
            "{:<7} {:<30} {:>6.1}% {:>10} {:>11} {:>11} {:>11} {:>4}",
            p.pid,
            trunc,
            p.cpu_pct,
            fmt_bytes(p.mem_bytes),
            fmt_rate(p.net_rx_bps),
            fmt_rate(p.net_tx_bps),
            fmt_rate(p.disk_bps()),
            p.threads
        );
    }
}

fn fmt_w(v: Option<f64>) -> String {
    v.map(|x| format!("{:.2}W", x)).unwrap_or_else(|| "N/A".to_string())
}

fn fmt_c(v: Option<f64>) -> String {
    v.map(|x| format!("{:.1}°C", x)).unwrap_or_else(|| "N/A".to_string())
}
