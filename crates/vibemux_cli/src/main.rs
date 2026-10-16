use std::{
    ffi::OsString,
    io::{self, IsTerminal, Write},
};
use vibemux_cli::{
    application::{self, Outcome},
    command::{self, Format},
    failure::Failure,
    output,
};

const EXIT_USAGE: i32 = 2;
const EXIT_RUNTIME: i32 = 4;

#[tokio::main]
async fn main() {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let terminal = io::stdout().is_terminal();
    let invocation = match command::parse(arguments.iter().cloned()) {
        Ok(invocation) => invocation,
        Err(error) => {
            let format = requested_format(&arguments, terminal);
            write_text(&output::error_text(&error, format), true);
            std::process::exit(EXIT_USAGE);
        }
    };
    let format = output::resolve(invocation.format, terminal);
    let result = application::run(invocation.command, format).await;
    let exit_code = match result {
        Ok(Outcome::Text(text)) => {
            if write_text(&text, false) {
                0
            } else {
                EXIT_RUNTIME
            }
        }
        Ok(Outcome::Interactive) => 0,
        Ok(Outcome::Data {
            kind,
            body,
            success,
        }) => match output::report(kind, &body, format) {
            Ok(text) => {
                if write_text(&text, false) && success {
                    0
                } else {
                    EXIT_RUNTIME
                }
            }
            Err(error) => {
                write_text(&output::error_text(&error, format), true);
                EXIT_RUNTIME
            }
        },
        Err(error) => {
            write_text(&output::error_text(&error, format), true);
            EXIT_RUNTIME
        }
    };
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn requested_format(arguments: &[OsString], terminal: bool) -> Format {
    let format = if arguments.first().is_some_and(|value| value == "--json") {
        Format::Json
    } else if arguments.first().is_some_and(|value| value == "--format") {
        match arguments.get(1).and_then(|value| value.to_str()) {
            Some("json") => Format::Json,
            Some("text") => Format::Text,
            _ => Format::Auto,
        }
    } else {
        Format::Auto
    };
    output::resolve(format, terminal)
}

fn write_text(text: &str, error: bool) -> bool {
    let mut stream: Box<dyn Write> = if error {
        Box::new(io::stderr())
    } else {
        Box::new(io::stdout())
    };
    let result = stream.write_all(text.as_bytes()).and_then(|_| {
        if text.ends_with('\n') {
            Ok(())
        } else {
            stream.write_all(b"\n")
        }
    });
    match result {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => true,
        Err(_) => {
            let _ = writeln!(io::stderr(), "{}", Failure::new("cli_output_failed"));
            false
        }
    }
}
