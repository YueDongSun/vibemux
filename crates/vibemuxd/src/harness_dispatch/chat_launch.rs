//! Persistent chat profiles derived from the daemon's pinned route catalog.
use super::*;
use vibemux_harness::{
    chat_session::{SessionConfig, SessionTarget},
    dispatch::Sha256Digest,
};
use vibemux_types::chat::ChatHarness;

pub(crate) const CHAT_PROFILE: &str = "chat_read_only_v1";
const CLAUDE_CHAT_PROFILE: &str = "chat_claude_read_only_v2";

pub(crate) struct PreparedChat {
    pub launch: LaunchPlan,
    pub protocol: SessionConfig,
    pub profile_sha256: String,
    pub resource_key: String,
    pub deadline: Duration,
}

impl PreparedChat {
    /// Retargets the prepared launch plan from the project root to an owned
    /// run workspace for owner-backed chat submissions.
    ///
    /// Trust boundary: this is a pure seam with no filesystem, Git, process,
    /// or inference effect, and it does not establish that the daemon owns
    /// the workspace. Before every launch or send, the caller must verify the
    /// actual ownership receipt, workspace inventory, base HEAD, and
    /// admission against the provisioned state. Here only the workspace
    /// record's shape is validated and the cwd is normalized to the
    /// vendor-facing form, so the launch, protocol, and resource key stay
    /// consistent with one exact working directory.
    pub(crate) fn for_workspace(
        mut self,
        workspace: &vibemux_types::a2a::RunWorkspace,
    ) -> Result<Self, DispatchServiceError> {
        workspace
            .validate()
            .map_err(|_| DispatchError::ConfigRouteInvalid)?;
        let cwd = child_path_text(Path::new(&workspace.path))
            .filter(|cwd| Path::new(cwd).is_absolute() && !cwd.chars().any(char::is_control))
            .ok_or(DispatchError::ConfigRouteInvalid)?;
        self.launch.working_directory = PathBuf::from(&cwd);
        self.protocol.working_directory = cwd.clone();
        self.resource_key = attempt::resource_key(&cwd).to_hex();
        Ok(self)
    }
}

impl HarnessDispatchService {
    /// The caller has explicit chat/persistence consent. No input is sent here.
    /// Existing one-turn route permissions and detection still constrain launch.
    pub(crate) async fn prepare_chat(
        &self,
        harness: ChatHarness,
        target: SessionTarget,
    ) -> Result<PreparedChat, DispatchServiceError> {
        let agent = match harness {
            ChatHarness::Claude => AgentKind::Claude,
            ChatHarness::Codex => AgentKind::Codex,
        };
        self.inner.ensure_accepting()?;
        let setup = self.inner.setup()?;
        let config = setup.config.config();
        let route = config.route(agent)?;
        if !route.allow_execution {
            return Err(DispatchError::ExecutionDisabled.into());
        }
        if !self.detected(agent).await? {
            return Err(DispatchError::NotDetected.into());
        }
        let mut launch = setup.launch_plan(route, SessionMode::Execute)?;
        launch.spec = persistent_spec(route, &target)?;
        let profile = match harness {
            ChatHarness::Claude => CLAUDE_CHAT_PROFILE,
            ChatHarness::Codex => CHAT_PROFILE,
        };
        let profile_binding = format!("{profile}:{}", setup.config.digest().to_hex());
        Ok(PreparedChat {
            launch,
            protocol: SessionConfig {
                target,
                generation_id: Uuid::new_v4(),
                working_directory: setup.working_directory.clone(),
                model: route.model.clone(),
            },
            profile_sha256: Sha256Digest::of(profile_binding.as_bytes()).to_hex(),
            resource_key: attempt::resource_key(&setup.working_directory).to_hex(),
            deadline: Duration::from_millis(config.limits.deadline_ms),
        })
    }
}

