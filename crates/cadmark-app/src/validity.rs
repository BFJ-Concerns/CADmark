// User-facing validity wording and export decisions. The kernel owns the
// check; this module only turns its plain results into application behaviour.

use cadmark_core::geometry::SolidValidity;

/// Whether the current model may be sent to the exporter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExportDecision {
    Ready,
    NoSolid,
    Invalid { parts: Vec<usize> },
}

/// Decide from the kernel's per-solid results without re-checking geometry.
pub(crate) fn export_decision(validity: &[SolidValidity]) -> ExportDecision {
    if validity.is_empty() {
        return ExportDecision::NoSolid;
    }

    let parts: Vec<_> = validity
        .iter()
        .enumerate()
        .filter_map(|(index, solid)| (!solid.is_printable()).then_some(index + 1))
        .collect();
    if parts.is_empty() {
        ExportDecision::Ready
    } else {
        ExportDecision::Invalid { parts }
    }
}

/// State each produced part's result so a mixed model does not hide the bad one.
pub(crate) fn describe_validity(validity: &[SolidValidity]) -> String {
    if validity.is_empty() {
        return "No solid was produced.".to_string();
    }

    validity
        .iter()
        .enumerate()
        .map(|(index, solid)| {
            if solid.is_printable() {
                format!("Part {} is closed and valid.", index + 1)
            } else {
                format!(
                    "Part {} is NOT a closed valid solid; it will not print.",
                    index + 1
                )
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Explain why export did not start. This is a non-blocking status message.
pub(crate) fn export_warning(decision: &ExportDecision) -> Option<String> {
    match decision {
        ExportDecision::Ready => None,
        ExportDecision::NoSolid => Some("Cannot export: no solid was produced.".to_string()),
        ExportDecision::Invalid { parts } => Some(format!(
            "Cannot export: {} {} not a closed valid solid.",
            parts_label(parts),
            if parts.len() == 1 { "is" } else { "are" }
        )),
    }
}

fn parts_label(parts: &[usize]) -> String {
    match parts {
        [part] => format!("Part {part}"),
        _ => format!(
            "Parts {}",
            parts
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: SolidValidity = SolidValidity {
        closed: true,
        valid: true,
    };
    const OPEN: SolidValidity = SolidValidity {
        closed: false,
        valid: true,
    };
    const INVALID: SolidValidity = SolidValidity {
        closed: true,
        valid: false,
    };

    #[test]
    fn export_allows_only_a_model_where_every_part_is_printable() {
        assert_eq!(export_decision(&[VALID, VALID]), ExportDecision::Ready);
        assert_eq!(
            export_decision(&[VALID, OPEN, INVALID]),
            ExportDecision::Invalid { parts: vec![2, 3] }
        );
    }

    #[test]
    fn no_solid_is_not_reported_as_an_invalid_part() {
        let decision = export_decision(&[]);
        assert_eq!(decision, ExportDecision::NoSolid);
        assert_eq!(
            export_warning(&decision).as_deref(),
            Some("Cannot export: no solid was produced.")
        );
    }

    #[test]
    fn validity_status_names_each_part_including_the_offending_one() {
        assert_eq!(
            describe_validity(&[VALID, OPEN]),
            "Part 1 is closed and valid. Part 2 is NOT a closed valid solid; it will not print."
        );
        assert_eq!(
            export_warning(&ExportDecision::Invalid { parts: vec![2] }).as_deref(),
            Some("Cannot export: Part 2 is not a closed valid solid.")
        );
    }
}
