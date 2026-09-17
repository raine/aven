use std::env;
use std::fmt::Debug;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use anyhow::{Context, Result, anyhow, bail};
use rig_core::client::{CompletionClient, Nothing};
use rig_core::completion::{
    CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
};
use rig_core::providers::{
    anthropic, azure, chatgpt, cohere, copilot, deepseek, doubleword, gemini, groq, huggingface,
    hyperbolic, llamafile, minimax, mira, mistral, moonshot, ollama, openai, openrouter,
    perplexity, together, venice, xai, xiaomimimo, zai,
};

use super::transport::BoundedHttpClient;

pub(crate) type CompletionFuture<'a> = Pin<
    Box<dyn Future<Output = std::result::Result<CompletionResponse, CompletionError>> + Send + 'a>,
>;

pub(crate) trait CompletionBackend: Send + Sync {
    fn complete(&self, request: CompletionRequest) -> CompletionFuture<'_>;
}

impl<M> CompletionBackend for M
where
    M: CompletionModel + Send + Sync,
{
    fn complete(&self, request: CompletionRequest) -> CompletionFuture<'_> {
        Box::pin(self.completion(request))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthKind {
    ApiKey(&'static str),
    OAuth,
    None,
}

impl AuthKind {
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::ApiKey(name) => name,
            Self::OAuth => "oauth",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchemaStrategy {
    NativeStrict,
    NativeVerbatim,
    Prompted,
}

impl SchemaStrategy {
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::NativeStrict => "native-strict",
            Self::NativeVerbatim => "native-verbatim",
            Self::Prompted => "prompted",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderInfo {
    pub(crate) id: &'static str,
    pub(crate) auth: AuthKind,
    pub(crate) schema: SchemaStrategy,
}

pub(crate) const PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        id: "anthropic",
        auth: AuthKind::ApiKey("ANTHROPIC_API_KEY"),
        schema: SchemaStrategy::NativeStrict,
    },
    ProviderInfo {
        id: "azure",
        auth: AuthKind::ApiKey("AZURE_API_KEY or AZURE_TOKEN"),
        schema: SchemaStrategy::NativeStrict,
    },
    ProviderInfo {
        id: "chatgpt",
        auth: AuthKind::OAuth,
        schema: SchemaStrategy::Prompted,
    },
    ProviderInfo {
        id: "cohere",
        auth: AuthKind::ApiKey("COHERE_API_KEY"),
        schema: SchemaStrategy::Prompted,
    },
    ProviderInfo {
        id: "copilot",
        auth: AuthKind::OAuth,
        schema: SchemaStrategy::Prompted,
    },
    ProviderInfo {
        id: "deepseek",
        auth: AuthKind::ApiKey("DEEPSEEK_API_KEY"),
        schema: SchemaStrategy::Prompted,
    },
    ProviderInfo {
        id: "doubleword",
        auth: AuthKind::ApiKey("DOUBLEWORD_API_KEY"),
        schema: SchemaStrategy::NativeStrict,
    },
    ProviderInfo {
        id: "gemini",
        auth: AuthKind::ApiKey("GEMINI_API_KEY"),
        schema: SchemaStrategy::NativeStrict,
    },
    ProviderInfo {
        id: "groq",
        auth: AuthKind::ApiKey("GROQ_API_KEY"),
        schema: SchemaStrategy::NativeStrict,
    },
    ProviderInfo {
        id: "huggingface",
        auth: AuthKind::ApiKey("HUGGINGFACE_API_KEY"),
        schema: SchemaStrategy::Prompted,
    },
    ProviderInfo {
        id: "hyperbolic",
        auth: AuthKind::ApiKey("HYPERBOLIC_API_KEY"),
        schema: SchemaStrategy::Prompted,
    },
    ProviderInfo {
        id: "llamafile",
        auth: AuthKind::None,
        schema: SchemaStrategy::Prompted,
    },
    ProviderInfo {
        id: "minimax",
        auth: AuthKind::ApiKey("MINIMAX_API_KEY"),
        schema: SchemaStrategy::NativeStrict,
    },
    ProviderInfo {
        id: "mira",
        auth: AuthKind::ApiKey("MIRA_API_KEY"),
        schema: SchemaStrategy::Prompted,
    },
    ProviderInfo {
        id: "mistral",
        auth: AuthKind::ApiKey("MISTRAL_API_KEY"),
        schema: SchemaStrategy::NativeStrict,
    },
    ProviderInfo {
        id: "moonshot",
        auth: AuthKind::ApiKey("MOONSHOT_API_KEY"),
        schema: SchemaStrategy::Prompted,
    },
    ProviderInfo {
        id: "ollama",
        auth: AuthKind::None,
        schema: SchemaStrategy::NativeVerbatim,
    },
    ProviderInfo {
        id: "openai",
        auth: AuthKind::ApiKey("OPENAI_API_KEY"),
        schema: SchemaStrategy::NativeStrict,
    },
    ProviderInfo {
        id: "openrouter",
        auth: AuthKind::ApiKey("OPENROUTER_API_KEY"),
        schema: SchemaStrategy::NativeStrict,
    },
    ProviderInfo {
        id: "perplexity",
        auth: AuthKind::ApiKey("PERPLEXITY_API_KEY"),
        schema: SchemaStrategy::Prompted,
    },
    ProviderInfo {
        id: "together",
        auth: AuthKind::ApiKey("TOGETHER_API_KEY"),
        schema: SchemaStrategy::Prompted,
    },
    ProviderInfo {
        id: "venice",
        auth: AuthKind::ApiKey("VENICE_API_KEY"),
        schema: SchemaStrategy::NativeStrict,
    },
    ProviderInfo {
        id: "xai",
        auth: AuthKind::ApiKey("XAI_API_KEY"),
        schema: SchemaStrategy::NativeStrict,
    },
    ProviderInfo {
        id: "xiaomi_mimo",
        auth: AuthKind::ApiKey("XIAOMI_MIMO_API_KEY"),
        schema: SchemaStrategy::NativeStrict,
    },
    ProviderInfo {
        id: "zai",
        auth: AuthKind::ApiKey("ZAI_API_KEY"),
        schema: SchemaStrategy::NativeStrict,
    },
];

pub(crate) const EXCLUSIONS: &[(&str, &str)] = &[(
    "voyageai",
    "Rig 0.42.0 exposes embeddings and reranking but no text completion model",
)];

pub(crate) fn provider_info(id: &str) -> Option<&'static ProviderInfo> {
    PROVIDERS.iter().find(|provider| provider.id == id)
}

pub(crate) fn credentials_present(info: ProviderInfo, private_dir: Option<&Path>) -> bool {
    match info.auth {
        AuthKind::ApiKey(name) => name.split(" or ").any(|name| {
            env::var(name)
                .ok()
                .is_some_and(|value| !value.trim().is_empty())
        }),
        AuthKind::OAuth if info.id == "chatgpt" => {
            private_dir.is_some_and(|dir| dir.join("chatgpt/auth.json").is_file())
        }
        AuthKind::OAuth if info.id == "copilot" => private_dir.is_some_and(|dir| {
            dir.join("copilot/access-token").is_file() || dir.join("copilot/api-key.json").is_file()
        }),
        AuthKind::OAuth => false,
        AuthKind::None => true,
    }
}

fn api_key(auth: AuthKind) -> Result<String> {
    let AuthKind::ApiKey(name) = auth else {
        bail!("provider does not use an API key")
    };
    env::var(name).map_err(|_| anyhow!("provider credentials or configuration unavailable"))
}

macro_rules! api_key_model {
    ($module:ident, $model:expr, $auth:expr) => {{
        let client = $module::Client::builder()
            .api_key(api_key($auth)?)
            .http_client(BoundedHttpClient::default())
            .build()
            .map_err(|_| anyhow!("provider configuration unavailable"))?;
        Box::new(client.completion_model($model)) as Box<dyn CompletionBackend>
    }};
}

fn chatgpt_client(
    private_dir: &Path,
    allow_device_flow: bool,
) -> Result<chatgpt::Client<BoundedHttpClient>> {
    let builder = chatgpt::Client::builder()
        .oauth()
        .token_dir(private_dir.join("chatgpt"))
        .allow_device_flow(allow_device_flow)
        .default_instructions("")
        .originator("aven")
        .user_agent(concat!("aven/", env!("CARGO_PKG_VERSION")))
        .http_client(BoundedHttpClient::default());
    let builder = if allow_device_flow {
        builder.on_device_code(|prompt| {
            println!(
                "Open {} and enter code {}",
                prompt.verification_uri, prompt.user_code
            )
        })
    } else {
        builder
    };
    builder.build().map_err(Into::into)
}

fn copilot_client(
    private_dir: &Path,
    allow_device_flow: bool,
) -> Result<copilot::Client<BoundedHttpClient>> {
    let builder = copilot::Client::builder()
        .oauth()
        .token_dir(private_dir.join("copilot"))
        .allow_device_flow(allow_device_flow)
        .http_client(BoundedHttpClient::default());
    let builder = if allow_device_flow {
        builder.on_device_code(|prompt| {
            println!(
                "Open {} and enter code {}",
                prompt.verification_uri, prompt.user_code
            )
        })
    } else {
        builder
    };
    builder.build().map_err(Into::into)
}

pub(crate) fn build_backend(
    info: ProviderInfo,
    model: &str,
    endpoint: Option<&str>,
    private_dir: Option<&Path>,
) -> Result<Box<dyn CompletionBackend>> {
    let backend = match info.id {
        "anthropic" => api_key_model!(anthropic, model, info.auth),
        "azure" => {
            let auth = if let Ok(key) = env::var("AZURE_API_KEY") {
                azure::AzureOpenAIAuth::ApiKey(key)
            } else {
                azure::AzureOpenAIAuth::Token(
                    env::var("AZURE_TOKEN").context("AZURE_TOKEN is not set")?,
                )
            };
            let endpoint = env::var("AZURE_ENDPOINT").context("AZURE_ENDPOINT is not set")?;
            let version = env::var("AZURE_API_VERSION").context("AZURE_API_VERSION is not set")?;
            let client = azure::Client::builder()
                .api_key(auth)
                .azure_endpoint(endpoint)
                .api_version(&version)
                .http_client(BoundedHttpClient::default())
                .build()
                .context("construct Azure provider")?;
            Box::new(client.completion_model(model))
        }
        "cohere" => api_key_model!(cohere, model, info.auth),
        "deepseek" => api_key_model!(deepseek, model, info.auth),
        "doubleword" => api_key_model!(doubleword, model, info.auth),
        "gemini" => api_key_model!(gemini, model, info.auth),
        "groq" => api_key_model!(groq, model, info.auth),
        "huggingface" => api_key_model!(huggingface, model, info.auth),
        "hyperbolic" => api_key_model!(hyperbolic, model, info.auth),
        "minimax" => api_key_model!(minimax, model, info.auth),
        "mira" => api_key_model!(mira, model, info.auth),
        "mistral" => api_key_model!(mistral, model, info.auth),
        "moonshot" => api_key_model!(moonshot, model, info.auth),
        "openrouter" => api_key_model!(openrouter, model, info.auth),
        "perplexity" => api_key_model!(perplexity, model, info.auth),
        "together" => api_key_model!(together, model, info.auth),
        "venice" => api_key_model!(venice, model, info.auth),
        "xai" => api_key_model!(xai, model, info.auth),
        "xiaomi_mimo" => api_key_model!(xiaomimimo, model, info.auth),
        "zai" => api_key_model!(zai, model, info.auth),
        "llamafile" => {
            let mut builder = llamafile::Client::builder()
                .api_key(Nothing)
                .http_client(BoundedHttpClient::default());
            if let Some(endpoint) = endpoint {
                builder = builder.base_url(endpoint);
            }
            let client = builder.build().context("construct llamafile provider")?;
            Box::new(client.completion_model(model))
        }
        "ollama" => {
            let mut builder = ollama::Client::builder()
                .api_key(ollama::OllamaApiKey::default())
                .http_client(BoundedHttpClient::default());
            if let Some(endpoint) = endpoint {
                builder = builder.base_url(endpoint);
            }
            let client = builder.build().context("construct Ollama provider")?;
            Box::new(client.completion_model(model))
        }
        "openai" => {
            let mut builder = openai::Client::builder()
                .api_key(api_key(info.auth)?)
                .http_client(BoundedHttpClient::default());
            if let Some(endpoint) = endpoint {
                builder = builder.base_url(endpoint);
            }
            let client = builder.build().context("construct OpenAI provider")?;
            Box::new(client.completion_model(model))
        }
        "chatgpt" => {
            let private_dir = private_dir.context("AI credential storage is unavailable")?;
            Box::new(
                chatgpt_client(private_dir, false)
                    .context("construct ChatGPT provider")?
                    .completion_model(model),
            )
        }
        "copilot" => {
            let private_dir = private_dir.context("AI credential storage is unavailable")?;
            Box::new(
                copilot_client(private_dir, false)
                    .context("construct Copilot provider")?
                    .completion_model(model),
            )
        }
        _ => bail!("unsupported AI provider {}", info.id),
    };
    Ok(backend)
}

pub(crate) async fn login(provider: &str, private_dir: &Path) -> Result<()> {
    match provider {
        "chatgpt" => {
            chatgpt_client(private_dir, true)
                .context("construct ChatGPT login")?
                .authorize()
                .await
                .context("ChatGPT login")?;
            Ok(())
        }
        "copilot" => {
            copilot_client(private_dir, true)
                .context("construct Copilot login")?
                .authorize()
                .await
                .context("Copilot login")?;
            Ok(())
        }
        other => bail!(
            "provider {other} uses {} instead of interactive login",
            provider_info(other)
                .map(|item| item.auth.description())
                .unwrap_or("an unsupported credential method")
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn provider_inventory_is_unique_and_local_provider_constructs_offline() {
        let ids = PROVIDERS
            .iter()
            .map(|provider| provider.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), PROVIDERS.len());
        assert!(provider_info("voyageai").is_none());

        build_backend(
            *provider_info("ollama").unwrap(),
            "fixture-model",
            Some("http://127.0.0.1:11434"),
            None,
        )
        .unwrap();
    }
}
