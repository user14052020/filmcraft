//! Plain-text identity for this independent FilmCraft fork. Upstream trademark artwork is omitted.

/// Draw the fork name without embedding upstream trademark artwork.
pub fn paint_wordmark(ui: &egui::Ui, left_center: egui::Pos2, height: f32, dark: bool) -> Option<egui::Rect> {
    let color = if dark { egui::Color32::WHITE } else { egui::Color32::BLACK };
    Some(ui.painter().text(
        left_center,
        egui::Align2::LEFT_CENTER,
        "FilmCraft · iPhone HDR",
        egui::FontId::proportional(height),
        color,
    ))
}
