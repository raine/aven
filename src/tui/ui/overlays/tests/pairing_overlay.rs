use super::*;

const TEST_SERVER: &str = "https://sync.example.test:8443";
const TEST_INVITATION: &str = "AVEN:AEMGC5TFNYXGK6DBNVYGYZJOORSXG5A";

fn presentation() -> std::sync::Arc<crate::pairing::PairingPresentation> {
    std::sync::Arc::new(
        crate::pairing::PairingPresentation::new(
            TEST_SERVER,
            TEST_INVITATION,
            crate::pairing::QrGlyphs::HalfBlock,
        )
        .unwrap(),
    )
}

fn rendered_buffer(
    presentation: &std::sync::Arc<crate::pairing::PairingPresentation>,
    width: u16,
    height: u16,
) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| render_pairing(frame, &PairingOverlay::Ready(presentation.clone())))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn region_text(buffer: &ratatui::buffer::Buffer, area: ratatui::layout::Rect) -> String {
    (area.top()..area.bottom())
        .map(|row| {
            (area.left()..area.right())
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn normal_overlay_renders_shared_compact_qr_rows() {
    let presentation = presentation();
    let layout = pairing_layout(ratatui::layout::Rect::new(0, 0, 160, 80), &presentation);
    let qr_area = layout.qr.expect("expected QR layout");
    let row_index = presentation
        .qr()
        .rows()
        .iter()
        .position(|row| !row.trim().is_empty())
        .unwrap();
    let buffer = rendered_buffer(&presentation, 160, 80);
    let rendered_row = (qr_area.left()..qr_area.right())
        .map(|column| buffer[(column, qr_area.y + row_index as u16)].symbol())
        .collect::<String>();

    assert_eq!(rendered_row, presentation.qr().rows()[row_index]);
    assert!(buffer.content.iter().any(|cell| {
        !cell.symbol().trim().is_empty()
            && cell.fg == ratatui::style::Color::Rgb(0, 0, 0)
            && cell.bg == ratatui::style::Color::Rgb(255, 255, 255)
    }));
}

#[test]
fn header_wraps_complete_copy_without_overlapping_qr_or_footer() {
    let server = format!("https://{}.example.test:8443", "a".repeat(63));
    let presentation = std::sync::Arc::new(
        crate::pairing::PairingPresentation::new(
            &server,
            TEST_INVITATION,
            crate::pairing::QrGlyphs::HalfBlock,
        )
        .unwrap(),
    );
    let layout = pairing_layout(ratatui::layout::Rect::new(0, 0, 200, 100), &presentation);
    let qr = layout.qr.expect("expected QR layout");
    let buffer = rendered_buffer(&presentation, 200, 100);
    let text = region_text(&buffer, layout.content);

    assert!(text.contains(NETWORK_REQUIREMENT));
    assert!(
        text.replace(' ', "")
            .contains(presentation.server_identity())
    );
    assert!(layout.content.bottom() <= qr.top());
    assert!(qr.bottom() <= layout.footer.top());
}

#[test]
fn low_correction_invitation_fits_a_39_row_terminal() {
    let (_, invitation) = crate::sync::encrypted::sample_invitations("https://sync.example.com");
    let presentation = crate::pairing::PairingPresentation::new_tui(
        TEST_SERVER,
        &invitation,
        crate::sync::encrypted::unix_now().unwrap() + 600,
        crate::pairing::QrGlyphs::HalfBlock,
    )
    .unwrap();

    let layout = pairing_layout(ratatui::layout::Rect::new(0, 0, 155, 39), &presentation);
    assert!(layout.qr.is_some());
}

#[test]
fn constrained_overlay_renders_complete_actionable_fallback_without_secrets() {
    let presentation = presentation();
    let layout = pairing_layout(ratatui::layout::Rect::new(0, 0, 40, 12), &presentation);
    assert!(layout.qr.is_none());

    let buffer = rendered_buffer(&presentation, 40, 12);
    let text = region_text(&buffer, layout.content);
    assert!(text.contains("Press c to copy"));
    assert!(text.contains("aven sync invite"));
    assert!(!text.contains('`'));
    assert!(
        buffer
            .content
            .iter()
            .any(|cell| cell.symbol() == "v" && cell.fg == crate::tui::theme::BLUE)
    );
    assert!(text.contains(NETWORK_REQUIREMENT));
    assert!(!text.contains("AVEN:"));
    assert!(!buffer.content.iter().any(|cell| {
        cell.fg == ratatui::style::Color::Rgb(0, 0, 0)
            && cell.bg == ratatui::style::Color::Rgb(255, 255, 255)
    }));
}

#[test]
fn layout_saturates_for_minimal_terminal_dimensions() {
    let presentation = presentation();
    for terminal in [
        ratatui::layout::Rect::new(0, 0, 0, 0),
        ratatui::layout::Rect::new(4, 7, 1, 1),
        ratatui::layout::Rect::new(4, 7, 2, 2),
    ] {
        let layout = pairing_layout(terminal, &presentation);
        assert!(layout.qr.is_none());
        assert!(layout.area.x >= terminal.x);
        assert!(layout.area.y >= terminal.y);
        assert!(layout.area.right() <= terminal.right());
        assert!(layout.area.bottom() <= terminal.bottom());
    }
}

#[test]
fn overlay_presents_only_safe_pairing_data() {
    let presentation = presentation();
    let rendered = render_overlay_view_at(
        OverlayView::Pairing(PairingOverlay::Ready(presentation)),
        160,
        80,
    );

    assert!(rendered.contains("Sync › Add device"));
    assert!(rendered.contains("https://sync.example.test:8443"));
    assert!(rendered.contains("c copy invitation"));
    assert!(!rendered.contains("AVEN:"));
}

#[test]
fn creating_page_shows_a_spinner_in_place_of_the_qr_code() {
    let rendered = render_overlay_view_at(
        OverlayView::Pairing(PairingOverlay::Creating {
            started_at: std::time::Instant::now(),
        }),
        160,
        80,
    );

    assert!(rendered.contains("Sync › Add device"));
    assert!(rendered.contains("⠋ Creating invitation…"));
    assert!(rendered.contains("Esc back"));
    assert!(!rendered.contains("copy invitation"));
}

#[test]
fn failed_page_offers_retry_and_back() {
    let rendered = render_overlay_view_at(
        OverlayView::Pairing(PairingOverlay::Failed(
            "Couldn't reach the sync server.".into(),
        )),
        160,
        80,
    );

    assert!(rendered.contains("Sync › Add device"));
    assert!(rendered.contains("Couldn't create the invitation"));
    assert!(rendered.contains("Couldn't reach the sync server."));
    assert!(rendered.contains("Enter retry"));
    assert!(rendered.contains("Esc back"));
}

fn tui_presentation(glyphs: crate::pairing::QrGlyphs) -> crate::pairing::PairingPresentation {
    let (_, invitation) = crate::sync::encrypted::sample_invitations("https://sync.example.com");
    crate::pairing::PairingPresentation::new_tui(
        TEST_SERVER,
        &invitation,
        crate::sync::encrypted::unix_now().unwrap() + 600,
        glyphs,
    )
    .unwrap()
}

const GLYPH_MODES: [crate::pairing::QrGlyphs; 2] = [
    crate::pairing::QrGlyphs::HalfBlock,
    crate::pairing::QrGlyphs::Sextant,
];

#[test]
fn large_terminal_widens_dialog_to_text_width_and_centers_qr() {
    for glyphs in GLYPH_MODES {
        let presentation = tui_presentation(glyphs);
        let layout = pairing_layout(ratatui::layout::Rect::new(0, 0, 200, 60), &presentation);
        let qr = layout.qr.expect("QR fits a large terminal");

        assert!(layout.area.width >= 64, "{glyphs:?}");
        assert!(layout.area.width >= qr.width + 4, "{glyphs:?}");
        let left = qr.x - layout.inner.x;
        let right = layout.inner.right() - qr.right();
        assert!(left.abs_diff(right) <= 1, "{glyphs:?}: {left} vs {right}");
    }
}

#[test]
fn qr_fit_depends_on_terminal_width_not_text_width() {
    let hints = "c copy invitation  Esc back".len() as u16;
    for glyphs in GLYPH_MODES {
        let presentation = tui_presentation(glyphs);
        // QR or footer hints, plus dialog chrome and the terminal margin.
        let needed = (presentation.qr().width() as u16).max(hints) + 4 + 2;
        assert!(needed < 64, "{glyphs:?} should fit below the text width");
        let fits = pairing_layout(ratatui::layout::Rect::new(0, 0, needed, 200), &presentation);
        assert!(fits.qr.is_some(), "{glyphs:?}");
        let narrow = pairing_layout(
            ratatui::layout::Rect::new(0, 0, needed - 1, 200),
            &presentation,
        );
        assert!(narrow.qr.is_none(), "{glyphs:?}");
    }
}

#[test]
fn qr_fit_depends_on_terminal_height() {
    for glyphs in GLYPH_MODES {
        let presentation = tui_presentation(glyphs);
        let tall = pairing_layout(ratatui::layout::Rect::new(0, 0, 160, 100), &presentation);
        let needed = tall.area.height + 2;
        let exact = pairing_layout(ratatui::layout::Rect::new(0, 0, 160, needed), &presentation);
        assert!(exact.qr.is_some(), "{glyphs:?}");
        let short = pairing_layout(
            ratatui::layout::Rect::new(0, 0, 160, needed - 1),
            &presentation,
        );
        assert!(short.qr.is_none(), "{glyphs:?}");
    }
}

#[test]
fn qr_dialog_keeps_header_lines_whole_and_footer_untruncated() {
    for glyphs in GLYPH_MODES {
        let presentation = std::sync::Arc::new(tui_presentation(glyphs));
        let buffer = rendered_buffer(&presentation, 160, 60);
        let layout = pairing_layout(ratatui::layout::Rect::new(0, 0, 160, 60), &presentation);
        assert!(layout.qr.is_some(), "{glyphs:?}");
        let footer = region_text(&buffer, layout.footer);
        assert_eq!(footer, "c copy invitation Esc back", "{glyphs:?}");
        let header = region_text(&buffer, layout.content);
        assert!(
            header.contains(&format!("Server: {TEST_SERVER}")),
            "{glyphs:?}"
        );
        assert!(header.contains("aven sync join"), "{glyphs:?}");
    }
}
