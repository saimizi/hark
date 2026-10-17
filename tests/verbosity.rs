use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn verbosity_is_accepted_after_subcommands() {
    for args in [
        vec!["list", "-v", "--help"],
        vec!["cancel", "-vv", "--help"],
        vec!["run", "-v", "--help"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hark"))
            .args(&args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
    }
}

#[test]
fn run_preserves_child_flags_after_separator() {
    let output = Command::new(env!("CARGO_BIN_EXE_hark"))
        .args(["run", "-v", "--to", "", "--", "sh", "-c", "exit 37", "-v"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(37), "{output:?}");
}

#[test]
fn background_jobs_receive_verbosity() {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "hark-verbosity-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let mock = dir.join("systemd-run");
    fs::write(&mock, "#!/bin/sh\nprintf '<%s>\\n' \"$@\"\n").unwrap();
    fs::set_permissions(&mock, fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths = vec![dir.clone()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let path = std::env::join_paths(paths).unwrap();
    let pid = std::process::id().to_string();
    let file = dir.join("watched").to_str().unwrap().to_owned();
    for count in 0..=2 {
        for (role, trigger) in [
            ("deliver", vec!["--in", "90s"]),
            ("watch-file", vec!["--on-file", file.as_str()]),
            ("watch-pid", vec!["--on-exit", pid.as_str()]),
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_hark"))
                .args(std::iter::repeat("-v").take(count))
                .args(trigger)
                .args(["-m", "review message"])
                .env("PATH", &path)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let stdout = String::from_utf8(output.stdout).unwrap();
            let args: Vec<_> = stdout.lines().collect();
            let exe = format!("<{}>", env!("CARGO_BIN_EXE_hark"));
            let start = args.iter().position(|arg| *arg == exe).unwrap() + 1;
            assert_eq!(&args[start..start + count], vec!["<-v>"; count]);
            assert_eq!(args[start + count], format!("<{role}>"));
        }
    }
}
