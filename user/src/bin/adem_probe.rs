#![no_std]
#![no_main]
#![allow(static_mut_refs)]

// ADEM 模拟器探针：用内联汇编强制生成每条非对齐访存指令（2RI12/3R/2RI14），
// 校验模拟后的字节结果。在 2K1000LA 真机（UAL=0）上运行：每条指令都会触发
// ADEM 异常进入内核模拟器，任何语义错误都会以 FAIL 形式暴露。

#[macro_use]
extern crate user_lib;

#[cfg(not(target_arch = "loongarch64"))]
#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("adem: unsupported architecture");
    0
}

#[cfg(target_arch = "loongarch64")]
use core::arch::asm;
#[cfg(target_arch = "loongarch64")]
use user_lib::{exit, fork, waitpid};

// 上下文相关测试用的静态缓冲区（fork 后即 COW 页）。
#[cfg(target_arch = "loongarch64")]
static mut SRC: [u8; 4096] = [0; 4096];
#[cfg(target_arch = "loongarch64")]
static mut DST: [u8; 4096] = [0; 4096];
#[cfg(target_arch = "loongarch64")]
static mut COW_BUF: [u8; 4096] = [0; 4096];
#[cfg(target_arch = "loongarch64")]
static mut BSS_CHECK: [u8; 4096] = [0; 4096];

#[cfg(target_arch = "loongarch64")]
fn report(name: &str, ok: bool) {
    if ok {
        println!("adem: {} OK", name);
    } else {
        println!("adem: {} FAIL", name);
    }
}

/// 非对齐 16 字节拷贝一轮（模拟 musl memcpy 主循环：4×ldptr.w + 4×st.w）。
#[cfg(target_arch = "loongarch64")]
unsafe fn copy16_round(src: usize, dst: usize) {
    unsafe {
        asm!(
            "ldptr.w $t0, {s}, 0",
            "ldptr.w $t1, {s}, 4",
            "ldptr.w $t2, {s}, 8",
            "ldptr.w $t3, {s}, 12",
            "st.w $t0, {d}, 0",
            "st.w $t1, {d}, 4",
            "st.w $t2, {d}, 8",
            "st.w $t3, {d}, 12",
            s = in(reg) src,
            d = in(reg) dst,
            out("$t0") _,
            out("$t1") _,
            out("$t2") _,
            out("$t3") _,
            options(nostack)
        );
    }
}

