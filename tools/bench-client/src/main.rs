//! SOCKS5 latency benchmark tool.
//!
//! Measures end-to-end latency through a Tor onion circuit
//! and compares it with direct connections. Also supports reading
//! pre-recorded CSV data, generating gnuplot-compatible summary files,
//! and rendering SVG plots via plotters.

mod plot;

use anyhow::Context;
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use std::path::PathBuf;
use std::time::Instant;
use tabled::{
    Table, Tabled,
    settings::{
        Alignment, Margin, Modify, Style,
        object::{Columns, Segment},
    },
};
use tokio::time::Duration;

#[derive(Parser)]
#[command(name = "bench-client")]
#[command(about = "Measure SOCKS5 latency and visualize recorded results")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:1080")]
    socks: String,

    #[arg(long, default_value = "http://example.com")]
    url: String,

    #[arg(long, default_value = "50")]
    count: u32,

    #[arg(long)]
    direct: bool,

    #[arg(long, default_value = "10")]
    direct_count: u32,

    #[arg(long, default_value = "500")]
    delay: u64,

    #[arg(long)]
    output: Option<String>,

    #[arg(
        long,
        help = "Parse and display a pre-recorded CSV file instead of running live"
    )]
    read_csv: Option<String>,

    #[arg(long, help = "Write gnuplot-compatible summary dat file")]
    output_summary: Option<String>,

    #[arg(long, default_value = "csv", help = "Directory for CSV data files")]
    csv_dir: String,

    #[arg(
        long,
        help = "Generate SVG plots to the given directory (e.g. lucrare/figuri)"
    )]
    plot: Option<String>,
}

type StatsTuple = (u64, u64, u64, u64);

#[derive(Debug, Clone)]
struct Timings {
    total_us: u64,
    first_byte_us: u64,
}

#[derive(Tabled)]
struct StatsRow {
    #[tabled(rename = "")]
    label: String,
    #[tabled(rename = "Mean")]
    mean: String,
    #[tabled(rename = "Min")]
    min: String,
    #[tabled(rename = "Max")]
    max: String,
    #[tabled(rename = "σ")]
    stddev: String,
}

#[derive(Debug)]
struct CsvData {
    onion_values: Vec<u64>,
    direct_values: Vec<u64>,
}

async fn request_through_socks(client: &reqwest::Client, url: &str) -> anyhow::Result<Timings> {
    let start = Instant::now();
    let resp = client.get(url).send().await?;
    let first_byte = start.elapsed().as_micros() as u64;
    let body = resp.text().await?;
    let total = start.elapsed().as_micros() as u64;
    let _ = body;
    Ok(Timings {
        first_byte_us: first_byte,
        total_us: total,
    })
}

fn stats(values: &[u64]) -> StatsTuple {
    let min = values.iter().min().copied().unwrap_or(0);
    let max = values.iter().max().copied().unwrap_or(0);
    let mean = if values.is_empty() {
        0
    } else {
        values.iter().sum::<u64>() / values.len() as u64
    };
    let variance = if values.is_empty() {
        0.0
    } else {
        let mean_i64 = mean as i64;
        let sum_sq: i64 = values
            .iter()
            .map(|v| {
                let diff = *v as i64 - mean_i64;
                diff * diff
            })
            .sum();
        sum_sq as f64 / values.len() as f64
    };
    let stddev = variance.sqrt() as u64;
    (min, max, mean, stddev)
}

fn ms(us: u64) -> f64 {
    us as f64 / 1000.0
}

fn ms_str(us: u64) -> String {
    format!("{:.1}ms", ms(us))
}

fn ascii_histogram(values: &[u64], bins: usize, width: usize, label: &str) {
    if values.is_empty() {
        return;
    }
    let min = values.iter().min().copied().unwrap_or(0);
    let max = values.iter().max().copied().unwrap_or(0);
    let range = if max > min { max - min } else { 1 };

    let mut buckets = vec![0u64; bins];
    for &v in values {
        let ratio = (v - min) as f64 / range as f64;
        let idx = (ratio * (bins as f64 - 1.0)) as usize;
        let idx = idx.min(bins - 1);
        buckets[idx] += 1;
    }
    let max_count = *buckets.iter().max().unwrap_or(&1).max(&1);

    println!(
        "\n  {} latency distribution ({} samples):",
        label.bright_cyan().bold(),
        values.len()
    );
    for (i, bucket) in buckets.iter().enumerate() {
        let lo = min + (range * i as u64 / bins as u64);
        let hi = min + (range * (i + 1) as u64 / bins as u64);
        let bar_len = if max_count > 0 {
            (*bucket as f64 / max_count as f64 * width as f64) as usize
        } else {
            0
        };
        let count_color = if *bucket > values.len() as u64 / 3 {
            owo_colors::AnsiColors::Green
        } else if *bucket > 0 {
            owo_colors::AnsiColors::Yellow
        } else {
            owo_colors::AnsiColors::White
        };
        let bar = "█".repeat(bar_len);
        let bar_str = bar.bright_yellow();
        let count_str = bucket.color(count_color);
        println!(
            "  {:>5}-{:<5} ms │ {} {}",
            lo / 1000,
            hi / 1000,
            bar_str,
            count_str
        );
    }
    println!();
}