fn persistent_spec(
    route: &DispatchRoute,
    target: &SessionTarget,
) -> Result<LaunchSpec, DispatchError> {
    let matches_target = matches!(
        (route.harness, route.protocol, target),
        (
            AgentKind::Claude,
            NativeProtocol::ClaudeStreamJson,
            SessionTarget::Claude { .. }
        ) | (
            AgentKind::Codex,
            NativeProtocol::CodexAppServer,
            SessionTarget::Codex { .. }
        )
    );
    if !matches_target {
        return Err(DispatchError::ConfigRouteInvalid);
    }
    let mut spec = build_launch_spec(route, SessionMode::Execute)?;
    if let SessionTarget::Claude { session_id, resume } = target {
        if session_id.is_nil() {
            return Err(DispatchError::ConfigRouteInvalid);
        }
        spec.arguments
            .retain(|argument| argument != "--no-session-persistence");
        spec.arguments.push("--replay-user-messages".to_owned());
        spec.arguments.push("--safe-mode".to_owned());
        spec.arguments.extend([
            if *resume { "--resume" } else { "--session-id" }.to_owned(),
            session_id.to_string(),
        ]);
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibemux_types::a2a::RunWorkspace;
    fn route(harness: AgentKind, protocol: NativeProtocol) -> DispatchRoute {
        DispatchRoute {
            harness,
            protocol,
            executable: PathBuf::from("fixture.exe"),
            entry_script: None,
            profile: None,
            provider: None,
            environment_names: vec![],
            enabled: true,
            allow_execution: true,
            model: None,
        }
    }
    #[test]
    fn persistent_claude_uses_exact_identity_and_keeps_one_turn_profile_unchanged() {
        let route = route(AgentKind::Claude, NativeProtocol::ClaudeStreamJson);
        let session_id = Uuid::new_v4();
        for resume in [false, true] {
            let spec = persistent_spec(&route, &SessionTarget::Claude { session_id, resume })
                .expect("session profile");
            assert!(
                !spec
                    .arguments
                    .iter()
                    .any(|value| value == "--no-session-persistence")
            );
            for required in [
                "--restricted",
                "--safe-mode",
                "--strict-mcp-config",
                "--tools=",
                "dontAsk",
                "--replay-user-messages",
            ] {
                assert!(spec.arguments.iter().any(|value| value == required));
            }
            assert!(spec.arguments.windows(2).any(|pair| pair[0]
                == if resume { "--resume" } else { "--session-id" }
                && pair[1] == session_id.to_string()));
            assert!(!spec.arguments.iter().any(|value| value == "--continue"));
        }
        assert!(
            build_launch_spec(&route, SessionMode::Execute)
                .expect("one turn")
                .arguments
                .contains(&"--no-session-persistence".to_owned())
        );
    }
    #[test]
    fn persistent_profile_refuses_disabled_or_mismatched_routes() {
        let mut route = route(AgentKind::Codex, NativeProtocol::CodexAppServer);
        let target = SessionTarget::Codex {
            resume_thread_id: None,
        };
        assert_eq!(
            persistent_spec(&route, &target)
                .expect("app server")
                .arguments,
            vec!["app-server", "--stdio"]
        );
        route.allow_execution = false;
        assert!(persistent_spec(&route, &target).is_err());
        route.allow_execution = true;
        assert!(
            persistent_spec(
                &route,
                &SessionTarget::Claude {
                    session_id: Uuid::new_v4(),
                    resume: false
                }
            )
            .is_err()
        );
    }

    #[cfg(windows)]
    const OWNED_CWD: &str = r"C:\owned\run_1";
    #[cfg(not(windows))]
    const OWNED_CWD: &str = "/owned/run_1";

    fn prepared_chat(harness: ChatHarness, target: SessionTarget) -> PreparedChat {
        let (agent, protocol) = match harness {
            ChatHarness::Claude => (AgentKind::Claude, NativeProtocol::ClaudeStreamJson),
            ChatHarness::Codex => (AgentKind::Codex, NativeProtocol::CodexAppServer),
        };
        let spec = persistent_spec(&route(agent, protocol), &target).expect("session profile");
        PreparedChat {
            launch: LaunchPlan {
                trampoline: PathBuf::from(r"C:\tools\vibemux_launch_trampoline.exe"),
                executable: PathBuf::from(r"C:\tools\vendor.exe"),
                spec,
                working_directory: PathBuf::from(r"C:\project_root"),
            },
            protocol: SessionConfig {
                target,
                generation_id: Uuid::new_v4(),
                working_directory: r"C:\project_root".to_string(),
                model: Some("pinned_model".to_string()),
            },
            profile_sha256: "ab".repeat(32),
            resource_key: attempt::resource_key(r"C:\project_root").to_hex(),
            deadline: Duration::from_millis(90_000),
        }
    }

    fn workspace(path: &str) -> RunWorkspace {
        RunWorkspace {
            path: path.to_string(),
            branch: "codex/owned_run_1".to_string(),
            base_commit: "a".repeat(40),
            ownership_token: "owner_token_1".to_string(),
        }
    }

    #[test]
    fn for_workspace_retargets_cwd_and_resource_key_and_keeps_identity() {
        for harness in [ChatHarness::Claude, ChatHarness::Codex] {
            let target = match harness {
                ChatHarness::Claude => SessionTarget::Claude {
                    session_id: Uuid::new_v4(),
                    resume: true,
                },
                ChatHarness::Codex => SessionTarget::Codex {
                    resume_thread_id: Some("owned_thread_1".to_string()),
                },
            };
            let prepared = prepared_chat(harness, target.clone());
            let expected_generation = prepared.protocol.generation_id;
            let expected_model = prepared.protocol.model.clone();
            let expected_profile = prepared.profile_sha256.clone();
            let expected_deadline = prepared.deadline;
            let expected_trampoline = prepared.launch.trampoline.clone();
            let expected_executable = prepared.launch.executable.clone();
            let expected_spec = prepared.launch.spec.clone();
            let retargeted = prepared
                .for_workspace(&workspace(OWNED_CWD))
                .expect("owned workspace");
            assert_eq!(
                retargeted.launch.working_directory,
                PathBuf::from(OWNED_CWD)
            );
            assert_eq!(retargeted.protocol.working_directory, OWNED_CWD);
            assert_eq!(
                retargeted.resource_key,
                attempt::resource_key(OWNED_CWD).to_hex()
            );
            assert_eq!(retargeted.protocol.target, target);
            assert_eq!(retargeted.protocol.generation_id, expected_generation);
            assert_eq!(retargeted.protocol.model, expected_model);
            assert_eq!(retargeted.profile_sha256, expected_profile);
            assert_eq!(retargeted.deadline, expected_deadline);
            assert_eq!(retargeted.launch.trampoline, expected_trampoline);
            assert_eq!(retargeted.launch.executable, expected_executable);
            assert_eq!(retargeted.launch.spec, expected_spec);
        }
    }

    #[cfg(windows)]
    #[test]
    fn for_workspace_normalizes_verbatim_prefixes_to_the_vendor_cwd() {
        let prepared = prepared_chat(
            ChatHarness::Claude,
            SessionTarget::Claude {
                session_id: Uuid::new_v4(),
                resume: false,
            },
        );
        let retargeted = prepared
            .for_workspace(&workspace(r"\\?\C:\owned\run_1"))
            .expect("verbatim workspace");
        assert_eq!(
            retargeted.launch.working_directory,
            PathBuf::from(r"C:\owned\run_1")
        );
        assert_eq!(retargeted.protocol.working_directory, r"C:\owned\run_1");
        assert_eq!(
            retargeted.resource_key,
            attempt::resource_key(r"C:\owned\run_1").to_hex()
        );

        let prepared = prepared_chat(
            ChatHarness::Codex,
            SessionTarget::Codex {
                resume_thread_id: None,
            },
        );
        let retargeted = prepared
            .for_workspace(&workspace(r"\\?\UNC\server\share\run_1"))
            .expect("verbatim UNC workspace");
        assert_eq!(
            retargeted.launch.working_directory,
            PathBuf::from(r"\\server\share\run_1")
        );
        assert_eq!(
            retargeted.protocol.working_directory,
            r"\\server\share\run_1"
        );
        assert_eq!(
            retargeted.resource_key,
            attempt::resource_key(r"\\server\share\run_1").to_hex()
        );
    }

    #[test]
    fn for_workspace_rejects_relative_control_and_foreign_absolute_paths() {
        // Absolute for the other platform: `RunWorkspace::validate` accepts
        // it, but it is not a usable cwd here.
        #[cfg(windows)]
        let foreign_absolute = "/owned/run_1";
        #[cfg(not(windows))]
        let foreign_absolute = r"C:\owned\run_1";
        let control = format!("{OWNED_CWD}\u{7}");
        for path in [
            "owned/run_1".to_string(),
            control,
            foreign_absolute.to_string(),
            String::new(),
        ] {
            let prepared = prepared_chat(
                ChatHarness::Codex,
                SessionTarget::Codex {
                    resume_thread_id: None,
                },
            );
            let error = prepared.for_workspace(&workspace(&path)).map(|_| ()).err();
            assert_eq!(
                error,
                Some(DispatchServiceError::Dispatch(
                    DispatchError::ConfigRouteInvalid
                ))
            );
        }
    }
}
