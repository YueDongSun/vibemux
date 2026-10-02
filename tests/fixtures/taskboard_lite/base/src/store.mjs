// TaskBoard Lite store: base stub owned by Track A (see CONTRACT.md section 8).
//
// The real module persists tasks in one data file. This stub keeps the public
// signature so importers load cleanly, and rejects every call with
// `not_implemented`.

function create_not_implemented_error() {
  const error = new Error("TaskBoard Lite store is not implemented yet");
  error.code = "not_implemented";
  return error;
}

export async function createStore(dataFile) {
  void dataFile;
  throw create_not_implemented_error();
}
