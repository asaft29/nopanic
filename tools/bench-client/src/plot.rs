use anyhow::{Context, Result};
use plotters::chart::MeshStyle;
use plotters::prelude::*;
use plotters::style::IntoFont;
use std::path::Path;

const ORANGE: RGBColor = RGBColor(0xe0, 0x90, 0x40);
const TEAL: RGBColor = RGBColor(0x5a, 0x90, 0x90);
const WHITE: RGBColor = RGBColor(0xff, 0xff, 0xff);

const FONT: &str = "serif";

fn small_font() -> (&'static str, u32) {
    (FONT, 12)
}

fn hide_grid<DB: DrawingBackend, X: Ranged, Y: Ranged>(mesh: &mut MeshStyle<'_, '_, X, Y, DB>) {
    mesh.light_line_style(TRANSPARENT)
        .bold_line_style(TRANSPARENT);
}

pub fn plot_latency_cdf(output_path: &Path, csv_path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(csv_path).context("Failed to read CSV")?;
    let mut onion_vals: Vec<f64> = Vec::new();
    let mut direct_vals: Vec<f64> = Vec::new();

    for line in content.lines().skip(1) {
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 4 {
            continue;
        }
        let ty = cols[1].trim();
        let is_ms = cols[2].trim().ends_with("_ms");
        let total: f64 = cols[2].trim().parse().unwrap_or(0.0);
        let ms_val = if is_ms { total } else { total / 1000.0 };
        if ty == "onion" {
            onion_vals.push(ms_val);
        } else if ty == "direct" {
            direct_vals.push(ms_val);
        }
    }

    if onion_vals.is_empty() {
        anyhow::bail!("No onion data in CSV");
    }

    onion_vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    direct_vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let onion_n = onion_vals.len() as f64;
    let direct_n = direct_vals.len() as f64;

    let onion_cdf: Vec<(f64, f64)> = onion_vals
        .iter()
        .enumerate()
        .map(|(i, &v)| (v, (i + 1) as f64 / onion_n))
        .collect();

    let direct_cdf: Vec<(f64, f64)> = direct_vals
        .iter()
        .enumerate()
        .map(|(i, &v)| (v, (i + 1) as f64 / direct_n))
        .collect();

    let max_x = onion_vals
        .last()
        .copied()
        .unwrap_or(100.0)
        .max(direct_vals.last().copied().unwrap_or(0.0))
        * 1.15;

    let root = SVGBackend::new(output_path, (900, 500)).into_drawing_area();
    root.fill(&WHITE)?;

    let mut chart = ChartBuilder::on(&root)
        .margin_top(20)
        .margin_bottom(30)
        .margin_left(50)
        .margin_right(30)
        .x_label_area_size(40)
        .y_label_area_size(50)
        .build_cartesian_2d(0.0..max_x, 0.0..1.05)?;

    let mut mesh = chart.configure_mesh();
    mesh.x_desc("Latency (ms)").y_desc("Cumulative probability");
    hide_grid(&mut mesh);
    mesh.draw()?;

    chart.draw_series(
        direct_cdf
            .iter()
            .step_by(10)
            .map(|(x, y)| Circle::new((*x, *y), 3, TEAL.filled())),
    )?;

    chart
        .draw_series(LineSeries::new(direct_cdf.clone(), TEAL.stroke_width(2)))?
        .label("Direct")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], TEAL.stroke_width(2)));

    chart.draw_series(
        onion_cdf
            .iter()
            .step_by(10)
            .map(|(x, y)| Circle::new((*x, *y), 3, ORANGE.filled())),
    )?;

    chart
        .draw_series(LineSeries::new(onion_cdf.clone(), ORANGE.stroke_width(2)))?
        .label("SOCKS5 onion")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], ORANGE.stroke_width(2)));

    chart
        .configure_series_labels()
        .position(SeriesLabelPosition::LowerRight)
        .background_style(WHITE.mix(0.9))
        .draw()?;

    root.present()?;
    Ok(())
}

