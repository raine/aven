use clap::{Args, Subcommand};

const AI_HELP: &str = r#"Run these commands on the server host as the user the server runs as, with the
same --data or --config as the server. They read the ai settings in server.yaml
and store login credentials in private/ai beside it. AVEN_AI_PROVIDER,
AVEN_AI_MODEL and AVEN_AI_ENDPOINT override the file. Changes apply after the
server restarts."#;

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
