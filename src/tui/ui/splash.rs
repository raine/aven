use std::sync::OnceLock;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::tui::app_onboarding::OnboardingIntroVisual;
use crate::tui::theme::BG;

const LOGO_WIDTH: usize = 60;
const LOGO_HEIGHT: usize = 28;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pixel {
    Purple,
    White,
}

struct Raster {
    pixels: Vec<Vec<Option<Pixel>>>,
    occupied_x_bounds: (usize, usize),
    white_x_bounds: (usize, usize),
    white_endpoint: (usize, usize),
}

pub(crate) fn render_onboarding_splash(frame: &mut Frame, visual: OnboardingIntroVisual) {
    render_splash(frame, visual);
}

pub(crate) fn render_dimmed_onboarding_splash(frame: &mut Frame) {
    render_splash(
        frame,
        OnboardingIntroVisual {
            purple_reveal: 1.0,
            check_reveal: 1.0,
            dim: 1.0,
            dialog_reveal: 1.0,
            afterglow: 1.0,
        },
    );
}

fn render_splash(frame: &mut Frame, visual: OnboardingIntroVisual) {
    let area = frame.area();
    frame.render_widget(Block::new().style(Style::new().bg(BG)), area);

    let Some((width, height)) = splash_size(area) else {
        return;
    };
    let source = logo();
    let lines = (0..height)
        .map(|y| {
            let spans = (0..width)
                .map(|x| splash_cell(source, x, y, width, height, visual))
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    let logo_area = Rect {
        x: area.x + area.width.saturating_sub(width as u16) / 2,
        y: area.y + area.height.saturating_sub(height as u16) / 2,
        width: width as u16,
        height: height as u16,
    };
    frame.render_widget(Paragraph::new(lines), logo_area);
}

fn splash_size(area: Rect) -> Option<(usize, usize)> {
    let available_height = area.height.saturating_sub(2) as usize;
    let height = LOGO_HEIGHT
        .min(available_height)
        .min(area.width as usize * LOGO_HEIGHT / LOGO_WIDTH);
    if height == 0 {
        return None;
    }
    let width = (height * LOGO_WIDTH / LOGO_HEIGHT).max(1);
    Some((width, height))
}

fn splash_cell(
    source: &Raster,
    cell_x: usize,
    cell_y: usize,
    width: usize,
    height: usize,
    visual: OnboardingIntroVisual,
) -> Span<'static> {
    let pixels = [
        sampled_pixel(
            source,
            cell_x * 2,
            cell_y * 2,
            width * 2,
            height * 2,
            visual,
        ),
        sampled_pixel(
            source,
            cell_x * 2 + 1,
            cell_y * 2,
            width * 2,
            height * 2,
            visual,
        ),
        sampled_pixel(
            source,
            cell_x * 2,
            cell_y * 2 + 1,
            width * 2,
            height * 2,
            visual,
        ),
        sampled_pixel(
            source,
            cell_x * 2 + 1,
            cell_y * 2 + 1,
            width * 2,
            height * 2,
            visual,
        ),
    ];
    let white_mask = pixel_mask(&pixels, Pixel::White);
    let purple_mask = pixel_mask(&pixels, Pixel::Purple);
    let occupied_mask = white_mask | purple_mask;
    let purple = purple_at(cell_y * 2, height * 2);
    let purple = dim_color(purple, visual.dim);
    let purple = afterglow_color(
        purple,
        cell_x * 2,
        cell_y * 2,
        width * 2,
        height * 2,
        source,
        visual.afterglow,
    );
    let white = dim_color(Color::White, visual.dim);

    if white_mask > 0 {
        let mut style = Style::new().fg(white);
        if occupied_mask == 0b1111 && purple_mask > 0 {
            style = style.bg(purple);
        }
        Span::styled(quadrant_glyph(white_mask).to_string(), style)
    } else if purple_mask > 0 {
        Span::styled(
            quadrant_glyph(purple_mask).to_string(),
            Style::new().fg(purple),
        )
    } else {
        Span::raw(" ")
    }
}

fn sampled_pixel(
    source: &Raster,
    x: usize,
    y: usize,
    target_width: usize,
    target_height: usize,
    visual: OnboardingIntroVisual,
) -> Option<Pixel> {
    let source_x = (x * source.pixels[0].len() / target_width).min(source.pixels[0].len() - 1);
    let source_y = (y * source.pixels.len() / target_height).min(source.pixels.len() - 1);
    let pixel = source.pixels[source_y][source_x]?;
    if !x_revealed(source_x, source.occupied_x_bounds, visual.purple_reveal) {
        return None;
    }
    if pixel == Pixel::White && !x_revealed(source_x, source.white_x_bounds, visual.check_reveal) {
        return Some(Pixel::Purple);
    }
    Some(pixel)
}

fn x_revealed(x: usize, bounds: (usize, usize), progress: f32) -> bool {
    progress >= 1.0 || normalized_x(x, bounds) < progress
}

fn normalized_x(x: usize, bounds: (usize, usize)) -> f32 {
    let width = bounds.1.saturating_sub(bounds.0).max(1);
    x.saturating_sub(bounds.0) as f32 / width as f32
}

fn logo() -> &'static Raster {
    static LOGO: OnceLock<Raster> = OnceLock::new();
    LOGO.get_or_init(|| {
        let image = image::load_from_memory(include_bytes!("assets/aven_logo_terminal.png"))
            .expect("embedded Aven logo raster is valid")
            .to_luma_alpha8();
        let (width, height) = image.dimensions();
        let pixels = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| {
                        let pixel = image.get_pixel(x, y).0;
                        if pixel[1] < 64 {
                            None
                        } else if pixel[0] > 160 {
                            Some(Pixel::White)
                        } else {
                            Some(Pixel::Purple)
                        }
                    })
                    .collect()
            })
            .collect::<Vec<Vec<_>>>();
        Raster {
            occupied_x_bounds: x_bounds(&pixels, |_| true),
            white_x_bounds: x_bounds(&pixels, |pixel| pixel == Pixel::White),
            white_endpoint: white_endpoint(&pixels),
            pixels,
        }
    })
}

