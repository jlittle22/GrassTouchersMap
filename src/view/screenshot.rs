//! "Copy screenshot link": the map writes down what it drew in the viewport as a scene (see
//! `GrepolisMapAPIReflector/src/screenshot.rs` for the format), the reflector renders and
//! hosts it. The link is the scene's hash, so it's copied right away while the upload runs
//! in the background.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

const UPLOAD_URL: &str = "https://reflector.grasstouchers.gg/screenshots";
/// the reflector's limit for either side of a screenshot, in pixels
const MAX_SIZE: f32 = 2000.0;

/// Builds a scene in screen pixels relative to the viewport's top left corner.
pub struct Scene {
    text: String,
    origin: egui::Pos2,
    /// pixels per egui point, so screenshots are as sharp as the screen without passing
    /// the reflector's size limit
    scale: f32,
    size: egui::Vec2,
}

impl Scene {
    pub fn new(viewport: egui::Rect, pixels_per_point: f32, background: egui::Color32) -> Self {
        let scale = pixels_per_point.min(MAX_SIZE / viewport.width().max(viewport.height()));
        let size = viewport.size() * scale;
        let [r, g, b, _] = background.to_srgba_unmultiplied();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let text = format!(
            "gtmap-screenshot 1\nsize {} {} {r:02x}{g:02x}{b:02x}\n",
            (size.x.round() as u32).max(1),
            (size.y.round() as u32).max(1),
        );
        Self {
            text,
            origin: viewport.left_top(),
            scale,
            size,
        }
    }

    fn px(&self, screen: egui::Pos2) -> egui::Pos2 {
        ((screen - self.origin) * self.scale).to_pos2()
    }

    /// a horizontal or vertical line between two screen positions, dashed if `dash` is given
    pub fn line(
        &mut self,
        from: egui::Pos2,
        to: egui::Pos2,
        width: f32,
        color: egui::Color32,
        dash: Option<(f32, f32)>,
    ) {
        let (from, to) = (self.px(from), self.px(to));
        let _ = write!(
            self.text,
            "line {:.1} {:.1} {:.1} {:.1} {:.1} {}",
            from.x,
            from.y,
            to.x,
            to.y,
            width * self.scale,
            hex(color)
        );
        if let Some((dash, gap)) = dash {
            let _ = write!(
                self.text,
                " {:.1} {:.1}",
                dash * self.scale,
                gap * self.scale
            );
        }
        self.text.push('\n');
    }

    /// filled circles at the given screen positions
    pub fn dots(
        &mut self,
        color: egui::Color32,
        radius: f32,
        at: impl IntoIterator<Item = egui::Pos2>,
    ) {
        let _ = writeln!(self.text, "dots {} {:.1}", hex(color), radius * self.scale);
        self.points(radius, at);
    }

    /// circle outlines at the given screen positions
    pub fn rings(
        &mut self,
        color: egui::Color32,
        radius: f32,
        width: f32,
        at: impl IntoIterator<Item = egui::Pos2>,
    ) {
        let _ = writeln!(
            self.text,
            "rings {} {:.1} {:.1}",
            hex(color),
            radius * self.scale,
            width * self.scale
        );
        self.points(radius + width, at);
    }

    /// convex polygons given by their corners on screen, filled and outlined
    pub fn polygons(
        &mut self,
        fill: egui::Color32,
        stroke: egui::Stroke,
        polygons: impl IntoIterator<Item = Vec<egui::Pos2>>,
    ) {
        let _ = writeln!(
            self.text,
            "polys {} {} {:.1}",
            hex(fill),
            hex(stroke.color),
            stroke.width * self.scale
        );
        let inside = egui::Rect::from_min_size(egui::Pos2::ZERO, self.size).expand(stroke.width);
        for corners in polygons {
            let corners: Vec<egui::Pos2> = corners.into_iter().map(|p| self.px(p)).collect();
            if corners.len() < 3 || !inside.intersects(egui::Rect::from_points(&corners)) {
                continue;
            }
            let line: Vec<String> = corners
                .iter()
                .map(|p| format!("{:.1} {:.1}", p.x, p.y))
                .collect();
            self.text.push_str(&line.join(" "));
            self.text.push('\n');
        }
    }

    /// the positions that are at least partly inside the viewport
    fn points(&mut self, radius: f32, at: impl IntoIterator<Item = egui::Pos2>) {
        let margin = radius * self.scale + 1.0;
        let inside = egui::Rect::from_min_size(egui::Pos2::ZERO, self.size).expand(margin);
        for screen in at {
            let p = self.px(screen);
            if inside.contains(p) {
                let _ = writeln!(self.text, "{:.1} {:.1}", p.x, p.y);
            }
        }
    }

