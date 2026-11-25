use std::{
    fs::{self, File},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

pub(crate) const MODEL_FILE: &str = "ggml-large-v3-turbo-q5_0.bin";
pub(crate) const VAD_MODEL_FILE: &str = "ggml-silero-v5.1.2.bin";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum Provider {
    Codex,
    Copilot,
    Claude,
}

impl Provider {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Copilot => "GitHub Copilot",
            Self::Claude => "Claude Code",
        }
    }

    pub(crate) fn executable(self) -> Option<PathBuf> {
        let mut paths: Vec<_> = std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect())
            .unwrap_or_default();
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            paths.push(home.join(".local/bin"));
            paths.push(home.join(".cargo/bin"));
            paths.push(home.join(".volta/bin"));
        }
        paths.extend(["/opt/homebrew/bin".into(), "/usr/local/bin".into()]);
        let name = match self {
            Self::Codex => "codex",
            Self::Copilot => "copilot",
            Self::Claude => "claude",
        };
        paths.into_iter().map(|dir| dir.join(name)).find(|path| {
            use std::os::unix::fs::PermissionsExt;
            path.is_absolute()
                && path
                    .metadata()
                    .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
    }
}

#[derive(Clone, Copy, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum TranscriptionMode {
    Greedy,
    #[default]
    BeamSearch,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub(crate) dark: Option<bool>,
    pub(crate) provider: Option<Provider>,
    pub(crate) automatic: bool,
    pub(crate) automatic_markers: bool,
    pub(crate) transcription_mode: TranscriptionMode,
    pub(crate) vad_enabled: Option<bool>,
    pub(crate) summary_prompt: Option<String>,
    pub(crate) markers_prompt: Option<String>,
    pub(crate) codex: AgentPreferences,
    pub(crate) copilot: AgentPreferences,
    pub(crate) claude: AgentPreferences,
    pub(crate) meeting_order: Vec<Uuid>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct AgentPreferences {
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
}

impl Settings {
    pub(crate) fn vad_enabled(&self) -> bool {
        self.vad_enabled.unwrap_or(true)
    }

    pub(crate) fn agent(&self, provider: Provider) -> &AgentPreferences {
        match provider {
            Provider::Codex => &self.codex,
            Provider::Copilot => &self.copilot,
            Provider::Claude => &self.claude,
        }
    }

    pub(crate) fn agent_mut(&mut self, provider: Provider) -> &mut AgentPreferences {
        match provider {
            Provider::Codex => &mut self.codex,
            Provider::Copilot => &mut self.copilot,
            Provider::Claude => &mut self.claude,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Meeting {
    pub(crate) id: Uuid,
    pub(crate) title: String,
    pub(crate) created: u64,
    pub(crate) duration_ms: u64,
    pub(crate) markers: Vec<Marker>,
    pub(crate) recording_warning: Option<String>,
    pub(crate) transcript: Option<Vec<Segment>>,
    pub(crate) summary: Option<Summary>,
}

impl Meeting {
    pub(crate) fn new(title: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            title,
            created: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            duration_ms: 0,
            markers: Vec::new(),
            recording_warning: None,
            transcript: None,
            summary: None,
        }
    }

    pub(crate) fn replace_generated_markers(&mut self, markers: Vec<Marker>) {
        self.markers.retain(|marker| !marker.generated);
        self.markers.extend(markers);
        self.markers.sort_by_key(|marker| marker.ms);
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Marker {
    pub(crate) id: Uuid,
    pub(crate) ms: u64,
    pub(crate) label: String,
    #[serde(default)]
    pub(crate) generated: bool,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
pub(crate) enum AudioSource {
    Mic,
    Teams,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Segment {
    pub(crate) id: usize,
    pub(crate) start_ms: u64,
    pub(crate) end_ms: u64,
    pub(crate) source: AudioSource,
    pub(crate) text: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Summary {
    pub(crate) provider: Provider,
    pub(crate) content: SummaryContent,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct SummaryContent {
    pub(crate) summary: String,
    pub(crate) decisions: Vec<ReferencedText>,
    pub(crate) actions: Vec<ActionItem>,
    pub(crate) questions: Vec<ReferencedText>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ReferencedText {
    pub(crate) text: String,
    pub(crate) segment_ids: Vec<usize>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ActionItem {
    pub(crate) text: String,
    pub(crate) owner: Option<String>,
    pub(crate) deadline: Option<String>,
    pub(crate) segment_ids: Vec<usize>,
}

pub(crate) struct Library {
    root: PathBuf,
}

impl Library {
    pub(crate) fn open() -> Result<Self> {
        let home = std::env::var_os("HOME").context("No se encontró tu carpeta de usuario")?;
        let root = PathBuf::from(home).join("Library/Application Support/Reuniones");
        fs::create_dir_all(root.join("meetings"))?;
        Ok(Self { root })
    }

    pub(crate) fn settings(&self) -> Result<Settings> {
        let path = self.root.join("settings.json");
        if path.try_exists()? {
            read_json(&path)
        } else {
            Ok(Settings::default())
        }
    }

    pub(crate) fn save_settings(&self, settings: &Settings) -> Result<()> {
        write_json(&self.root.join("settings.json"), settings)
    }

    pub(crate) fn meetings_dir(&self) -> PathBuf {
        self.root.join("meetings")
    }

    pub(crate) fn meeting_dir(&self, id: Uuid) -> PathBuf {
        self.meetings_dir().join(id.to_string())
    }

    pub(crate) fn model_path(&self) -> PathBuf {
        self.root.join(MODEL_FILE)
    }

    pub(crate) fn vad_model_path(&self) -> PathBuf {
        self.root.join(VAD_MODEL_FILE)
    }

    pub(crate) fn transcription_ready(&self, use_vad: bool) -> bool {
        self.model_path().is_file() && (!use_vad || self.vad_model_path().is_file())
    }

    pub(crate) fn meetings(&self) -> Result<Vec<Meeting>> {
        let mut meetings = Vec::new();
        for entry in fs::read_dir(self.meetings_dir())? {
            let path = entry?.path().join("meeting.json");
            if path.is_file() {
                meetings.push(read_json::<Meeting>(&path)?);
            }
        }
        meetings.sort_by_key(|meeting| std::cmp::Reverse(meeting.created));
        Ok(meetings)
    }

    pub(crate) fn save_meeting(&self, meeting: &Meeting) -> Result<()> {
        let path = self.meeting_dir(meeting.id);
        fs::create_dir_all(&path)?;
        write_json(&path.join("meeting.json"), meeting)
    }

    pub(crate) fn delete_meeting(&self, id: Uuid) -> Result<()> {
        fs::remove_dir_all(self.meeting_dir(id))?;
        Ok(())
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_reader(BufReader::new(File::open(path)?))
        .with_context(|| format!("No se pudo leer {}", path.display()))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .context("No se encontró la carpeta de destino")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut writer = BufWriter::new(temp.as_file_mut());
        serde_json::to_writer(&mut writer, value)?;
        writer.flush()?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