fn x_bounds(pixels: &[Vec<Option<Pixel>>], include: impl Fn(Pixel) -> bool) -> (usize, usize) {
    let mut bounds: Option<(usize, usize)> = None;
    for row in pixels {
        for (x, pixel) in row.iter().enumerate() {
            if pixel.is_some_and(&include) {
                bounds = Some(match bounds {
                    Some((start, end)) => (start.min(x), end.max(x)),
                    None => (x, x),
                });
            }
        }
    }
    bounds.expect("embedded Aven logo layer contains pixels")
}

fn white_endpoint(pixels: &[Vec<Option<Pixel>>]) -> (usize, usize) {
    let x = x_bounds(pixels, |pixel| pixel == Pixel::White).1;
    let rows = pixels
        .iter()
        .enumerate()
        .filter_map(|(y, row)| (row[x] == Some(Pixel::White)).then_some(y))
        .collect::<Vec<_>>();
    let y = rows.iter().sum::<usize>() / rows.len();
    (x, y)
}

fn pixel_mask(pixels: &[Option<Pixel>; 4], target: Pixel) -> usize {
    pixels.iter().enumerate().fold(0, |mask, (index, pixel)| {
        mask | usize::from(*pixel == Some(target)) << index
    })
}

fn quadrant_glyph(mask: usize) -> char {
    match mask {
        0b0000 => ' ',
        0b0001 => '▘',
        0b0010 => '▝',
        0b0011 => '▀',
        0b0100 => '▖',
        0b0101 => '▌',
        0b0110 => '▞',
        0b0111 => '▛',
        0b1000 => '▗',
        0b1001 => '▚',
        0b1010 => '▐',
        0b1011 => '▜',
        0b1100 => '▄',
        0b1101 => '▙',
        0b1110 => '▟',
        0b1111 => '█',
        _ => unreachable!(),
    }
}

fn purple_at(y: usize, height: usize) -> Color {
    let progress = y as f32 / height.saturating_sub(1).max(1) as f32;
    let (start, end, mix) = if progress < 0.52 {
        ((135.0, 40.0, 250.0), (150.0, 63.0, 255.0), progress / 0.52)
    } else {
        (
            (150.0, 63.0, 255.0),
            (176.0, 108.0, 255.0),
            (progress - 0.52) / 0.48,
        )
    };
    let rgb = mix_rgb(start, end, mix);
    Color::Rgb(rgb.0 as u8, rgb.1 as u8, rgb.2 as u8)
}

