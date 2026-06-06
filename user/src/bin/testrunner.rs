#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{chdir, exec, exit, fork, poweroff, waitpid};

const BASIC_TESTS: &[&str] = &[
    // "brk\0",
    // "chdir\0",
    // "clone\0",
    // "close\0",
    // "dup2\0",
    // "dup\0",
    // "execve\0",
    // "exit\0",
    // "fork\0",
    // "fstat\0",
    // "getcwd\0",
    // "getdents\0",
    // "getpid\0",
    // "getppid\0",
    // "gettimeofday\0",
    // "mkdir_\0",
    // "mmap\0",
    // "mount\0",
    // "munmap\0",
    // "openat\0",
    // "open\0",
    // "pipe\0",
    // "read\0",
    // "sleep\0",
    // "times\0",
    // "umount\0",
    // "uname\0",
    // "unlink\0",
    // "wait\0",
    // "waitpid\0",
    // "write\0",
    // "yield\0",
];

/// libctest 静态测例列表（来自 run-static.sh，共 107 个）
const LIBC_STATIC_TESTS: &[&str] = &[
    // "argv\0",
    // "basename\0",
    // "clocale_mbfuncs\0",
    // "clock_gettime\0",
    // "dirname\0",
    // "env\0",
    // "fdopen\0",
    // "fnmatch\0",
    // "fscanf\0",
    // "fwscanf\0",
    // "iconv_open\0",
    // "inet_pton\0",
    // "mbc\0",
    // "memstream\0",
    // "pthread_cancel_points\0",
    // "pthread_cancel\0",
    // "pthread_cond\0",
    // "pthread_tsd\0",
    // "qsort\0",
    // "random\0",
    // "search_hsearch\0",
    // "search_insque\0",
    // "search_lsearch\0",
    // "search_tsearch\0",
    // "setjmp\0",
    // "snprintf\0",
    // "socket\0",
    // "sscanf\0",
    // "sscanf_long\0",
    "stat\0",
    // "strftime\0",
    // "string\0",
    // "string_memcpy\0",
    // "string_memmem\0",
    // "string_memset\0",
    // "string_strchr\0",
    // "string_strcspn\0",
    // "string_strstr\0",
    // "strptime\0",
    // "strtod\0",
    // "strtod_simple\0",
    // "strtof\0",
    // "strtol\0",
    // "strtold\0",
    // "swprintf\0",
    // "tgmath\0",
    // "time\0",
    // "tls_align\0",
    // "udiv\0",
    "ungetc\0",
    "utime\0",
    // "wcsstr\0",
    // "wcstol\0",
    "daemon_failure\0",
    // "dn_expand_empty\0",
    // "dn_expand_ptr_0\0",
    "fflush_exit\0",
    // "fgets_eof\0",
    // "fgetwc_buffering\0",
    // "fpclassify_invalid_ld80\0",
    // "ftello_unflushed_append\0",
    // "getpwnam_r_crash\0",
    // "getpwnam_r_errno\0",
    // "iconv_roundtrips\0",
    // "inet_ntop_v4mapped\0",
    // "inet_pton_empty_last_field\0",
    // "iswspace_null\0",
    // "lrand48_signextend\0",
    // "lseek_large\0",
    // "malloc_0\0",
    // "mbsrtowcs_overflow\0",
    // "memmem_oob_read\0",
    // "memmem_oob\0",
    // "mkdtemp_failure\0",
    // "mkstemp_failure\0",
    // "printf_1e9_oob\0",
    // "printf_fmt_g_round\0",
    // "printf_fmt_g_zeros\0",
    // "printf_fmt_n\0",
    "pthread_robust_detach\0",
    // "pthread_cancel_sem_wait\0",
    // "pthread_cond_smasher\0",
    // "pthread_condattr_setclock\0",
    // "pthread_exit_cancel\0",
    // "pthread_once_deadlock\0",
    // "pthread_rwlock_ebusy\0",
    // "putenv_doublefree\0",
    // "regex_backref_0\0",
    // "regex_bracket_icase\0",
    // "regex_ere_backref\0",
    // "regex_escaped_high_byte\0",
    // "regex_negated_range\0",
    // "regexec_nosub\0",
    // "rewind_clear_error\0",
    "rlimit_open_files\0",
    // "scanf_bytes_consumed\0",
    // "scanf_match_literal_eof\0",
    // "scanf_nullbyte_char\0",
    // "setvbuf_unget\0",
    // "sigprocmask_internal\0",
    // "sscanf_eof\0",
    "statvfs\0",
    // "strverscmp\0",
    "syscall_sign_extend\0",
    // "uselocale_0\0",
    // "wcsncpy_read_overflow\0",
    // "wcsstr_false_negative\0",
];

