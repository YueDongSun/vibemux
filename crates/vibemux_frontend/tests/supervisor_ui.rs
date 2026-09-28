#![forbid(unsafe_code)]

use vibemux_frontend::{
    ConnectionState, ConnectionStatus, RunView, SupervisorSnapshot, TaskView,
    gui::{MainPage, NewTaskOutcome, SupervisorUiState, TaskDetailTab},
};

fn task(task_id: &str, title: &str) -> TaskView {
    TaskView {
        task_id: task_id.to_string(),
        title: title.to_string(),
        state: "running".to_string(),
        executor: "Claude".to_string(),
        latest_update: "Worker started".to_string(),
        runs: vec![RunView {
            run_id: format!("run_{task_id}"),
            harness: "Claude Code".to_string(),
            role: "implementer".to_string(),
            state: "running".to_string(),
            ..RunView::default()
        }],
        ..TaskView::default()
    }
}

fn snapshot(tasks: Vec<TaskView>) -> SupervisorSnapshot {
    SupervisorSnapshot {
        project_id: "project_1".to_string(),
        project_name: "Workspace".to_string(),
        coordinator: Some("Coordinator".to_string()),
        connection: ConnectionState {
            status: ConnectionStatus::Connected,
            detail: None,
        },
        tasks,
        next_cursor: None,
        observed_at: Some("2026-09-27T00:00:00Z".to_string()),
        ..SupervisorSnapshot::default()
    }
}

#[test]
fn multiple_task_windows_close_independently_without_changing_snapshot() {
    let snapshot = snapshot(vec![task("task_1", "First"), task("task_2", "Second")]);
    let mut state = SupervisorUiState::default();

    assert!(state.open_task_window(&snapshot, "task_1"));
    assert!(state.open_task_window(&snapshot, "task_2"));
    assert!(state.open_task_window(&snapshot, "task_1"));
    assert_eq!(state.open_task_windows(), vec!["task_1", "task_2"]);

    assert!(state.close_task_window("task_1"));
    assert!(!state.task_window_is_open("task_1"));
    assert!(state.task_window_is_open("task_2"));
    assert_eq!(snapshot.tasks.len(), 2);
    assert_eq!(snapshot.tasks[0].state, "running");
}

#[test]
fn selecting_another_task_rebinds_the_drawer_and_run_selection() {
    let snapshot = snapshot(vec![task("task_1", "First"), task("task_2", "Second")]);
    let mut state = SupervisorUiState::default();

    assert!(state.select_task(&snapshot, "task_1"));
    assert_eq!(state.selected_task_id(), Some("task_1"));
    assert_eq!(
        state
            .details_selection()
            .selected_run(&snapshot.tasks[0])
            .map(|run| run.run_id.as_str()),
        Some("run_task_1")
    );
    state.details_selection_mut().tab = TaskDetailTab::Terminal;

    assert!(state.select_task(&snapshot, "task_2"));
    assert_eq!(state.selected_task_id(), Some("task_2"));
    assert_eq!(state.details_selection().tab, TaskDetailTab::Terminal);
    assert_eq!(
        state
            .details_selection()
            .selected_run(&snapshot.tasks[1])
            .map(|run| run.run_id.as_str()),
        Some("run_task_2")
    );
    assert!(!state.select_task(&snapshot, "unknown"));
}

#[test]
fn disabled_send_preserves_multiline_cjk_draft() {
    let mut state = SupervisorUiState::default();
    let draft = "请检查这个长路径：C:\\工作区\\研发\\很长的子目录\\src\\supervisor_app.rs\n第二行继续描述需求。";
    state.set_composer_draft(draft.to_string());

    assert!(!state.send_enabled());
    assert!(
        state
            .send_disabled_reason()
            .contains("no continuous chat service")
    );
    assert!(!state.try_send());
    assert_eq!(state.composer_draft(), draft);
}

