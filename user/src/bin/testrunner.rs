#![no_std]
#![no_main]
#![allow(unused)]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::string::String;
use user_lib::{
    O_RDONLY, chdir, close, exec, execve, exit, fork, link, mkdir, open, poweroff, read, unlink,
    waitpid,
};

const BUSYBOX_PATH: &str = "/musl/busybox\0";
const GLIBC_BUSYBOX_PATH: &str = "/glibc/busybox\0";
const BASIC_SCRIPT: &str = "basic_testcode.sh\0";
const LIBCBENCH_SCRIPT: &str = "libcbench_testcode.sh\0";
const RUN_STATIC_SCRIPT: &str = "run-static.sh\0";
const RUN_DYNAMIC_SCRIPT: &str = "run-dynamic.sh\0";
const BUSYBOX_CMD_FILE: &str = "busybox_cmd.txt\0";
const LUA_SCRIPT: &str = "lua_testcode.sh\0";
const LMBENCH_SCRIPT: &str = "lmbench_testcode.sh\0";
const LTP_SCRIPT: &str = "ltp_testcode.sh\0";
const IOZONE_SCRIPT: &str = "iozone_testcode.sh\0";

const RV_MUSL_LOADER: &str = "/lib/ld-musl-riscv64.so.1\0";
const RV_MUSL_SF_LOADER: &str = "/lib/ld-musl-riscv64-sf.so.1\0";
const LA_MUSL_LOADER: &str = "/lib64/ld-musl-loongarch-lp64d.so.1\0";
const RV_GLIBC_LOADER: &str = "/lib/ld-linux-riscv64-lp64d.so.1\0";
const LA_GLIBC_LOADER: &str = "/lib64/ld-linux-loongarch-lp64d.so.1\0";

fn strip_nul(s: &str) -> &str {
    &s[..s.len() - 1]
}

fn run_shell_script(workdir: &str, shell_path: &str, script_path: &str) {
    run_shell_script_with_env(workdir, shell_path, script_path, &[core::ptr::null()]);
}

fn run_shell_script_with_env(
    workdir: &str,
    shell_path: &str,
    script_path: &str,
    envp: &[*const u8],
) {
    if chdir(workdir) < 0 {
        println!("[testrunner] cannot enter {}", strip_nul(workdir));
        return;
    }
    let pid = fork();
    if pid == 0 {
        let argv: &[*const u8] = &[
            "busybox\0".as_ptr(),
            "sh\0".as_ptr(),
            script_path.as_ptr(),
            core::ptr::null(),
        ];
        let ret = execve(shell_path, argv, envp);
        println!(
            "[testrunner] exec {} sh {} failed: {}",
            strip_nul(shell_path),
            strip_nul(script_path),
            ret
        );
        exit(-1);
    }

    if pid < 0 {
        println!("[testrunner] fork {} failed", strip_nul(script_path));
    } else {
        let mut ec = 0;
        let waited = waitpid(pid as usize, &mut ec);
        if waited < 0 {
            println!(
                "[testrunner] wait {} failed: {}",
                strip_nul(script_path),
                waited
            );
        } else if ec != 0 {
            println!(
                "[testrunner] {} exited with code {}",
                strip_nul(script_path),
                ec
            );
        }
    }

    let _ = chdir("/\0");
}

// 脚本解析环境配置
fn prepare_bin_shell(shell_path: &str) {
    let _ = mkdir("/bin\0", 0o755);
    let _ = unlink("/bin/busybox\0");
    let _ = unlink("/bin/sh\0");
    let _ = link(shell_path, "/bin/busybox\0");
    let _ = link(shell_path, "/bin/sh\0");
}

fn _run_basic_musl() {
    run_shell_script("/musl/\0", BUSYBOX_PATH, BASIC_SCRIPT);
}

fn _run_basic_glibc() {
    run_shell_script("/glibc/\0", GLIBC_BUSYBOX_PATH, BASIC_SCRIPT);
}

