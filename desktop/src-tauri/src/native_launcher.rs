use tauri::window::Window;

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSColor, NSFont, NSTextAlignment, NSTextField, NSView,
        NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
        NSVisualEffectView,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use objc2_quartz_core::CALayer;
    use tauri::window::Window;

    const OVERLAY_BASE_ALPHA: f64 = 0.98;
    const INITIAL_VISIBLE_PROGRESS: f64 = 0.21;
    const ONE_SECOND_VISIBLE_PROGRESS: f64 = 0.80;
    const FINAL_VIRTUAL_PROGRESS: f64 = 0.99;
    const VIRTUAL_STAGE_ONE_SECONDS: f64 = 1.0;
    const VIRTUAL_STAGE_TWO_SECONDS: f64 = 1.5;
    const HANDOFF_PROGRESS_RANGE: f64 = 1.0 - FINAL_VIRTUAL_PROGRESS;
    const PROGRESS_BAR_WIDTH: f64 = 188.0;
    const PROGRESS_BAR_HEIGHT: f64 = 4.0;
    const PROGRESS_HIGHLIGHT_WIDTH: f64 = 34.0;
    const TITLE_MAX_ALPHA: f64 = 0.94;
    const ANIMATION_FRAME_DELAY: Duration = Duration::from_millis(54);

    struct LauncherOverlayHandle {
        overlay: Retained<NSView>,
        backdrop: Retained<NSVisualEffectView>,
        title: Retained<NSTextField>,
        progress_track: Retained<NSView>,
        progress_fill: Retained<NSView>,
        progress_highlight: Retained<NSView>,
        current_progress: Mutex<f64>,
        animation_running: Arc<AtomicBool>,
        animation_thread: Mutex<Option<JoinHandle<()>>>,
    }

    struct OverlayLayout {
        title_frame: NSRect,
        progress_track_frame: NSRect,
        progress_fill_frame: NSRect,
        progress_highlight_frame: NSRect,
    }

    pub fn install(window: &Window) -> tauri::Result<Option<usize>> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            crate::anyhow("native launcher must be installed on the main thread".to_string())
        })?;
        let content_view = content_view(window)?;
        let bounds = content_view.bounds();
        let overlay = NSView::initWithFrame(NSView::alloc(mtm), bounds);
        overlay.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        overlay.setAlphaValue(1.0);
        overlay.setWantsLayer(true);
        if let Some(layer) = overlay.layer() {
            layer.setBackgroundColor(Some(&NSColor::clearColor().CGColor()));
        }

        let backdrop = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), bounds);
        backdrop.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        backdrop.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
        backdrop.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        backdrop.setState(NSVisualEffectState::Active);
        backdrop.setWantsLayer(true);
        let palette = launcher_palette(window);
        if let Some(layer) = backdrop.layer() {
            let tint = color_from_rgba(palette.backdrop_tint);
            layer.setBackgroundColor(Some(&tint.CGColor()));
        }
        overlay.addSubview(&backdrop);

        let progress_track = create_progress_view(mtm, color_from_rgba(palette.track))?;
        overlay.addSubview(&progress_track);

        let title = NSTextField::labelWithString(&NSString::from_str("Bifrost"), mtm);
        let title_color = color_from_rgba(palette.title);
        title.setTextColor(Some(&title_color));
        title.setAlignment(NSTextAlignment::Center);
        title.setFont(Some(&NSFont::systemFontOfSize_weight(21.0, 0.38)));
        overlay.addSubview(&title);

        let progress_fill = create_progress_view(mtm, color_from_rgba(palette.fill))?;
        overlay.addSubview(&progress_fill);

        let progress_highlight = create_progress_view(mtm, color_from_rgba(palette.highlight))?;
        overlay.addSubview(&progress_highlight);
        content_view.addSubview(&overlay);

        let handle = Box::new(LauncherOverlayHandle {
            overlay,
            backdrop,
            title,
            progress_track,
            progress_fill,
            progress_highlight,
            current_progress: Mutex::new(INITIAL_VISIBLE_PROGRESS),
            animation_running: Arc::new(AtomicBool::new(true)),
            animation_thread: Mutex::new(None),
        });

        apply_visible_progress(handle.as_ref(), INITIAL_VISIBLE_PROGRESS);
        apply_tick_effects(handle.as_ref(), 0);

        Ok(Some(Box::into_raw(handle) as usize))
    }

    pub fn start_animation(window: &Window, overlay_ptr: usize) -> tauri::Result<()> {
        let handle = unsafe { &mut *(overlay_ptr as *mut LauncherOverlayHandle) };
        let Ok(mut animation_thread) = handle.animation_thread.lock() else {
            return Ok(());
        };
        if animation_thread.is_some() {
            return Ok(());
        }

        let window = window.clone();
        let running = handle.animation_running.clone();
        *animation_thread = Some(thread::spawn(move || {
            let mut tick = 0_u64;
            while running.load(Ordering::Relaxed) {
                let window_for_tick = window.clone();
                let _ = window.run_on_main_thread(move || {
                    let _ = tick_overlay(&window_for_tick, overlay_ptr, tick);
                });
                thread::sleep(ANIMATION_FRAME_DELAY);
                tick = tick.wrapping_add(1);
            }
        }));

        Ok(())
    }

    pub fn set_overlay_alpha(window: &Window, overlay_ptr: usize, alpha: f64) -> tauri::Result<()> {
        let handle = unsafe { &*(overlay_ptr as *mut LauncherOverlayHandle) };
        sync_overlay_frame(window, handle)?;
        handle
            .overlay
            .setAlphaValue(alpha.clamp(0.0, 1.0) * OVERLAY_BASE_ALPHA);
        Ok(())
    }

    pub fn set_overlay_progress(
        window: &Window,
        overlay_ptr: usize,
        progress: f64,
    ) -> tauri::Result<()> {
        let handle = unsafe { &*(overlay_ptr as *mut LauncherOverlayHandle) };
        sync_overlay_frame(window, handle)?;
        let visible_progress =
            FINAL_VIRTUAL_PROGRESS + progress.clamp(0.0, 1.0) * HANDOFF_PROGRESS_RANGE;
        apply_visible_progress(handle, visible_progress);
        Ok(())
    }

    pub fn tick_overlay(window: &Window, overlay_ptr: usize, tick: u64) -> tauri::Result<()> {
        let handle = unsafe { &*(overlay_ptr as *mut LauncherOverlayHandle) };
        sync_overlay_frame(window, handle)?;
        let virtual_progress = virtual_progress_for_tick(tick);
        let current_progress = handle
            .current_progress
            .lock()
            .map(|current_progress| *current_progress)
            .unwrap_or(INITIAL_VISIBLE_PROGRESS);
        apply_visible_progress(handle, current_progress.max(virtual_progress));
        apply_tick_effects(handle, tick);
        Ok(())
    }

    pub fn remove_overlay(window: &Window, overlay_ptr: usize) -> tauri::Result<()> {
        let handle = unsafe { &*(overlay_ptr as *mut LauncherOverlayHandle) };
        let _ = sync_overlay_frame(window, handle);
        handle.animation_running.store(false, Ordering::Relaxed);
        if let Ok(mut animation_thread) = handle.animation_thread.lock() {
            // The animation thread may already have queued a main-thread tick with
            // this raw pointer. Detach it and keep the handle alive for the app
            // lifetime so late ticks cannot touch freed Objective-C objects.
            let _ = animation_thread.take();
        }
        handle.overlay.removeFromSuperview();
        Ok(())
    }

    struct LauncherPalette {
        backdrop_tint: (f64, f64, f64, f64),
        title: (f64, f64, f64, f64),
        track: (f64, f64, f64, f64),
        fill: (f64, f64, f64, f64),
        highlight: (f64, f64, f64, f64),
    }

    fn launcher_palette(window: &Window) -> LauncherPalette {
        let is_dark = matches!(window.theme().ok(), Some(tauri::Theme::Dark));
        if is_dark {
            LauncherPalette {
                backdrop_tint: (0.03, 0.07, 0.10, 0.18),
                title: (0.97, 0.98, 0.99, 0.96),
                track: (1.0, 1.0, 1.0, 0.30),
                fill: (0.97, 0.98, 0.99, 0.98),
                highlight: (1.0, 1.0, 1.0, 0.56),
            }
        } else {
            LauncherPalette {
                backdrop_tint: (0.97, 0.98, 0.98, 0.16),
                title: (0.10, 0.13, 0.16, 0.90),
                track: (0.10, 0.13, 0.16, 0.24),
                fill: (0.10, 0.13, 0.16, 0.78),
                highlight: (0.10, 0.13, 0.16, 0.36),
            }
        }
    }

    fn color_from_rgba(rgba: (f64, f64, f64, f64)) -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(rgba.0, rgba.1, rgba.2, rgba.3)
    }

    fn sync_overlay_frame(window: &Window, handle: &LauncherOverlayHandle) -> tauri::Result<()> {
        let content_view = content_view(window)?;
        handle.overlay.setFrame(content_view.bounds());
        Ok(())
    }

    fn create_progress_view(
        mtm: MainThreadMarker,
        color: Retained<NSColor>,
    ) -> tauri::Result<Retained<NSView>> {
        let view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(10.0, PROGRESS_BAR_HEIGHT),
            ),
        );
        view.setWantsLayer(true);
        let layer: Retained<CALayer> = view
            .layer()
            .ok_or_else(|| crate::anyhow("failed to create launcher progress layer".to_string()))?;
        layer.setBackgroundColor(Some(&color.CGColor()));
        layer.setCornerRadius(PROGRESS_BAR_HEIGHT * 0.5);
        Ok(view)
    }

    fn apply_visible_progress(handle: &LauncherOverlayHandle, progress: f64) {
        let progress = progress.clamp(0.0, 1.0);
        if let Ok(mut current_progress) = handle.current_progress.lock() {
            *current_progress = progress;
        }
        let layout = overlay_layout(handle.overlay.bounds(), progress);
        handle.backdrop.setFrame(handle.overlay.bounds());
        handle.title.setFrame(layout.title_frame);
        handle.title.setAlphaValue(TITLE_MAX_ALPHA);
        handle.progress_track.setFrame(layout.progress_track_frame);
        handle.progress_fill.setFrame(layout.progress_fill_frame);
        handle
            .progress_highlight
            .setFrame(layout.progress_highlight_frame);

        if let Some(layer) = handle.overlay.layer() {
            layer.setBackgroundColor(Some(&NSColor::clearColor().CGColor()));
        }
        if let Some(layer) = handle.title.layer() {
            let shadow = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, 0.16);
            layer.setShadowColor(Some(&shadow.CGColor()));
            layer.setShadowOpacity(0.14);
            layer.setShadowRadius(5.0);
            layer.setShadowOffset(NSSize::new(0.0, 0.0));
        }
        if let Some(layer) = handle.progress_fill.layer() {
            let glow = NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 1.0, 1.0, 0.18);
            layer.setShadowColor(Some(&glow.CGColor()));
            layer.setShadowOpacity((0.12 + progress * 0.10) as f32);
            layer.setShadowRadius(5.0);
            layer.setShadowOffset(NSSize::new(0.0, 0.0));
        }
    }

    fn apply_tick_effects(handle: &LauncherOverlayHandle, tick: u64) {
        let progress = handle
            .current_progress
            .lock()
            .map(|current_progress| *current_progress)
            .unwrap_or(INITIAL_VISIBLE_PROGRESS);
        let pulse = ((tick as f64 / 18.0).sin() + 1.0) * 0.5;
        handle.progress_highlight.setAlphaValue(if progress > 0.28 {
            0.18 + pulse * 0.12
        } else {
            0.0
        });
        handle.title.setAlphaValue(TITLE_MAX_ALPHA - pulse * 0.04);
    }

    fn overlay_layout(bounds: NSRect, progress: f64) -> OverlayLayout {
        let local_width = bounds.size.width.max(1.0);
        let local_height = bounds.size.height.max(1.0);
        let center_x = local_width * 0.5;
        let title_width = local_width.min(260.0);
        let track_width = PROGRESS_BAR_WIDTH.min((local_width - 72.0).max(120.0));
        let fill_width = (track_width * progress.clamp(0.0, 1.0)).max(0.0);
        let highlight_width = PROGRESS_HIGHLIGHT_WIDTH.min(fill_width).max(0.0);
        let progress_x = center_x - track_width * 0.5;
        let progress_y = local_height * 0.5 - 30.0;

        OverlayLayout {
            title_frame: NSRect::new(
                NSPoint::new(center_x - title_width * 0.5, local_height * 0.5 + 16.0),
                NSSize::new(title_width, 32.0),
            ),
            progress_track_frame: NSRect::new(
                NSPoint::new(progress_x, progress_y),
                NSSize::new(track_width, PROGRESS_BAR_HEIGHT),
            ),
            progress_fill_frame: NSRect::new(
                NSPoint::new(progress_x, progress_y),
                NSSize::new(fill_width, PROGRESS_BAR_HEIGHT),
            ),
            progress_highlight_frame: NSRect::new(
                NSPoint::new(progress_x + fill_width - highlight_width, progress_y),
                NSSize::new(highlight_width, PROGRESS_BAR_HEIGHT),
            ),
        }
    }

    fn virtual_progress_for_tick(tick: u64) -> f64 {
        let elapsed_seconds = tick as f64 * ANIMATION_FRAME_DELAY.as_secs_f64();
        virtual_progress_for_elapsed(elapsed_seconds)
    }

    fn virtual_progress_for_elapsed(elapsed_seconds: f64) -> f64 {
        if elapsed_seconds <= VIRTUAL_STAGE_ONE_SECONDS {
            let progress = ease_out_cubic(elapsed_seconds / VIRTUAL_STAGE_ONE_SECONDS);
            lerp(
                INITIAL_VISIBLE_PROGRESS,
                ONE_SECOND_VISIBLE_PROGRESS,
                progress,
            )
        } else if elapsed_seconds <= VIRTUAL_STAGE_TWO_SECONDS {
            let progress = ease_out_cubic(
                (elapsed_seconds - VIRTUAL_STAGE_ONE_SECONDS)
                    / (VIRTUAL_STAGE_TWO_SECONDS - VIRTUAL_STAGE_ONE_SECONDS),
            );
            lerp(
                ONE_SECOND_VISIBLE_PROGRESS,
                FINAL_VIRTUAL_PROGRESS,
                progress,
            )
        } else {
            FINAL_VIRTUAL_PROGRESS
        }
    }

    fn ease_out_cubic(progress: f64) -> f64 {
        let inverse = 1.0 - progress.clamp(0.0, 1.0);
        1.0 - inverse * inverse * inverse
    }

    fn lerp(start: f64, end: f64, progress: f64) -> f64 {
        start + (end - start) * progress
    }

    fn content_view(window: &Window) -> tauri::Result<&NSView> {
        let ns_view = window.ns_view()?;
        let Some(content_view) = (unsafe { (ns_view as *mut NSView).as_ref() }) else {
            return Err(crate::anyhow(
                "failed to access macOS content view".to_string(),
            ));
        };
        Ok(content_view)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn assert_near(actual: f64, expected: f64) {
            assert!(
                (actual - expected).abs() < 0.000_001,
                "expected {expected}, got {actual}"
            );
        }

        #[test]
        fn virtual_progress_matches_startup_milestones() {
            assert_near(virtual_progress_for_elapsed(0.0), 0.21);
            assert_near(virtual_progress_for_elapsed(1.0), 0.80);
            assert_near(virtual_progress_for_elapsed(1.5), 0.99);
            assert_near(virtual_progress_for_elapsed(3.0), 0.99);
        }

        #[test]
        fn handoff_progress_uses_only_final_one_percent() {
            assert_near(FINAL_VIRTUAL_PROGRESS + 0.0 * HANDOFF_PROGRESS_RANGE, 0.99);
            assert_near(FINAL_VIRTUAL_PROGRESS + 1.0 * HANDOFF_PROGRESS_RANGE, 1.0);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use tauri::window::Window;

    pub fn install(_window: &Window) -> tauri::Result<Option<usize>> {
        Ok(None)
    }

    pub fn start_animation(_window: &Window, _overlay_ptr: usize) -> tauri::Result<()> {
        Ok(())
    }

    pub fn set_overlay_alpha(
        _window: &Window,
        _overlay_ptr: usize,
        _alpha: f64,
    ) -> tauri::Result<()> {
        Ok(())
    }

    pub fn set_overlay_progress(
        _window: &Window,
        _overlay_ptr: usize,
        _progress: f64,
    ) -> tauri::Result<()> {
        Ok(())
    }

    pub fn tick_overlay(_window: &Window, _overlay_ptr: usize, _tick: u64) -> tauri::Result<()> {
        Ok(())
    }

    pub fn remove_overlay(_window: &Window, _overlay_ptr: usize) -> tauri::Result<()> {
        Ok(())
    }
}

pub fn install(window: &Window) -> tauri::Result<Option<usize>> {
    imp::install(window)
}

pub fn start_animation(window: &Window, overlay_ptr: usize) -> tauri::Result<()> {
    imp::start_animation(window, overlay_ptr)
}

pub fn set_overlay_alpha(window: &Window, overlay_ptr: usize, alpha: f64) -> tauri::Result<()> {
    imp::set_overlay_alpha(window, overlay_ptr, alpha)
}

pub fn set_overlay_progress(
    window: &Window,
    overlay_ptr: usize,
    progress: f64,
) -> tauri::Result<()> {
    imp::set_overlay_progress(window, overlay_ptr, progress)
}

pub fn remove_overlay(window: &Window, overlay_ptr: usize) -> tauri::Result<()> {
    imp::remove_overlay(window, overlay_ptr)
}
