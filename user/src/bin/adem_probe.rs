#![no_std]
#![no_main]

// ADEM 模拟器探针：用内联汇编强制生成每条非对齐访存指令（2RI12/3R/2RI14），
// 校验模拟后的字节结果。在 2K1000LA 真机（UAL=0）上运行：每条指令都会触发
// ADEM 异常进入内核模拟器，任何语义错误都会以 FAIL 形式暴露。

#[macro_use]
extern crate user_lib;

use core::arch::asm;

fn report(name: &str, ok: bool) {
    if ok {
        println!("adem: {} OK", name);
    } else {
        println!("adem: {} FAIL", name);
    }
}

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
        report("st.w", buf[3] == 0x44 && buf[4] == 0x33 && buf[5] == 0x22 && buf[6] == 0x11);

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
        report("stx.w", buf[3] == 0xdd && buf[4] == 0xcc && buf[5] == 0xbb && buf[6] == 0xaa);

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
        report("stptr.w", buf[3] == 0xde && buf[4] == 0xc0 && buf[5] == 0xad && buf[6] == 0x0b);

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
    println!("adem: done");
    0
}