fn _run_libcbench_musl() {
    run_shell_script("/musl/\0", BUSYBOX_PATH, LIBCBENCH_SCRIPT);
}

fn _run_libcbench_glibc() {
    run_shell_script("/glibc/\0", GLIBC_BUSYBOX_PATH, LIBCBENCH_SCRIPT);
}

fn _run_static_musl() {
    run_shell_script("/musl/\0", BUSYBOX_PATH, RUN_STATIC_SCRIPT);
}

fn _run_dynamic_musl() {
    run_shell_script("/musl/\0", BUSYBOX_PATH, RUN_DYNAMIC_SCRIPT);
}

fn _run_libctest_musl() {
    println!("#### OS COMP TEST GROUP START libctest-musl ####");
    _run_static_musl();
    _run_dynamic_musl();
    println!("#### OS COMP TEST GROUP END libctest-musl ####");
}

fn read_file(path: &str, buf: &mut [u8]) -> isize {
    let fd = open(path, O_RDONLY, 0);
    if fd < 0 {
        return fd;
    }
    let n = read(fd as usize, buf);
    let _ = close(fd as usize);
    n
}

fn run_busybox_command(line: &str) -> i32 {
    let mut command = String::from("./busybox ");
    command.push_str(line);
    command.push('\0');

    let pid = fork();
    if pid == 0 {
        let argv: &[*const u8] = &[
            "busybox\0".as_ptr(),
            "sh\0".as_ptr(),
            "-c\0".as_ptr(),
            command.as_ptr(),
            core::ptr::null(),
        ];
        let ret = exec(BUSYBOX_PATH, argv);
        println!("[testrunner] exec busybox command failed: {}", ret);
        exit(-1);
    }

    if pid < 0 {
        return -1;
    }
    let mut ec = 0;
    if waitpid(pid as usize, &mut ec) < 0 {
        return -1;
    }
    ec
}

fn ensure_busybox_applet_links_musl() {
    let _ = mkdir("/bin\0", 0o755);
    cleanup_busybox_applet_links_musl();
    let _ = link("/musl/busybox\0", "/bin/ls\0");
    let _ = link("/musl/busybox\0", "/bin/sleep\0");
}

fn cleanup_busybox_applet_links_musl() {
    let _ = unlink("/bin/ls\0");
    let _ = unlink("/bin/sleep\0");
}

fn _run_busybox_musl() {
    if chdir("/musl/\0") < 0 {
        println!("[testrunner] cannot enter /musl");
        return;
    }
    ensure_busybox_applet_links_musl();

    println!("#### OS COMP TEST GROUP START busybox-musl ####");

    let mut buf = [0u8; 2048];
    let n = read_file(BUSYBOX_CMD_FILE, &mut buf);
    if n < 0 {
        println!("[testrunner] cannot read {}", strip_nul(BUSYBOX_CMD_FILE));
        let _ = chdir("/\0");
        cleanup_busybox_applet_links_musl();
        println!("#### OS COMP TEST GROUP END busybox-musl ####");
        return;
    }

    let data = &buf[..n as usize];
    let mut start = 0usize;
    for i in 0..=data.len() {
        if i != data.len() && data[i] != b'\n' {
            continue;
        }
        let raw = &data[start..i];
        start = i + 1;
        let line = core::str::from_utf8(raw).unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        let mut ec = run_busybox_command(line);
        if line == "false" {
            ec = 0;
        }
        if ec == 0 {
            println!("testcase busybox {} success", line);
        } else {
            println!("testcase busybox {} fail", line);
        }
    }

    let _ = chdir("/\0");
    cleanup_busybox_applet_links_musl();
    println!("#### OS COMP TEST GROUP END busybox-musl ####");
}

fn ensure_busybox_applet_links_glibc() {
    let _ = mkdir("/bin\0", 0o755);
    cleanup_busybox_applet_links_glibc();
    let _ = link("/glibc/busybox\0", "/bin/ls\0");
    let _ = link("/glibc/busybox\0", "/bin/sleep\0");
}

