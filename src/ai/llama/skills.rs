use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

// ── Core Types ───────────────────────────────────────────────────

/// A loaded skill package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub version: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub system_prompt: String,
    pub context_docs: Vec<String>,
    pub tools: Vec<ToolDef>,
    pub config_defaults: HashMap<String, toml::Value>,
}

/// A single tool definition with execution config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub executor: ExecutorKind,
    pub exec: Option<ExecConfig>,
}

/// How a tool is executed — only two modes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutorKind {
    /// Hardcoded Rust implementation (secure, allowlisted)
    Builtin,
    /// HTTP request to a local daemon/service
    Http,
}

/// Execution configuration for HTTP tools
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecConfig {
    /// HTTP method: GET or POST
    pub method: Option<String>,
    /// URL path template with {param} placeholders (e.g. "/file/{path}")
    pub path: Option<String>,
    /// Full URL template (overrides path + config base URL)
    pub url: Option<String>,
    /// POST body template with {param} placeholders (JSON string)
    #[serde(default)]
    pub body: Option<String>,
    /// Which config key holds the host (default: auto-detect from config_defaults)
    #[serde(default)]
    pub host_key: Option<String>,
    /// Which config key holds the port (default: auto-detect from config_defaults)
    #[serde(default)]
    pub port_key: Option<String>,
}

/// Result of executing a tool locally
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub call_id: String,
    pub tool_name: String,
    pub content: String,
}

/// Tracks tool-calling conversation state
#[derive(Debug, Clone, Default)]
pub struct ToolConversation {
    pub messages: Vec<serde_json::Value>,
}

impl ToolConversation {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    pub fn push_assistant(&mut self, msg: &serde_json::Value) {
        self.messages.push(msg.clone());
    }

    pub fn push_tool_result(&mut self, call_id: &str, _name: &str, content: &str) {
        self.messages.push(serde_json::json!({
            "role": "tool",
            "tool_call_id": call_id,
            "content": content
        }));
    }
}

// ── Skill Loading ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SkillManifest {
    skill: SkillManifestMeta,
    #[serde(default)]
    tools: Vec<ToolManifest>,
    #[serde(default)]
    config_defaults: HashMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
struct SkillManifestMeta {
    name: String,
    version: Option<String>,
    description: String,
    #[serde(default)]
    triggers: Vec<String>,
    system_prompt: Option<String>,
    #[serde(default)]
    context_files: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ToolManifest {
    name: String,
    description: String,
    #[serde(default)]
    parameters: HashMap<String, ParamDef>,
    executor: String,
    #[serde(default)]
    exec: Option<ExecConfig>,
}

#[derive(Debug, Deserialize)]
struct ParamDef {
    #[serde(rename = "type")]
    param_type: Option<String>,
    description: Option<String>,
    #[serde(default)]
    required: bool,
}

/// Load a skill from a directory containing skill.toml
pub fn load_skill_from_dir(dir: &Path) -> Result<Skill, String> {
    let manifest_path = dir.join("skill.toml");
    let toml_str = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("Cannot read {:?}: {}", manifest_path, e))?;

    let manifest: SkillManifest = toml::from_str(&toml_str)
        .map_err(|e| format!("Cannot parse {:?}: {}", manifest_path, e))?;

    // ── Load system prompt ───────────────────────────────────
    let system_prompt = if let Some(ref sp_file) = manifest.skill.system_prompt {
        let sp_path = dir.join(sp_file);
        std::fs::read_to_string(&sp_path)
            .map_err(|e| format!("Cannot read system prompt {:?}: {}", sp_path, e))?
    } else {
        manifest.skill.description.clone()
    };

    // ── Load context files ──────────────────────────────────
    let mut context_docs = Vec::new();
    for cf in &manifest.skill.context_files {
        let cf_path = dir.join(cf);
        if let Ok(content) = std::fs::read_to_string(&cf_path) {
            context_docs.push(content);
        }
    }

