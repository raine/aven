use clap::{Args, Subcommand};

const AI_HELP: &str = r#"Run these commands on the server host as the user the server runs as, with the
same --data or --config as the server. They read the ai settings in server.yaml
and store login credentials in private/ai beside it. AVEN_AI_PROVIDER,
AVEN_AI_MODEL and AVEN_AI_ENDPOINT override the file. Changes apply after the
server restarts.

These commands check this shell's view of the configuration, not the running
server. To ask the running server from a paired device, use `aven ai status`."#;

const AI_CLIENT_HELP: &str = r#"These commands ask the running sync server from this device, over the sync
server URL and with this device's sync credential, the same way paired devices
such as the iPhone app do. They need sync set up on this database.

To configure the server's AI provider, run `aven server ai` on the server host.
Server AI changes apply after the server restarts."#;

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
    /// Log in to an OAuth provider (chatgpt or copilot)
    #[command(after_long_help = LOGIN_HELP)]
    Login {
        /// OAuth provider: chatgpt or copilot; defaults to ai.provider
        provider: Option<String>,
    },
    /// Remove stored OAuth credentials for a provider
    Logout {
        /// OAuth provider: chatgpt or copilot
        provider: String,
    },
    /// Show the AI configuration and credential readiness this host sees
    Status {
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Run a billable two-round tool-call exchange with the configured provider
    #[command(after_long_help = CHECK_HELP)]
    Check {
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
}

const LOGIN_HELP: &str = r#"Only chatgpt and copilot use interactive login. API-key providers read their
key from the server process environment; set it there and restart the server.
Login stores credentials but does not select the provider: set ai.provider and
ai.model in server.yaml, restart the server, then run `aven server ai check`."#;

const CHECK_HELP: &str = r#"The check sends two requests from this process using server.yaml and this
environment, not through the running server: one that offers an inert tool and
expects a call to it, then one that replays that call with a tool result and
expects a text answer. The Ask Aven agent needs both to work."#;

#[derive(Args)]
#[command(after_long_help = AI_CLIENT_HELP)]
pub(crate) struct AiClientCommand {
    #[command(subcommand)]
    pub(crate) command: AiClientSubcommand,
}

#[derive(Subcommand)]
pub(crate) enum AiClientSubcommand {
    /// Ask the running sync server whether its AI is ready for this device
    Status {
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
}
