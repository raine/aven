use std::io::{self, IsTerminal, Write};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{Parser, ValueEnum};
use crossterm::QueueableCommand;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{
    self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
    LeaveAlternateScreen,
};

const FRAME_RATE_MIN: u16 = 1;
const FRAME_RATE_MAX: u16 = 120;
const SECONDS_PER_STYLE: f32 = 3.0;
const SHIMMER_SWEEPS_PER_STYLE: f32 = 2.0;
const HINT_WIDTH: usize = 32;
const SHOWCASE: &[Style] = &[Style::Shimmer, Style::Draw, Style::Tilt, Style::Hires];

type Raster = Vec<Vec<Option<Pixel>>>;

#[derive(Parser)]
#[command(about = "Animate Aven's A-check logo in the terminal")]
struct Args {
    /// Animation treatment
    #[arg(long, value_enum, default_value_t = Style::All)]
    style: Style,

    /// Animation frame rate
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u16).range(FRAME_RATE_MIN as i64..=FRAME_RATE_MAX as i64))]
    fps: u16,

    /// Number of complete cycles before exiting, or 0 to run until interrupted
    #[arg(long, default_value_t = 1)]
    turns: u32,

    /// Disable ANSI colors
    #[arg(long)]
    plain: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Style {
    /// Run every treatment in sequence
    All,
    /// Sweep a highlight over a stable logo
    Shimmer,
    /// Reveal the A, then draw its check
    Draw,
    /// Rock the logo around its vertical axis
    Tilt,
    /// Render the undistorted quadrant-cell logo
    Hires,
}

