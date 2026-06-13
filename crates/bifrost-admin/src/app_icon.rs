use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};
#[cfg(target_os = "macos")]
use tracing::trace;
use tracing::{debug, info, warn};

const CACHE_VERSION: u32 = 2;
const MEMORY_CACHE_MAX_ENTRIES: usize = 256;
const MEMORY_CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;
const MEMORY_CACHE_MAX_SINGLE_ICON_BYTES: usize = 2 * 1024 * 1024;
#[cfg(target_os = "macos")]
const APP_ICON_WORKER_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
struct MemoryIconEntry {
    data: Option<Arc<[u8]>>,
    bytes: usize,
}

#[derive(Debug)]
struct AppIconMemoryCache {
    entries: HashMap<String, MemoryIconEntry>,
    order: VecDeque<String>,
    max_entries: usize,
    max_bytes: usize,
    max_single_icon_bytes: usize,
    current_bytes: usize,
}

impl AppIconMemoryCache {
    fn new() -> Self {
        Self::with_limits(
            MEMORY_CACHE_MAX_ENTRIES,
            MEMORY_CACHE_MAX_BYTES,
            MEMORY_CACHE_MAX_SINGLE_ICON_BYTES,
        )
    }

    fn with_limits(max_entries: usize, max_bytes: usize, max_single_icon_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            max_entries,
            max_bytes,
            max_single_icon_bytes,
            current_bytes: 0,
        }
    }

    fn get(&mut self, cache_key: &str) -> Option<Option<Arc<[u8]>>> {
        let data = self
            .entries
            .get(cache_key)
            .map(|entry| entry.data.clone())?;
        self.touch(cache_key);
        Some(data)
    }

    fn insert(&mut self, cache_key: String, data: Option<Vec<u8>>) {
        self.remove_existing(&cache_key);

        let bytes = data.as_ref().map_or(0, Vec::len);
        if bytes > self.max_single_icon_bytes {
            debug!(
                cache_key = cache_key,
                bytes = bytes,
                max_single_icon_bytes = self.max_single_icon_bytes,
                "Skipping oversized app icon memory cache entry"
            );
            return;
        }

        let data = data.map(|value| Arc::<[u8]>::from(value.into_boxed_slice()));
        self.current_bytes += bytes;
        self.entries
            .insert(cache_key.clone(), MemoryIconEntry { data, bytes });
        self.order.push_back(cache_key);
        self.evict_if_needed();
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.current_bytes = 0;
    }

    fn remove_existing(&mut self, cache_key: &str) {
        if let Some(entry) = self.entries.remove(cache_key) {
            self.current_bytes = self.current_bytes.saturating_sub(entry.bytes);
        }
        self.order.retain(|key| key != cache_key);
    }

    fn touch(&mut self, cache_key: &str) {
        self.order.retain(|key| key != cache_key);
        self.order.push_back(cache_key.to_string());
    }

    fn evict_if_needed(&mut self) {
        while self.entries.len() > self.max_entries || self.current_bytes > self.max_bytes {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.current_bytes = self.current_bytes.saturating_sub(entry.bytes);
            }
        }
    }
}

pub struct AppIconCache {
    cache_dir: PathBuf,
    memory_cache: RwLock<AppIconMemoryCache>,
    extract_lock: Mutex<()>,
}

impl AppIconCache {
    pub fn new(data_dir: &Path) -> Self {
        let cache_dir = data_dir.join("app_info");
        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            warn!(error = %e, "Failed to create app_info cache directory");
        }

        let cache = Self {
            cache_dir,
            memory_cache: RwLock::new(AppIconMemoryCache::new()),
            extract_lock: Mutex::new(()),
        };

        cache.check_and_migrate_cache();