pub fn plot_latency_scatter(output_path: &Path, csv_path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(csv_path).context("Failed to read CSV")?;
    let mut onion: Vec<(f64, f64)> = Vec::new();
    let mut direct_values: Vec<f64> = Vec::new();

    for line in content.lines().skip(1) {
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 4 {
            continue;
        }
        let req_num: f64 = cols[0].trim().parse().unwrap_or(0.0);
        let ty = cols[1].trim();
        let is_ms = cols[2].trim().ends_with("_ms");
        let total: f64 = cols[2].trim().parse().unwrap_or(0.0);
        let val = if is_ms { total } else { total / 1000.0 };
        if ty == "onion" {
            onion.push((req_num, val));
        } else if ty == "direct" {
            direct_values.push(val);
        }
    }

    if onion.is_empty() {
        anyhow::bail!("No onion data in CSV");
    }

    let max_x = onion.last().map(|r| r.0).unwrap_or(50.0) + 2.0;
    let max_y = onion
        .iter()
        .map(|r| r.1)
        .fold(0.0f64, f64::max)
        .max(direct_values.iter().cloned().fold(0.0f64, f64::max))
        * 1.2;

    let direct_mean = if direct_values.is_empty() {
        0.0
    } else {
        direct_values.iter().sum::<f64>() / direct_values.len() as f64
    };

    let root = SVGBackend::new(output_path, (900, 500)).into_drawing_area();
    root.fill(&WHITE)?;

    let mut chart = ChartBuilder::on(&root)
        .margin_top(20)
        .margin_bottom(30)
        .margin_left(50)
        .margin_right(30)
        .x_label_area_size(40)
        .y_label_area_size(50)
        .build_cartesian_2d(0.0..max_x, 0.0..max_y)?;

    let mut mesh = chart.configure_mesh();
    mesh.x_desc("Request #").y_desc("Total time (ms)");
    hide_grid(&mut mesh);
    mesh.draw()?;

    chart
        .draw_series(
            onion
                .iter()
                .map(|(x, y)| Circle::new((*x, *y), 3, ORANGE.filled())),
        )?
        .label("SOCKS5 onion")
        .legend(|(x, y)| Circle::new((x, y), 5, ORANGE.filled()));

    chart
        .draw_series(LineSeries::new(
            onion.clone(),
            ORANGE.mix(0.3).stroke_width(2),
        ))?
        .label("Trend")
        .legend(|(x, y)| {
            PathElement::new(vec![(x, y), (x + 20, y)], ORANGE.mix(0.3).stroke_width(2))
        });

    if direct_mean > 0.0 {
        chart
            .draw_series(LineSeries::new(
                [(0.0, direct_mean), (max_x, direct_mean)],
                TEAL.stroke_width(2),
            ))?
            .label(format!("Direct (avg: {:.0} ms)", direct_mean))
            .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], TEAL.stroke_width(2)));
    }

    chart
        .configure_series_labels()
        .position(SeriesLabelPosition::UpperRight)
        .background_style(WHITE.mix(0.9))
        .draw()?;
    root.present()?;
    Ok(())
}

pub fn plot_nhop_comparison(output_path: &Path, dat_path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(dat_path).context("Failed to read nhop-data.dat")?;
    let mut entries: Vec<(f64, f64)> = Vec::new();

    for line in content.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 2 {
            continue;
        }
        let hops: f64 = cols[0].parse().unwrap_or(0.0);
        let mean: f64 = cols[1].parse().unwrap_or(0.0);
        entries.push((hops, mean));
    }

    if entries.is_empty() {
        anyhow::bail!("No data in nhop-data.dat");
    }

    let max_x = entries.last().map(|e| e.0).unwrap_or(10.0) + 2.0;
    let max_y = entries.iter().map(|e| e.1).fold(0.0f64, f64::max) * 1.3;

    let root = SVGBackend::new(output_path, (700, 500)).into_drawing_area();
    root.fill(&WHITE)?;

    let mut chart = ChartBuilder::on(&root)
        .margin_top(20)
        .margin_bottom(30)
        .margin_left(50)
        .margin_right(30)
        .x_label_area_size(30)
        .y_label_area_size(50)
        .build_cartesian_2d(0.0..max_x, 0.0..max_y)?;

    let mut mesh = chart.configure_mesh();
    mesh.x_desc("Hops").y_desc("Time (ms)");
    hide_grid(&mut mesh);
    mesh.draw()?;

    chart.draw_series(
        entries
            .iter()
            .map(|(x, y)| Circle::new((*x, *y), 5, ORANGE.filled())),
    )?;
    chart.draw_series(LineSeries::new(entries.clone(), TEAL.stroke_width(2)))?;

    for (x, y) in &entries {
        chart.draw_series(std::iter::once(Text::new(
            format!("{:.1}", y),
            (*x, *y + max_y * 0.05),
            small_font().into_font().color(&BLACK),
        )))?;
    }

    root.present()?;
    Ok(())
}

