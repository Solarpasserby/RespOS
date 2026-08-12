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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContestMode {
    Preliminary,
    Final,
    Diagnostic,
}

fn contest_mode() -> ContestMode {
    let fd = open(PROFILE, O_RDONLY, 0);
    if fd < 0 {
        println!("[contest_launcher] no profile; using preliminary mode");
        return ContestMode::Preliminary;
    }

    let mut data = [0u8; 512];
    let size = read(fd as usize, &mut data);
    let _ = close(fd as usize);
    if size <= 0 {
        println!("[contest_launcher] empty profile; using preliminary mode");
        return ContestMode::Preliminary;
    }

    let Ok(text) = core::str::from_utf8(&data[..size as usize]) else {
        println!("[contest_launcher] invalid UTF-8 profile; using preliminary mode");
        return ContestMode::Preliminary;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line {
            "mode=final" | "final" => return ContestMode::Final,
            "mode=preliminary" | "preliminary" => return ContestMode::Preliminary,
            "mode=diagnostic" | "diagnostic" => return ContestMode::Diagnostic,
            _ => {}
        }
    }

    println!("[contest_launcher] profile has no known mode; using preliminary mode");
    ContestMode::Preliminary
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

fn script_exists(path: &str) -> bool {
    let fd = open(path, O_RDONLY, 0);
    if fd < 0 {
        false
    } else {
        let _ = close(fd as usize);
        true
    }
}

fn run_final_script(script: &str) {
    let display = script.strip_suffix('\0').unwrap_or(script);
    if !script_exists(script) {
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
    match contest_mode() {
        ContestMode::Preliminary => run_preliminary(),
        ContestMode::Final => run_final(),
        ContestMode::Diagnostic => run_diagnostic(),
    }
}