fn afterglow_color(
    color: Color,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    source: &Raster,
    progress: f32,
) -> Color {
    let progress = progress.clamp(0.0, 1.0);
    if progress <= 0.0 || progress >= 1.0 {
        return color;
    }
    let horizontal = x as f32 / width.saturating_sub(1).max(1) as f32;
    let vertical = y as f32 / height.saturating_sub(1).max(1) as f32;
    let endpoint_x =
        source.white_endpoint.0 as f32 / source.pixels[0].len().saturating_sub(1).max(1) as f32;
    let endpoint_y =
        source.white_endpoint.1 as f32 / source.pixels.len().saturating_sub(1).max(1) as f32;
    let dx = horizontal - endpoint_x;
    let dy = (vertical - endpoint_y) * 0.55;
    let distance = (dx * dx + dy * dy).sqrt();
    let radius = progress * 1.25;
    let ring = (1.0 - (distance - radius).abs() / 0.40).clamp(0.0, 1.0);
    let amount = ring * (std::f32::consts::PI * progress).sin() * 0.35;

    match color {
        Color::Rgb(red, green, blue) => {
            let rgb = mix_rgb(
                (red as f32, green as f32, blue as f32),
                (232.0, 213.0, 255.0),
                amount,
            );
            Color::Rgb(rgb.0 as u8, rgb.1 as u8, rgb.2 as u8)
        }
        _ => color,
    }
}

fn dim_color(color: Color, amount: f32) -> Color {
    let amount = amount.clamp(0.0, 1.0);
    match color {
        Color::Rgb(red, green, blue) => Color::Rgb(
            mix_channel(red, red / 3, amount),
            mix_channel(green, green / 3, amount),
            mix_channel(blue, blue / 3, amount),
        ),
        Color::White => {
            let channel = mix_channel(255, 85, amount);
            Color::Rgb(channel, channel, channel)
        }
        _ => color,
    }
}

fn mix_channel(start: u8, end: u8, amount: f32) -> u8 {
    (start as f32 + (end as f32 - start as f32) * amount) as u8
}

fn mix_rgb(start: (f32, f32, f32), end: (f32, f32, f32), amount: f32) -> (f32, f32, f32) {
    (
        start.0 + (end.0 - start.0) * amount,
        start.1 + (end.1 - start.1) * amount,
        start.2 + (end.2 - start.2) * amount,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splash_scales_within_terminal() {
        assert_eq!(splash_size(Rect::new(0, 0, 80, 40)), Some((60, 28)));
        assert_eq!(splash_size(Rect::new(0, 0, 70, 18)), Some((34, 16)));
        assert_eq!(splash_size(Rect::new(0, 0, 1, 1)), None);
    }

    #[test]
    fn embedded_logo_contains_both_layers() {
        let logo = logo();
        assert!(
            logo.pixels
                .iter()
                .flatten()
                .any(|pixel| *pixel == Some(Pixel::Purple))
        );
        assert!(
            logo.pixels
                .iter()
                .flatten()
                .any(|pixel| *pixel == Some(Pixel::White))
        );
    }

    #[test]
    fn completed_afterglow_restores_the_dimmed_gradient() {
        let logo = logo();
        let color = Color::Rgb(50, 21, 85);
        assert_eq!(afterglow_color(color, 60, 28, 120, 56, logo, 0.0), color);
        assert_eq!(afterglow_color(color, 60, 28, 120, 56, logo, 1.0), color);
    }

    #[test]
    fn completed_draw_matches_source_raster() {
        let logo = logo();
        let width = logo.pixels[0].len();
        let height = logo.pixels.len();
        let complete = OnboardingIntroVisual {
            purple_reveal: 1.0,
            check_reveal: 1.0,
            dim: 1.0,
            dialog_reveal: 1.0,
            afterglow: 1.0,
        };

        for (y, row) in logo.pixels.iter().enumerate() {
            for (x, pixel) in row.iter().enumerate() {
                assert_eq!(sampled_pixel(logo, x, y, width, height, complete), *pixel);
            }
        }
    }

    #[test]
    fn check_pixels_are_purple_until_the_stroke_reaches_them() {
        let logo = logo();
        let (y, x) = logo
            .pixels
            .iter()
            .enumerate()
            .find_map(|(y, row)| {
                row.iter()
                    .position(|pixel| *pixel == Some(Pixel::White))
                    .map(|x| (y, x))
            })
            .expect("embedded Aven logo contains white pixels");
        let width = logo.pixels[0].len();
        let height = logo.pixels.len();

        assert_eq!(
            sampled_pixel(
                logo,
                x,
                y,
                width,
                height,
                OnboardingIntroVisual {
                    purple_reveal: 1.0,
                    check_reveal: 0.0,
                    dim: 0.0,
                    dialog_reveal: 0.0,
                    afterglow: 0.0,
                },
            ),
            Some(Pixel::Purple)
        );
        assert_eq!(
            sampled_pixel(
                logo,
                x,
                y,
                width,
                height,
                OnboardingIntroVisual {
                    purple_reveal: 1.0,
                    check_reveal: 1.0,
                    dim: 0.0,
                    dialog_reveal: 0.0,
                    afterglow: 0.0,
                },
            ),
            Some(Pixel::White)
        );
    }
}