#[test]
fn coordinator_chat_is_the_default_page_and_agent_shortcuts_are_secondary() {
    let mut state = SupervisorUiState::default();
    assert_eq!(state.page(), MainPage::CoordinatorChat);
    assert!(state.select_agent(9, 10));
    assert_eq!(state.page(), MainPage::Agents);
    assert_eq!(state.selected_agent(), Some(9));
    state.show_coordinator_chat();
    assert_eq!(state.page(), MainPage::CoordinatorChat);
    assert_eq!(state.selected_agent(), None);
}

#[test]
fn long_unicode_and_path_labels_wrap_in_headless_egui() {
    let context = egui::Context::default();
    let mut measured = (0.0, 0.0);
    let _full_output = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(280.0, 480.0),
            )),
            ..Default::default()
        },
        |context| {
            egui::CentralPanel::default()
                .show(context, |ui| {
                    ui.set_max_width(240.0);
                    let cjk = format!("{}：处理中文路径和更新内容。", "任务详情".repeat(24));
                    let cjk_response = vibemux_frontend::gui::render_wrapped_label(
                        ui,
                        egui::RichText::new(cjk).size(13.0),
                    );
                    let path =
                        "C:\\workspace\\很长的项目路径\\modules\\nested\\unicode_file_name.rs";
                    let path_response = vibemux_frontend::gui::render_wrapped_label(
                        ui,
                        egui::RichText::new(path).size(13.0),
                    );
                    measured = (cjk_response.rect.height(), path_response.rect.height());
                })
                .inner
        },
    );

    assert!(measured.0 > 20.0, "long CJK text should wrap: {measured:?}");
    assert!(
        measured.1 > 20.0,
        "long path text should wrap: {measured:?}"
    );
}

#[test]
fn new_task_on_an_empty_draft_starts_the_welcome_state() {
    let mut state = SupervisorUiState::default();
    assert_eq!(state.request_new_task(), NewTaskOutcome::Started);
    assert!(state.welcome_active());
    assert!(state.take_composer_focus_request());
    assert!(!state.take_composer_focus_request());
}

#[test]
fn whitespace_draft_starts_a_new_task_without_prompting() {
    let mut state = SupervisorUiState::default();
    state.set_composer_draft("  \n\t".to_string());
    assert_eq!(state.request_new_task(), NewTaskOutcome::Started);
    assert!(!state.discard_prompt_open());
    assert_eq!(state.composer_draft(), "");
}

#[test]
fn nonempty_draft_needs_confirmation_and_keep_preserves_it() {
    let mut state = SupervisorUiState::default();
    state.set_composer_draft("请检查 draft".to_string());
    assert_eq!(state.request_new_task(), NewTaskOutcome::NeedsConfirmation);
    assert!(state.discard_prompt_open());
    assert!(!state.welcome_active());
    state.keep_draft();
    assert!(!state.discard_prompt_open());
    assert_eq!(state.composer_draft(), "请检查 draft");
    assert!(!state.try_send());
}

#[test]
fn confirming_discard_clears_the_draft_and_starts_the_welcome_state() {
    let mut state = SupervisorUiState::default();
    state.set_composer_draft("draft".to_string());
    let _ = state.request_new_task();
    state.confirm_discard_draft();
    assert_eq!(state.composer_draft(), "");
    assert!(state.welcome_active());
    assert!(!state.discard_prompt_open());
}

#[test]
fn navigation_leaves_the_welcome_state() {
    let snapshot = snapshot(vec![task("task_1", "First")]);
    let mut state = SupervisorUiState::default();
    let _ = state.request_new_task();
    state.show_agents();
    assert!(!state.welcome_active());
    let _ = state.request_new_task();
    state.show_coordinator_chat();
    assert!(!state.welcome_active());
    let _ = state.request_new_task();
    assert!(state.select_task(&snapshot, "task_1"));
    assert!(!state.welcome_active());
}
