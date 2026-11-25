//! The one place for platform-specific process code.

use std::ffi::OsStr;
use std::process::Command;

/// A `Command` for a program the app runs in the background.
///
/// On Windows it starts with `CREATE_NO_WINDOW`; otherwise a GUI app that
/// spawns a console program flashes a console window. Convert it with
/// `smol::process::Command::from` for async use.
pub(crate) fn background_command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
