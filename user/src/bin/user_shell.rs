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
use user_lib::{
    chdir, close, dup2, exec, exit, fork, getcwd, getdents64, open, waitpid,
    O_APPEND, O_CREATE, O_DIRECTORY, O_RDONLY, O_TRUNC, O_WRONLY,
};
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

const DIRENT64_HEADER_SIZE: usize = 19;

const DT_DIR: u8 = 4; // InodeType::Directory as u8
const DT_REG: u8 = 8; // InodeType::Regular as u8

fn run_builtin_runall(command: &str) -> bool {
    let mut parts = command.split_whitespace();
    if parts.next() != Some("runall") {
        return false;
    }

    let target = parts.next().unwrap_or(".");
    let mut path = String::new();
    path.push_str(target);
    path.push('\0');

    let fd = open(path.as_str(), O_RDONLY | O_DIRECTORY, 0);
    if fd < 0 {
        println!("runall: cannot open directory");
        return true;
    }

    let fd = fd as usize;
    let mut buf = [0u8; 8192];
    let size = getdents64(fd, &mut buf);
    close(fd);

    if size < 0 {
        println!("runall: cannot read directory");
        return true;
    }

    let size = size as usize;
    let mut offset = 0;
    let mut programs: Vec<String> = Vec::new();

    while offset + DIRENT64_HEADER_SIZE <= size {
        let reclen = u16::from_le_bytes([buf[offset + 16], buf[offset + 17]]) as usize;
        if reclen < DIRENT64_HEADER_SIZE || offset + reclen > size {
            break;
        }
        let d_type = buf[offset + 18];
        let name_start = offset + DIRENT64_HEADER_SIZE;
        let name_end = name_start
            + buf[name_start..offset + reclen]
                .iter()
                .position(|&ch| ch == 0)
                .unwrap_or(offset + reclen - name_start);
        if name_end > name_start {
            if let Ok(name) = str::from_utf8(&buf[name_start..name_end]) {
                if name != "." && name != ".." && d_type != DT_DIR {  // 只要不是目录
    programs.push(String::from(name));
}
            }
        }
        offset += reclen;
    }

    if programs.is_empty() {
        println!("runall: no regular files found");
        return true;
    }

    println!("============================================================");
    println!("Running test suite in directory: {}", target);
    println!("============================================================");

    for name in &programs {
        println!("");
        println!("============================================================");
        println!("                    Running test: {}", name);
        println!("============================================================");

        let mut prog = String::new();
        prog.push_str(name);
        prog.push('\0');

        let pid = fork();
        if pid == 0 {
            let ret = exec(prog.as_str(), &[prog.as_ptr(), core::ptr::null()]);
            println!("[{}] exec failed, code = {}", name, ret);
            exit(-1);
        } else {
            let mut exit_code: i32 = 0;
            waitpid(pid as usize, &mut exit_code);

            if exit_code == 0 {
                println!("[{}] PASSED", name);
            } else {
                println!("[{}] FAILED, exit code = {}", name, exit_code);
            }
        }
    }

    println!("");
    println!("============================================================");
    println!("                      All tests finished");
    println!("============================================================");
    true
}

#[derive(Clone, Copy)]
enum OutputMode {
    Truncate,
    Append,
}

struct Command {
    args: Vec<String>,
    input: Option<String>,
    output: Option<(String, OutputMode)>,
}

fn tokenize(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in command.chars() {
        match ch {
            ' ' | '\t' | '\n' | '\r' => {
                if !current.is_empty() {
                    tokens.push(current);
                    current = String::new();
                }
            }
            '<' | '>' => {
                if !current.is_empty() {
                    tokens.push(current);
                    current = String::new();
                }
                if ch == '>' && tokens.last().map(|s| s.as_str()) == Some(">") {
                    tokens.pop();
                    tokens.push(String::from(">>"));
                } else {
                    let mut token = String::new();
                    token.push(ch);
                    tokens.push(token);
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn nul_terminate(path: &str) -> String {
    let mut buf = String::new();
    buf.push_str(path);
    buf.push('\0');
    buf
}

fn parse_command(command: &str) -> Result<Command, &'static str> {
    let tokens = tokenize(command);
    let mut args = Vec::new();
    let mut input = None;
    let mut output = None;
    let mut i = 0;

    while i < tokens.len() {
        match tokens[i].as_str() {
            "<" | ">" | ">>" => {
                if i + 1 >= tokens.len() {
                    return Err("missing redirection target");
                }
                let target = tokens[i + 1].as_str();
                if target == "<" || target == ">" || target == ">>" {
                    return Err("invalid redirection target");
                }
                match tokens[i].as_str() {
                    "<" => input = Some(nul_terminate(target)),
                    ">" => output = Some((nul_terminate(target), OutputMode::Truncate)),
                    ">>" => output = Some((nul_terminate(target), OutputMode::Append)),
                    _ => {}
                }
                i += 2;
            }
            _ => {
                args.push(nul_terminate(tokens[i].as_str()));
                i += 1;
            }
        }
    }

    if args.is_empty() {
        Err("empty command")
    } else {
        Ok(Command { args, input, output })
    }
}

fn redirect_fd(path: &str, flags: usize, target_fd: usize) -> Result<(), isize> {
    let fd = open(path, flags, 0o644);
    if fd < 0 {
        return Err(fd);
    }
    let fd = fd as usize;
    let ret = dup2(fd, target_fd);
    if fd != target_fd {
        close(fd);
    }
    if ret < 0 {
        Err(ret)
    } else {
        Ok(())
    }
}

fn apply_redirections(command: &Command) -> Result<(), isize> {
    if let Some(path) = command.input.as_ref() {
        redirect_fd(path.as_str(), O_RDONLY, 0)?;
    }
    if let Some((path, mode)) = command.output.as_ref() {
        let flags = match mode {
            OutputMode::Truncate => O_WRONLY | O_CREATE | O_TRUNC,
            OutputMode::Append => O_WRONLY | O_CREATE | O_APPEND,
        };
        redirect_fd(path.as_str(), flags, 1)?;
    }
    Ok(())
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
                if !command.is_empty() && !run_builtin_cd(command) && !run_builtin_runall(command) {
                    let command = match parse_command(command) {
                        Ok(command) => command,
                        Err(err) => {
                            println!("shell: {}", err);
                            line.clear();
                            print_prompt();
                            continue;
                        }
                    };
                    let mut argv: Vec<*const u8> = command.args.iter()
                        .map(|arg| arg.as_ptr())
                        .collect();
                    argv.push(core::ptr::null());
                    let pid = fork();
                    if pid == 0 {
                        // child process
                        if let Err(ret) = apply_redirections(&command) {
                            println!("shell: redirection failed: {}", ret);
                            return ret as i32;
                        }
                        let ret = exec(command.args[0].as_str(), argv.as_slice());
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