        cache
    }

    fn check_and_migrate_cache(&self) {
        let version_file = self.cache_dir.join(".cache_version");

        let current_version = std::fs::read_to_string(&version_file)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);

        if current_version < CACHE_VERSION {
            info!(
                old_version = current_version,
                new_version = CACHE_VERSION,
                "Cache version mismatch, clearing old cache"
            );
            self.clear_all_disk_cache();

            if let Err(e) = std::fs::write(&version_file, CACHE_VERSION.to_string()) {
                warn!(error = %e, "Failed to write cache version file");
            }
        }
    }

    fn clear_all_disk_cache(&self) {
        if let Ok(entries) = std::fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().map(|e| e == "png").unwrap_or(false) {
                    if let Err(e) = std::fs::remove_file(&path) {
                        warn!(error = %e, path = %path.display(), "Failed to remove old cache file");
                    }
                }
            }
        }
        info!("Old icon cache cleared");
    }

    pub fn get_icon(&self, app_name: &str, app_path: Option<&str>) -> Option<Vec<u8>> {
        let cache_key = sanitize_app_name(app_name);

        if let Some(cached) = self.get_from_memory(&cache_key) {
            return cached;
        }

        if let Some(cached) = self.get_from_disk(&cache_key) {
            self.set_memory_cache(&cache_key, Some(cached.clone()));
            return Some(cached);
        }

        if let Some(path) = app_path {
            let _extract_guard = self.extract_lock.lock();

            if let Some(cached) = self.get_from_memory(&cache_key) {
                return cached;
            }

            if let Some(cached) = self.get_from_disk(&cache_key) {
                self.set_memory_cache(&cache_key, Some(cached.clone()));
                return Some(cached);
            }

            if let Some(icon_data) = extract_app_icon(path) {
                self.save_to_disk(&cache_key, &icon_data);
                self.set_memory_cache(&cache_key, Some(icon_data.clone()));
                return Some(icon_data);
            }
        }

        self.set_memory_cache(&cache_key, None);
        None
    }

    fn get_from_memory(&self, cache_key: &str) -> Option<Option<Vec<u8>>> {
        let mut cache = self.memory_cache.write();
        cache
            .get(cache_key)
            .map(|entry| entry.map(|data| data.as_ref().to_vec()))
    }

    fn set_memory_cache(&self, cache_key: &str, data: Option<Vec<u8>>) {
        let mut cache = self.memory_cache.write();
        cache.insert(cache_key.to_string(), data);
    }

    fn get_from_disk(&self, cache_key: &str) -> Option<Vec<u8>> {
        let file_path = self.cache_dir.join(format!("{}.png", cache_key));
        std::fs::read(&file_path).ok()
    }

    fn save_to_disk(&self, cache_key: &str, data: &[u8]) {
        let file_path = self.cache_dir.join(format!("{}.png", cache_key));
        if let Err(e) = std::fs::write(&file_path, data) {
            warn!(error = %e, cache_key = cache_key, "Failed to save icon to disk");
        } else {
            debug!(cache_key = cache_key, "Saved app icon to disk cache");
        }
    }

    pub fn clear_cache(&self) {
        let mut cache = self.memory_cache.write();
        cache.clear();

        self.clear_all_disk_cache();
    }
}

fn sanitize_app_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn extract_app_icon(app_path: &str) -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        extract_app_icon_macos(app_path)
    }

    #[cfg(target_os = "windows")]
    {
        extract_app_icon_windows(app_path)
    }

    #[cfg(target_os = "linux")]
    {
        extract_app_icon_linux(app_path)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = app_path;
        None
    }
}

#[cfg(target_os = "macos")]
fn extract_app_icon_macos(app_path: &str) -> Option<Vec<u8>> {
    info!(app_path = %app_path, "Extracting app icon from macOS");

    let Some(icon_path) = resolve_macos_icon_path(app_path) else {
        debug!(
            app_path = %app_path,
            "Skipping macOS app icon extraction for path without app bundle"
        );
        return None;
    };

    if let Some(icon_data) = extract_icon_via_icns(&icon_path) {
        debug!(size = icon_data.len(), "Got icon via icns");
        return Some(icon_data);
    }

    debug!(
        icon_path = %icon_path.display(),
        "icns extraction failed, falling back to isolated NSWorkspace helper"
    );
    extract_icon_via_worker(&icon_path)
}

#[cfg(target_os = "macos")]
fn resolve_macos_icon_path(app_path: &str) -> Option<PathBuf> {
    let path = Path::new(app_path);
    let app_bundle = get_toplevel_app_bundle(path)?;
    app_bundle.exists().then_some(app_bundle)
}

