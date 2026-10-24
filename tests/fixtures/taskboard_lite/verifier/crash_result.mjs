// Last-chance result file for a verifier run that is about to crash.
//
// run_verifier.mjs promises a result file for every exit except a usage
// error. Without this module an uncaught exception or an unhandled rejection
// ends Node with exit code 1 and no file, which the daemon can only report as
// `verifier_output_missing`, and the daemon never retains stderr. The writer
// records a `blocked` result whose reason names the failure's origin, error
// class, error code, and first verifier frame (file name and line only):
// never a message or a directory, which may carry a path.

import { writeFileSync } from "node:fs";

export const crash_result_config = Object.freeze({
  blocked_exit_code: 2,
  reason_prefix: "verifier_crash",
  // The daemon keeps at most 160 characters of a blocked reason.
  max_reason_chars: 150,
  class_name_pattern: /^[A-Za-z_][A-Za-z0-9_]{0,63}$/,
  error_code_pattern: /^[A-Z][A-Z0-9_]{0,39}$/,
  // `name.mjs:line` of a stack frame; the file name has no separators.
  frame_pattern: /([A-Za-z0-9_.-]+\.mjs):(\d+):\d+/,
});

function first_module_frame(stack) {
  if (typeof stack !== "string") {
    return null;
  }
  for (const line of stack.split("\n").slice(1)) {
    const frame = crash_result_config.frame_pattern.exec(line);
    if (frame !== null) {
      return `${frame[1]}:${frame[2]}`;
    }
  }
  return null;
}

// A path-free description of `error`: `origin` is `uncaught_exception` or
// `unhandled_rejection`.
export function crash_reason(origin, error) {
  const config = crash_result_config;
  const is_object = error !== null && typeof error === "object";
  const parts = [`${config.reason_prefix}: ${origin}`];
  if (is_object && config.class_name_pattern.test(error.constructor?.name ?? "")) {
    parts.push(error.constructor.name);
  } else {
    parts.push(error === null ? "null" : typeof error);
  }
  if (is_object && typeof error.code === "string" && config.error_code_pattern.test(error.code)) {
    parts.push(error.code);
  }
  const frame = is_object ? first_module_frame(error.stack) : null;
  if (frame !== null) {
    parts.push(`at ${frame}`);
  }
  return parts.join(" ").slice(0, config.max_reason_chars);
}

// Installs process handlers that write `result_for(reason)` to `out_file`
// and exit with the blocked code. Only the first failure is recorded.
export function install_crash_result_writer({ out_file, result_for }) {
  let recorded = false;
  const record = (origin, error) => {
    if (recorded) {
      return;
    }
    recorded = true;
    try {
      writeFileSync(out_file, `${JSON.stringify(result_for(crash_reason(origin, error)), null, 2)}\n`, "utf8");
    } catch {
      // Nothing more can be reported; the daemon sees a missing result.
    }
    process.exit(crash_result_config.blocked_exit_code);
  };
  process.on("uncaughtException", (error) => record("uncaught_exception", error));
  process.on("unhandledRejection", (reason) => record("unhandled_rejection", reason));
}
