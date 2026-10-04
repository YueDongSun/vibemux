// TaskBoard Lite HTTP server: base stub owned by Track A.
//
// This stub only parses the launch argv described in CONTRACT.md section 2 and
// then reports `not_implemented`. Track A replaces it with the real server.
// It never listens and never writes to stdout, so the trusted verifier sees no
// listening receipt and fails every API check.

const stub_exit_code = 3;
const known_options = new Set(["--host", "--port", "--data"]);

function parse_launch_arguments(argv_list) {
  const options = {};
  for (let index = 0; index < argv_list.length; index += 2) {
    const option_name = argv_list[index];
    const option_value = argv_list[index + 1];
    if (!known_options.has(option_name) || option_value === undefined) {
      return null;
    }
    options[option_name.slice(2)] = option_value;
  }
  return options;
}

function report_startup_failure(code, exit_code) {
  process.stderr.write(`${JSON.stringify({ error: { code } })}\n`);
  process.exitCode = exit_code;
}

parse_launch_arguments(process.argv.slice(2));
report_startup_failure("not_implemented", stub_exit_code);