impl Style {
    fn label(self) -> &'static str {
        match self {
            Self::All => "showcase",
            Self::Shimmer => "shimmer",
            Self::Draw => "draw",
            Self::Tilt => "tilt",
            Self::Hires => "hires",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pixel {
    Purple,
    White,
}

#[derive(Clone, Copy)]
struct Canvas {
    left: u16,
    top: u16,
    width: usize,
    height: usize,
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter(stdout: &mut io::Stdout) -> Result<Self> {
        terminal::enable_raw_mode()?;
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide, Clear(ClearType::All)) {
            let _ = terminal::disable_raw_mode();
            return Err(error.into());
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = execute!(stdout, ResetColor, Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let logo = load_logo()?;
    let mut stdout = io::stdout();

    if !stdout.is_terminal() {
        render_static(&mut stdout, &logo, args.plain)?;
        return Ok(());
    }

    let _terminal = TerminalGuard::enter(&mut stdout)?;
    let frame_time = Duration::from_secs_f64(1.0 / f64::from(args.fps));
    let started = Instant::now();

    loop {
        let frame_started = Instant::now();
        let elapsed = started.elapsed().as_secs_f32();
        let (style, phase, finished) = animation_at(args.style, args.turns, elapsed);
        if finished {
            break;
        }

        render_frame(&mut stdout, &logo, style, phase, args.plain)?;

        if should_quit(frame_time.saturating_sub(frame_started.elapsed()))? {
            break;
        }
        thread::sleep(frame_time.saturating_sub(frame_started.elapsed()));
    }

    Ok(())
}

fn load_logo() -> Result<Raster> {
    let image = image::load_from_memory(include_bytes!(
        "../src/tui/ui/assets/aven_logo_terminal.png"
    ))?
    .to_luma_alpha8();
    let (width, height) = image.dimensions();
    Ok((0..height)
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
        .collect())
}

fn animation_at(selected: Style, turns: u32, elapsed: f32) -> (Style, f32, bool) {
    let style_count = if matches!(selected, Style::All) {
        SHOWCASE.len()
    } else {
        1
    };
    let cycle_duration = SECONDS_PER_STYLE * style_count as f32;
    let finished = turns > 0 && elapsed >= cycle_duration * turns as f32;
    let within_cycle = elapsed % cycle_duration;
    let index = (within_cycle / SECONDS_PER_STYLE) as usize;
    let style = if matches!(selected, Style::All) {
        SHOWCASE[index.min(SHOWCASE.len() - 1)]
    } else {
        selected
    };
    let phase = (within_cycle % SECONDS_PER_STYLE) / SECONDS_PER_STYLE;
    (style, phase, finished)
}

fn should_quit(wait: Duration) -> Result<bool> {
    if !event::poll(wait)? {
        return Ok(false);
    }
    let Event::Key(key) = event::read()? else {
        return Ok(false);
    };
    if key.kind != KeyEventKind::Press {
        return Ok(false);
    }
    Ok(matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
        || key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

fn render_frame(
    stdout: &mut io::Stdout,
    logo: &Raster,
    style: Style,
    phase: f32,
    plain: bool,
) -> Result<()> {
    let raster = match style {
        Style::Shimmer => logo.clone(),
        Style::Draw => reveal_logo(logo, phase),
        Style::Tilt => {
            let angle = 0.52 * (phase * std::f32::consts::TAU).sin();
            project_logo(logo, angle)
        }
        Style::Hires => logo.clone(),
        Style::All => unreachable!(),
    };
    let canvas_width = logo[0].len().div_ceil(2);
    let canvas_height = logo.len().div_ceil(2);
    let (terminal_width, terminal_height) = terminal::size()?;
    let canvas = Canvas {
        left: terminal_width.saturating_sub(canvas_width as u16) / 2,
        top: terminal_height.saturating_sub(canvas_height as u16 + 2) / 2,
        width: canvas_width,
        height: canvas_height,
    };

    stdout.queue(BeginSynchronizedUpdate)?;
    paint_quadrants(stdout, &raster, canvas, style, phase, plain)?;

    let hint = format!("Aven  •  {}  •  q to quit", style.label());
    let hint_left = terminal_width.saturating_sub(HINT_WIDTH as u16) / 2;
    let hint = format!("{hint:^width$}", width = HINT_WIDTH);
    stdout
        .queue(ResetColor)?
        .queue(SetForegroundColor(Color::DarkGrey))?
        .queue(MoveTo(hint_left, canvas.top + canvas.height as u16 + 1))?
        .queue(Print(hint))?
        .queue(ResetColor)?
        .queue(EndSynchronizedUpdate)?;
    stdout.flush()?;
    Ok(())
}

fn reveal_logo(logo: &Raster, phase: f32) -> Raster {
    let height = logo.len() - 1;
    let width = logo[0].len() - 1;
    logo.iter()
        .enumerate()
        .map(|(y, row)| {
            row.iter()
                .enumerate()
                .map(|(x, pixel)| match pixel {
                    Some(Pixel::Purple) if phase >= 0.04 + y as f32 / height as f32 * 0.50 => {
                        *pixel
                    }
                    Some(Pixel::White) if phase >= 0.56 + x as f32 / width as f32 * 0.28 => *pixel,
                    _ => None,
                })
                .collect()
        })
        .collect()
}

fn project_logo(logo: &Raster, angle: f32) -> Raster {
    let source_width = logo[0].len();
    let scale = angle.cos().max(0.08);
    let projected_width = ((source_width as f32 * scale).round() as usize).max(1);
    let mut output = vec![vec![None; projected_width]; logo.len()];

    for (y, row) in output.iter_mut().enumerate() {
        for (x, pixel) in row.iter_mut().enumerate() {
            let source_x = (((x as f32 + 0.5) / projected_width as f32) * source_width as f32)
                .floor()
                .clamp(0.0, (source_width - 1) as f32) as usize;
            *pixel = logo[y][source_x];
        }
    }

    output
}

fn paint_quadrants(
    stdout: &mut io::Stdout,
    raster: &Raster,
    canvas: Canvas,
    style: Style,
    phase: f32,
    plain: bool,
) -> Result<()> {
    let art_width = raster[0].len().div_ceil(2);
    let art_height = raster.len().div_ceil(2);
    let horizontal_padding = (canvas.width - art_width) / 2;
    let vertical_padding = (canvas.height - art_height) / 2;

    for canvas_y in 0..canvas.height {
        stdout
            .queue(MoveTo(canvas.left, canvas.top + canvas_y as u16))?
            .queue(ResetColor)?;
        let Some(cell_y) = canvas_y.checked_sub(vertical_padding) else {
            stdout.queue(Print(" ".repeat(canvas.width)))?;
            continue;
        };
        if cell_y >= art_height {
            stdout.queue(Print(" ".repeat(canvas.width)))?;
            continue;
        }

        stdout.queue(Print(" ".repeat(horizontal_padding)))?;
        for cell_x in 0..art_width {
            let pixels = [
                pixel_at(raster, cell_x * 2, cell_y * 2),
                pixel_at(raster, cell_x * 2 + 1, cell_y * 2),
                pixel_at(raster, cell_x * 2, cell_y * 2 + 1),
                pixel_at(raster, cell_x * 2 + 1, cell_y * 2 + 1),
            ];
            let white_mask = pixel_mask(&pixels, Pixel::White);
            let purple_mask = pixel_mask(&pixels, Pixel::Purple);
            let occupied_mask = white_mask | purple_mask;

            if plain {
                stdout.queue(Print(quadrant_glyph(occupied_mask)))?;
                continue;
            }

            stdout.queue(ResetColor)?;
            if white_mask > 0 {
                stdout.queue(SetForegroundColor(Color::White))?;
                if occupied_mask == 0b1111 && purple_mask > 0 {
                    stdout.queue(SetBackgroundColor(pixel_color(
                        Pixel::Purple,
                        cell_x * 2,
                        cell_y * 2,
                        raster[0].len(),
                        raster.len(),
                        style,
                        phase,
                    )))?;
                }
                stdout.queue(Print(quadrant_glyph(white_mask)))?;
            } else if purple_mask > 0 {
                stdout
                    .queue(SetForegroundColor(pixel_color(
                        Pixel::Purple,
                        cell_x * 2,
                        cell_y * 2,
                        raster[0].len(),
                        raster.len(),
                        style,
                        phase,
                    )))?
                    .queue(Print(quadrant_glyph(purple_mask)))?;
            } else {
                stdout.queue(Print(' '))?;
            }
        }
        stdout.queue(ResetColor)?.queue(Print(
            " ".repeat(canvas.width - art_width - horizontal_padding),
        ))?;
    }
    Ok(())
}

fn pixel_mask(pixels: &[Option<Pixel>; 4], target: Pixel) -> usize {
    pixels.iter().enumerate().fold(0, |mask, (index, pixel)| {
        mask | usize::from(*pixel == Some(target)) << index
    })
}

fn pixel_at(raster: &Raster, x: usize, y: usize) -> Option<Pixel> {
    raster.get(y).and_then(|row| row.get(x)).copied().flatten()
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

fn pixel_color(
    pixel: Pixel,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    style: Style,
    phase: f32,
) -> Color {
    if pixel == Pixel::White {
        return Color::White;
    }

    let progress = y as f32 / (height - 1) as f32;
    let (start, end, mix) = if progress < 0.52 {
        ((135.0, 40.0, 250.0), (150.0, 63.0, 255.0), progress / 0.52)
    } else {
        (
            (150.0, 63.0, 255.0),
            (176.0, 108.0, 255.0),
            (progress - 0.52) / 0.48,
        )
    };
    let mut rgb = mix_rgb(start, end, mix);

    if matches!(style, Style::Shimmer | Style::Hires) {
        let u = x as f32 / (width - 1) as f32;
        let center = -0.15 + (phase * SHIMMER_SWEEPS_PER_STYLE).fract() * 1.5;
        let distance = (u + progress * 0.24 - center).abs();
        let band = (1.0 - distance / 0.11).clamp(0.0, 1.0);
        rgb = mix_rgb(rgb, (224.0, 195.0, 255.0), band * band * 0.38);
    }

    Color::Rgb {
        r: rgb.0.clamp(0.0, 255.0) as u8,
        g: rgb.1.clamp(0.0, 255.0) as u8,
        b: rgb.2.clamp(0.0, 255.0) as u8,
    }
}

fn mix_rgb(start: (f32, f32, f32), end: (f32, f32, f32), amount: f32) -> (f32, f32, f32) {
    (
        start.0 + (end.0 - start.0) * amount,
        start.1 + (end.1 - start.1) * amount,
        start.2 + (end.2 - start.2) * amount,
    )
}

fn render_static(stdout: &mut io::Stdout, logo: &Raster, plain: bool) -> Result<()> {
    for cell_y in 0..logo.len().div_ceil(2) {
        for cell_x in 0..logo[0].len().div_ceil(2) {
            let pixels = [
                pixel_at(logo, cell_x * 2, cell_y * 2),
                pixel_at(logo, cell_x * 2 + 1, cell_y * 2),
                pixel_at(logo, cell_x * 2, cell_y * 2 + 1),
                pixel_at(logo, cell_x * 2 + 1, cell_y * 2 + 1),
            ];
            let mask = pixels.iter().enumerate().fold(0, |mask, (index, pixel)| {
                mask | usize::from(pixel.is_some()) << index
            });
            let pixel = if pixels.contains(&Some(Pixel::White)) {
                Some(Pixel::White)
            } else if mask > 0 {
                Some(Pixel::Purple)
            } else {
                None
            };
            if !plain && let Some(pixel) = pixel {
                stdout.queue(SetForegroundColor(pixel_color(
                    pixel,
                    cell_x * 2,
                    cell_y * 2,
                    logo[0].len(),
                    logo.len(),
                    Style::Shimmer,
                    0.0,
                )))?;
            }
            stdout.queue(Print(quadrant_glyph(mask)))?;
        }
        if !plain {
            stdout.queue(ResetColor)?;
        }
        stdout.queue(Print('\n'))?;
    }
    stdout.flush()?;
    Ok(())
}
