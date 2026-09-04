// The part scripts a project folder holds: which ones exist, which one is
// open, and what the user's save key means for it. A folder may hold any
// number of parts and they share the folder's one design history.
//
// A part created in the application has no name of its own until the user
// saves it. Until then it is written to `UNTITLED_PART` so it can be
// modelled, edited and executed like any other; the first save asks for a
// name, renames the file, and from then on the same save key names a
// version of it.

use std::path::Path;

/// The extension every part script carries.
pub const PART_EXTENSION: &str = "py";

/// The part a folder with no scripts is opened at, so an empty project
/// behaves as it always has.
pub const DEFAULT_PART: &str = "part.py";

/// Where a part lives before its first save names it. Reserved: the user
/// cannot choose this name, because it is the name that means "unnamed".
pub const UNTITLED_PART: &str = "Untitled.py";

/// The part of a folder the application currently has open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenPart {
    /// Created in the application and not yet named by a save.
    Untitled,
    /// A part with a name of its own, which is its file name.
    Named(String),
}

impl OpenPart {
    /// The file this part is read from and written to. An untitled part
    /// still has a file — what it lacks is a name the user chose.
    pub fn file_name(&self) -> &str {
        match self {
            Self::Untitled => UNTITLED_PART,
            Self::Named(name) => name,
        }
    }

    /// Whether the user has named this part.
    pub fn is_named(&self) -> bool {
        matches!(self, Self::Named(_))
    }

    /// How the part is shown in the toolbar and the code panel.
    pub fn display_name(&self) -> String {
        part_display_name(self.file_name())
    }

    /// The part a folder should open at: the file named if it is a part
    /// script, else the first part in the folder, else the default.
    pub fn for_folder(dir: &Path, preferred: Option<&str>) -> Self {
        let parts = list_parts(dir);
        let chosen = preferred
            .filter(|name| parts.iter().any(|part| part == name))
            .map(str::to_string)
            .or_else(|| parts.first().cloned());
        match chosen {
            Some(name) if name == UNTITLED_PART => Self::Untitled,
            Some(name) => Self::Named(name),
            None => Self::Named(DEFAULT_PART.to_string()),
        }
    }
}

/// What the save key does for the part now open. The meaning changes once,
/// when the part is first named; it is not a mode the user selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveTarget {
    /// The part has no name: the save asks for one, which becomes its file
    /// name.
    PartName,
    /// The part is named: the save names a version of it.
    VersionName,
}

/// Resolve the save key against the open part.
pub fn save_target(part: &OpenPart) -> SaveTarget {
    match part {
        OpenPart::Untitled => SaveTarget::PartName,
        OpenPart::Named(_) => SaveTarget::VersionName,
    }
}

/// The part scripts in a folder, by file name, sorted so the list the user
/// switches between is stable between frames. Hidden files, directories
/// and anything that is not a script are left out.
pub fn list_parts(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut parts: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|extension| extension == PART_EXTENSION)
        })
        .collect();
    parts.sort();
    parts
}

/// How a part file is shown to the user: its name without the extension.
pub fn part_display_name(file_name: &str) -> String {
    file_name
        .strip_suffix(&format!(".{PART_EXTENSION}"))
        .unwrap_or(file_name)
        .to_string()
}

/// Turn what the user typed in the naming prompt into a part file name,
/// or say why it cannot be one. `existing` is the folder's parts.
pub fn part_file_name(input: &str, existing: &[String]) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Give the part a name".to_string());
    }
    if trimmed.contains(['/', '\\']) {
        return Err("A part name cannot contain a path separator".to_string());
    }
    if trimmed.starts_with('.') {
        return Err("A part name cannot start with a dot".to_string());
    }
    if trimmed == ".." || trimmed == "." {
        return Err("That is not a part name".to_string());
    }

    let file_name = match trimmed.strip_suffix(&format!(".{PART_EXTENSION}")) {
        Some("") => return Err("Give the part a name".to_string()),
        Some(_) => trimmed.to_string(),
        None => format!("{trimmed}.{PART_EXTENSION}"),
    };

    if file_name.eq_ignore_ascii_case(UNTITLED_PART) {
        return Err(
            "\u{201C}Untitled\u{201D} is what an unnamed part is called; \
                    choose another name"
                .to_string(),
        );
    }
    if existing
        .iter()
        .any(|part| part.eq_ignore_ascii_case(&file_name))
    {
        return Err(format!(
            "This folder already has a part called {}",
            part_display_name(&file_name)
        ));
    }
    Ok(file_name)
}