    // Also load references/*.md
    let ref_dir = dir.join("references");
    if ref_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&ref_dir) {
            for entry in entries.flatten() {
                if entry.path().extension().map(|e| e == "md").unwrap_or(false) {
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        context_docs.push(content);
                    }
                }
            }
        }
    }

    // Merge context into system prompt
    let full_system_prompt = if context_docs.is_empty() {
        system_prompt
    } else {
        let mut prompt = system_prompt;
        prompt.push_str("\n\n## Reference Documentation\n\n");
        for doc in &context_docs {
            prompt.push_str(doc);
            prompt.push_str("\n\n---\n\n");
        }
        prompt
    };

    // ── Convert tool manifests ───────────────────────────────
    let tools = manifest
        .tools
        .iter()
        .map(|tm| {
            let params_json = manifest_params_to_json(&tm.parameters);
            let executor = match tm.executor.as_str() {
                "builtin" => ExecutorKind::Builtin,
                "http" => ExecutorKind::Http,
                other => {
                    log::warn!(
                        "Unknown executor '{}' in tool '{}', falling back to builtin",
                        other,
                        tm.name
                    );
                    ExecutorKind::Builtin
                }
            };

            ToolDef {
                name: tm.name.clone(),
                description: tm.description.clone(),
                parameters: params_json,
                executor,
                exec: tm.exec.clone(),
            }
        })
        .collect();

    Ok(Skill {
        name: manifest.skill.name,
        version: manifest.skill.version.unwrap_or_else(|| "1.0.0".into()),
        description: manifest.skill.description,
        triggers: manifest.skill.triggers,
        system_prompt: full_system_prompt,
        context_docs,
        tools,
        config_defaults: manifest.config_defaults,
    })
}

/// Load a skill from a .skill.zip file
pub fn load_skill_from_zip(zip_path: &Path) -> Result<Skill, String> {
    let stem = zip_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let tmp_dir = std::env::temp_dir().join(format!("ce-skill-{}", stem));

    if tmp_dir.exists() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("Cannot create temp dir: {}", e))?;

    let output = std::process::Command::new("unzip")
        .args(["-o", "-q"])
        .arg(zip_path)
        .arg("-d")
        .arg(&tmp_dir)
        .output()
        .map_err(|e| format!("unzip failed: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "unzip failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    load_skill_from_dir(&tmp_dir)
}

/// Load all skills: built-ins + user-installed from ~/.config/ce/skills/
pub fn load_all_skills() -> Vec<Skill> {
    let mut skills = Vec::new();

    // 1. Built-in skills (always available)
    skills.push(file_skill());

    // 2. User-installed skills
    let skills_dir = skill_dir();
    if skills_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&skills_dir) {
            for entry in entries.flatten() {
                let path = entry.path();

                if path.is_dir() && path.join("skill.toml").exists() {
                    match load_skill_from_dir(&path) {
                        Ok(skill) => skills.push(skill),
                        Err(e) => log::warn!("Failed to load skill {:?}: {}", path, e),
                    }
                } else if path
                    .extension()
                    .map(|e| e == "skill" || e == "zip")
                    .unwrap_or(false)
                {
                    match load_skill_from_zip(&path) {
                        Ok(skill) => skills.push(skill),
                        Err(e) => log::warn!("Failed to load skill zip {:?}: {}", path, e),
                    }
                }
            }
        }
    }

    skills
}

/// Return the user skills directory, creating it if needed
fn skill_dir() -> std::path::PathBuf {
    let d = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("ce")
        .join("skills");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn manifest_params_to_json(params: &HashMap<String, ParamDef>) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for (name, def) in params {
        let ptype = def.param_type.as_deref().unwrap_or("string");
        let mut prop = serde_json::json!({"type": ptype});
        if let Some(desc) = &def.description {
            prop["description"] = serde_json::json!(desc);
        }
        properties.insert(name.clone(), prop);
        if def.required {
            required.push(name.clone());
        }
    }

    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required
    })
}

// ── Tool Execution ───────────────────────────────────────────────

/// Execute a tool call. Dispatches to builtin or http executor.
pub fn execute_tool(
    tool: &ToolDef,
    args: &serde_json::Value,
    config_defaults: &HashMap<String, toml::Value>,
) -> Result<String, String> {
    match tool.executor {
        ExecutorKind::Builtin => execute_builtin(&tool.name, args),
        ExecutorKind::Http => execute_http(tool, args, config_defaults),
    }
}

