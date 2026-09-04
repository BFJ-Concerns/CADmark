// What the binary does with its command line: choose a folder, or choose
// nothing and let the user pick. Kept apart from the frame loop so the
// decision can be read — and asserted — without a window.

use std::path::PathBuf;

/// What the application shows when it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchTarget {
    /// No folder was named: the start view offers recent projects, opening
    /// a folder and creating one, and nothing loads until one is chosen.
    StartView,
    /// A folder was named on the command line: open it directly.
    Folder(PathBuf),
}

/// The launch decision for a process's arguments, as `std::env::args`
/// yields them: the program name, then what the user typed. This is the
/// entry point's whole decision — `main` reads no argument of its own —
/// so the working directory has nowhere left to enter from.
pub fn target_from_args(args: impl IntoIterator<Item = String>) -> LaunchTarget {
    launch_target(args.into_iter().nth(1).map(PathBuf::from))
}

/// Decide what to show for the folder argument, if any. Absence is a
/// choice the user has not made yet, never the working directory.
pub fn launch_target(argument: Option<PathBuf>) -> LaunchTarget {
    match argument {
        Some(dir) => LaunchTarget::Folder(dir),
        None => LaunchTarget::StartView,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_folder_argument_starts_without_a_project() {
        assert_eq!(launch_target(None), LaunchTarget::StartView);

        // The working directory in particular must not be reached for:
        // loading it is the falsifier this decision exists to prevent.
        let cwd = std::env::current_dir().unwrap();
        assert_ne!(launch_target(None), LaunchTarget::Folder(cwd));
    }

    #[test]
    fn a_command_line_carrying_only_the_program_name_starts_without_a_project() {
        // The argument vector the binary is actually given, not the parsed
        // option: this is the step that used to fall back to the working
        // directory, and the only step `main` has.
        let cwd = std::env::current_dir().unwrap();
        let argv = |args: &[&str]| {
            target_from_args(args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
        };

        assert_eq!(argv(&["cadmark"]), LaunchTarget::StartView);
        assert_ne!(argv(&["cadmark"]), LaunchTarget::Folder(cwd));
        assert_eq!(argv(&[]), LaunchTarget::StartView);
        assert_eq!(
            argv(&["cadmark", "/parts/bracket"]),
            LaunchTarget::Folder(PathBuf::from("/parts/bracket"))
        );
    }

    #[test]
    fn a_named_folder_opens_directly() {
        assert_eq!(
            launch_target(Some(PathBuf::from("/tmp/bracket"))),
            LaunchTarget::Folder(PathBuf::from("/tmp/bracket"))
        );
    }
}
