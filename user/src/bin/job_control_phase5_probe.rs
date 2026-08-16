#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::null_mut;
use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    SIGCONT, SIGHUP, SIGKILL, SIGSTOP, SIGTTIN, SIGTTOU, SignalAction, exit_group, fork, getpgid,
    getpid, getsid, ioctl, kill, pipe, read, setpgid, setsid, sigaction, wait4_raw, write, yield_,
};

const TIOCSCTTY: usize = 0x540e;
const TIOCGPGRP: usize = 0x540f;
const TIOCSPGRP: usize = 0x5410;
const TIOCNOTTY: usize = 0x5422;
const TIOCGSID: usize = 0x5429;
const ENOTTY: isize = 25;
const ESRCH: isize = 3;
const TCGETS: usize = 0x5401;
const TCSETS: usize = 0x5402;
const TOSTOP: u32 = 0x100;
const WUNTRACED: usize = 1 << 1;
const WCONTINUED: usize = 1 << 3;
static ORPHAN_HUP_SEEN: AtomicUsize = AtomicUsize::new(0);

fn orphan_hup_handler() {
    ORPHAN_HUP_SEEN.store(1, Ordering::SeqCst);
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KernelTermios {
    iflag: u32,
    oflag: u32,
    cflag: u32,
    lflag: u32,
    line: u8,
    cc: [u8; 19],
}

fn wait_child(pid: isize) -> Option<i32> {
    let mut status = 0;
    loop {
        let result = wait4_raw(pid, &mut status, 0, null_mut());
        if result == pid {
            return Some(status);
        }
        if result != -4 {
            println!("JOB_CONTROL wait4 pid={} failed: {}", pid, result);
            return None;
        }
    }
}

fn run_session_leader_contract() -> i32 {
    let pid = getpid();
    if setsid() != pid {
        println!("JOB_CONTROL_FAIL setsid pid={}", pid);
        return 10;
    }
    if ioctl(0, TIOCSCTTY, 1) != 0 || ioctl(0, TIOCSCTTY, 0) != 0 {
        println!("JOB_CONTROL_FAIL TIOCSCTTY");
        return 11;
    }

    let mut sid = -1i32;
    let mut foreground = -1i32;
    if ioctl(0, TIOCGSID, &mut sid as *mut i32 as usize) != 0
        || ioctl(0, TIOCGPGRP, &mut foreground as *mut i32 as usize) != 0
        || sid as isize != getsid(0)
        || foreground as isize != getpgid(0)
    {
        println!(
            "JOB_CONTROL_FAIL initial sid={} getsid={} fg={} pgrp={}",
            sid,
            getsid(0),
            foreground,
            getpgid(0)
        );
        return 12;
    }

    let mut ready = [0i32; 2];
    if pipe(&mut ready) != 0 {
        return 13;
    }
    let member = fork();
    if member < 0 {
        return 14;
    }
    if member == 0 {
        if setpgid(0, 0) != 0 || write(ready[1] as usize, b"x") != 1 {
            let _ = exit_group(15);
        }
        loop {
            let _ = yield_();
        }
    }

    let mut byte = [0u8; 1];
    if read(ready[0] as usize, &mut byte) != 1 {
        return 16;
    }
    let ignore = SignalAction {
        handler: 1,
        ..SignalAction::default()
    };
    if sigaction(SIGTTOU, Some(&ignore), None) != 0 {
        return 17;
    }
    let member_group = member as i32;
    if ioctl(0, TIOCSPGRP, &member_group as *const i32 as usize) != 0
        || ioctl(0, TIOCGPGRP, &mut foreground as *mut i32 as usize) != 0
        || foreground != member_group
    {
        println!(
            "JOB_CONTROL_FAIL foreground member={} observed={}",
            member_group, foreground
        );
        return 18;
    }

    let stopped_reader = fork();
    if stopped_reader < 0 {
        return 19;
    }
    if stopped_reader == 0 {
        if setpgid(0, 0) != 0 {
            let _ = exit_group(25);
        }
        let _ = read(0, &mut byte);
        let _ = exit_group(26);
    }
    let mut stopped_status = 0;
    if wait4_raw(stopped_reader, &mut stopped_status, WUNTRACED, null_mut()) != stopped_reader
        || stopped_status != (SIGTTIN << 8) | 0x7f
    {
        println!(
            "JOB_CONTROL_FAIL background stop pid={} status={:#x}",
            stopped_reader, stopped_status
        );
        return 27;
    }
    let stopped_group = stopped_reader as i32;
    if ioctl(0, TIOCSPGRP, &stopped_group as *const i32 as usize) != 0
        || kill(stopped_reader as usize, SIGCONT) != 0
        || wait4_raw(stopped_reader, &mut stopped_status, WCONTINUED, null_mut()) != stopped_reader
        || stopped_status != 0xffff
    {
        println!(
            "JOB_CONTROL_FAIL background continue pid={} status={:#x}",
            stopped_reader, stopped_status
        );
        return 28;
    }
    let _ = kill(stopped_reader as usize, SIGKILL);
    if wait_child(stopped_reader).is_none()
        || ioctl(0, TIOCSPGRP, &member_group as *const i32 as usize) != 0
    {
        return 29;
    }

    if sigaction(SIGTTIN, Some(&ignore), None) != 0 || read(0, &mut byte) != -5 {
        println!("JOB_CONTROL_FAIL background read did not return EIO");
        return 30;
    }

    let mut old_termios = KernelTermios::default();
    if ioctl(1, TCGETS, &mut old_termios as *mut KernelTermios as usize) != 0 {
        return 20;
    }
    let mut termios = old_termios;
    termios.lflag |= TOSTOP;
    if ioctl(1, TCSETS, &termios as *const KernelTermios as usize) != 0
        || ioctl(1, TCGETS, &mut termios as *mut KernelTermios as usize) != 0
        || termios.lflag & TOSTOP == 0
        || write(1, b"J\n") != 2
        || ioctl(1, TCSETS, &old_termios as *const KernelTermios as usize) != 0
    {
        println!("JOB_CONTROL_FAIL termios/TOSTOP");
        return 21;
    }

    let invalid = 0x3fff_ffffi32;
    if ioctl(0, TIOCSPGRP, &invalid as *const i32 as usize) != -ESRCH {
        println!("JOB_CONTROL_FAIL invalid foreground group");
        return 22;
    }

    let self_group = getpgid(0) as i32;
    if sigaction(SIGHUP, Some(&ignore), None) != 0 || sigaction(SIGCONT, Some(&ignore), None) != 0 {
        return 23;
    }
    if ioctl(0, TIOCSPGRP, &self_group as *const i32 as usize) != 0
        || ioctl(0, TIOCNOTTY, 0) != 0
        || ioctl(0, TIOCGPGRP, &mut foreground as *mut i32 as usize) != -ENOTTY
    {
        println!("JOB_CONTROL_FAIL detach");
        return 24;
    }

    let _ = kill(member as usize, SIGKILL);
    let _ = wait_child(member);
    println!("JOB_CONTROL_RESPOS controlling_tty_foreground PASS");
    println!("JOB_CONTROL_RESPOS background_stop_continue PASS");
    println!("JOB_CONTROL_RESPOS background_read_eio PASS");
    println!("JOB_CONTROL_RESPOS termios_tostop_ignored PASS");
    0
}

fn run_orphaned_stopped_group_contract() -> i32 {
    let pid = getpid();
    if setsid() != pid {
        return 40;
    }
    let mut report = [0i32; 2];
    if pipe(&mut report) != 0 {
        return 41;
    }
    let bridge = fork();
    if bridge < 0 {
        return 42;
    }
    if bridge == 0 {
        let stopped = fork();
        if stopped < 0 {
            let _ = exit_group(43);
        }
        if stopped == 0 {
            if setpgid(0, 0) != 0 {
                let _ = exit_group(44);
            }
            let hup = SignalAction {
                handler: orphan_hup_handler as usize,
                ..SignalAction::default()
            };
            if sigaction(SIGHUP, Some(&hup), None) != 0 || kill(getpid() as usize, SIGSTOP) != 0 {
                let _ = exit_group(45);
            }
            for _ in 0..1000 {
                if ORPHAN_HUP_SEEN.load(Ordering::SeqCst) == 1 {
                    break;
                }
                let _ = yield_();
            }
            if ORPHAN_HUP_SEEN.load(Ordering::SeqCst) != 1 || write(report[1] as usize, b"h") != 1 {
                let _ = exit_group(46);
            }
            let _ = exit_group(0);
        }
        let mut status = 0;
        if wait4_raw(stopped, &mut status, WUNTRACED, null_mut()) != stopped
            || status != (SIGSTOP << 8) | 0x7f
        {
            let _ = exit_group(47);
        }
        // Exiting this bridge removes the last same-session parent outside
        // the stopped child's process group, so that group becomes orphaned.
        let _ = exit_group(0);
    }

    let mut byte = [0u8; 1];
    if read(report[0] as usize, &mut byte) != 1 || byte[0] != b'h' {
        return 48;
    }
    if wait_child(bridge) != Some(0) {
        return 49;
    }
    println!("JOB_CONTROL_RESPOS orphan_hup_cont PASS");
    0
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let child = fork();
    if child < 0 {
        return 1;
    }
    if child == 0 {
        return run_session_leader_contract();
    }

    if wait_child(child) != Some(0) {
        println!("JOB_CONTROL_RESPOS child contract FAIL");
        return 1;
    }
    let orphan = fork();
    if orphan < 0 {
        return 2;
    }
    if orphan == 0 {
        return run_orphaned_stopped_group_contract();
    }
    if wait_child(orphan) != Some(0) {
        println!("JOB_CONTROL_RESPOS orphan contract FAIL");
        return 2;
    }
    println!("JOB_CONTROL_RESPOS ALL PASS");
    0
}
