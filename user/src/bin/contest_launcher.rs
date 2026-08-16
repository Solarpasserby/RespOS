#![no_std]
#![no_main]

extern crate user_lib;

use user_lib::{
    O_RDONLY, chdir, close, exec, execve, exit, fork, open, poweroff, println, read, waitpid,
};

const PROFILE: &str = "/respos/profile\0";
const GLIBC_DIR: &str = "/glibc\0";
const GLIBC_BASH: &str = "/bin/bash\0";

// Final-round scripts published in the current official RV64 and LA64 images.
// Keep this policy in user space; the kernel only provides mount and exec.
const FINAL_SCRIPTS: &[&str] = &[
    "/glibc/cagent_testcode.sh\0",
    "/glibc/buildstorm_testcode.sh\0",
];
const PRELIMINARY_MARKERS: &[&str] = &["/musl/basic_testcode.sh\0", "/glibc/basic_testcode.sh\0"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContestMode {
    Auto,
    Preliminary,
    Final,
    Diagnostic,
    Software,
}

fn contest_mode() -> ContestMode {
    let fd = open(PROFILE, O_RDONLY, 0);
    if fd < 0 {
        println!("[contest_launcher] no profile; detecting the root image");
        return ContestMode::Auto;
    }

    let mut data = [0u8; 512];
    let size = read(fd as usize, &mut data);
    let _ = close(fd as usize);
    if size <= 0 {
        println!("[contest_launcher] empty profile; detecting the root image");
        return ContestMode::Auto;
    }

    let Ok(text) = core::str::from_utf8(&data[..size as usize]) else {
        println!("[contest_launcher] invalid UTF-8 profile; detecting the root image");
        return ContestMode::Auto;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line {
            "mode=auto" | "auto" => return ContestMode::Auto,
            "mode=final" | "final" => return ContestMode::Final,
            "mode=preliminary" | "preliminary" => return ContestMode::Preliminary,
            "mode=diagnostic" | "diagnostic" => return ContestMode::Diagnostic,
            "mode=software" | "software" => return ContestMode::Software,
            _ => {}
        }
    }

    println!("[contest_launcher] profile has no known mode; detecting the root image");
    ContestMode::Auto
}

fn run_preliminary() -> i32 {
    println!("[contest_launcher] preliminary mode: starting embedded testrunner");
    let argv = ["testrunner\0".as_ptr(), core::ptr::null()];
    let ret = exec("testrunner\0", &argv);
    println!("[contest_launcher] cannot exec testrunner: {}", ret);
    let _ = exit(127);
    127
}

fn run_diagnostic() -> i32 {
    println!("[contest_launcher] diagnostic mode: starting embedded user shell");
    let argv = ["user_shell\0".as_ptr(), core::ptr::null()];
    let ret = exec("user_shell\0", &argv);
    println!("[contest_launcher] cannot exec user_shell: {}", ret);
    let _ = exit(127);
    127
}

fn run_software() -> i32 {
    println!("[contest_launcher] software mode: starting Alpine /bin/sh");
    let argv = ["sh\0".as_ptr(), "-i\0".as_ptr(), core::ptr::null()];
    let envp = [
        "HOME=/tmp\0".as_ptr(),
        "TMPDIR=/tmp\0".as_ptr(),
        "TERM=xterm\0".as_ptr(),
        "LC_ALL=C\0".as_ptr(),
        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\0".as_ptr(),
        core::ptr::null(),
    ];
    let ret = execve("/bin/sh\0", &argv, &envp);
    println!("[contest_launcher] cannot exec /bin/sh: {}", ret);
    let _ = exit(127);
    127
}

fn path_exists(path: &str) -> bool {
    let fd = open(path, O_RDONLY, 0);
    if fd < 0 {
        false
    } else {
        let _ = close(fd as usize);
        true
    }
}

fn detect_root_image_mode() -> ContestMode {
    // Final markers take precedence in case a later final image also keeps
    // compatibility files from the preliminary suite.
    if FINAL_SCRIPTS.iter().any(|path| path_exists(path)) {
        println!("[contest_launcher] auto-detected final-round root image");
        return ContestMode::Final;
    }
    if PRELIMINARY_MARKERS.iter().any(|path| path_exists(path)) {
        println!("[contest_launcher] auto-detected preliminary-round root image");
        return ContestMode::Preliminary;
    }

    println!("[contest_launcher] unknown root image; falling back to preliminary mode");
    ContestMode::Preliminary
}

fn run_final_script(script: &str) {
    let display = script.strip_suffix('\0').unwrap_or(script);
    if !path_exists(script) {
        println!(
            "[contest_launcher] final script missing; skipping {}",
            display
        );
        return;
    }

    println!("[contest_launcher] starting {}", display);
    let pid = fork();
    if pid == 0 {
        let argv = ["bash\0".as_ptr(), script.as_ptr(), core::ptr::null()];
        let envp = [core::ptr::null()];
        let ret = execve(GLIBC_BASH, &argv, &envp);
        println!("[contest_launcher] cannot exec {}: {}", display, ret);
        exit(127);
    }
    if pid < 0 {
        println!("[contest_launcher] cannot fork {}: {}", display, pid);
        return;
    }

    let mut exit_code = 0;
    let waited = waitpid(pid as usize, &mut exit_code);
    if waited < 0 {
        println!("[contest_launcher] waitpid {} failed: {}", display, waited);
    } else {
        println!(
            "[contest_launcher] finished {} with exit code {}",
            display, exit_code
        );
    }
}

fn run_final() -> i32 {
    println!("[contest_launcher] final mode: running fixed scripts serially");
    if chdir(GLIBC_DIR) < 0 {
        println!("[contest_launcher] cannot enter /glibc; powering off");
        let _ = poweroff();
        return 127;
    }
    for script in FINAL_SCRIPTS {
        run_final_script(script);
    }
    println!("[contest_launcher] all final scripts finished; powering off");
    let _ = poweroff();
    0
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let mode = match contest_mode() {
        ContestMode::Auto => detect_root_image_mode(),
        mode => mode,
    };
    match mode {
        ContestMode::Auto => unreachable!(),
        ContestMode::Preliminary => run_preliminary(),
        ContestMode::Final => run_final(),
        ContestMode::Diagnostic => run_diagnostic(),
        ContestMode::Software => run_software(),
    }
}