#[cfg(target_os = "macos")]
fn extract_icon_via_nsworkspace(app_path: &Path) -> Option<Vec<u8>> {
    use objc2::rc::{autoreleasepool, Retained};
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSWorkspace};
    use objc2_foundation::{NSDictionary, NSRange, NSSize, NSString};

    autoreleasepool(|_| {
        debug!(
            icon_path = %app_path.display(),
            "Using NSWorkspace for icon extraction"
        );

        let path_str = NSString::from_str(&app_path.to_string_lossy());
        let workspace = NSWorkspace::sharedWorkspace();
        let icon: Retained<NSImage> = workspace.iconForFile(&path_str);

        let size = icon.size();
        if size.width < 1.0 || size.height < 1.0 {
            warn!("Icon has invalid size");
            return None;
        }

        let target_size = 64.0f64;
        icon.setSize(NSSize::new(target_size, target_size));

        let tiff_data = icon.TIFFRepresentation()?;
        let bitmap_rep = NSBitmapImageRep::imageRepWithData(&tiff_data)?;

        let empty_dict: Retained<NSDictionary<NSString, objc2::runtime::AnyObject>> =
            NSDictionary::new();
        let png_data = unsafe {
            bitmap_rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &empty_dict)
        }?;

        let len = png_data.len();
        if len == 0 {
            warn!("PNG data is empty");
            return None;
        }

        let mut result = vec![0u8; len];
        let range = NSRange::new(0, len);
        unsafe {
            let ptr = std::ptr::NonNull::new(result.as_mut_ptr().cast()).unwrap();
            png_data.getBytes_range(ptr, range);
        }
        Some(result)
    })
}

#[cfg(target_os = "macos")]
fn get_toplevel_app_bundle(path: &Path) -> Option<PathBuf> {
    let mut toplevel_app: Option<PathBuf> = None;

    for ancestor in path.ancestors() {
        if ancestor.extension().map(|e| e == "app").unwrap_or(false) {
            toplevel_app = Some(ancestor.to_path_buf());
        }
    }

    toplevel_app
}

#[cfg(target_os = "macos")]
fn extract_icon_via_worker(app_path: &Path) -> Option<Vec<u8>> {
    let exe = std::env::current_exe().ok()?;
    let output_file = tempfile::Builder::new()
        .prefix("bifrost-app-icon-")
        .suffix(".png")
        .tempfile()
        .ok()?;
    let output_path = output_file.path().to_path_buf();

    let mut child = Command::new(exe)
        .arg("app-icon-worker")
        .arg("--path")
        .arg(app_path)
        .arg("--output")
        .arg(&output_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let started_at = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    debug!(
                        status = ?status.code(),
                        app_path = %app_path.display(),
                        "App icon worker failed"
                    );
                    return None;
                }
                break;
            }
            Ok(None) if started_at.elapsed() <= APP_ICON_WORKER_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                warn!(
                    app_path = %app_path.display(),
                    timeout_ms = APP_ICON_WORKER_TIMEOUT.as_millis(),
                    "App icon worker timed out"
                );
                return None;
            }
            Err(e) => {
                warn!(error = %e, app_path = %app_path.display(), "Failed to poll app icon worker");
                return None;
            }
        }
    }

    let metadata = std::fs::metadata(&output_path).ok()?;
    if metadata.len() == 0 || metadata.len() > MEMORY_CACHE_MAX_SINGLE_ICON_BYTES as u64 {
        warn!(
            app_path = %app_path.display(),
            bytes = metadata.len(),
            max_bytes = MEMORY_CACHE_MAX_SINGLE_ICON_BYTES,
            "App icon worker output size is invalid"
        );
        return None;
    }

    std::fs::read(output_path).ok()
}

#[cfg(target_os = "macos")]
fn extract_icon_via_icns(app_path: &Path) -> Option<Vec<u8>> {
    let app_bundle = find_app_bundle_macos(app_path)?;

    info!(app_bundle = %app_bundle.display(), "Found app bundle");

    let info_plist_path = app_bundle.join("Contents/Info.plist");

    let plist_value: plist::Value = plist::from_file(&info_plist_path).ok()?;
    let dict = plist_value.as_dictionary()?;

    let icon_file = dict
        .get("CFBundleIconFile")
        .or_else(|| dict.get("CFBundleIconName"))
        .and_then(|v| v.as_string())?;

    let icon_name = if icon_file.ends_with(".icns") {
        icon_file.to_string()
    } else {
        format!("{}.icns", icon_file)
    };

    let icon_path = app_bundle.join("Contents/Resources").join(&icon_name);

    if !icon_path.exists() {
        warn!(icon_path = %icon_path.display(), "Icon file not found");
        return None;
    }

    let file = std::fs::File::open(&icon_path).ok()?;
    let icon_family = icns::IconFamily::read(file).ok()?;

    let icon_types = [
        icns::IconType::RGBA32_32x32_2x,
        icns::IconType::RGBA32_32x32,
        icns::IconType::RGBA32_64x64,
        icns::IconType::RGBA32_128x128,
        icns::IconType::RGBA32_16x16_2x,
        icns::IconType::RGBA32_16x16,
    ];

    for icon_type in icon_types {
        if let Ok(image) = icon_family.get_icon_with_type(icon_type) {
            let mut png_data = Vec::new();
            if image.write_png(&mut png_data).is_ok() {
                trace!(
                    icon_type = ?icon_type,
                    size = png_data.len(),
                    "Extracted icon from icns"
                );
                return Some(png_data);
            }
        }
    }

    warn!(icon_path = %icon_path.display(), "No suitable icon found in icns file");
    None
}