/// Name the folder's untitled part: validate what the user typed and give
/// the file that name. An untitled part with nothing written yet has no
/// file to rename, and the name still takes effect.
pub fn name_untitled_part(dir: &Path, typed: &str) -> Result<String, String> {
    let file_name = part_file_name(typed, &list_parts(dir))?;
    let from = dir.join(UNTITLED_PART);
    let to = dir.join(&file_name);
    if from.exists() {
        std::fs::rename(&from, &to).map_err(|error| format!("Could not name the part: {error}"))?;
    }
    Ok(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unnamed_part_cannot_be_saved_to_a_file_name_until_it_has_one() {
        // The save key on an unnamed part asks for the part's name, not a
        // version's; a named part's save names a version.
        assert_eq!(save_target(&OpenPart::Untitled), SaveTarget::PartName);
        assert_eq!(
            save_target(&OpenPart::Named("bracket.py".into())),
            SaveTarget::VersionName
        );

        // And nothing the prompt can be given without a name yields one.
        for empty in ["", "   ", "\t", ".py"] {
            assert!(
                part_file_name(empty, &[]).is_err(),
                "{empty:?} must not name a part"
            );
        }
        assert!(part_file_name("Untitled", &[]).is_err());
        assert!(part_file_name("untitled.py", &[]).is_err());
    }

    #[test]
    fn a_typed_name_becomes_the_file_name() {
        assert_eq!(part_file_name("bracket", &[]).unwrap(), "bracket.py");
        assert_eq!(part_file_name("  bracket ", &[]).unwrap(), "bracket.py");
        assert_eq!(part_file_name("bracket.py", &[]).unwrap(), "bracket.py");
        assert_eq!(part_display_name("bracket.py"), "bracket");
        assert_eq!(part_display_name(UNTITLED_PART), "Untitled");
    }

    #[test]
    fn a_name_that_would_escape_the_folder_or_collide_is_refused() {
        assert!(part_file_name("../escape", &[]).is_err());
        assert!(part_file_name("sub/bracket", &[]).is_err());
        assert!(part_file_name(".hidden", &[]).is_err());
        let existing = vec!["bracket.py".to_string()];
        assert!(part_file_name("bracket", &existing).is_err());
        assert!(part_file_name("BRACKET", &existing).is_err());
        assert!(part_file_name("housing", &existing).is_ok());
    }

    #[test]
    fn naming_an_untitled_part_gives_its_file_the_name_and_leaves_it_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        std::fs::write(path.join(UNTITLED_PART), "part = Box(10, 10, 10)").unwrap();

        // Until a name is accepted the part stays where it is.
        assert!(name_untitled_part(path, "  ").is_err());
        assert!(path.join(UNTITLED_PART).exists());
        assert_eq!(list_parts(path), vec![UNTITLED_PART]);

        let named = name_untitled_part(path, "bracket").unwrap();

        assert_eq!(named, "bracket.py");
        assert!(!path.join(UNTITLED_PART).exists());
        assert_eq!(
            std::fs::read_to_string(path.join("bracket.py")).unwrap(),
            "part = Box(10, 10, 10)"
        );
        assert_eq!(list_parts(path), vec!["bracket.py"]);

        // A name already taken by another part is refused, and refusing it
        // leaves both files alone.
        std::fs::write(path.join(UNTITLED_PART), "part = Cylinder(5, 10)").unwrap();
        assert!(name_untitled_part(path, "bracket").is_err());
        assert_eq!(
            std::fs::read_to_string(path.join("bracket.py")).unwrap(),
            "part = Box(10, 10, 10)"
        );
        assert!(path.join(UNTITLED_PART).exists());
    }

    #[test]
    fn a_folder_holds_any_number_of_parts_and_opens_at_one_of_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        // An empty folder still opens at a part, so a new project behaves
        // as it always has.
        assert!(list_parts(path).is_empty());
        assert_eq!(
            OpenPart::for_folder(path, None),
            OpenPart::Named(DEFAULT_PART.to_string())
        );

        std::fs::write(path.join("housing.py"), "").unwrap();
        std::fs::write(path.join("bracket.py"), "").unwrap();
        std::fs::write(path.join("notes.txt"), "").unwrap();
        std::fs::write(path.join(".hidden.py"), "").unwrap();
        std::fs::create_dir(path.join("nested.py")).unwrap();

        assert_eq!(list_parts(path), vec!["bracket.py", "housing.py"]);
        assert_eq!(
            OpenPart::for_folder(path, None),
            OpenPart::Named("bracket.py".to_string())
        );
        assert_eq!(
            OpenPart::for_folder(path, Some("housing.py")),
            OpenPart::Named("housing.py".to_string())
        );
        // A remembered part that has since been deleted falls back rather
        // than opening a file that is not there.
        assert_eq!(
            OpenPart::for_folder(path, Some("gone.py")),
            OpenPart::Named("bracket.py".to_string())
        );

        std::fs::remove_file(path.join("bracket.py")).unwrap();
        std::fs::remove_file(path.join("housing.py")).unwrap();
        std::fs::write(path.join(UNTITLED_PART), "").unwrap();
        assert_eq!(OpenPart::for_folder(path, None), OpenPart::Untitled);
        assert!(!OpenPart::Untitled.is_named());
        assert_eq!(OpenPart::Untitled.display_name(), "Untitled");
    }
}
