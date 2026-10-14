# FilmCraft HDR presentation patch

Source: egui-wgpu 0.36.2, https://github.com/emilk/egui/tree/0.36.2/crates/egui-wgpu
License: MIT OR Apache-2.0; original licenses are included alongside this file.

Local changes: optional presentation format/color-space pair with capability-checked SDR
fallback; linear UI rendering and gamma clear-color conversion for RGBA16Float;
RGBA16Float screenshot readback converted to SDR sRGB for the existing PNG interface.

On macOS wgpu 30 configures the Metal layer for ExtendedSrgbLinear and enables EDR.
Video is drawn by FilmCraft's float preview callback without passing through 8-bit UI textures.
