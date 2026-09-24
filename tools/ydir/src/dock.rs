//! Dock mode (`ydir --dock`): one listing plus a preview.
//!
//! ymux's own file dock no longer runs this: it is a GUI files pane now,
//! follows the active pane's directory itself (spec §5 step 2) and opens
//! files in an editor pane with a direct call (step 3). So `--dock` is only
//! a layout here — Enter on a file runs ycode inline, as plain `ydir` does.
//! It stays for the one release in which `ydir` is still on PATH.

use std::path::PathBuf;

/// Command line: `ydir [--dock] [DIR]`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub dir: Option<PathBuf>,
    pub dock: bool,
}

pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Args {
    let mut out = Args::default();
    for arg in args {
        if arg == "--dock" {
            out.dock = true;
        } else if out.dir.is_none() {
            out.dir = Some(PathBuf::from(arg));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Args {
        parse_args(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parse_args_keeps_the_plain_dir_argument() {
        assert_eq!(args(&[]), Args::default());
        assert_eq!(
            args(&["/work"]),
            Args {
                dir: Some(PathBuf::from("/work")),
                dock: false
            }
        );
    }

    #[test]
    fn parse_args_accepts_dock_before_or_after_the_dir() {
        let want = Args {
            dir: Some(PathBuf::from("/work")),
            dock: true,
        };
        assert_eq!(args(&["--dock", "/work"]), want);
        assert_eq!(args(&["/work", "--dock"]), want);
    }
}
