// user/src/bin/user_shell.rs

#![no_std]
#![no_main]

extern crate alloc;

#[macro_use]
extern crate user_lib;

const LF: u8 = 0x0au8;
const CR: u8 = 0x0du8;
const DL: u8 = 0x7fu8;
const BS: u8 = 0x08u8;

use alloc::string::String;
use alloc::vec::Vec;
use core::str;
use user_lib::{chdir, fork, exec, getcwd, waitpid};
use user_lib::console::getchar;

fn print_prompt() {
    let mut cwd = [0u8; 128];
    if getcwd(&mut cwd) < 0 {
        print!("<?> ");
        return;
    }
    let len = cwd.iter().position(|&ch| ch == 0).unwrap_or(cwd.len());
    match str::from_utf8(&cwd[..len]) {
        Ok(path) => print!("{}> ", path),
        Err(_) => print!("<?> "),
    }
}

fn run_builtin_cd(command: &str) -> bool {
    let mut parts = command.split_whitespace();
    if parts.next() != Some("cd") {
        return false;
    }

    let target = parts.next().unwrap_or("/");
    let mut path = String::new();
    path.push_str(target);
    path.push('\0');

    if chdir(path.as_str()) < 0 {
        println!("cd: failed to change directory");
    }
    true
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("Rust user shell");
    let mut line: String = String::new();
    print_prompt();
    loop {
        let c = getchar();
        match c {
            LF | CR => {
                println!("");
                let command = line.trim();
                if !command.is_empty() && !run_builtin_cd(command) {
                    let args: Vec<String> = command
                        .split_whitespace()
                        .map(|arg| {
                            let mut arg_buf = String::new();
                            arg_buf.push_str(arg);
                            arg_buf.push('\0');
                            arg_buf
                        })
                        .collect();
                    let mut argv: Vec<*const u8> = args.iter()
                        .map(|arg| arg.as_ptr())
                        .collect();
                    argv.push(core::ptr::null());
                    let pid = fork();
                    if pid == 0 {
                        // child process
                        let ret = exec(args[0].as_str(), argv.as_slice());
                        if ret < 0 {
                            println!("Error when executing! ret = {}", ret);
                            return ret as i32;
                        }
                        println!("exec returned unexpectedly with {}", ret);
                        return -1;
                    } else {
                        let mut exit_code: i32 = 0;
                        let exit_pid = waitpid(pid as usize, &mut exit_code);
                        assert_eq!(pid, exit_pid);
                        println!(
                            "Shell: Process {} exited with code {}",
                            pid, exit_code
                        );
                    }
                }
                line.clear();
                print_prompt();
            }
            BS | DL => {
                if !line.is_empty() {
                    print!("{}", BS as char);
                    print!(" ");
                    print!("{}", BS as char);
                    line.pop();
                }
            }
            _ => {
                print!("{}", c as char);
                line.push(c as char);
            }
        }
    }
}