#[cfg(target_os = "macos")]
fn find_app_bundle_macos(path: &Path) -> Option<PathBuf> {
    let mut found_bundles: Vec<PathBuf> = Vec::new();

    for ancestor in path.ancestors() {
        if ancestor.extension().map(|e| e == "app").unwrap_or(false) {
            found_bundles.push(ancestor.to_path_buf());
        }
    }

    found_bundles.reverse();

    for bundle in &found_bundles {
        let info_plist = bundle.join("Contents/Info.plist");
        if info_plist.exists() {
            if let Ok(plist_value) = plist::from_file::<_, plist::Value>(&info_plist) {
                if let Some(dict) = plist_value.as_dictionary() {
                    if dict.get("CFBundleIconFile").is_some()
                        || dict.get("CFBundleIconName").is_some()
                    {
                        return Some(bundle.clone());
                    }
                }
            }
        }
    }

    found_bundles.first().cloned()
}

#[cfg(target_os = "windows")]
fn extract_app_icon_windows(app_path: &str) -> Option<Vec<u8>> {
    info!(app_path = %app_path, "Extracting app icon from Windows executable");

    let path = Path::new(app_path);
    let exe_path = find_executable_windows(path)?;

    debug!(exe_path = %exe_path.display(), "Found executable");

    let file_data = std::fs::read(&exe_path).ok()?;

    let pe_file = pelite::PeFile::from_bytes(&file_data).ok()?;

    let resources = pe_file.resources().ok()?;

    for (_name, group) in resources.icons().flatten() {
        for entry in group.entries() {
            if let Ok(image_data) = group.image(entry.nId) {
                if image_data.starts_with(b"\x89PNG") {
                    debug!(size = image_data.len(), "Found PNG icon in PE resources");
                    return Some(image_data.to_vec());
                }

                if let Some(png_data) = convert_ico_to_png(image_data) {
                    debug!(
                        original_size = image_data.len(),
                        png_size = png_data.len(),
                        "Converted ICO to PNG"
                    );
                    return Some(png_data);
                }
            }
        }
    }

    warn!(exe_path = %exe_path.display(), "No suitable icon found in executable");
    None
}

