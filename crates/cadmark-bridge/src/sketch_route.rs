// Reading the route the model committed to out of what it said. An
// element with both a sketch ancestor and a 3D form can be changed at
// either level, and the prompt asks for the level to be announced on a
// line of its own before the script changes. This module turns that line
// back into a value, so whatever scores a reply — a grader over sampled
// turns, a later check that the change matched — reads the announcement
// one way rather than each inventing its own.

/// The level an announced edit changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SketchRoute {
    /// The sketch profile the element descends from.
    Sketch,
    /// The 3D form the element belongs to.
    Solid,
}

/// A route the model announced, with the element it named if it named one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteAnnouncement {
    pub route: SketchRoute,
    /// What the announcement said it was changing, absent when the line
    /// carried only the level.
    pub element: Option<String>,
}

const MARKER: &str = "route:";

/// The route announced in this text, or `None` when it announces none.
///
/// A turn may correct itself before running again, so the last
/// announcement is the one that stands. Prose about sketches and solids
/// is not an announcement: only a line whose first content is the marker
/// followed by exactly `sketch` or `solid` counts.
pub fn announced_route(text: &str) -> Option<RouteAnnouncement> {
    text.lines().rev().find_map(announcement_on_line)
}

fn announcement_on_line(line: &str) -> Option<RouteAnnouncement> {
    let line = line.trim().trim_start_matches(['-', '*', '#', '>', ' ']);
    let rest = line
        .get(..MARKER.len())
        .filter(|start| start.eq_ignore_ascii_case(MARKER))
        .map(|_| &line[MARKER.len()..])?;
    let rest = rest.trim();
    let separators = ['\u{2014}', '-', ':', ','];
    let (level, element) = match rest.find(separators) {
        Some(cut) => (&rest[..cut], rest[cut..].trim_start_matches(separators)),
        None => (rest, ""),
    };
    let tidy = |text: &str| {
        text.trim_matches(|c: char| c.is_whitespace() || matches!(c, '*' | '`' | '.' | '"'))
            .to_string()
    };
    let route = match tidy(level).to_ascii_lowercase().as_str() {
        "sketch" => SketchRoute::Sketch,
        "solid" => SketchRoute::Solid,
        _ => return None,
    };
    let element = tidy(element);
    Some(RouteAnnouncement {
        route,
        element: (!element.is_empty()).then_some(element),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announces_the_level_and_the_element_it_names() {
        let announcement =
            announced_route("Route: sketch — the corner of the base profile\n\nRounding it there.")
                .expect("the line announces a route");
        assert_eq!(announcement.route, SketchRoute::Sketch);
        assert_eq!(
            announcement.element.as_deref(),
            Some("the corner of the base profile")
        );

        let solid = announced_route("Route: solid — the vertical edge of the boss")
            .expect("the line announces a route");
        assert_eq!(solid.route, SketchRoute::Solid);
    }

    #[test]
    fn a_level_without_an_element_still_announces_a_route() {
        let announcement = announced_route("Route: solid").expect("the line announces a route");
        assert_eq!(announcement.route, SketchRoute::Solid);
        assert_eq!(announcement.element, None);
    }

    #[test]
    fn prose_about_sketches_and_solids_is_not_an_announcement() {
        for text in [
            "I filleted the sketch corner rather than the solid edge.",
            "The route: sketch or solid, depending on the case.",
            "Reroute: sketch",
            "Route: both — the profile and the boss",
            "Route:",
            "",
        ] {
            assert_eq!(announced_route(text), None, "parsed a route from {text:?}");
        }
    }

    #[test]
    fn a_corrected_route_replaces_the_one_before_it() {
        let announcement = announced_route(
            "Route: sketch — the profile corner\nThat run failed.\nRoute: solid — the boss edge",
        )
        .expect("the later line announces a route");
        assert_eq!(announcement.route, SketchRoute::Solid);
        assert_eq!(announcement.element.as_deref(), Some("the boss edge"));
    }

    #[test]
    fn a_marked_up_or_indented_line_still_announces() {
        for text in [
            "- **Route:** solid, the vertical edge",
            "    route: SOLID - the vertical edge",
        ] {
            let announcement = announced_route(text).expect("the line announces a route");
            assert_eq!(announcement.route, SketchRoute::Solid, "on {text:?}");
            assert_eq!(
                announcement.element.as_deref(),
                Some("the vertical edge"),
                "on {text:?}"
            );
        }
    }
}
