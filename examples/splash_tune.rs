//! Interactive tuner for the onboarding splash raster.
//!
//! Rasterizes the A-check vector from `docs/public/favicon.svg` at an
//! adjustable size and draws it with the same quadrant glyphs as the splash.
//!
//! Keys: up/down change height, left/right change width (one cell each),
//! `[`/`]` change the coverage threshold, `s` saves the raster to the splash
//! asset, `q` quits.

use std::io::{Write, stdout};

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{Event, KeyCode, KeyEventKind, read};
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode, size,
};
use crossterm::{execute, queue};

const SVG: &str = "docs/public/favicon.svg";
const ASSET: &str = "src/tui/ui/assets/aven_logo_terminal.png";
const SUPERSAMPLE: usize = 8;
const GRADIENT: [(f32, f32, f32); 3] = [
    (135.0, 40.0, 250.0),
    (150.0, 63.0, 255.0),
    (176.0, 108.0, 255.0),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pixel {
    Purple,
    White,
}

type Polygon = Vec<Vec<(f32, f32)>>;

fn main() -> std::io::Result<()> {
    let svg = std::fs::read_to_string(SVG)?;
    let paths = svg
        .split(" d=\"")
        .skip(1)
        .map(|rest| parse_path(&rest[..rest.find('"').expect("closed d attribute")]))
        .collect::<Vec<_>>();
    let (a, check) = (&paths[0], &paths[1]);

    let mut cells = (60usize, 28usize);
    let mut threshold = 0.5f32;
    let mut status = String::new();

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, Hide)?;
    let result = (|| -> std::io::Result<()> {
        loop {
            let raster = rasterize(a, check, cells.0 * 2, cells.1 * 2, threshold);
            draw(&raster, cells, threshold, &status)?;
            let Event::Key(key) = read()? else { continue };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            status.clear();
            match key.code {
                KeyCode::Up => cells.1 += 1,
                KeyCode::Down => cells.1 = cells.1.saturating_sub(1).max(4),
                KeyCode::Right => cells.0 += 1,
                KeyCode::Left => cells.0 = cells.0.saturating_sub(1).max(4),
                KeyCode::Char(']') => threshold = (threshold + 0.05).min(0.95),
                KeyCode::Char('[') => threshold = (threshold - 0.05).max(0.05),
                KeyCode::Char('s') => {
                    save(&raster).map_err(std::io::Error::other)?;
                    status = format!(
                        "saved {ASSET}; set LOGO_WIDTH = {}, LOGO_HEIGHT = {}",
                        cells.0, cells.1
                    );
                }
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                _ => {}
            }
        }
    })();
    execute!(stdout(), Show, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    result
}

fn parse_path(d: &str) -> Polygon {
    let mut polygon = Vec::new();
    let mut current = Vec::new();
    let mut tokens = d.split_whitespace();
    while let Some(token) = tokens.next() {
        match token {
            "M" | "L" => {
                if token == "M" && !current.is_empty() {
                    polygon.push(std::mem::take(&mut current));
                }
                let x = tokens.next().unwrap().parse().unwrap();
                let y = tokens.next().unwrap().parse().unwrap();
                current.push((x, y));
            }
            "Z" => polygon.push(std::mem::take(&mut current)),
            other => panic!("unsupported path token {other}"),
        }
    }
    if !current.is_empty() {
        polygon.push(current);
    }
    polygon
}

fn contains(polygon: &Polygon, x: f32, y: f32) -> bool {
    let mut inside = false;
    for ring in polygon {
        let mut j = ring.len() - 1;
        for i in 0..ring.len() {
            let (xi, yi) = ring[i];
            let (xj, yj) = ring[j];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
    }
    inside
}

fn bounds(polygon: &Polygon) -> (f32, f32, f32, f32) {
    polygon.iter().flatten().fold(
        (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
        |(x0, y0, x1, y1), &(x, y)| (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
    )
}

fn rasterize(
    a: &Polygon,
    check: &Polygon,
    width: usize,
    height: usize,
    threshold: f32,
) -> Vec<Vec<Option<Pixel>>> {
    let (x0, y0, x1, y1) = bounds(a);
    let samples = (SUPERSAMPLE * SUPERSAMPLE) as f32;
    (0..height)
        .map(|py| {
            (0..width)
                .map(|px| {
                    let (mut a_hits, mut check_hits) = (0.0, 0.0);
                    for sy in 0..SUPERSAMPLE {
                        for sx in 0..SUPERSAMPLE {
                            let u =
                                (px as f32 + (sx as f32 + 0.5) / SUPERSAMPLE as f32) / width as f32;
                            let v = (py as f32 + (sy as f32 + 0.5) / SUPERSAMPLE as f32)
                                / height as f32;
                            let (x, y) = (x0 + u * (x1 - x0), y0 + v * (y1 - y0));
                            if contains(check, x, y) {
                                check_hits += 1.0;
                            } else if contains(a, x, y) {
                                a_hits += 1.0;
                            }
                        }
                    }
                    if check_hits / samples >= threshold {
                        Some(Pixel::White)
                    } else if (a_hits + check_hits) / samples >= threshold {
                        Some(Pixel::Purple)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .collect()
}

fn purple_at(y: usize, height: usize) -> Color {
    let progress = y as f32 / height.saturating_sub(1).max(1) as f32;
    let (start, end, mix) = if progress < 0.52 {
        (GRADIENT[0], GRADIENT[1], progress / 0.52)
    } else {
        (GRADIENT[1], GRADIENT[2], (progress - 0.52) / 0.48)
    };
    let channel = |s: f32, e: f32| (s + (e - s) * mix) as u8;
    Color::Rgb {
        r: channel(start.0, end.0),
        g: channel(start.1, end.1),
        b: channel(start.2, end.2),
    }
}

fn quadrant_glyph(mask: usize) -> char {
    [
        ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
    ][mask]
}

fn draw(
    raster: &[Vec<Option<Pixel>>],
    cells: (usize, usize),
    threshold: f32,
    status: &str,
) -> std::io::Result<()> {
    let mut out = stdout();
    let (cols, rows) = size()?;
    let left = (cols as usize).saturating_sub(cells.0) / 2;
    let top = (rows as usize).saturating_sub(cells.1 + 2) / 2;
    queue!(out, ResetColor, Clear(ClearType::All))?;
    for cy in 0..cells.1 {
        queue!(out, MoveTo(left as u16, (top + cy) as u16))?;
        for cx in 0..cells.0 {
            let pixels = [
                raster[cy * 2][cx * 2],
                raster[cy * 2][cx * 2 + 1],
                raster[cy * 2 + 1][cx * 2],
                raster[cy * 2 + 1][cx * 2 + 1],
            ];
            let mask = |target| {
                pixels.iter().enumerate().fold(0, |mask, (index, pixel)| {
                    mask | usize::from(*pixel == Some(target)) << index
                })
            };
            let (white, purple) = (mask(Pixel::White), mask(Pixel::Purple));
            let purple_color = purple_at(cy * 2, cells.1 * 2);
            if white > 0 {
                queue!(out, SetForegroundColor(Color::White))?;
                if white | purple == 0b1111 && purple > 0 {
                    queue!(out, SetBackgroundColor(purple_color))?;
                }
                queue!(out, Print(quadrant_glyph(white)), ResetColor)?;
            } else if purple > 0 {
                queue!(
                    out,
                    SetForegroundColor(purple_color),
                    Print(quadrant_glyph(purple)),
                    ResetColor
                )?;
            } else {
                queue!(out, Print(' '))?;
            }
        }
    }
    queue!(
        out,
        MoveTo(0, (top + cells.1 + 1) as u16),
        Print(format!(
            "cells {}x{}  raster {}x{}  threshold {:.2}  [arrows resize, [ ] threshold, s save, q quit]",
            cells.0,
            cells.1,
            cells.0 * 2,
            cells.1 * 2,
            threshold
        )),
        MoveTo(0, (top + cells.1 + 2) as u16),
        Print(status)
    )?;
    out.flush()
}

fn save(raster: &[Vec<Option<Pixel>>]) -> image::ImageResult<()> {
    let height = raster.len() as u32;
    let width = raster[0].len() as u32;
    image::GrayAlphaImage::from_fn(width, height, |x, y| {
        image::LumaA(match raster[y as usize][x as usize] {
            None => [0, 0],
            Some(Pixel::Purple) => [0, 255],
            Some(Pixel::White) => [255, 255],
        })
    })
    .save(ASSET)
}
