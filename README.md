# MTop (`mtop`)

Apple Silicon performance monitor & Task Manager–style process explorer written in Rust for **macOS 27+**.

## Features

- **Hardware Telemetry (no `sudo` required)**:
  - **CPU**: E-cluster & P-cluster utilization, weighted DVFS frequencies (`pmgr` + `IOReport`), and per-core utilization bars
  - **GPU**: Utilization, DVFS frequency, and live GPU power (`IOReport` `Energy Model`)
  - **Power & Thermals**: System total power (`PSTR`/`PDTR`) and SoC/GPU die temperatures (`AppleSMC`), plus thermal pressure level
  - **Memory & Swap**: RAM Used / Active / Wired / Compressed (`mach` `host_statistics64`) and Swap (`sysctl vm.swapusage`)
  - **Optional `powermetrics` enrichment**: Run with `sudo mtop` (or `--powermetrics`) for CPU, ANE, and DRAM power rails when gated for non-root processes on macOS 27
- **Process Explorer**:
  - Live per-process **CPU%**, **Memory** (`ri_phys_footprint`), **Net ↓/s**, **Net ↑/s** (`nettop`), **Disk/s**, and **Thread count**
  - `.app` bundle detection with `[App]` filter
  - Interactive sorting across all columns, live search (`/`), and process termination (`x`)

## Build & Run

```bash
cargo build --release
./target/release/mtop
```

### CLI Options

```bash
# Single text or JSON snapshot
./target/release/mtop --once
./target/release/mtop --once --json

# Pre-sort or filter to .app bundles
./target/release/mtop --sort memory --apps

# Include CPU/ANE/DRAM power rails via powermetrics
sudo ./target/release/mtop
```

## Keybindings

| Key | Action |
| --- | --- |
| `Tab` | Switch view (`Split` → `Processes` → `Hardware` → `Settings`) |
| `←` / `→` / `s` | Cycle sort column (`PID`, `App`, `CPU%`, `Memory`, `Net ↓/s`, `Net ↑/s`, `Disk/s`, `Thr`) |
| `c` / `m` / `n` / `u` / `a` / `d` / `i` | Sort by CPU, Memory, Net ↓, Net ↑, App, Disk, or PID |
| `r` | Reverse sort order |
| `f` | Toggle filter (`All` / `Apps`) |
| `/` | Live search by name or PID |
| `↑` / `↓` (`j`/`k`), `PgUp`/`PgDn`, `g`/`G` | Select process |
| `x` / `Delete` | Terminate selected process (`SIGTERM`, with confirmation) |
| `p` / `Space` | Pause / Resume |
| `t` | Cycle color theme (`cyan`, `green`, `amber`, `magenta`) |
| `+` / `-` | Adjust sampling interval |
| `q` / `Esc` | Quit |
