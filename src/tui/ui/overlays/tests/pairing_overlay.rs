use super::*;

const TEST_SERVER: &str = "https://sync.example.test:8443/aven";
const TEST_TOKEN: &str = "pairing-token-fixture-0123456789";

fn presentation() -> std::sync::Arc<crate::pairing::PairingPresentation> {
    std::sync::Arc::new(
        crate::pairing::PairingPresentation::new(TEST_SERVER.to_string(), TEST_TOKEN.to_string())
            .unwrap(),
    )
}

fn rendered_buffer(
    presentation: &crate::pairing::PairingPresentation,
    width: u16,
    height: u16,
) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| render_pairing(frame, presentation))
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
            && cell.fg == ratatui::style::Color::Black
            && cell.bg == ratatui::style::Color::White
    }));
}

#[test]
fn header_wraps_complete_copy_without_overlapping_qr_or_footer() {
    let server = format!("https://{}.example.test:8443/aven", "a".repeat(63));
    let presentation =
        crate::pairing::PairingPresentation::new(server, TEST_TOKEN.to_string()).unwrap();
    let layout = pairing_layout(ratatui::layout::Rect::new(0, 0, 200, 100), &presentation);
    let qr = layout.qr.expect("expected QR layout");
    let buffer = rendered_buffer(&presentation, 200, 100);
    let text = region_text(&buffer, layout.content);

    assert!(text.contains(NETWORK_REQUIREMENT));
    assert!(
        text.replace(' ', "")
            .contains(presentation.server_identity())
    );
    assert!(layout.content.bottom() < qr.top());
    assert!(qr.bottom() < layout.footer.top());
}

#[test]
fn constrained_overlay_renders_complete_actionable_fallback_without_secrets() {
    let presentation = presentation();
    let layout = pairing_layout(ratatui::layout::Rect::new(0, 0, 40, 12), &presentation);
    assert!(layout.qr.is_none());

    let buffer = rendered_buffer(&presentation, 40, 12);
    let text = region_text(&buffer, layout.content);
    assert!(text.contains("Run `aven sync pair` in a larger terminal."));
    assert!(text.contains(NETWORK_REQUIREMENT));
    assert!(!text.contains(TEST_TOKEN));
    assert!(!text.contains("aven://pair/"));
    assert!(!buffer.content.iter().any(|cell| {
        cell.fg == ratatui::style::Color::Black && cell.bg == ratatui::style::Color::White
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
    let rendered = render_overlay_view_at(OverlayView::Pairing(presentation), 160, 80);

    assert!(rendered.contains("Pair mobile device"));
    assert!(rendered.contains("https://sync.example.test:8443"));
    assert!(!rendered.contains(TEST_TOKEN));
    assert!(!rendered.contains("aven://pair/"));
}
