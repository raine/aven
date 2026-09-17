use clap::{Args, Subcommand};

#[derive(Args)]
pub(crate) struct AiCommand {
    #[command(subcommand)]
    pub(crate) command: AiSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum AiSubcommand {
    /// List supported providers and authentication requirements
    Providers,
    /// Authenticate an OAuth provider
    Login {
        /// Provider to authenticate; defaults to ai.provider
        provider: Option<String>,
    },
    /// Show local AI configuration and credential readiness
    Status {
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Make one billable synthetic completion with the configured provider
    Check,
}