#[cfg(target_os = "windows")]
fn find_executable_windows(path: &Path) -> Option<PathBuf> {
    if path.extension().map(|e| e == "exe").unwrap_or(false) && path.exists() {
        return Some(path.to_path_buf());
    }

    for ancestor in path.ancestors() {
        if ancestor.extension().map(|e| e == "exe").unwrap_or(false) && ancestor.exists() {
            return Some(ancestor.to_path_buf());
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn convert_ico_to_png(ico_data: &[u8]) -> Option<Vec<u8>> {
    use image::ImageFormat;
    use std::io::Cursor;

    let cursor = Cursor::new(ico_data);

    if let Ok(img) = image::load(cursor, ImageFormat::Ico) {
        let mut png_data = Vec::new();
        let mut cursor = Cursor::new(&mut png_data);
        if img.write_to(&mut cursor, ImageFormat::Png).is_ok() {
            return Some(png_data);
        }
    }

    if ico_data.len() >= 40 {
        let width = ico_data.get(4).copied().unwrap_or(0) as u32;
        let height = ico_data.get(8).copied().unwrap_or(0) as u32;
        let bit_count = u16::from_le_bytes([
            ico_data.get(14).copied().unwrap_or(0),
            ico_data.get(15).copied().unwrap_or(0),
        ]);

        if width > 0 && height > 0 && bit_count == 32 {
            let header_size = 40;
            let pixel_data = &ico_data[header_size..];
            let actual_height = height / 2;

            if pixel_data.len() >= (width * actual_height * 4) as usize {
                let mut rgba_data = Vec::with_capacity((width * actual_height * 4) as usize);

                for y in (0..actual_height).rev() {
                    let row_start = (y * width * 4) as usize;
                    let row_end = row_start + (width * 4) as usize;
                    if row_end <= pixel_data.len() {
                        for x in 0..width as usize {
                            let idx = row_start + x * 4;
                            let b = pixel_data[idx];
                            let g = pixel_data[idx + 1];
                            let r = pixel_data[idx + 2];
                            let a = pixel_data[idx + 3];
                            rgba_data.extend_from_slice(&[r, g, b, a]);
                        }
                    }
                }

                if let Some(img) = image::RgbaImage::from_raw(width, actual_height, rgba_data) {
                    let mut png_data = Vec::new();
                    let mut cursor = Cursor::new(&mut png_data);
                    if image::DynamicImage::ImageRgba8(img)
                        .write_to(&mut cursor, image::ImageFormat::Png)
                        .is_ok()
                    {
                        return Some(png_data);
                    }
                }
            }
        }
    }

    None
}

#[cfg(target_os = "linux")]
fn extract_app_icon_linux(app_path: &str) -> Option<Vec<u8>> {
    info!(app_path = %app_path, "Extracting app icon on Linux");

    let path = Path::new(app_path);
    let app_name = path.file_name()?.to_str()?;

    let app_name_lower = app_name.to_lowercase();
    let app_name_normalized = app_name_lower.replace([' ', '_'], "-");

    let icon_names = [
        app_name_normalized.clone(),
        app_name_lower.clone(),
        app_name.to_string(),
    ];

    let icon_dirs = get_linux_icon_dirs();

    let sizes = ["256x256", "128x128", "64x64", "48x48", "32x32", "scalable"];
    let themes = ["hicolor", "Adwaita", "breeze", "gnome", "Papirus"];

    for icon_name in &icon_names {
        for dir in &icon_dirs {
            for theme in &themes {
                for size in &sizes {
                    let icon_path = dir
                        .join(theme)
                        .join(size)
                        .join("apps")
                        .join(format!("{}.png", icon_name));
                    if icon_path.exists() {
                        if let Ok(data) = std::fs::read(&icon_path) {
                            debug!(icon_path = %icon_path.display(), "Found PNG icon");
                            return Some(data);
                        }
                    }

                    if *size == "scalable" {
                        let svg_path = dir
                            .join(theme)
                            .join(size)
                            .join("apps")
                            .join(format!("{}.svg", icon_name));
                        if svg_path.exists() {
                            debug!(svg_path = %svg_path.display(), "Found SVG icon (not converting)");
                        }
                    }
                }
            }
        }
    }

    for icon_name in &icon_names {
        let pixmaps_path = PathBuf::from("/usr/share/pixmaps").join(format!("{}.png", icon_name));
        if pixmaps_path.exists() {
            if let Ok(data) = std::fs::read(&pixmaps_path) {
                debug!(pixmaps_path = %pixmaps_path.display(), "Found icon in pixmaps");
                return Some(data);
            }
        }

        let pixmaps_xpm = PathBuf::from("/usr/share/pixmaps").join(format!("{}.xpm", icon_name));
        if pixmaps_xpm.exists() {
            debug!(pixmaps_xpm = %pixmaps_xpm.display(), "Found XPM icon (not converting)");
        }
    }

    for icon_name in &icon_names {
        if let Some(data) = search_desktop_file_for_icon(icon_name) {
            return Some(data);
        }
    }

    warn!(app_path = %app_path, "No icon found for Linux application");
    None
}

#[cfg(target_os = "linux")]
fn get_linux_icon_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/icons"),
        PathBuf::from("/usr/local/share/icons"),
    ];

    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".local/share/icons"));
        dirs.push(PathBuf::from(&home).join(".icons"));
    }

    if let Ok(xdg_data_dirs) = std::env::var("XDG_DATA_DIRS") {
        for dir in xdg_data_dirs.split(':') {
            dirs.push(PathBuf::from(dir).join("icons"));
        }
    }

    dirs
}

#[cfg(target_os = "linux")]
fn search_desktop_file_for_icon(app_name: &str) -> Option<Vec<u8>> {
    let desktop_dirs = [
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];

    let home_desktop = std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".local/share/applications"));

    for dir in desktop_dirs.iter().chain(home_desktop.iter()) {
        let desktop_file = dir.join(format!("{}.desktop", app_name));
        if desktop_file.exists() {
            if let Ok(content) = std::fs::read_to_string(&desktop_file) {
                for line in content.lines() {
                    if line.starts_with("Icon=") {
                        let icon_name = line.trim_start_matches("Icon=").trim();

                        if Path::new(icon_name).is_absolute() && Path::new(icon_name).exists() {
                            if let Ok(data) = std::fs::read(icon_name) {
                                return Some(data);
                            }
                        }

                        return find_icon_by_name_linux(icon_name);
                    }
                }
            }
        }
    }

    None
}

