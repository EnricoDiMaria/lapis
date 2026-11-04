use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn command<S: AsRef<OsStr>>(program: S) -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

pub fn ffmpeg() -> Command {
    command(ffmpeg_path())
}

fn ffmpeg_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LAPIS_FFMPEG").map(PathBuf::from) {
        if p.is_file() {
            return p;
        }
    }
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        for c in [dir.join(exe), dir.join("ffmpeg").join(exe), dir.join("bin").join(exe)] {
            if c.is_file() {
                return c;
            }
        }
    }
    PathBuf::from(exe)
}