// ── Builtin Executor ─────────────────────────────────────────────

fn execute_builtin(name: &str, args: &serde_json::Value) -> Result<String, String> {
    match name {
        "list_files" => {
            let path = args["path"].as_str().ok_or("missing 'path' argument")?;
            let entries =
                std::fs::read_dir(path).map_err(|e| format!("Cannot read {}: {}", path, e))?;
            let mut lines = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|e| format!("Dir entry error: {}", e))?;
                let name = entry.file_name().to_string_lossy().to_string();
                let suffix = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    "/"
                } else {
                    ""
                };
                lines.push(format!("{}{}", name, suffix));
            }
            lines.sort();
            Ok(lines.join("\n"))
        }
        _ => Err(format!("Unknown builtin tool: {}", name)),
    }
}

// ── HTTP Executor ────────────────────────────────────────────────

fn execute_http(
    tool: &ToolDef,
    args: &serde_json::Value,
    config_defaults: &HashMap<String, toml::Value>,
) -> Result<String, String> {
    let exec = tool.exec.as_ref().ok_or("No exec config for http tool")?;

    // ── Resolve base URL from config defaults ───────────────
    let host =
        find_config_str(config_defaults, exec.host_key.as_deref(), "host").unwrap_or("127.0.0.1");
    let port = find_config_int(config_defaults, exec.port_key.as_deref(), "port").unwrap_or(7890);

    // ── Build URL ───────────────────────────────────────────
    let url_template = exec
        .url
        .as_deref()
        .or(exec.path.as_deref())
        .ok_or("No URL or path template")?;

    let mut url = url_template.to_string();

    // Replace {config.xxx} placeholders
    for (key, value) in config_defaults {
        let placeholder = format!("{{config.{}}}", key);
        let val_str = match value {
            toml::Value::String(s) => s.clone(),
            toml::Value::Integer(i) => i.to_string(),
            other => other.to_string(),
        };
        url = url.replace(&placeholder, &val_str);
    }

    // Replace {param} placeholders with argument values
    if let Some(obj) = args.as_object() {
        for (key, value) in obj {
            let placeholder = format!("{{{}}}", key);
            let val_str = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };

            // In execute_http, where you build the encoded URL:
            let mut encoded = String::new();
            for c in val_str.chars() {
                match c {
                    'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' => {
                        encoded.push(c)
                    }
                    _ => {
                        let mut buf = [0u8; 4];
                        let len = c.encode_utf8(&mut buf).len();
                        for &b in &buf[..len] {
                            encoded.push('%');
                            encoded.push(hex_digit((b >> 4) & 0xF));
                            encoded.push(hex_digit(b & 0xF));
                        }
                    }
                }
            }

            url = url.replace(&placeholder, &encoded);
        }
    }

    // Prepend base URL if not absolute
    if !url.starts_with("http") {
        url = format!("http://{}:{}{}", host, port, url);
    }

    let method = exec.method.as_deref().unwrap_or("GET");

    log::info!("[HTTP] {} {}", method, url);

    // ── Execute via curl ────────────────────────────────────
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-s", "-X", method, &url]);

    // Add body for POST
    if method == "POST" {
        if let Some(body_template) = &exec.body {
            let mut body = body_template.clone();
            if let Some(obj) = args.as_object() {
                for (key, value) in obj {
                    let placeholder = format!("{{{}}}", key);
                    let val_str = match value {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    body = body.replace(&placeholder, &val_str);
                }
            }
            cmd.args(["-H", "Content-Type: application/json", "-d", &body]);
            log::info!("[HTTP] POST body: {}", body);
        }
    }

    let output = cmd.output().map_err(|e| format!("curl failed: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    if !output.status.success() || stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !stderr.is_empty() {
            return Err(format!("HTTP request failed: {}", stderr.trim()));
        }
        if stdout.is_empty() {
            return Err("Empty response from HTTP endpoint".into());
        }
    }

    Ok(stdout)
}

fn hex_digit(n: u8) -> char {
    char::from_digit(n as u32, 16).unwrap().to_ascii_uppercase()
}