fn print_banner() {
    println!();
    println!(
        "  {}",
        "╔══════════════════════════════════════════╗"
            .bright_cyan()
            .bold()
    );
    println!(
        "  {}",
        "║     SOCKS5 Latency Benchmark Tool        ║"
            .bright_cyan()
            .bold()
    );
    println!(
        "  {}",
        "╚══════════════════════════════════════════╝"
            .bright_cyan()
            .bold()
    );
    println!();
}

fn print_stats_table(
    onion: StatsTuple,
    direct: Option<StatsTuple>,
    onion_cold: Option<StatsTuple>,
    onion_warm: Option<StatsTuple>,
) {
    let (omin, omax, omean, ostd) = onion;
    let mut rows = Vec::new();

    if let Some((cmin, cmax, cmean, cstd)) = onion_cold {
        rows.push(StatsRow {
            label: "Onion (cold)".to_string(),
            mean: ms_str(cmean),
            min: ms_str(cmin),
            max: ms_str(cmax),
            stddev: ms_str(cstd),
        });
    }
    if let Some((wmin, wmax, wmean, wstd)) = onion_warm {
        rows.push(StatsRow {
            label: "Onion (warm)".to_string(),
            mean: ms_str(wmean),
            min: ms_str(wmin),
            max: ms_str(wmax),
            stddev: ms_str(wstd),
        });
    }
    rows.push(StatsRow {
        label: "Onion (all)".to_string(),
        mean: ms_str(omean),
        min: ms_str(omin),
        max: ms_str(omax),
        stddev: ms_str(ostd),
    });

    if let Some((dmin, dmax, dmean, dstd)) = direct {
        rows.push(StatsRow {
            label: "Direct".to_string(),
            mean: ms_str(dmean),
            min: ms_str(dmin),
            max: ms_str(dmax),
            stddev: ms_str(dstd),
        });

        let overhead = if dmean > 0 {
            omean as f64 / dmean as f64
        } else {
            0.0
        };
        println!();
        println!(
            "  Overhead: {}",
            format!("{:.1}x slower than direct", overhead)
                .green()
                .bold()
        );
    }

    println!();
    let mut table = Table::new(rows);
    table
        .with(Style::rounded())
        .with(Margin::new(2, 0, 0, 0))
        .with(Modify::new(Segment::all()).with(Alignment::right()))
        .with(Modify::new(Columns::first()).with(Alignment::left()));
    println!("{}", table);
}

fn parse_csv(path: &str) -> anyhow::Result<CsvData> {
    let content = std::fs::read_to_string(path).context("Failed to read CSV file")?;
    let mut lines = content.lines();

    let header = lines.next().context("CSV is empty")?;
    let columns: Vec<&str> = header.split(',').map(|s| s.trim()).collect();

    let type_idx = columns
        .iter()
        .position(|&c| c == "type")
        .context("CSV must have a 'type' column")?;

    let total_col = columns
        .iter()
        .position(|&c| c.starts_with("total"))
        .context("CSV must have a 'total_us' or 'total_ms' column")?;

    let unit = if columns[total_col].ends_with("_us") {
        "us"
    } else if columns[total_col].ends_with("_ms") {
        "ms"
    } else {
        "us"
    };

    let mut onion_values: Vec<u64> = Vec::new();
    let mut direct_values: Vec<u64> = Vec::new();

    for line in lines {
        let fields: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if fields.len() <= total_col || fields.len() <= type_idx {
            continue;
        }
        let ty = fields[type_idx];
        let raw_val: f64 = fields[total_col]
            .parse()
            .context("Invalid numeric value in CSV")?;
        let val_us = if unit == "ms" {
            (raw_val * 1000.0) as u64
        } else {
            raw_val as u64
        };

        match ty {
            "onion" => onion_values.push(val_us),
            "direct" => direct_values.push(val_us),
            _ => {}
        }
    }

    Ok(CsvData {
        onion_values,
        direct_values,
    })
}

