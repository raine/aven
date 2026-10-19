use clap::{Args, Subcommand};

const AI_HELP: &str = r#"Run these commands on the server host as the user the server runs as. They
read that user's config.yaml and store login credentials in $AVEN_CONFIG_DIR
(default ~/.config/aven), the same directory the server uses."#;

#[derive(Args)]
#[command(after_long_help = AI_HELP)]
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
    /// Show the server AI configuration and credential readiness
    Status {
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Make one billable synthetic completion with the configured provider
    Check,
}