#[cfg(target_arch = "loongarch64")]
#[unsafe(no_mangle)]
fn main() -> i32 {
    unsafe {
        let mut buf = [0u8; 64];
        // 非对齐基址（3 mod 8，对 1/2/4/8 字节访问都非对齐）
        let p = buf.as_mut_ptr().add(3) as usize;
        let p2 = p + 2;

        // ===== stores（写后逐字节校验）=====
        buf.fill(0);
        asm!("st.b {v}, {p}, 0", v = in(reg) 0xabu64, p = in(reg) p);
        report("st.b", buf[3] == 0xab);

        // si12 = -2：目标 p2-2 = p
        buf.fill(0);
        asm!("st.h {v}, {p}, -2", v = in(reg) 0x1234u64, p = in(reg) p2);
        report("st.h/si12-neg", buf[3] == 0x34 && buf[4] == 0x12);

        buf.fill(0);
        asm!("st.w {v}, {p}, 0", v = in(reg) 0x11223344u64, p = in(reg) p);
        report(
            "st.w",
            buf[3] == 0x44 && buf[4] == 0x33 && buf[5] == 0x22 && buf[6] == 0x11,
        );

        buf.fill(0);
        asm!("st.d {v}, {p}, 0", v = in(reg) 0x1122334455667788u64, p = in(reg) p);
        report(
            "st.d",
            buf[3] == 0x88
                && buf[4] == 0x77
                && buf[5] == 0x66
                && buf[6] == 0x55
                && buf[7] == 0x44
                && buf[8] == 0x33
                && buf[9] == 0x22
                && buf[10] == 0x11,
        );

        buf.fill(0);
        asm!("stx.w {v}, {p}, {k}", v = in(reg) 0xaabbccddu64, p = in(reg) p, k = in(reg) 0usize);
        report(
            "stx.w",
            buf[3] == 0xdd && buf[4] == 0xcc && buf[5] == 0xbb && buf[6] == 0xaa,
        );

        buf.fill(0);
        asm!("stx.d {v}, {p}, {k}", v = in(reg) 0x0102030405060708u64, p = in(reg) p, k = in(reg) 0usize);
        report(
            "stx.d",
            buf[3] == 8
                && buf[4] == 7
                && buf[5] == 6
                && buf[6] == 5
                && buf[7] == 4
                && buf[8] == 3
                && buf[9] == 2
                && buf[10] == 1,
        );

        buf.fill(0);
        asm!("stptr.w {v}, {p}, 0", v = in(reg) 0x0badc0deu64, p = in(reg) p);
        report(
            "stptr.w",
            buf[3] == 0xde && buf[4] == 0xc0 && buf[5] == 0xad && buf[6] == 0x0b,
        );

        buf.fill(0);
        asm!("stptr.d {v}, {p}, 0", v = in(reg) 0x1020304050607080u64, p = in(reg) p);
        report(
            "stptr.d",
            buf[3] == 0x80
                && buf[4] == 0x70
                && buf[5] == 0x60
                && buf[6] == 0x50
                && buf[7] == 0x40
                && buf[8] == 0x30
                && buf[9] == 0x20
                && buf[10] == 0x10,
        );

        // ===== loads（预置字节，加载后校验符号/零扩展）=====
        let mut out: u64;

        buf.fill(0);
        buf[3] = 0x85;
        asm!("ld.b {o}, {p}, 0", o = out(reg) out, p = in(reg) p);
        report("ld.b", out == 0xffff_ffff_ffff_ff85);
        asm!("ld.bu {o}, {p}, 0", o = out(reg) out, p = in(reg) p);
        report("ld.bu", out == 0x85);

        buf.fill(0);
        buf[3] = 0x85;
        buf[4] = 0x90;
        asm!("ld.h {o}, {p}, 0", o = out(reg) out, p = in(reg) p);
        report("ld.h", out == 0xffff_ffff_ffff_9085);
        asm!("ld.hu {o}, {p}, 0", o = out(reg) out, p = in(reg) p);
        report("ld.hu", out == 0x9085);

        buf.fill(0);
        buf[3] = 0x85;
        buf[4] = 0;
        buf[5] = 0;
        buf[6] = 0x80;
        asm!("ld.w {o}, {p}, 0", o = out(reg) out, p = in(reg) p);
        report("ld.w", out == 0xffff_ffff_8000_0085);
        asm!("ld.wu {o}, {p}, 0", o = out(reg) out, p = in(reg) p);
        report("ld.wu", out == 0x8000_0085);

        buf.fill(0);
        buf[3] = 0x85;
        buf[10] = 0x80;
        asm!("ld.d {o}, {p}, 0", o = out(reg) out, p = in(reg) p);
        report("ld.d", out == 0x8000_0000_0000_0085);

        buf.fill(0);
        buf[3] = 0x7f;
        asm!("ldx.b {o}, {p}, {k}", o = out(reg) out, p = in(reg) p, k = in(reg) 0usize);
        report("ldx.b", out == 0x7f);

        buf.fill(0);
        buf[3] = 0x85;
        asm!("ldx.bu {o}, {p}, {k}", o = out(reg) out, p = in(reg) p, k = in(reg) 0usize);
        report("ldx.bu", out == 0x85);

        buf.fill(0);
        buf[3] = 0x85;
        buf[4] = 0x90;
        asm!("ldx.hu {o}, {p}, {k}", o = out(reg) out, p = in(reg) p, k = in(reg) 0usize);
        report("ldx.hu", out == 0x9085);

        buf.fill(0);
        buf[3] = 0x85;
        buf[4] = 0;
        buf[5] = 0;
        buf[6] = 0x80;
        asm!("ldx.wu {o}, {p}, {k}", o = out(reg) out, p = in(reg) p, k = in(reg) 0usize);
        report("ldx.wu", out == 0x8000_0085);

        buf.fill(0);
        buf[3] = 0x85;
        buf[10] = 0x80;
        asm!("ldx.d {o}, {p}, {k}", o = out(reg) out, p = in(reg) p, k = in(reg) 0usize);
        report("ldx.d", out == 0x8000_0000_0000_0085);

        buf.fill(0);
        buf[3] = 0x85;
        buf[4] = 0;
        buf[5] = 0;
        buf[6] = 0x80;
        asm!("ldptr.w {o}, {p}, 0", o = out(reg) out, p = in(reg) p);
        report("ldptr.w", out == 0xffff_ffff_8000_0085);

        buf.fill(0);
        buf[3] = 0x85;
        buf[10] = 0x80;
        asm!("ldptr.d {o}, {p}, 0", o = out(reg) out, p = in(reg) p);
        report("ldptr.d", out == 0x8000_0000_0000_0085);

        // si12 = -2 的 ld
        buf.fill(0);
        buf[3] = 0x85;
        buf[4] = 0x90;
        asm!("ld.hu {o}, {p}, -2", o = out(reg) out, p = in(reg) p2);
        report("ld.hu/si12-neg", out == 0x9085);

        // ll.w / sc.w
        buf.fill(0);
        buf[3] = 0x85;
        buf[4] = 0;
        buf[5] = 0;
        buf[6] = 0x80;
        asm!("ll.w {o}, {p}, 0", o = out(reg) out, p = in(reg) p);
        report("ll.w", out == 0xffff_ffff_8000_0085);

        buf.fill(0);
        let mut sc: u64 = 0x11223344;
        asm!("sc.w {o}, {p}, 0", o = inout(reg) sc, p = in(reg) p);
        report(
            "sc.w",
            buf[3] == 0x44 && buf[4] == 0x33 && buf[5] == 0x22 && buf[6] == 0x11 && sc == 1,
        );
    }

    // ===== 上下文相关测试 =====
    unsafe {
        // A. 非对齐 16 字节拷贝循环（模拟 musl memcpy 主循环，不同对齐组合）
        for i in 0..SRC.len() {
            SRC[i] = (i as u8).wrapping_mul(7);
        }
        DST.fill(0);
        let mut ok = true;
        for round in 0..32 {
            let off = round * 16;
            copy16_round(
                SRC.as_ptr().add(off + 1) as usize,
                DST.as_ptr().add(off + 3) as usize,
            );
        }
        let mut mismatches = 0usize;
        for i in 0..512 {
            if DST[i + 3] != SRC[i + 1] {
                if mismatches < 12 {
                    println!(
                        "adem: copy16 mismatch i={} exp={:#04x} got={:#04x}",
                        i,
                        SRC[i + 1],
                        DST[i + 3]
                    );
                }
                mismatches += 1;
                ok = false;
            }
        }
        if !ok {
            println!("adem: copy16 total mismatches={}", mismatches);
        }
        report("copy16-loop", ok);

        // A2. 只源非对齐（仅 load 走模拟器）
        DST.fill(0);
        let mut ok2 = true;
        for round in 0..32 {
            let off = round * 16;
            copy16_round(
                SRC.as_ptr().add(off + 1) as usize,
                DST.as_ptr().add(off) as usize,
            );
        }
        for i in 0..512 {
            if DST[i] != SRC[i + 1] {
                ok2 = false;
            }
        }
        report("copy16-src-misaligned", ok2);

        // A3. 只目标非对齐（仅 store 走模拟器）
        DST.fill(0);
        let mut ok3 = true;
        for round in 0..32 {
            let off = round * 16;
            copy16_round(
                SRC.as_ptr().add(off) as usize,
                DST.as_ptr().add(off + 1) as usize,
            );
        }
        for i in 0..512 {
            if DST[i + 1] != SRC[i] {
                ok3 = false;
            }
        }
        report("copy16-dst-misaligned", ok3);

        // C. BSS 段应为零（exec 时清零）
        let mut zero = true;
        for b in BSS_CHECK.iter() {
            if *b != 0 {
                zero = false;
            }
        }
        report("bss-zero", zero);

        // B. COW：fork 后子进程对共享页做非对齐 8 字节写（模拟 git 写 malloc 缓冲），
        //    验证子进程写入正确且父进程页隔离未受破坏。
        for i in 0..COW_BUF.len() {
            COW_BUF[i] = (i as u8) ^ 0x5a;
        }
        let pid = fork();
        if pid == 0 {
            asm!(
                "st.d {v}, {p}, 0",
                v = in(reg) 0x1122334455667788u64,
                p = in(reg) COW_BUF.as_ptr().add(1) as usize
            );
            let child_ok = COW_BUF[0] == 0x5a
                && COW_BUF[1] == 0x88
                && COW_BUF[2] == 0x77
                && COW_BUF[3] == 0x66
                && COW_BUF[4] == 0x55
                && COW_BUF[5] == 0x44
                && COW_BUF[6] == 0x33
                && COW_BUF[7] == 0x22
                && COW_BUF[8] == 0x11
                && COW_BUF[9] == (9u8 ^ 0x5a);
            if child_ok {
                println!("adem: cow-child OK");
            } else {
                println!("adem: cow-child FAIL");
            }
            exit(0);
        }
        let mut code = 0;
        let _ = waitpid(pid as usize, &mut code);
        let mut iso = true;
        for i in 0..COW_BUF.len() {
            if COW_BUF[i] != (i as u8) ^ 0x5a {
                iso = false;
            }
        }
        report("cow-isolation", iso);
    }
    println!("adem: done");
    0
}