fn cleanup_busybox_applet_links_glibc() {
    let _ = unlink("/bin/ls\0");
    let _ = unlink("/bin/sleep\0");
}

fn _run_busybox_glibc() {
    if chdir("/glibc/\0") < 0 {
        println!("[testrunner] cannot enter /glibc");
        return;
    }
    ensure_busybox_applet_links_glibc();

    println!("#### OS COMP TEST GROUP START busybox-glibc ####");

    let mut buf = [0u8; 2048];
    let n = read_file(BUSYBOX_CMD_FILE, &mut buf);
    if n < 0 {
        println!("[testrunner] cannot read {}", strip_nul(BUSYBOX_CMD_FILE));
        let _ = chdir("/\0");
        cleanup_busybox_applet_links_glibc();
        println!("#### OS COMP TEST GROUP END busybox-glibc ####");
        return;
    }

    let data = &buf[..n as usize];
    let mut start = 0usize;
    for i in 0..=data.len() {
        if i != data.len() && data[i] != b'\n' {
            continue;
        }
        let raw = &data[start..i];
        start = i + 1;
        let line = core::str::from_utf8(raw).unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        let mut ec = run_busybox_command(line);
        if line == "false" {
            ec = 0;
        }
        if ec == 0 {
            println!("testcase busybox {} success", line);
        } else {
            println!("testcase busybox {} fail", line);
        }
    }

    let _ = chdir("/\0");
    cleanup_busybox_applet_links_glibc();
    println!("#### OS COMP TEST GROUP END busybox-glibc ####");
}

fn _run_lua_musl() {
    prepare_bin_shell(BUSYBOX_PATH);
    run_shell_script("/musl/\0", BUSYBOX_PATH, LUA_SCRIPT);
}

fn _run_lua_glibc() {
    prepare_bin_shell(GLIBC_BUSYBOX_PATH);
    run_shell_script("/glibc/\0", GLIBC_BUSYBOX_PATH, LUA_SCRIPT);
}

fn _run_iozone_musl() {
    prepare_iozone_musl_loader_links();
    let envp: &[*const u8] = &[
        "LD_LIBRARY_PATH=/musl/lib:/musl\0".as_ptr(),
        core::ptr::null(),
    ];
    run_shell_script_with_env("/musl/\0", BUSYBOX_PATH, IOZONE_SCRIPT, envp);
}

fn _run_iozone_glibc() {
    prepare_iozone_glibc_loader_links();
    let envp: &[*const u8] = &["LD_LIBRARY_PATH=/glibc/lib\0".as_ptr(), core::ptr::null()];
    run_shell_script_with_env("/glibc/\0", GLIBC_BUSYBOX_PATH, IOZONE_SCRIPT, envp);
}

fn prepare_loader_dirs() {
    let _ = mkdir("/lib\0", 0o755);
    let _ = mkdir("/lib64\0", 0o755);
}

fn relink_loader(src: &str, dst: &str) {
    let _ = unlink(dst);
    let _ = link(src, dst);
}

fn prepare_iozone_musl_loader_links() {
    prepare_loader_dirs();
    relink_loader("/musl/lib/libc.so\0", RV_MUSL_LOADER);
    relink_loader("/musl/lib/libc.so\0", RV_MUSL_SF_LOADER);
    relink_loader("/musl/lib/libc.so\0", LA_MUSL_LOADER);
}

fn prepare_iozone_glibc_loader_links() {
    prepare_loader_dirs();
    relink_loader("/glibc/lib/ld-linux-riscv64-lp64d.so.1\0", RV_GLIBC_LOADER);
    relink_loader(
        "/glibc/lib/ld-linux-loongarch-lp64d.so.1\0",
        LA_GLIBC_LOADER,
    );
}

