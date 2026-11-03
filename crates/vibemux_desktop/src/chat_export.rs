//! Native-only destination selection and authenticated private snapshot export.
use crate::command_error::CommandError;
use std::path::PathBuf;
use vibemux_client::chat_export::{
    self, ExportError, ExportFormat, ExportReceipt, ExportRequest, ExportStatus,
};
use vibemuxd::process::DaemonPaths;

pub(crate) fn request(
    paths: &DaemonPaths,
    request: &ExportRequest,
    picker: impl FnOnce(ExportFormat) -> Result<Option<PathBuf>, ExportError>,
    verify_project: impl Fn() -> Result<(), CommandError>,
) -> Result<ExportReceipt, CommandError> {
    request.validate().map_err(error)?;
    let destination = picker(request.format).map_err(error)?;
    verify_project()?;
    let Some(destination) = destination else {
        return Ok(ExportReceipt {
            status: ExportStatus::Cancelled,
            kind: request.kind,
            format: request.format,
            chat_id: request.chat_id,
            generation: request.generation,
            version: request.expected_version,
            record_count: 0,
            byte_count: 0,
            partial: false,
        });
    };
    let (runtime, client) = crate::chat_bridge::connect(paths)?;
    let snapshot = runtime
        .block_on(chat_export::collect_conversation(&client, request))
        .map_err(error)?;
    let bytes = chat_export::format_export(&snapshot, request.format).map_err(error)?;
    verify_project()?;
    runtime
        .block_on(chat_export::verify_snapshot(&client, &snapshot))
        .map_err(error)?;
    verify_project()?;
    chat_export::write_export(&destination, &bytes, request.format).map_err(error)?;
    Ok(ExportReceipt {
        status: ExportStatus::Saved,
        kind: request.kind,
        format: request.format,
        chat_id: request.chat_id,
        generation: request.generation,
        version: snapshot.session.version,
        record_count: snapshot.records.len() as u64,
        byte_count: bytes.len() as u64,
        partial: snapshot.partial,
    })
}
fn error(error: ExportError) -> CommandError {
    CommandError::new(error.code(), error.message())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use vibemux_client::chat_export::ExportKind;
    #[test]
    fn cancellation_needs_no_daemon_or_history_read_and_never_returns_a_path() {
        let temp = tempfile::tempdir().expect("fixture");
        let paths = DaemonPaths::from_project_root_with_local_app_data(temp.path(), temp.path())
            .expect("paths");
        let req = ExportRequest {
            kind: ExportKind::Chat,
            format: ExportFormat::Json,
            chat_id: vibemux_types::ChatId::new(),
            generation: 1,
            expected_version: 1,
        };
        let result = request(&paths, &req, |_| Ok(None), || Ok(())).expect("cancel without daemon");
        assert_eq!(result.status, ExportStatus::Cancelled);
        assert_eq!(result.byte_count, 0);
        assert_eq!(result.record_count, 0);
        assert!(
            !serde_json::to_string(&result)
                .expect("receipt")
                .contains(&temp.path().display().to_string())
        );
    }
    #[test]
    fn invalid_request_precedes_picker_and_project_change_precedes_reads() {
        let temp = tempfile::tempdir().expect("fixture");
        let paths = DaemonPaths::from_project_root_with_local_app_data(temp.path(), temp.path())
            .expect("paths");
        let mut req = ExportRequest {
            kind: ExportKind::Workspace,
            format: ExportFormat::Text,
            chat_id: vibemux_types::ChatId::new(),
            generation: 0,
            expected_version: 1,
        };
        assert_eq!(
            request(&paths, &req, |_| panic!("picker must not run"), || Ok(()))
                .expect_err("invalid")
                .code,
            "chat_export_invalid_request"
        );
        req.generation = 1;
        assert_eq!(
            request(
                &paths,
                &req,
                |_| Ok(None),
                || Err(CommandError::new("project_changed", "changed"))
            )
            .expect_err("changed project")
            .code,
            "project_changed"
        );
    }
}