    /// the Grass Touchers logo, fitted into the given screen rectangle and centered like
    /// egui's `Image` does
    pub fn logo(&mut self, rect: egui::Rect) {
        /// width over height of `assets/grass_touchers_logo.png`
        const ASPECT: f32 = 140.0 / 144.0;
        let size = if rect.width() / rect.height() > ASPECT {
            egui::vec2(rect.height() * ASPECT, rect.height())
        } else {
            egui::vec2(rect.width(), rect.width() / ASPECT)
        };
        let fitted = egui::Rect::from_center_size(rect.center(), size);
        let min = self.px(fitted.min);
        let size = fitted.size() * self.scale;
        let _ = writeln!(
            self.text,
            "logo {:.1} {:.1} {:.1} {:.1}",
            min.x, min.y, size.x, size.y
        );
    }

    /// Upload the scene and return the link it will be at.
    pub fn upload(self) -> String {
        let id = Sha256::digest(self.text.as_bytes())[..8].iter().fold(
            String::new(),
            |mut hex, byte| {
                let _ = write!(hex, "{byte:02x}");
                hex
            },
        );
        let request = ehttp::Request::post(UPLOAD_URL, self.text.into_bytes());
        ehttp::fetch(request, |response| {
            let error = match response {
                Ok(response) if response.ok => return,
                Ok(response) => format!(
                    "{} {}",
                    response.status,
                    response.text().unwrap_or_default()
                ),
                Err(err) => err,
            };
            #[cfg(target_arch = "wasm32")]
            log::error!("Screenshot upload failed: {error}");
            #[cfg(not(target_arch = "wasm32"))]
            eprintln!("Screenshot upload failed: {error}");
        });
        format!("{UPLOAD_URL}/{id}.png")
    }
}

/// `RRGGBBAA`, un-premultiplied in sRGB like egui blends. `Color32::to_srgba_unmultiplied`
/// goes through linear space, which darkens translucent colors.
fn hex(color: egui::Color32) -> String {
    let [r, g, b, a] = color.to_array();
    let unmultiply = |c: u8| {
        if a == 0 {
            0
        } else {
            u8::try_from((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).unwrap_or(255)
        }
    };
    format!(
        "{:02x}{:02x}{:02x}{a:02x}",
        unmultiply(r),
        unmultiply(g),
        unmultiply(b)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_every_command() {
        let viewport = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(640.0, 360.0));
        let mut scene = Scene::new(viewport, 2.0, egui::Color32::from_gray(10));
        let gray = egui::Color32::DARK_GRAY;
        scene.line(
            egui::pos2(100.0, 80.0),
            egui::pos2(900.0, 80.0),
            2.0,
            gray,
            None,
        );
        scene.line(
            egui::pos2(300.0, 0.0),
            egui::pos2(300.0, 900.0),
            1.0,
            gray,
            Some((7.0, 7.0)),
        );
        let towns = [egui::pos2(110.0, 60.0), egui::pos2(5000.0, 60.0)];
        scene.dots(egui::Color32::from_gray(180), 1.5, towns);
        scene.rings(egui::Color32::from_rgb(143, 199, 62), 3.5, 2.0, towns);
        // one square in view, one far outside it that gets left out
        let square = [(110.0, 60.0), (120.0, 60.0), (120.0, 70.0), (110.0, 70.0)];
        let far = [(5000.0, 60.0), (5010.0, 60.0), (5010.0, 70.0)];
        scene.polygons(
            egui::Color32::from_rgb(88, 112, 128).gamma_multiply(0.2),
            egui::Stroke::new(1.0, egui::Color32::from_rgb(88, 112, 128)),
            [&square[..], &far[..]]
                .map(|corners| corners.iter().map(|&(x, y)| egui::pos2(x, y)).collect()),
        );
        scene.logo(egui::Rect::from_min_size(
            egui::pos2(108.0, 58.0),
            egui::vec2(48.0, 48.0),
        ));
        assert_eq!(
            scene.text,
            "gtmap-screenshot 1\nsize 1280 720 0a0a0a\n\
             line 0.0 60.0 1600.0 60.0 4.0 606060ff\n\
             line 400.0 -100.0 400.0 1700.0 2.0 606060ff 14.0 14.0\n\
             dots b4b4b4ff 3.0\n20.0 20.0\n\
             rings 8fc73eff 7.0 4.0\n20.0 20.0\n\
             polys 5a6e8233 587080ff 2.0\n20.0 20.0 40.0 20.0 40.0 40.0 20.0 40.0\n\
             logo 17.3 16.0 93.3 96.0\n"
        );
    }

    #[test]
    fn caps_the_size() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 900.0));
        let scene = Scene::new(viewport, 2.0, egui::Color32::BLACK);
        assert!(scene.text.contains("size 2000 1125 000000"));
    }
}
