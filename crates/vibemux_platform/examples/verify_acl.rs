#[cfg(windows)]
use std::path::PathBuf;

#[cfg(windows)]
fn main() {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("verify_acl requires one path argument");
    match vibemux_platform::verify_restricted_path_acl(&path) {
        Ok(summary) => println!(
            "restricted={} allow_rule_count={}",
            summary.restricted, summary.allow_rule_count
        ),
        Err(error) => {
            eprintln!("verification failed: {error:?}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("verify_acl is available only on Windows");
    std::process::exit(2);
}
