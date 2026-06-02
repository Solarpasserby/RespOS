use std::env;
use std::fs::{File, read_dir};
use std::io::{Result, Write};

fn main() {
    println!("cargo:rerun-if-changed=../user/src/");
    println!("cargo:rerun-if-env-changed=PROFILE");
    println!("cargo:rerun-if-env-changed=RESPOS_USER_TARGET");
    println!("cargo:rerun-if-env-changed=RESPOS_USER_PROFILE_DIR");
    println!("cargo:rerun-if-env-changed=RESPOS_APP_REBUILD_STAMP");

    insert_app_data().unwrap();
}

fn target_path() -> String {
    let profile = env::var("PROFILE").expect("PROFILE is not set by Cargo");
    let user_target = env::var("RESPOS_USER_TARGET").unwrap_or_else(|_| {
        let kernel_target = env::var("TARGET").expect("TARGET is not set by Cargo");
        match kernel_target.as_str() {
            "riscv64gc-unknown-none-elf" => String::from("riscv64gc-unknown-none-elf"),
            "loongarch64-unknown-none" => String::from("loongarch64-unknown-none"),
            other => other.to_string(),
        }
    });
    let profile_dir = env::var("RESPOS_USER_PROFILE_DIR").unwrap_or(profile);
    format!("../user/target/{user_target}/{profile_dir}/")
}

fn insert_app_data() -> Result<()> {
    let mut f = File::create("src/link_app.S").unwrap();
    let target_path = target_path();
    let rebuild_stamp =
        env::var("RESPOS_APP_REBUILD_STAMP").unwrap_or_else(|_| String::from("local"));
    println!("cargo:rerun-if-changed={}", target_path);
    let mut apps: Vec<_> = read_dir("../user/src/bin")
        .unwrap()
        .map(|dir_entry| {
            let mut name_with_ext = dir_entry.unwrap().file_name().into_string().unwrap();
            name_with_ext.drain(name_with_ext.find('.').unwrap()..name_with_ext.len());
            name_with_ext
        })
        .collect();
    apps.sort();

    writeln!(
        f,
        r#"
    # generated rebuild stamp: {rebuild_stamp}
    .align 3
    .section .data
    .global _app_num
_app_num:
    .quad {}"#,
        apps.len()
    )?;

    for i in 0..apps.len() {
        writeln!(f, r#"    .quad app_{}_start"#, i)?;
    }
    writeln!(f, r#"    .quad app_{}_end"#, apps.len() - 1)?;

    writeln!(
        f,
        r#"
    .global _app_names
_app_names:"#
    )?;
    for app in apps.iter() {
        writeln!(f, r#"    .string "{}""#, app)?;
    }

    for (idx, app) in apps.iter().enumerate() {
        println!("app_{}: {}", idx, app);
        writeln!(
            f,
            r#"
    .section .data
    .global app_{0}_start
    .global app_{0}_end
    .align 3
app_{0}_start:
    .incbin "{2}{1}"
app_{0}_end:"#,
            idx, app, target_path
        )?;
    }
    Ok(())
}
