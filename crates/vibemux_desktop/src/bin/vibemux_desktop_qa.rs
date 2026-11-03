#![forbid(unsafe_code)]
#[cfg(windows)]
fn main() {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() == 1 && arguments[0] == "--help" {
        println!("{}", vibemux_desktop::native_qa::USAGE);
        return;
    }
    match vibemux_desktop::native_qa::execute(arguments) {
        Ok(report) => match serde_json::to_string(&report) {
            Ok(value) => println!("{value}"),
            Err(_) => std::process::exit(2),
        },
        Err(error) => {
            eprintln!("{{\"ok\":false,\"code\":\"{}\"}}", error.code());
            std::process::exit(2);
        }
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("native_qa_windows_only");
    std::process::exit(2);
}