#[cfg(target_os = "linux")]
fn find_icon_by_name_linux(icon_name: &str) -> Option<Vec<u8>> {
    let icon_dirs = get_linux_icon_dirs();
    let sizes = ["256x256", "128x128", "64x64", "48x48", "32x32"];
    let themes = ["hicolor", "Adwaita", "breeze", "gnome", "Papirus"];

    for dir in &icon_dirs {
        for theme in &themes {
            for size in &sizes {
                let icon_path = dir
                    .join(theme)
                    .join(size)
                    .join("apps")
                    .join(format!("{}.png", icon_name));
                if icon_path.exists() {
                    if let Ok(data) = std::fs::read(&icon_path) {
                        return Some(data);
                    }
                }
            }
        }
    }

    None
}

pub type SharedAppIconCache = Arc<AppIconCache>;

pub fn create_app_icon_cache(data_dir: &Path) -> SharedAppIconCache {
    Arc::new(AppIconCache::new(data_dir))
}

pub fn run_app_icon_worker(path: &Path, output: &Path) -> Result<(), String> {
    let data = extract_app_icon_worker_in_process(path)
        .ok_or_else(|| format!("failed to extract app icon for {}", path.display()))?;

    if data.is_empty() || data.len() > MEMORY_CACHE_MAX_SINGLE_ICON_BYTES {
        return Err(format!(
            "invalid app icon size: {} bytes (max {})",
            data.len(),
            MEMORY_CACHE_MAX_SINGLE_ICON_BYTES
        ));
    }

    let mut file = std::fs::File::create(output)
        .map_err(|e| format!("failed to create output {}: {e}", output.display()))?;
    file.write_all(&data)
        .map_err(|e| format!("failed to write output {}: {e}", output.display()))?;
    Ok(())
}

fn extract_app_icon_worker_in_process(path: &Path) -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        // The worker is only spawned after the main process already failed the
        // pure-file `.icns` path on this same bundle, so retrying it here would
        // just repeat a guaranteed-failing plist parse + file open. Go straight
        // to NSWorkspace, which is the whole reason this isolated worker exists.
        extract_icon_via_nsworkspace(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        extract_app_icon(&path.to_string_lossy())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_cache_evicts_by_total_bytes() {
        let mut cache = AppIconMemoryCache::with_limits(10, 3, 10);

        cache.insert("a".to_string(), Some(vec![1]));
        cache.insert("b".to_string(), Some(vec![2, 2]));
        cache.insert("c".to_string(), Some(vec![3, 3]));

        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_none());
        assert_eq!(
            cache.get("c").and_then(|data| data).map(|data| data.len()),
            Some(2)
        );
        assert_eq!(cache.current_bytes, 2);
    }

    #[test]
    fn memory_cache_keeps_negative_entries_without_byte_cost() {
        let mut cache = AppIconMemoryCache::with_limits(10, 1, 10);

        cache.insert("missing".to_string(), None);

        assert!(matches!(cache.get("missing"), Some(None)));
        assert_eq!(cache.current_bytes, 0);
    }

    #[test]
    fn memory_cache_skips_oversized_single_icon() {
        let mut cache = AppIconMemoryCache::with_limits(10, 100, 2);

        cache.insert("big".to_string(), Some(vec![1, 2, 3]));

        assert!(cache.get("big").is_none());
        assert_eq!(cache.current_bytes, 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_toplevel_app_bundle_normalizes_inner_executable() {
        let path = Path::new("/Applications/Foo.app/Contents/MacOS/Foo");

        assert_eq!(
            get_toplevel_app_bundle(path),
            Some(PathBuf::from("/Applications/Foo.app"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_icon_path_rejects_non_app_process_paths() {
        let path = "/tmp/bifrost/target/debug/deps/some-test-binary";

        assert!(resolve_macos_icon_path(path).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_icon_path_accepts_existing_app_bundle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let app_path = temp.path().join("Sample.app");
        let executable_path = app_path.join("Contents/MacOS/Sample");
        std::fs::create_dir_all(executable_path.parent().unwrap()).expect("create app dirs");
        std::fs::write(&executable_path, b"fake").expect("write fake executable");

        assert_eq!(
            resolve_macos_icon_path(&executable_path.to_string_lossy()),
            Some(app_path)
        );
    }
}
