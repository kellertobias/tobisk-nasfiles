use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=NASFILES_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=NASFILES_BUILD_DATE");
    println!("cargo:rerun-if-changed=../../build-info.env");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/packed-refs");
    if let Some(head_ref) = git_head_ref() {
        println!("cargo:rerun-if-changed=../../.git/{head_ref}");
    }

    let commit = env_value("NASFILES_BUILD_COMMIT")
        .or_else(|| build_info_value("NASFILES_BUILD_COMMIT"))
        .or_else(git_commit)
        .unwrap_or_else(|| "unknown".to_string());
    let date = env_value("NASFILES_BUILD_DATE")
        .or_else(|| build_info_value("NASFILES_BUILD_DATE"))
        .or_else(utc_build_date)
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=NASFILES_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=NASFILES_BUILD_DATE={date}");
}

fn env_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| is_known(value))
}

fn build_info_value(key: &str) -> Option<String> {
    let contents = std::fs::read_to_string("../../build-info.env").ok()?;
    contents.lines().find_map(|line| {
        let (line_key, value) = line.split_once('=')?;
        if line_key.trim() == key {
            let value = value.trim().to_string();
            is_known(&value).then_some(value)
        } else {
            None
        }
    })
}

fn is_known(value: &str) -> bool {
    !value.is_empty() && value != "unknown"
}

fn git_head_ref() -> Option<String> {
    let head = std::fs::read_to_string("../../.git/HEAD").ok()?;
    head.strip_prefix("ref: ")
        .map(str::trim)
        .map(str::to_string)
        .filter(|value| !value.is_empty())
}

fn git_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn utc_build_date() -> Option<String> {
    let output = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
