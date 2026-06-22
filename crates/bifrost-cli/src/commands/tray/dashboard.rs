use super::runtime::ServiceState;
use super::system_stats::SystemStatsSnapshot;
use super::tray::{cached_menu_bar_glyph, menu_bar_stats_font};

pub const DASHBOARD_WIDTH: u32 = 680;
pub const DASHBOARD_HEIGHT: u32 = 352;

#[derive(Debug, Clone, PartialEq)]
pub struct TrayDashboardSnapshot {
    pub service_state: ServiceState,
    pub runtime_label: String,
    pub system_proxy_label: String,
    pub cpu_percent: f32,
    pub cpu_logical_cores: Option<usize>,
    pub cpu_performance_cores: Option<usize>,
    pub cpu_efficiency_cores: Option<usize>,
    pub load_one: f32,
    pub load_five: f32,
    pub load_fifteen: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub memory_pressure_percent: Option<f32>,
    pub memory_compressed_bytes: u64,
    pub memory_cached_bytes: u64,
    pub swap_used_bytes: Option<u64>,
    pub swap_total_bytes: Option<u64>,
    pub disk_used_percent: Option<f32>,
    pub disk_free_bytes: Option<u64>,
    pub disk_total_bytes: Option<u64>,
    pub disk_mount_label: Option<String>,
    pub disk_total_bytes_per_sec: Option<u64>,
    pub disk_read_bytes_per_sec: Option<u64>,
    pub disk_write_bytes_per_sec: Option<u64>,
    pub network_up_bytes_per_sec: Option<u64>,
    pub network_down_bytes_per_sec: Option<u64>,
    pub network_interface_label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TrayDashboardBitmap {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardTheme {
    Light,
    Dark,
}

#[derive(Debug, Default)]
pub struct TrayDashboardHistory;

impl TrayDashboardHistory {
    pub fn snapshot(
        &mut self,
        service_state: ServiceState,
        runtime_label: String,
        system_proxy_label: String,
        stats: &SystemStatsSnapshot,
    ) -> TrayDashboardSnapshot {
        TrayDashboardSnapshot {
            service_state,
            runtime_label,
            system_proxy_label,
            cpu_percent: stats.cpu_percent,
            cpu_logical_cores: stats.cpu_logical_cores,
            cpu_performance_cores: stats.cpu_performance_cores,
            cpu_efficiency_cores: stats.cpu_efficiency_cores,
            load_one: stats.load_one,
            load_five: stats.load_five,
            load_fifteen: stats.load_fifteen,
            memory_used_bytes: stats.memory_used_bytes,
            memory_total_bytes: stats.memory_total_bytes,
            memory_pressure_percent: stats.memory_pressure_percent,
            memory_compressed_bytes: stats.memory_compressed_bytes,
            memory_cached_bytes: stats.memory_cached_bytes,
            swap_used_bytes: stats.swap_used_bytes,
            swap_total_bytes: stats.swap_total_bytes,
            disk_used_percent: stats.disk_used_percent,
            disk_free_bytes: stats.disk_free_bytes,
            disk_total_bytes: stats.disk_total_bytes,
            disk_mount_label: stats.disk_mount_point.clone(),
            disk_total_bytes_per_sec: stats.disk_total_bytes_per_sec,
            disk_read_bytes_per_sec: stats.disk_read_bytes_per_sec,
            disk_write_bytes_per_sec: stats.disk_write_bytes_per_sec,
            network_up_bytes_per_sec: stats.network_up_bytes_per_sec,
            network_down_bytes_per_sec: stats.network_down_bytes_per_sec,
            network_interface_label: stats.network_interface.clone(),
        }
    }
}

pub fn dashboard_enabled_from_env(value: Option<&str>) -> bool {
    !matches!(
        value,
        Some("0" | "false" | "FALSE" | "no" | "NO" | "off" | "OFF")
    )
}

pub fn render_dashboard_with_theme(
    snapshot: &TrayDashboardSnapshot,
    theme: DashboardTheme,
) -> Option<TrayDashboardBitmap> {
    let font = menu_bar_stats_font()?;
    let palette = DashboardPalette::for_theme(theme);
    let mut bitmap = TrayDashboardBitmap {
        rgba: vec![0; (DASHBOARD_WIDTH * DASHBOARD_HEIGHT * 4) as usize],
        width: DASHBOARD_WIDTH,
        height: DASHBOARD_HEIGHT,
    };

    fill_rect(
        &mut bitmap,
        0,
        0,
        DASHBOARD_WIDTH,
        DASHBOARD_HEIGHT,
        palette.background,
    );
    draw_cpu_row(snapshot, &mut bitmap, &palette, font);
    draw_line(
        &mut bitmap,
        24,
        82,
        DASHBOARD_WIDTH - 24,
        82,
        palette.separator,
    );
    draw_memory_row(snapshot, &mut bitmap, &palette, font);
    draw_line(
        &mut bitmap,
        24,
        164,
        DASHBOARD_WIDTH - 24,
        164,
        palette.separator,
    );
    draw_disk_row(snapshot, &mut bitmap, &palette, font);
    draw_line(
        &mut bitmap,
        24,
        246,
        DASHBOARD_WIDTH - 24,
        246,
        palette.separator,
    );
    draw_network_row(snapshot, &mut bitmap, &palette, font);
    draw_line(
        &mut bitmap,
        24,
        328,
        DASHBOARD_WIDTH - 24,
        328,
        palette.separator,
    );

    Some(bitmap)
}

fn draw_cpu_row(
    snapshot: &TrayDashboardSnapshot,
    bitmap: &mut TrayDashboardBitmap,
    palette: &DashboardPalette,
    font: &fontdue::Font,
) {
    let color = palette.status_color(cpu_status(snapshot.cpu_percent));
    draw_metric_label(bitmap, 30, "CPU", palette, font);
    draw_text(
        bitmap,
        36.0,
        66,
        36.0,
        &format_percent(snapshot.cpu_percent),
        color,
        font,
    );
    draw_text(
        bitmap,
        250.0,
        54,
        24.0,
        &format_cpu_topology(
            snapshot.cpu_logical_cores,
            snapshot.cpu_performance_cores,
            snapshot.cpu_efficiency_cores,
        ),
        palette.secondary_text,
        font,
    );
}

fn draw_memory_row(
    snapshot: &TrayDashboardSnapshot,
    bitmap: &mut TrayDashboardBitmap,
    palette: &DashboardPalette,
    font: &fontdue::Font,
) {
    let percent = memory_status_percent(
        snapshot.memory_pressure_percent,
        snapshot.memory_used_bytes,
        snapshot.memory_total_bytes,
    )
    .unwrap_or(0.0);
    let color = palette.status_color(memory_status(percent));
    draw_metric_label(bitmap, 112, "Memory", palette, font);
    let pressure_percent = snapshot.memory_pressure_percent.unwrap_or(percent);
    let pressure_label = format_percent(pressure_percent);
    draw_text(bitmap, 36.0, 148, 32.0, &pressure_label, color, font);
    draw_text(
        bitmap,
        36.0 + measure_text(&pressure_label, 32.0, font) + 8.0,
        148,
        24.0,
        &format!("({})", format_memory_health(memory_status(percent))),
        color,
        font,
    );
    let usage_label = format!(
        "{} / {}",
        format_bytes_compact(snapshot.memory_used_bytes),
        format_bytes_compact(snapshot.memory_total_bytes)
    );
    draw_text(
        bitmap,
        250.0,
        118,
        26.0,
        &format!("used {usage_label}"),
        palette.secondary_text,
        font,
    );
    draw_text(
        bitmap,
        250.0,
        148,
        24.0,
        &format!(
            "comp {}  cache {}  swap {}",
            format_bytes_compact(snapshot.memory_compressed_bytes),
            format_bytes_compact(snapshot.memory_cached_bytes),
            format_swap(snapshot.swap_used_bytes, snapshot.swap_total_bytes)
        ),
        palette.tertiary_text,
        font,
    );
}

fn draw_disk_row(
    snapshot: &TrayDashboardSnapshot,
    bitmap: &mut TrayDashboardBitmap,
    palette: &DashboardPalette,
    font: &fontdue::Font,
) {
    let percent = snapshot.disk_used_percent.unwrap_or(0.0);
    let color = palette.status_color(disk_status(percent));
    draw_metric_label(bitmap, 194, "Disk", palette, font);
    draw_text(
        bitmap,
        36.0,
        230,
        34.0,
        &snapshot
            .disk_used_percent
            .map(format_percent)
            .unwrap_or_else(|| "--%".to_string()),
        color,
        font,
    );
    draw_text(
        bitmap,
        250.0,
        200,
        26.0,
        &format!(
            "free {} of {}",
            format_optional_bytes(snapshot.disk_free_bytes),
            format_optional_bytes(snapshot.disk_total_bytes)
        ),
        palette.secondary_text,
        font,
    );
    draw_text(
        bitmap,
        250.0,
        230,
        24.0,
        &format!(
            "read {}   write {}",
            format_optional_disk_rate(snapshot.disk_read_bytes_per_sec),
            format_optional_disk_rate(snapshot.disk_write_bytes_per_sec)
        ),
        palette.tertiary_text,
        font,
    );
}

fn draw_network_row(
    snapshot: &TrayDashboardSnapshot,
    bitmap: &mut TrayDashboardBitmap,
    palette: &DashboardPalette,
    font: &fontdue::Font,
) {
    draw_metric_label(bitmap, 276, "Network", palette, font);
    let down_color = palette.network_down;
    let up_color = palette.network_up;
    draw_text(
        bitmap,
        250.0,
        282,
        26.0,
        &format!(
            "up {}",
            format_optional_rate(snapshot.network_up_bytes_per_sec)
        ),
        up_color,
        font,
    );
    draw_text(
        bitmap,
        250.0,
        312,
        26.0,
        &format!(
            "down {}",
            format_optional_rate(snapshot.network_down_bytes_per_sec)
        ),
        down_color,
        font,
    );
}

fn draw_metric_label(
    bitmap: &mut TrayDashboardBitmap,
    baseline_y: i32,
    text: &str,
    palette: &DashboardPalette,
    font: &fontdue::Font,
) {
    draw_text(
        bitmap,
        36.0,
        baseline_y,
        26.0,
        text,
        palette.label_text,
        font,
    );
}

fn draw_text(
    bitmap: &mut TrayDashboardBitmap,
    x: f32,
    baseline_y: i32,
    font_px: f32,
    text: &str,
    color: Color,
    font: &fontdue::Font,
) {
    let mut cursor_x = x;
    for ch in text.chars() {
        let glyph = cached_menu_bar_glyph(font, ch, font_px);
        let metrics = &glyph.metrics;
        let glyph_x = cursor_x.round() as i32 + metrics.xmin;
        let glyph_y = baseline_y - metrics.ymin - metrics.height as i32;
        draw_glyph(
            bitmap,
            glyph_x,
            glyph_y,
            metrics.width,
            &glyph.bitmap,
            color,
        );
        cursor_x += metrics.advance_width;
    }
}

fn measure_text(text: &str, font_px: f32, font: &fontdue::Font) -> f32 {
    text.chars()
        .map(|ch| {
            cached_menu_bar_glyph(font, ch, font_px)
                .metrics
                .advance_width
        })
        .sum()
}

fn draw_glyph(
    bitmap: &mut TrayDashboardBitmap,
    x: i32,
    y: i32,
    glyph_width: usize,
    glyph: &[u8],
    color: Color,
) {
    for (idx, alpha) in glyph.iter().enumerate() {
        if *alpha == 0 {
            continue;
        }
        let px = x + (idx % glyph_width) as i32;
        let py = y + (idx / glyph_width) as i32;
        blend_pixel(bitmap, px, py, color.with_alpha_scaled(*alpha));
    }
}

fn fill_rect(
    bitmap: &mut TrayDashboardBitmap,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: Color,
) {
    for py in y..y.saturating_add(height).min(bitmap.height) {
        for px in x..x.saturating_add(width).min(bitmap.width) {
            blend_pixel(bitmap, px as i32, py as i32, color);
        }
    }
}

fn draw_line(bitmap: &mut TrayDashboardBitmap, x0: u32, y0: u32, x1: u32, y1: u32, color: Color) {
    let mut x0 = x0 as i32;
    let mut y0 = y0 as i32;
    let x1 = x1 as i32;
    let y1 = y1 as i32;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        blend_pixel(bitmap, x0, y0, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn blend_pixel(bitmap: &mut TrayDashboardBitmap, x: i32, y: i32, color: Color) {
    if x < 0 || y < 0 || x as u32 >= bitmap.width || y as u32 >= bitmap.height {
        return;
    }
    let idx = ((y as u32 * bitmap.width + x as u32) * 4) as usize;
    if idx + 3 >= bitmap.rgba.len() {
        return;
    }
    let src_a = color.a as f32 / 255.0;
    let dst_a = bitmap.rgba[idx + 3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 0.0 {
        return;
    }
    for (offset, src) in [color.r, color.g, color.b].iter().enumerate() {
        let dst = bitmap.rgba[idx + offset] as f32 / 255.0;
        let src = *src as f32 / 255.0;
        let out = (src * src_a + dst * dst_a * (1.0 - src_a)) / out_a;
        bitmap.rgba[idx + offset] = (out * 255.0).round() as u8;
    }
    bitmap.rgba[idx + 3] = (out_a * 255.0).round() as u8;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Color {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

impl Color {
    const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    fn with_alpha_scaled(self, glyph_alpha: u8) -> Self {
        Self {
            a: ((self.a as u16 * glyph_alpha as u16) / 255) as u8,
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct DashboardPalette {
    background: Color,
    secondary_text: Color,
    tertiary_text: Color,
    label_text: Color,
    separator: Color,
    good: Color,
    warning: Color,
    critical: Color,
    network_down: Color,
    network_up: Color,
}

impl DashboardPalette {
    fn for_theme(theme: DashboardTheme) -> Self {
        match theme {
            DashboardTheme::Light => Self {
                background: Color::rgba(247, 250, 253, 16),
                secondary_text: Color::rgba(85, 96, 113, 255),
                tertiary_text: Color::rgba(118, 128, 144, 255),
                label_text: Color::rgba(58, 68, 82, 255),
                separator: Color::rgba(0, 0, 0, 24),
                good: Color::rgba(0, 168, 96, 255),
                warning: Color::rgba(209, 137, 0, 255),
                critical: Color::rgba(222, 65, 78, 255),
                network_down: Color::rgba(0, 132, 204, 255),
                network_up: Color::rgba(154, 78, 220, 255),
            },
            DashboardTheme::Dark => Self {
                background: Color::rgba(32, 34, 38, 24),
                secondary_text: Color::rgba(178, 184, 194, 255),
                tertiary_text: Color::rgba(162, 169, 181, 255),
                label_text: Color::rgba(211, 216, 225, 255),
                separator: Color::rgba(255, 255, 255, 30),
                good: Color::rgba(78, 216, 142, 255),
                warning: Color::rgba(244, 178, 64, 255),
                critical: Color::rgba(255, 92, 104, 255),
                network_down: Color::rgba(73, 190, 255, 255),
                network_up: Color::rgba(205, 124, 255, 255),
            },
        }
    }

    fn status_color(&self, status: StatusLevel) -> Color {
        match status {
            StatusLevel::Good => self.good,
            StatusLevel::Warning => self.warning,
            StatusLevel::Critical => self.critical,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusLevel {
    Good,
    Warning,
    Critical,
}

fn cpu_status(percent: f32) -> StatusLevel {
    if percent >= 85.0 {
        StatusLevel::Critical
    } else if percent >= 60.0 {
        StatusLevel::Warning
    } else {
        StatusLevel::Good
    }
}

fn memory_status(percent: f32) -> StatusLevel {
    if percent >= 80.0 {
        StatusLevel::Critical
    } else if percent >= 60.0 {
        StatusLevel::Warning
    } else {
        StatusLevel::Good
    }
}

fn format_memory_health(status: StatusLevel) -> &'static str {
    match status {
        StatusLevel::Good => "Healthy",
        StatusLevel::Warning => "Pressure",
        StatusLevel::Critical => "Critical",
    }
}

fn disk_status(percent: f32) -> StatusLevel {
    if percent >= 90.0 {
        StatusLevel::Critical
    } else if percent >= 75.0 {
        StatusLevel::Warning
    } else {
        StatusLevel::Good
    }
}

fn memory_status_percent(pressure: Option<f32>, used: u64, total: u64) -> Option<f32> {
    pressure.or_else(|| {
        if total == 0 {
            None
        } else {
            Some((used as f32 / total as f32 * 100.0).clamp(0.0, 100.0))
        }
    })
}

fn format_percent(value: f32) -> String {
    if value >= 99.5 {
        "100%".to_string()
    } else {
        format!("{:.0}%", value.max(0.0))
    }
}

fn format_optional_bytes(value: Option<u64>) -> String {
    value
        .map(format_bytes_compact)
        .unwrap_or_else(|| "--".to_string())
}

fn format_optional_rate(value: Option<u64>) -> String {
    value
        .map(|value| format!("{}/s", format_bytes_compact(value)))
        .unwrap_or_else(|| "--".to_string())
}

fn format_optional_disk_rate(value: Option<u64>) -> String {
    value
        .map(|value| format!("{}/s", format_bytes_compact(value)))
        .unwrap_or_else(|| "collecting".to_string())
}

fn format_cpu_topology(
    logical: Option<usize>,
    performance: Option<usize>,
    efficiency: Option<usize>,
) -> String {
    let logical = logical
        .map(|value| value.to_string())
        .unwrap_or_else(|| "--".to_string());
    match (performance, efficiency) {
        (Some(performance), Some(efficiency)) => {
            format!("cores {logical} (P{performance} / E{efficiency})")
        }
        _ => format!("logical cores {logical}"),
    }
}

fn format_swap(used: Option<u64>, total: Option<u64>) -> String {
    match (used, total) {
        (Some(used), Some(total)) if total > 0 => {
            format!(
                "{} / {}",
                format_bytes_compact(used),
                format_bytes_compact(total)
            )
        }
        (Some(used), _) => format_bytes_compact(used),
        _ => "--".to_string(),
    }
}

fn format_bytes_compact(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1}G", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1}M", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1}K", bytes / KIB)
    } else {
        format!("{bytes:.0}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_thresholds_match_design() {
        assert_eq!(cpu_status(59.9), StatusLevel::Good);
        assert_eq!(cpu_status(60.0), StatusLevel::Warning);
        assert_eq!(cpu_status(85.0), StatusLevel::Critical);
        assert_eq!(memory_status(79.9), StatusLevel::Warning);
        assert_eq!(memory_status(80.0), StatusLevel::Critical);
        assert_eq!(disk_status(74.9), StatusLevel::Good);
        assert_eq!(disk_status(90.0), StatusLevel::Critical);
    }

    #[test]
    fn memory_status_falls_back_to_used_percent() {
        assert_eq!(memory_status_percent(Some(42.0), 9, 10), Some(42.0));
        assert_eq!(memory_status_percent(None, 5, 10), Some(50.0));
        assert_eq!(memory_status_percent(None, 5, 0), None);
    }

    #[test]
    fn memory_health_label_names_pressure_status() {
        assert_eq!(format_memory_health(StatusLevel::Good), "Healthy");
        assert_eq!(format_memory_health(StatusLevel::Warning), "Pressure");
        assert_eq!(format_memory_health(StatusLevel::Critical), "Critical");
    }

    #[test]
    fn formatting_handles_rates_and_swap() {
        assert_eq!(format_optional_rate(Some(1536)), "1.5K/s");
        assert_eq!(format_optional_rate(None), "--");
        assert_eq!(format_optional_disk_rate(Some(0)), "0B/s");
        assert_eq!(format_optional_disk_rate(None), "collecting");
        assert_eq!(
            format_swap(Some(512 * 1024 * 1024), Some(2 * 1024 * 1024 * 1024)),
            "512.0M / 2.0G"
        );
        assert_eq!(format_swap(None, None), "--");
    }

    #[test]
    fn cpu_topology_uses_perf_efficiency_cores_when_available() {
        assert_eq!(
            format_cpu_topology(Some(16), Some(12), Some(4)),
            "cores 16 (P12 / E4)"
        );
        assert_eq!(
            format_cpu_topology(Some(10), None, None),
            "logical cores 10"
        );
        assert_eq!(format_cpu_topology(None, None, None), "logical cores --");
    }

    #[test]
    fn render_dashboard_returns_non_empty_bitmap_when_font_exists() {
        let Some(font) = menu_bar_stats_font() else {
            return;
        };
        let _ = font;
        let snapshot = TrayDashboardSnapshot {
            service_state: ServiceState::Running,
            runtime_label: "127.0.0.1:8800".to_string(),
            system_proxy_label: "System proxy Off".to_string(),
            cpu_percent: 23.0,
            cpu_logical_cores: Some(10),
            cpu_performance_cores: Some(6),
            cpu_efficiency_cores: Some(4),
            load_one: 1.2,
            load_five: 1.5,
            load_fifteen: 1.8,
            memory_used_bytes: 18 * 1024 * 1024 * 1024,
            memory_total_bytes: 32 * 1024 * 1024 * 1024,
            memory_pressure_percent: Some(42.0),
            memory_compressed_bytes: 1_200_000_000,
            memory_cached_bytes: 4_800_000_000,
            swap_used_bytes: Some(512 * 1024 * 1024),
            swap_total_bytes: Some(2 * 1024 * 1024 * 1024),
            disk_used_percent: Some(59.0),
            disk_free_bytes: Some(410 * 1024 * 1024 * 1024),
            disk_total_bytes: Some(1_000 * 1024 * 1024 * 1024),
            disk_mount_label: Some("/".to_string()),
            disk_total_bytes_per_sec: Some(24 * 1024 * 1024),
            disk_read_bytes_per_sec: Some(10 * 1024 * 1024),
            disk_write_bytes_per_sec: Some(14 * 1024 * 1024),
            network_up_bytes_per_sec: Some(1_200_000),
            network_down_bytes_per_sec: Some(512_000),
            network_interface_label: Some("en0".to_string()),
        };

        let bitmap =
            render_dashboard_with_theme(&snapshot, DashboardTheme::Dark).expect("dashboard bitmap");
        assert_eq!(bitmap.width, DASHBOARD_WIDTH);
        assert_eq!(bitmap.height, DASHBOARD_HEIGHT);
        assert!(bitmap.rgba.iter().any(|value| *value != 0));
    }
}
