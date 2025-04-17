use std::io;

cfg_if::cfg_if! {
    if #[cfg(target_os = "macos")] {
        const SCRIPT: &str = include_str!("entrypoint-macos.sh");
    } else if #[cfg(target_os = "linux")] {
        const SCRIPT: &str = include_str!("entrypoint-linux.sh");
    } else {
        compile_error!("Unsupported OS");
    }
}

pub(crate) fn print(w: &mut dyn io::Write) -> io::Result<()> {
    w.write_all(SCRIPT.as_bytes())
}
