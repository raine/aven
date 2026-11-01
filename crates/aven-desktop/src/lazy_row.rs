//! A list entry of known height, built only if it is painted.
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Style, Window, px, relative, size,
};

type Build = Box<dyn FnOnce(&mut Window, &mut App) -> AnyElement>;

/// Measuring a list lays out every entry and throws the elements away. This one
/// takes its height as given, so only entries on screen pay for their content.
/// The content must be exactly `height` tall.
pub struct LazyRow {
    height: Pixels,
    build: Option<Build>,
}

impl LazyRow {
    pub fn new(
        height: f32,
        build: impl FnOnce(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            height: px(height),
            build: Some(Box::new(build)),
        }
    }
}

impl IntoElement for LazyRow {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for LazyRow {
    type RequestLayoutState = ();
    type PrepaintState = Option<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            size: size(relative(1.).into(), self.height.into()),
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let mut content = self.build.take()?(window, cx);
        let space = size(
            AvailableSpace::Definite(bounds.size.width),
            AvailableSpace::MinContent,
        );
        content.prepaint_as_root(bounds.origin, space, window, cx);
        Some(content)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        content: &mut Option<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(content) = content {
            content.paint(window, cx);
        }
    }
}