fn compute_cold_warm(
    values: &[u64],
    cold_count: usize,
) -> (Option<StatsTuple>, Option<StatsTuple>) {
    let cold = if values.len() > cold_count {
        let cold_slice = values.get(..cold_count).unwrap_or(&[]);
        if cold_slice.is_empty() {
            None
        } else {
            Some(stats(cold_slice))
        }
    } else {
        None
    };
    let warm = if values.len() > cold_count {
        let warm_slice = values.get(cold_count..).unwrap_or(&[]);
        if warm_slice.is_empty() {
            None
        } else {
            Some(stats(warm_slice))
        }
    } else {
        None
    };
    (cold, warm)
}

fn write_summary_dat(path: &str, onion: &[u64], direct: &[u64]) -> anyhow::Result<()> {
    let cold_count = 2;
    let (cold_opt, warm_opt) = compute_cold_warm(onion, cold_count);

    let cold_mean = cold_opt.map(|c| c.2).unwrap_or(0);
    let cold_min = cold_opt.map(|c| c.0).unwrap_or(0);
    let cold_max = cold_opt.map(|c| c.1).unwrap_or(0);

    let warm_mean = warm_opt.map(|w| w.2).unwrap_or(0);
    let warm_min = warm_opt.map(|w| w.0).unwrap_or(0);
    let warm_max = warm_opt.map(|w| w.1).unwrap_or(0);

    let dir_stats = stats(direct);
    let dir_mean = dir_stats.2;
    let dir_min = dir_stats.0;
    let dir_max = dir_stats.1;

    let content = format!(
        "type mean_ms min_ms max_ms\n\
         onion_cold {:.3} {:.3} {:.3}\n\
         onion_warm {:.3} {:.3} {:.3}\n\
         direct {:.3} {:.3} {:.3}\n",
        ms(cold_mean),
        ms(cold_min),
        ms(cold_max),
        ms(warm_mean),
        ms(warm_min),
        ms(warm_max),
        ms(dir_mean),
        ms(dir_min),
        ms(dir_max),
    );

    std::fs::write(path, content)?;
    println!("  {} saved to: {}", "Summary".green().bold(), path);
    Ok(())
}

fn resolve_path(maybe_rel: &str, csv_dir: &str) -> String {
    let p = PathBuf::from(maybe_rel);
    if p.is_absolute() {
        maybe_rel.to_string()
    } else {
        PathBuf::from(csv_dir)
            .join(maybe_rel)
            .to_string_lossy()
            .to_string()
    }
}

fn ensure_csv_dir(csv_dir: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(csv_dir).context("Failed to create CSV directory")?;
    Ok(())
}

// ═══════════════════════════════════════════════════════════════
//  Live benchmark
// ═══════════════════════════════════════════════════════════════

