//! Small vector icons drawn with the egui painter (no icon font needed).

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Select,
    Pen,
    Highlighter,
    Eraser,
    Text,
    Image,
    Undo,
    Redo,
    Save,
    More,
    Pages,
    Notes,
    Plus,
    Minus,
    Close,
    Trash,
    Palette,
    Fit,
}

/// Paints `icon` inside `rect` (designed on a 24×24 grid).
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32, accent: Color32) {
    let s = rect.width().min(rect.height()) / 24.0;
    let o = rect.center() - Vec2::splat(12.0 * s);
    let p = |x: f32, y: f32| o + Vec2::new(x * s, y * s);
    let w = (1.7 * s).max(1.2);
    let st = Stroke::new(w, color);
    let line = |pts: &[(f32, f32)]| {
        painter.add(Shape::line(pts.iter().map(|&(x, y)| p(x, y)).collect(), st));
    };
    let closed = |pts: &[(f32, f32)]| {
        painter.add(Shape::closed_line(
            pts.iter().map(|&(x, y)| p(x, y)).collect(),
            st,
        ));
    };
    let fill = |pts: &[(f32, f32)], c: Color32| {
        painter.add(Shape::convex_polygon(
            pts.iter().map(|&(x, y)| p(x, y)).collect(),
            c,
            Stroke::NONE,
        ));
    };
    let arc = |cx: f32, cy: f32, r: f32, a0: f32, a1: f32| -> Vec<(f32, f32)> {
        (0..=16)
            .map(|i| {
                let a = (a0 + (a1 - a0) * i as f32 / 16.0).to_radians();
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect()
    };
    match icon {
        Icon::Select => {
            let pts: Vec<Pos2> = arc(12.0, 10.0, 8.0, 0.0, 360.0)
                .into_iter()
                .map(|(x, y)| p(x, y * 0.8 + 2.0))
                .collect();
            painter.extend(Shape::dashed_line(&pts, st, 2.2 * s, 1.8 * s));
            line(&[(8.0, 16.5), (6.5, 19.0), (8.5, 21.5)]);
        }
        Icon::Pen => {
            fill(&[(4.0, 20.0), (5.5, 15.5), (8.5, 18.5)], accent);
            closed(&[(5.5, 15.5), (15.5, 5.5), (18.5, 8.5), (8.5, 18.5)]);
            line(&[(13.5, 7.5), (16.5, 10.5)]);
            line(&[(4.0, 20.0), (5.5, 15.5)]);
            line(&[(4.0, 20.0), (8.5, 18.5)]);
        }
        Icon::Highlighter => {
            fill(&[(4.0, 20.0), (5.0, 16.0), (8.0, 19.0)], accent);
            closed(&[(5.0, 16.0), (14.0, 7.0), (17.0, 10.0), (8.0, 19.0)]);
            closed(&[(14.0, 7.0), (16.5, 4.5), (19.5, 7.5), (17.0, 10.0)]);
            line(&[(3.0, 21.5), (21.0, 21.5)]);
        }
        Icon::Eraser => {
            fill(
                &[(3.5, 14.5), (7.5, 10.5), (13.5, 16.5), (9.5, 20.5)],
                color.gamma_multiply(0.35),
            );
            closed(&[(3.5, 14.5), (12.0, 6.0), (18.0, 12.0), (9.5, 20.5)]);
            line(&[(7.5, 10.5), (13.5, 16.5)]);
            line(&[(9.5, 20.5), (20.5, 20.5)]);
        }
        Icon::Text => {
            line(&[(5.0, 7.0), (5.0, 5.0), (19.0, 5.0), (19.0, 7.0)]);
            line(&[(12.0, 5.0), (12.0, 19.0)]);
            line(&[(9.0, 19.0), (15.0, 19.0)]);
        }
        Icon::Image => {
            painter.rect_stroke(
                Rect::from_min_max(p(3.0, 5.0), p(21.0, 19.0)),
                2.0 * s,
                st,
                eframe::egui::StrokeKind::Middle,
            );
            line(&[
                (4.0, 17.0),
                (9.5, 11.5),
                (13.5, 15.5),
                (16.0, 13.0),
                (20.0, 17.0),
            ]);
            painter.circle_filled(p(16.0, 9.0), 1.6 * s, color);
        }
        Icon::Undo | Icon::Redo => {
            let m = |x: f32| if icon == Icon::Redo { 24.0 - x } else { x };
            let mut pts = vec![(m(6.0), 9.0), (m(14.0), 9.0)];
            let (a0, a1) = if icon == Icon::Redo {
                (270.0, 90.0)
            } else {
                (-90.0, 90.0)
            };
            pts.extend(arc(m(14.0), 13.5, 4.5, a0, a1).into_iter());
            pts.push((m(8.0), 18.0));
            line(&pts);
            line(&[(m(9.5), 5.5), (m(6.0), 9.0), (m(9.5), 12.5)]);
        }
        Icon::Save => {
            line(&[(12.0, 4.0), (12.0, 14.5)]);
            line(&[(7.5, 10.5), (12.0, 15.0), (16.5, 10.5)]);
            line(&[(4.0, 15.0), (4.0, 20.0), (20.0, 20.0), (20.0, 15.0)]);
        }
        Icon::More => {
            for y in [6.0, 12.0, 18.0] {
                painter.circle_filled(p(12.0, y), 1.8 * s, color);
            }
        }
        Icon::Pages => {
            painter.rect_stroke(
                Rect::from_min_max(p(8.0, 3.0), p(19.0, 16.0)),
                1.5 * s,
                st,
                eframe::egui::StrokeKind::Middle,
            );
            painter.rect_stroke(
                Rect::from_min_max(p(5.0, 7.0), p(16.0, 21.0)),
                1.5 * s,
                st,
                eframe::egui::StrokeKind::Middle,
            );
        }
        Icon::Notes => {
            line(&[(5.0, 6.0), (19.0, 6.0)]);
            line(&[(5.0, 10.0), (19.0, 10.0)]);
            line(&[(5.0, 14.0), (19.0, 14.0)]);
            line(&[(5.0, 18.0), (13.0, 18.0)]);
        }
        Icon::Plus => {
            line(&[(12.0, 5.0), (12.0, 19.0)]);
            line(&[(5.0, 12.0), (19.0, 12.0)]);
        }
        Icon::Minus => line(&[(5.0, 12.0), (19.0, 12.0)]),
        Icon::Close => {
            line(&[(6.0, 6.0), (18.0, 18.0)]);
            line(&[(18.0, 6.0), (6.0, 18.0)]);
        }
        Icon::Trash => {
            line(&[(4.5, 7.0), (19.5, 7.0)]);
            line(&[(9.5, 7.0), (9.5, 4.5), (14.5, 4.5), (14.5, 7.0)]);
            closed(&[(6.5, 7.0), (7.5, 20.0), (16.5, 20.0), (17.5, 7.0)]);
            line(&[(10.5, 10.5), (10.5, 16.5)]);
            line(&[(13.5, 10.5), (13.5, 16.5)]);
        }
        Icon::Palette => {
            let colors = [
                Color32::from_rgb(229, 57, 53),
                Color32::from_rgb(251, 140, 0),
                Color32::from_rgb(253, 216, 53),
                Color32::from_rgb(67, 160, 71),
                Color32::from_rgb(30, 136, 229),
                Color32::from_rgb(142, 36, 170),
            ];
            for (i, c) in colors.iter().enumerate() {
                let a0 = i as f32 * 60.0 - 90.0;
                let mut pts = vec![(12.0, 12.0)];
                pts.extend(arc(12.0, 12.0, 9.0, a0, a0 + 60.0));
                fill(&pts, *c);
            }
            painter.circle_filled(p(12.0, 12.0), 3.0 * s, Color32::WHITE);
        }
        Icon::Fit => {
            line(&[(4.0, 9.0), (4.0, 4.0), (9.0, 4.0)]);
            line(&[(15.0, 4.0), (20.0, 4.0), (20.0, 9.0)]);
            line(&[(20.0, 15.0), (20.0, 20.0), (15.0, 20.0)]);
            line(&[(9.0, 20.0), (4.0, 20.0), (4.0, 15.0)]);
        }
    }
}
