use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

macro_rules! isolated_dir {
    ($test:ident) => {
        match $crate::test_support::isolated_dir_for(concat!(
            module_path!(),
            "::",
            stringify!($test)
        )) {
            Some(root) => root,
            None => return,
        }
    };
}

pub(crate) use isolated_dir;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The child uses normal platform paths with an isolated environment and clean process caches.
pub(crate) fn isolated_dir_for(qualified_test: &str) -> Option<PathBuf> {
    const CHILD_ROOT: &str = "LAPIS_TEST_CHILD_ROOT";
    if let Some(root) = env::var_os(CHILD_ROOT) {
        return Some(root.into());
    }

    let (_, test) = qualified_test.split_once("::").unwrap();
    let root = TempDir(env::temp_dir().join(format!("lapis-test-{}", crate::docid::new_id())));
    fs::create_dir(&root.0).expect("create isolated test directory");
    let output = Command::new(env::current_exe().expect("locate test executable"))
        .args(["--include-ignored", "--exact", test, "--nocapture"])
        .env(CHILD_ROOT, &root.0)
        .env("XDG_DATA_HOME", root.0.join("data"))
        .env("XDG_CONFIG_HOME", root.0.join("config"))
        .env("LOCALAPPDATA", root.0.join("data"))
        .env("APPDATA", root.0.join("config"))
        .output()
        .expect("run isolated test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("running 1 test"),
        "isolated test {test} failed ({})\n{stdout}\n{stderr}",
        output.status,
    );
    print!("{stdout}");
    eprint!("{stderr}");
    None
}

pub(crate) fn check_pdf_with_qpdf(path: &Path) {
    match Command::new("qpdf").arg("--check").arg(path).output() {
        Ok(output) => {
            assert!(
                output.status.success(),
                "qpdf rejected {}\n{}\n{}",
                path.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            println!("qpdf --check passed for {}", path.display());
        }
        Err(err) if err.kind() == ErrorKind::NotFound => println!("qpdf is unavailable"),
        Err(err) => panic!("could not run qpdf: {err}"),
    }
}