async fn run_live_bench(args: &Args) -> anyhow::Result<()> {
    let proxy = format!("socks5://{}", args.socks);
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy)?)
        .build()?;

    ensure_csv_dir(&args.csv_dir)?;

    println!("  ── SOCKS5 Onion ({}) ─────────────────────", args.count);

    let pb = ProgressBar::new(args.count as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "  {spinner:.green} [{elapsed_precise}] [{bar:30.cyan/blue}] {pos}/{len} {msg}",
        )
        .context("Invalid progress bar template")?
        .progress_chars("█▓▒░  "),
    );

    let mut onion_times: Vec<u64> = Vec::new();
    let mut csv_rows: Vec<String> = Vec::new();
    csv_rows.push("req,type,total_us,first_byte_us".to_string());

    for i in 1..=args.count {
        let timings = request_through_socks(&client, &args.url)
            .await
            .context("SOCKS5 request failed — is tor-client running?")?;
        csv_rows.push(format!(
            "{},onion,{},{}",
            i, timings.total_us, timings.first_byte_us
        ));
        onion_times.push(timings.total_us);

        let tag = if i <= 2 {
            " ◆ cold".yellow().to_string()
        } else {
            String::new()
        };
        pb.set_message(format!("{:.1}ms{}", ms(timings.total_us), tag));
        pb.inc(1);

        tokio::time::sleep(Duration::from_millis(args.delay)).await;
    }
    pb.finish_and_clear();
    println!();

    let onion_stats = stats(&onion_times);
    let (cold_opt, warm_opt) = compute_cold_warm(&onion_times, 2);

    let mut direct_stats: Option<StatsTuple> = None;
    let mut direct_csv: Vec<String> = Vec::new();

    if args.direct {
        let direct_client = reqwest::Client::new();
        println!(
            "  ── Direct ({}) ────────────────────────────",
            args.direct_count
        );

        let dpb = ProgressBar::new(args.direct_count as u64);
        dpb.set_style(
            ProgressStyle::with_template("  {spinner:.blue} [{bar:30.green}] {pos}/{len} {msg}")
                .context("Invalid progress bar template")?
                .progress_chars("━─"),
        );

        for i in 1..=args.direct_count as u64 {
            let start = Instant::now();
            let _resp = direct_client.get(&args.url).send().await?;
            let elapsed = start.elapsed().as_micros() as u64;
            direct_csv.push(format!("{},direct,{},{}", i, elapsed, elapsed));

            dpb.set_message(format!("{:.1}ms", ms(elapsed)));
            dpb.inc(1);
        }
        dpb.finish_and_clear();
        println!();

        let direct_times: Vec<u64> = direct_csv
            .iter()
            .map(|row| {
                row.split(',')
                    .nth(2)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0)
            })
            .collect();
        direct_stats = Some(stats(&direct_times));

        for r in direct_csv {
            csv_rows.push(r);
        }
    }

    print_stats_table(onion_stats, direct_stats, cold_opt, warm_opt);
    ascii_histogram(&onion_times, 10, 30, "Onion");

    if direct_stats.is_some() {
        let direct_vals: Vec<u64> = csv_rows
            .iter()
            .filter_map(|r| {
                if r.contains(",direct,") {
                    r.split(',').nth(2).and_then(|v| v.parse().ok())
                } else {
                    None
                }
            })
            .collect();
        if !direct_vals.is_empty() {
            ascii_histogram(&direct_vals, 5, 20, "Direct");
        }
    }

    let default_output = resolve_path("latency.csv", &args.csv_dir);
    let output_path = args.output.as_deref().unwrap_or(&default_output);
    if let Some(parent) = PathBuf::from(output_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(output_path, csv_rows.join("\n") + "\n")?;
    println!("  {} saved to: {}", "CSV".green().bold(), output_path);

    let default_summary = resolve_path("latency-summary.dat", &args.csv_dir);
    let summary_path = args.output_summary.as_deref().unwrap_or(&default_summary);
    if let Some(parent) = PathBuf::from(summary_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let direct_vals: Vec<u64> = csv_rows
        .iter()
        .filter_map(|r| {
            if r.contains(",direct,") {
                r.split(',').nth(2).and_then(|v| v.parse().ok())
            } else {
                None
            }
        })
        .collect();
    write_summary_dat(summary_path, &onion_times, &direct_vals)?;

    if let Some(plot_dir) = &args.plot {
        println!();
        plot::generate_all_plots(&PathBuf::from(plot_dir), &PathBuf::from(&args.csv_dir))?;
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════
//  CSV playback
// ═══════════════════════════════════════════════════════════════

fn run_csv_playback(args: &Args) -> anyhow::Result<()> {
    let path = args.read_csv.as_deref().context("--read-csv is required")?;

    let data = parse_csv(path)?;
    let onion_stats = stats(&data.onion_values);
    let direct_stats = if data.direct_values.is_empty() {
        None
    } else {
        Some(stats(&data.direct_values))
    };

    let (cold_opt, warm_opt) = compute_cold_warm(&data.onion_values, 2);

    println!("  Source: {}", path.bright_cyan());
    println!(
        "  Onion samples: {} | Direct samples: {}",
        data.onion_values.len().to_string().bright_yellow(),
        data.direct_values.len().to_string().bright_yellow()
    );

    print_stats_table(onion_stats, direct_stats, cold_opt, warm_opt);
    ascii_histogram(&data.onion_values, 15, 40, "Onion (pre‑recorded)");

    if !data.direct_values.is_empty() {
        ascii_histogram(&data.direct_values, 5, 20, "Direct (pre‑recorded)");
    }

    if let Some(out_path) = &args.output_summary {
        ensure_csv_dir(&args.csv_dir)?;
        let full = resolve_path(out_path, &args.csv_dir);
        if let Some(parent) = PathBuf::from(&full).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        write_summary_dat(&full, &data.onion_values, &data.direct_values)?;
    }

    if let Some(plot_dir) = &args.plot {
        let csv_dir = if PathBuf::from(path).is_absolute() {
            PathBuf::from(path)
                .parent()
                .unwrap_or(&PathBuf::from("."))
                .to_path_buf()
        } else {
            PathBuf::from(&args.csv_dir)
        };
        println!();
        plot::generate_all_plots(&PathBuf::from(plot_dir), &csv_dir)?;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    print_banner();

    if args.read_csv.is_some() {
        run_csv_playback(&args)?;
        println!();
        return Ok(());
    }

    println!("  Proxy:  {}", args.socks.bright_cyan());
    println!("  Target: {}", args.url.bright_cyan());
    println!();

    run_live_bench(&args).await?;

    println!();
    Ok(())
}
