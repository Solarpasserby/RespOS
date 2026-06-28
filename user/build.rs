use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn phase_name_from_comment(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    if !lower.contains("phase") {
        return None;
    }
    line.rsplit(':')
        .next()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| name.split_whitespace().next().unwrap_or(name))
        .map(|name| {
            name.chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect()
        })
}

fn read_ltp_list(path: &Path) -> Vec<(String, Vec<String>)> {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read {}: {}", path.display(), err));
    let mut phases: Vec<(String, Vec<String>)> = Vec::new();
    let mut current_name = String::from("default");
    let mut current_cases: Vec<String> = Vec::new();

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(comment) = line.strip_prefix('#') {
            if let Some(name) = phase_name_from_comment(comment) {
                if !current_cases.is_empty() {
                    phases.push((current_name, current_cases));
                    current_cases = Vec::new();
                }
                current_name = name;
            }
            continue;
        }

        let case = line.split('#').next().unwrap_or("").trim();
        if !case.is_empty() {
            current_cases.push(case.to_string());
        }
    }

    if !current_cases.is_empty() {
        phases.push((current_name, current_cases));
    }
    if let Ok(filter) = env::var("LTP_CASE_FILTER") {
        let wanted: std::collections::BTreeSet<_> = filter
            .split(',')
            .map(str::trim)
            .filter(|case| !case.is_empty())
            .map(str::to_string)
            .collect();
        if !wanted.is_empty() {
            phases = phases
                .into_iter()
                .filter_map(|(name, cases)| {
                    let cases: Vec<_> = cases
                        .into_iter()
                        .filter(|case| wanted.contains(case))
                        .collect();
                    (!cases.is_empty()).then_some((name, cases))
                })
                .collect();
        }
    }

    phases
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let list_path = manifest_dir.join("oscomp_ltp_list.txt");
    println!("cargo:rerun-if-changed={}", list_path.display());
    println!("cargo:rerun-if-env-changed=LTP_CASE_FILTER");

    let phases = read_ltp_list(&list_path);
    let mut generated = String::from(
        "pub struct LtpPhase {\n    pub name: &'static str,\n    pub cases: &'static [&'static str],\n}\n\n",
    );
    generated.push_str("pub const LTP_OSCOMP: &[LtpPhase] = &[\n");
    for (name, cases) in phases {
        generated.push_str("    LtpPhase {\n");
        generated.push_str(&format!("        name: {:?},\n", name));
        generated.push_str("        cases: &[\n");
        for case in cases {
            generated.push_str(&format!("            {:?},\n", case));
        }
        generated.push_str("        ],\n");
        generated.push_str("    },\n");
    }
    generated.push_str("];\n");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("ltp_cases.rs"), generated).unwrap();
}