fn strip_nul(s: &str) -> &str {
    &s[..s.len() - 1]
}

fn run_program(path: &str) -> i32 {
    println!("Testing {} :", strip_nul(path));

    let pid = fork();
    if pid == 0 {
        let argv = [path.as_ptr(), core::ptr::null()];
        let ret = exec(path, &argv);
        println!("[testrunner] exec {} failed: {}", strip_nul(path), ret);
        exit(-1);
        unreachable!();
    }

    if pid < 0 {
        println!("[testrunner] fork failed");
        return -1;
    }

    let mut exit_code = 0;
    let waited = waitpid(pid as usize, &mut exit_code);
    if waited < 0 {
        println!("[testrunner] wait failed: {}", waited);
        return -1;
    }
    exit_code
}

fn run_basic_musl() {
    println!("#### OS COMP TEST GROUP START basic-musl ####");
    if chdir("/musl/basic\0") < 0 {
        println!("[testrunner] skip basic-musl: cannot enter /musl/basic");
        println!("#### OS COMP TEST GROUP END basic-musl ####");
        return;
    }

    for test in BASIC_TESTS {
        let exit_code = run_program(test);
        if exit_code != 0 {
            println!(
                "[testrunner] {} exited with code {}",
                strip_nul(test),
                exit_code
            );
        }
    }
    let _ = chdir("/\0");
    println!("#### OS COMP TEST GROUP END basic-musl ####");
}

/// 运行 libctest 静态测例（通过 runtest.exe 执行）
fn run_libctest_static() {
    println!("#### OS COMP TEST GROUP START libctest-musl ####");
    if chdir("/musl\0") < 0 {
        println!("[testrunner] skip libctest-musl: cannot enter /musl");
        println!("#### OS COMP TEST GROUP END libctest-musl ####");
        return;
    }

    let runtest = "runtest.exe\0";
    let w_flag = "-w\0";
    let entry = "entry-static.exe\0";

    for (i, test) in LIBC_STATIC_TESTS.iter().enumerate() {
        let name = strip_nul(test);
        println!(
            "[libctest {}/{}] {} :",
            i + 1,
            LIBC_STATIC_TESTS.len(),
            name
        );

        let pid = fork();
        if pid == 0 {
            let argv: [*const u8; 5] = [
                runtest.as_ptr(),
                w_flag.as_ptr(),
                entry.as_ptr(),
                test.as_ptr(),
                core::ptr::null(),
            ];
            let ret = exec(runtest, &argv);
            println!("[testrunner] exec runtest.exe failed: {}", ret);
            exit(-1);
            unreachable!();
        }

        if pid < 0 {
            println!("[testrunner] fork failed for {}", name);
            continue;
        }

        let mut exit_code = 0;
        let waited = waitpid(pid as usize, &mut exit_code);
        if waited < 0 {
            println!("[testrunner] wait failed for {}: {}", name, waited);
        } else if exit_code != 0 {
            println!("[testrunner] {} exited with code {}", name, exit_code);
        }
    }

    let _ = chdir("/\0");
    println!("#### OS COMP TEST GROUP END libctest-musl ####");
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("[testrunner] start");

    run_basic_musl();
    run_libctest_static();

    println!("[testrunner] all selected tests finished, powering off");
    poweroff();
    0
}