/// Find a config value by explicit key name, or by substring match
fn find_config_str<'a>(
    config: &'a HashMap<String, toml::Value>,
    explicit_key: Option<&str>,
    substring: &str,
) -> Option<&'a str> {
    if let Some(key) = explicit_key {
        return config.get(key).and_then(|v| v.as_str());
    }
    for (key, value) in config {
        if key.contains(substring) {
            if let Some(s) = value.as_str() {
                return Some(s);
            }
        }
    }
    None
}

fn find_config_int(
    config: &HashMap<String, toml::Value>,
    explicit_key: Option<&str>,
    substring: &str,
) -> Option<i64> {
    if let Some(key) = explicit_key {
        return config.get(key).and_then(|v| v.as_integer());
    }
    for (key, value) in config {
        if key.contains(substring) {
            if let Some(i) = value.as_integer() {
                return Some(i);
            }
        }
    }
    None
}

// ── Built-in Skills (always available) ───────────────────────────

pub fn file_skill() -> Skill {
    Skill {
        name: "file-ls".into(),
        version: "1.0.0".into(),
        description: "ls files".into(),
        triggers: vec!["list files".into(), "what files are in".into()],
        system_prompt: "You are a helpful assistant with access to the local filesystem. \
                        Use the provided tools to answer questions about files and directories. \
                        ALWAYS use tools rather than guessing file contents."
            .into(),
        context_docs: Vec::new(),
        tools: vec![ToolDef {
            name: "list_files".into(),
            description: "List files and directories at the given path.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute directory path"}
                },
                "required": ["path"]
            }),
            executor: ExecutorKind::Builtin,
            exec: None,
        }],
        config_defaults: HashMap::new(),
    }
}

/// Create a composite skill that merges ALL available skills
/// (including the built-in file-explorer) into a single skill.
///
/// Merge strategy:
/// - **Tools**: collected from every skill; duplicates by name are replaced
///   (last loaded skill wins).
/// - **System prompt**: each skill's prompt becomes a `## skill-name`
///   section in the combined prompt so the LLM can distinguish context.
/// - **Context docs**: concatenated from all skills.
/// - **Triggers**: collected from all skills.
/// - **Config defaults**: merged (later skills override earlier keys).
pub fn merge_all_skills() -> Skill {
    let all = load_all_skills();

    let mut combined_tools: Vec<ToolDef> = Vec::new();
    let mut seen_tool_names = std::collections::HashSet::new();
    let mut combined_prompt_parts: Vec<String> = Vec::new();
    let mut combined_context_docs: Vec<String> = Vec::new();
    let mut combined_triggers: Vec<String> = Vec::new();
    let mut combined_config: HashMap<String, toml::Value> = HashMap::new();
    let mut descriptions: Vec<String> = Vec::new();

    for skill in all {
        // ── Merge tools (deduplicate by name, last wins) ─────────
        for tool in skill.tools {
            if seen_tool_names.contains(&tool.name) {
                if let Some(pos) = combined_tools.iter().position(|t| t.name == tool.name) {
                    combined_tools[pos] = tool;
                }
            } else {
                seen_tool_names.insert(tool.name.clone());
                combined_tools.push(tool);
            }
        }

        // ── Combine system prompts with section headers ──────────
        let prompt = skill.system_prompt.trim();
        if !prompt.is_empty() {
            combined_prompt_parts.push(format!("## {}\n{}", skill.name, prompt));
        }

        // ── Collect context docs ─────────────────────────────────
        combined_context_docs.extend(skill.context_docs);

        // ── Collect triggers ─────────────────────────────────────
        combined_triggers.extend(skill.triggers);

        // ── Merge config defaults (later overrides earlier) ──────
        for (key, value) in skill.config_defaults {
            combined_config.insert(key, value);
        }

        descriptions.push(skill.name.clone());
    }

    let combined_system_prompt = if combined_prompt_parts.is_empty() {
        "You are a helpful assistant with access to multiple tools.".to_string()
    } else {
        combined_prompt_parts.join("\n\n")
    };

    Skill {
        name: "all".to_string(),
        version: "1.0.0".to_string(),
        description: format!("All skills ({})", descriptions.join(", ")),
        triggers: combined_triggers,
        system_prompt: combined_system_prompt,
        context_docs: combined_context_docs,
        tools: combined_tools,
        config_defaults: combined_config,
    }
}
