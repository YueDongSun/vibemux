use vibemux_harness::cli::{self, Command};

#[tokio::main]
async fn main() {
    let result = match cli::parse_arguments(std::env::args_os().skip(1)) {
        Ok(Command::Help) => {
            print!("{}", cli::help("vibemux_harness"));
            return;
        }
        Ok(Command::Version) => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Ok(command) => cli::run(command).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(reply) => {
            println!("{}", reply.body);
            if !reply.success {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{}", serde_json::json!({"error":error.code()}));
            std::process::exit(1);
        }
    }
}
