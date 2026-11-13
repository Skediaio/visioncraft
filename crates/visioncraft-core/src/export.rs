//! Image sizes offered for export.

/// Name, width and height in pixels.
pub fn export_presets() -> &'static [(&'static str, u32, u32)] {
    &[
        ("Screen", 1280, 720),
        ("HD", 1920, 1080),
        ("QHD", 2560, 1440),
        ("4K", 3840, 2160),
        ("Square", 2048, 2048),
        ("Portrait", 1080, 1920),
    ]
}