fn _run_lmbench_musl() {
    if chdir("/musl\0") < 0 {
        println!("[testrunner] cannot enter /musl");
        return;
    }

    prepare_bin_shell(BUSYBOX_PATH);
    let _ = mkdir("/tmp\0", 0o777);
    // hello 脚本硬编码了构建机路径 /code/lmbench_src/bin/build/lmbench_all
    let _ = mkdir("/code\0", 0o755);
    let _ = mkdir("/code/lmbench_src\0", 0o755);
    let _ = mkdir("/code/lmbench_src/bin\0", 0o755);
    let _ = mkdir("/code/lmbench_src/bin/build\0", 0o755);
    let _ = link(
        "/musl/lmbench_all\0",
        "/code/lmbench_src/bin/build/lmbench_all\0",
    );
    let _ = link(BUSYBOX_PATH, "/bin/cp\0");
    let _ = link(BUSYBOX_PATH, "cp\0");
    let _ = unlink("hello\0");
    let _ = link("lmbench_all\0", "hello\0");
    run_shell_script("/musl/\0", BUSYBOX_PATH, LMBENCH_SCRIPT);
}

fn _run_lmbench_glibc() {
    if chdir("/glibc\0") < 0 {
        println!("[testrunner] cannot enter /glibc");
        return;
    }

    prepare_bin_shell(GLIBC_BUSYBOX_PATH);
    let _ = mkdir("/tmp\0", 0o777);
    // hello 脚本硬编码了构建机路径 /code/lmbench_src/bin/build/lmbench_all
    let _ = mkdir("/code\0", 0o755);
    let _ = mkdir("/code/lmbench_src\0", 0o755);
    let _ = mkdir("/code/lmbench_src/bin\0", 0o755);
    let _ = mkdir("/code/lmbench_src/bin/build\0", 0o755);
    let _ = link(
        "/glibc/lmbench_all\0",
        "/code/lmbench_src/bin/build/lmbench_all\0",
    );
    let _ = link(GLIBC_BUSYBOX_PATH, "/bin/cp\0");
    let _ = link(GLIBC_BUSYBOX_PATH, "cp\0");
    let _ = unlink("hello\0");
    let _ = link("lmbench_all\0", "hello\0");
    run_shell_script("/glibc/\0", GLIBC_BUSYBOX_PATH, LMBENCH_SCRIPT);
}
fn _run_ltp_musl() {
    run_shell_script("/musl/\0", BUSYBOX_PATH, LTP_SCRIPT);
}

fn _run_ltp_glibc() {
    run_shell_script("/glibc/\0", GLIBC_BUSYBOX_PATH, LTP_SCRIPT);
}

#[cfg(target_arch = "riscv64")]
#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("[testrunner] start");
    _run_basic_musl();
    _run_basic_glibc();
    _run_libcbench_musl();
    _run_libcbench_glibc();
    _run_busybox_musl();
    _run_busybox_glibc();
    _run_libctest_musl();
    _run_lua_musl();
    _run_lua_glibc();
    _run_lmbench_musl();
    _run_lmbench_glibc();
    _run_iozone_musl();
    _run_iozone_glibc();
    // _run_ltp_musl();
    // _run_ltp_glibc();
    println!("[testrunner] all selected tests finished, powering off");
    poweroff();
    0
}

#[cfg(target_arch = "loongarch64")]
#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("[testrunner] start");
    _run_basic_musl();
    _run_basic_glibc();
    _run_libcbench_musl();
    _run_libcbench_glibc();
    _run_busybox_musl();
    _run_busybox_glibc();
    _run_libctest_musl();
    _run_lua_musl();
    _run_lua_glibc();
    _run_lmbench_musl();
    _run_lmbench_glibc(); // 会报错，还要修改
    _run_iozone_musl();
    _run_iozone_glibc();
    // _run_ltp_musl();
    // _run_ltp_glibc();
    println!("[testrunner] all selected tests finished, powering off");
    poweroff();
    0
}