pub fn plot_concurrency(output_path: &Path, csv_path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(csv_path).context("Failed to read concurrency.csv")?;
    let mut entries: Vec<(f64, f64)> = Vec::new();

    for line in content.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 2 {
            continue;
        }
        let clients: f64 = cols[0].parse().unwrap_or(0.0);
        let time_ms: f64 = cols[1].parse::<f64>().unwrap_or(0.0) * 1000.0;
        entries.push((clients, time_ms));
    }

    if entries.is_empty() {
        anyhow::bail!("No data in concurrency.csv");
    }

    let max_x = entries.last().map(|e| e.0).unwrap_or(16.0) + 2.0;
    let max_y = entries.iter().map(|e| e.1).fold(0.0f64, f64::max) * 1.25;

    let root = SVGBackend::new(output_path, (800, 500)).into_drawing_area();
    root.fill(&WHITE)?;

    let mut chart = ChartBuilder::on(&root)
        .margin_top(20)
        .margin_bottom(30)
        .margin_left(50)
        .margin_right(30)
        .x_label_area_size(40)
        .y_label_area_size(50)
        .build_cartesian_2d(0.0..max_x, 0.0..max_y)?;

    let mut mesh = chart.configure_mesh();
    mesh.x_desc("Clients").y_desc("Total time (ms)");
    hide_grid(&mut mesh);
    mesh.draw()?;

    chart.draw_series(
        entries
            .iter()
            .map(|(x, y)| Circle::new((*x, *y), 4, ORANGE.filled())),
    )?;
    chart.draw_series(LineSeries::new(entries.clone(), TEAL.stroke_width(2)))?;

    for (x, y) in &entries {
        chart.draw_series(std::iter::once(Text::new(
            format!("{:.1}", y),
            (*x, *y + max_y * 0.05),
            small_font().into_font().color(&BLACK),
        )))?;
    }

    root.present()?;
    Ok(())
}

pub fn generate_all_plots(plot_dir: &Path, csv_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(plot_dir).context("Failed to create plot directory")?;

    let latency_csv = csv_dir.join("latency.csv");
    let nhop_dat = csv_dir.join("nhop-data.dat");
    let conc_csv = csv_dir.join("concurrency.csv");

    if latency_csv.exists() {
        let out = plot_dir.join("latency-comparison.svg");
        plot_latency_cdf(&out, &latency_csv)?;
        println!("  CDF               -> {}", out.display());
    }
    if latency_csv.exists() {
        let out = plot_dir.join("latency-scatter.svg");
        plot_latency_scatter(&out, &latency_csv)?;
        println!("  Scatter plot      -> {}", out.display());
    }
    if nhop_dat.exists() {
        let out = plot_dir.join("nhop-comparison.svg");
        plot_nhop_comparison(&out, &nhop_dat)?;
        println!("  N-hop chart       -> {}", out.display());
    }
    if conc_csv.exists() {
        let out = plot_dir.join("concurrency.svg");
        plot_concurrency(&out, &conc_csv)?;
        println!("  Concurrency chart -> {}", out.display());
    }

    Ok(())
}
