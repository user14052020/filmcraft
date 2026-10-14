//! Native float EDR presentation. The input file stays untouched: wgpu configures
//! macOS's Metal layer for extended linear sRGB, and AppKit reports the current
//! display headroom so preview adapts to brightness and moving between displays.

pub fn configure(options: &mut eframe::NativeOptions) {
    #[cfg(target_os = "macos")]
    {
        use eframe::egui_wgpu::wgpu::{SurfaceColorSpace, TextureFormat};
        options.wgpu_options.preferred_surface_format = Some(TextureFormat::Rgba16Float);
        options.wgpu_options.preferred_surface_color_space = SurfaceColorSpace::ExtendedSrgbLinear;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = options;
}

pub fn attach(app: &mut filmcraft_ui_egui::FilmcraftApp, state: &eframe::egui_wgpu::RenderState) {
    #[cfg(target_os = "macos")]
    {
        use eframe::egui_wgpu::wgpu::{SurfaceColorSpace, TextureFormat};
        if state.target_format == TextureFormat::Rgba16Float && state.target_color_space == SurfaceColorSpace::ExtendedSrgbLinear {
            app.set_hdr_display_headroom(headroom());
            app.hooks.hdr_display_headroom = Some(Box::new(headroom));
            log::info!("HDR display: RGBA16Float, macOS extended linear sRGB / EDR");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, state);
}

#[cfg(target_os = "macos")]
fn headroom() -> f32 {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let Some(main_thread) = MainThreadMarker::new() else { return 1.0 };
    let application = NSApplication::sharedApplication(main_thread);
    // mainWindow remains the editor while a file dialog is key. During startup
    // or an unfocused launch, find an existing visible main-capable app window.
    let window = application.mainWindow().or_else(|| application.windows().iter().find(|window| window.isVisible() && window.canBecomeMainWindow()));
    let Some(screen) = window.and_then(|window| window.screen()) else { return 1.0 };
    let value = screen.maximumExtendedDynamicRangeColorComponentValue();
    if value.is_finite() { value.clamp(1.0, 64.0) as f32 } else { 1.0 }
}
