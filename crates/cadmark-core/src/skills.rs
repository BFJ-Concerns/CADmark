//! Built-in, explicitly invoked modelling guidance shared by chat and the turn loop.

#[derive(Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub instructions: &'static str,
}

pub const BUILT_IN: &[Skill] = &[Skill {
    name: "3d-printing",
    title: "3D printing",
    description: "Review or design a part for filament printing",
    instructions: include_str!("../../../agent-docs/guides/3d-printing.md"),
}];

/// Recognise a whole command at the start of a message. Mentions, paths,
/// quoted text and code elsewhere in the message remain ordinary text.
pub fn invoked(text: &str) -> Option<&'static Skill> {
    let command = text.split_whitespace().next()?;
    let name = command
        .strip_prefix('/')
        .or_else(|| command.strip_prefix('$'))?;
    BUILT_IN.iter().find(|skill| skill.name == name)
}

/// Select each skill once from the current turn's chat and comment text.
pub fn for_turn<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<&'static Skill> {
    let mut selected = Vec::new();
    for text in texts {
        if let Some(skill) = invoked(text)
            && !selected.contains(&skill)
        {
            selected.push(skill);
        }
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_accept_both_prefixes_and_whitespace() {
        for text in [
            "/3d-printing",
            "$3d-printing review this",
            " \n/3d-printing\nmake a bolt",
        ] {
            assert_eq!(invoked(text), Some(&BUILT_IN[0]), "{text}");
        }
        assert_eq!(
            for_turn(["/3d-printing", "$3d-printing"]),
            vec![&BUILT_IN[0]]
        );
    }

    #[test]
    fn ordinary_text_and_incomplete_commands_do_not_activate_skills() {
        for text in [
            "",
            "/",
            "$",
            "/unknown",
            "/3d-printing-extra",
            "/3d-printing/file",
            "$3d-printing.py",
            "Explain /3d-printing",
            "`/3d-printing`",
            "```\n/3d-printing\n```",
            "https://example.com/3d-printing",
        ] {
            assert_eq!(invoked(text), None, "{text}");
        }
    }
}
