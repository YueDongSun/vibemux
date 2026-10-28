//! Typed controller identity for opt-in qualification. Never a live admission.
use super::{broker_owner::BrokerOwner, native_process::LaunchPlan};
use vibemux_harness::{
    AgentKind,
    dispatch::{
        DispatchError, DispatchRoute,
        fixture_guard::{FixtureLaunchGuard, build_writable_fixture_launch_spec},
        installed_controller_guard::{CLAUDE_BROKER_QUALIFICATION_PROFILE, InstalledClaudeGuard},
        launch_spec::LaunchSpec,
        writable_profile::WritableGrant,
    },
};

pub(crate) enum FixtureExecutionGuard {
    Synthetic(FixtureLaunchGuard),
    InstalledClaude {
        controller: InstalledClaudeGuard,
        synthetic: Box<FixtureLaunchGuard>,
    },
}

impl FixtureExecutionGuard {
    pub(super) fn profile_id(&self, harness: AgentKind) -> Result<&'static str, DispatchError> {
        match self {
            Self::Synthetic(guard) => guard.profile_id(harness),
            Self::InstalledClaude { .. } if harness == AgentKind::Claude => {
                Ok(CLAUDE_BROKER_QUALIFICATION_PROFILE)
            }
            Self::InstalledClaude { synthetic, .. } => synthetic.profile_id(harness),
        }
    }

    pub(super) fn launch_spec(
        &self,
        route: &DispatchRoute,
        grant: WritableGrant,
    ) -> Result<LaunchSpec, DispatchError> {
        match self {
            Self::Synthetic(guard) => build_writable_fixture_launch_spec(route, grant, guard),
            Self::InstalledClaude { controller, .. } if route.harness == AgentKind::Claude => {
                controller.build_launch_spec(route, grant)
            }
            Self::InstalledClaude { synthetic, .. } => {
                build_writable_fixture_launch_spec(route, grant, synthetic)
            }
        }
    }

    pub(super) fn uses_file_broker(&self, harness: AgentKind) -> bool {
        match self {
            Self::Synthetic(guard) => guard.file_broker_enabled(),
            Self::InstalledClaude { .. } if harness == AgentKind::Claude => true,
            Self::InstalledClaude { synthetic, .. } => synthetic.file_broker_enabled(),
        }
    }

    pub(super) fn configure_broker(
        &self,
        harness: AgentKind,
        owner: &BrokerOwner,
        launch: &mut LaunchPlan,
    ) -> Result<(), DispatchError> {
        match self {
            Self::Synthetic(_) => owner.configure_launch(launch),
            Self::InstalledClaude { controller, .. } if harness == AgentKind::Claude => {
                owner.configure_installed_controller(launch, controller)
            }
            Self::InstalledClaude { .. } => owner.configure_launch(launch),
        }
    }
}
