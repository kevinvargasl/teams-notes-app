use std::{
    fs::File,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::library::{AgentPreferences, Provider};

pub(crate) const DEFAULT: &str = "Predeterminado de la herramienta";

pub(crate) struct AgentOptions {
    pub(crate) models: Vec<String>,
    pub(crate) efforts: Vec<String>,
}

pub(crate) fn discover(provider: Provider, preferences: &AgentPreferences) -> Result<AgentOptions> {
    match provider {
        Provider::Codex => codex_options(preferences),
        Provider::Copilot => copilot_options(preferences),
        Provider::Claude => Ok(AgentOptions {
            // The CLI resolves these aliases using the user's account and configuration.
            models: ["sonnet", "opus", "haiku"].map(str::to_owned).to_vec(),
            efforts: if preferences.model.is_some() {
                ["low", "medium", "high", "xhigh", "max"]
                    .map(str::to_owned)
                    .to_vec()
            } else {
                Vec::new()
            },
        }),
    }
}

fn codex_options(preferences: &AgentPreferences) -> Result<AgentOptions> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .context("No se encontró la configuración de Codex")?;
    let data: Value = serde_json::from_reader(BufReader::new(
        File::open(home.join("models_cache.json"))
            .context("Abre Codex una vez para actualizar su catálogo de modelos")?,
    ))?;
    let models = data["models"]
        .as_array()
        .context("El catálogo de Codex no contiene modelos")?;
    let mut options = AgentOptions {
        models: Vec::new(),
        efforts: Vec::new(),
    };
    for model in models {
        if model["visibility"].as_str() == Some("hide") {
            continue;
        }
        if let Some(id) = model["slug"].as_str() {
            options.models.push(id.to_owned());
            if preferences.model.as_deref() == Some(id) {
                options.efforts = model["supported_reasoning_levels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|level| level["effort"].as_str().map(str::to_owned))
                    .collect();
            }
        }
    }
    Ok(options)
}

fn copilot_options(preferences: &AgentPreferences) -> Result<AgentOptions> {
    let executable = Provider::Copilot
        .executable()
        .context("Instala GitHub Copilot CLI")?;
    let directory = tempfile::tempdir()?;
    let mut child = Command::new(executable)
        .args([
            "--acp",
            "--no-custom-instructions",
            "--disable-builtin-mcps",
        ])
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = (|| {
        let mut input = child.stdin.take().context("No se pudo abrir Copilot")?;
        let output = child.stdout.take().context("No se pudo leer Copilot")?;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut request = |id, method, params| -> Result<Value> {
            writeln!(
                input,
                "{}",
                json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params})
            )?;
            input.flush()?;
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let line = receiver
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .context("Copilot tardó demasiado en cargar los modelos")??;
                let response: Value = serde_json::from_str(&line)?;
                if response["id"].as_u64() == Some(id) {
                    ensure!(
                        response.get("error").is_none(),
                        "Copilot no pudo cargar las opciones del modelo"
                    );
                    return Ok(response["result"].clone());
                }
            }
        };
        request(
            1,
            "initialize",
            json!({"protocolVersion":1,"clientCapabilities":{}}),
        )?;
        let session = request(
            2,
            "session/new",
            json!({"cwd":directory.path(),"mcpServers":[]}),
        )?;
        let models = session["models"]["availableModels"]
            .as_array()
            .context("Copilot no devolvió el catálogo de modelos")?;
        let mut options = AgentOptions {
            models: models
                .iter()
                .filter(|model| model["_meta"]["copilotEnablement"].as_str() != Some("disabled"))
                .filter_map(|model| model["modelId"].as_str().map(str::to_owned))
                .collect(),
            efforts: Vec::new(),
        };
        options.models.sort();
        options.models.dedup();
        if let Some(model) = preferences.model.as_ref() {
            let response = request(
                3,
                "session/set_config_option",
                json!({"sessionId": session["sessionId"], "configId":"model", "value":model}),
            )?;
            options.efforts = response["configOptions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|option| option["id"].as_str() == Some("reasoning_effort"))
                .flat_map(|option| option["options"].as_array().into_iter().flatten())
                .filter_map(|option| option["value"].as_str().map(str::to_owned))
                .collect();
        }
        Ok(options)
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}
