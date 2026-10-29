use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct UnifiedRequest {
    pub model: String,
    pub system: Vec<ContentPart>,
    pub messages: Vec<UnifiedMessage>,
    pub tools: Vec<ToolDef>,
    pub tool_choice: ToolChoice,
    pub parallel_tool_calls: Option<bool>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    pub stop: Vec<String>,
    pub stream: bool,
    pub reasoning: Option<ReasoningConfig>,
    pub response_format: Option<ResponseFormat>,
    pub metadata_user_id: Option<String>,
    pub previous_response_id: Option<String>,
    pub store: Option<bool>,
    pub n: Option<u32>,
    pub extras: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnifiedMessage {
    pub role: Role,
    pub content: Vec<ContentPart>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContentPart {
    Text {
        text: String,
    },
    Image {
        source: ImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<ContentPart>,
        is_error: bool,
    },
    Reasoning {
        text: String,
        signature: Option<String>,
    },
    RedactedReasoning {
        data: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImageSource {
    Url(String),
    Base64 { media_type: String, data: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Value,
    pub strict: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Tool {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReasoningConfig {
    pub effort: Option<ReasoningEffort>,
    pub budget_tokens: Option<u32>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema {
        name: Option<String>,
        schema: Value,
        strict: Option<bool>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnifiedResponse {
    pub id: String,
    pub model: String,
    pub created_at: i64,
    pub content: Vec<ContentPart>,
    pub stop_reason: Option<StopReason>,
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    ContentFilter,
    StopSequence,
    Other,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

impl Usage {
    pub fn recompute_total(&mut self) {
        if self.total_tokens.is_none() {
            if let (Some(input), Some(output)) = (self.input_tokens, self.output_tokens) {
                self.total_tokens = Some(input.saturating_add(output));
            }
        }
    }

    /// Fill counters the upstream omitted with local estimates. Reported values
    /// always win; this only prevents a missing usage block from being billed as
    /// zero tokens.
    pub fn fill_missing(&mut self, input_tokens: u64, output_tokens: u64) {
        if self.input_tokens.is_none() {
            self.input_tokens = Some(input_tokens);
        }
        if self.output_tokens.is_none() {
            self.output_tokens = Some(output_tokens);
        }
        self.recompute_total();
    }

    /// Like [`fill_missing`], but also replaces a zero output count with the
    /// estimate. Streaming usage often arrives before generation finishes, and a
    /// client that disconnects early must still be accounted for.
    pub fn fill_partial_estimate(&mut self, input_tokens: u64, output_tokens: u64) {
        if self.input_tokens.is_none() {
            self.input_tokens = Some(input_tokens);
        }
        match self.output_tokens {
            None => self.output_tokens = Some(output_tokens),
            Some(0) if output_tokens > 0 => self.output_tokens = Some(output_tokens),
            Some(_) => {}
        }
        if self.total_tokens == Some(0) {
            self.total_tokens = None;
        }
        self.recompute_total();
    }

    /// Later stream events carry more complete counters than earlier ones, so a
    /// field is only replaced when the newer event actually reports it.
    pub fn merge(&mut self, other: &Usage) {
        if other.input_tokens.is_some() {
            self.input_tokens = other.input_tokens;
        }
        if other.output_tokens.is_some() {
            self.output_tokens = other.output_tokens;
        }
        if other.total_tokens.is_some() {
            self.total_tokens = other.total_tokens;
        }
        if other.cached_tokens.is_some() {
            self.cached_tokens = other.cached_tokens;
        }
        if other.cache_creation_tokens.is_some() {
            self.cache_creation_tokens = other.cache_creation_tokens;
        }
        if other.reasoning_tokens.is_some() {
            self.reasoning_tokens = other.reasoning_tokens;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.input_tokens.is_none()
            && self.output_tokens.is_none()
            && self.total_tokens.is_none()
            && self.cached_tokens.is_none()
            && self.cache_creation_tokens.is_none()
            && self.reasoning_tokens.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Text,
    Reasoning,
    ToolUse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamBlock {
    pub index: usize,
    pub kind: BlockKind,
    pub id: Option<String>,
    pub name: Option<String>,
}

/// Protocol-neutral streaming events. Decoders are responsible for allocating a
/// stable `index` per content block; encoders map those indexes to their own
/// protocol-specific block numbering.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Start {
        id: String,
        model: String,
        created_at: i64,
    },
    BlockStart(StreamBlock),
    TextDelta {
        index: usize,
        text: String,
    },
    ReasoningDelta {
        index: usize,
        text: String,
    },
    /// Anthropic thinking signature, delivered as a separate `signature_delta`
    /// frame. Other protocols ignore it, but carrying it through the IR lets
    /// conversions back to Anthropic keep a valid signature when one exists.
    ReasoningSignature {
        index: usize,
        signature: String,
    },
    ToolInputDelta {
        index: usize,
        partial_json: String,
    },
    BlockStop {
        index: usize,
    },
    Usage(Usage),
    Stop {
        reason: Option<StopReason>,
        stop_sequence: Option<String>,
    },
    Done,
    Error {
        error_type: String,
        message: String,
    },
}

pub fn text_content(parts: &[ContentPart]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

pub fn image_url(source: &ImageSource) -> String {
    match source {
        ImageSource::Url(url) => url.clone(),
        ImageSource::Base64 { media_type, data } => {
            format!("data:{media_type};base64,{data}")
        }
    }
}

pub fn parse_image_string(value: &str) -> ImageSource {
    if let Some(rest) = value.strip_prefix("data:") {
        if let Some((metadata, data)) = rest.split_once(',') {
            if let Some(media_type) = metadata.strip_suffix(";base64") {
                return ImageSource::Base64 {
                    media_type: media_type.to_string(),
                    data: data.to_string(),
                };
            }
        }
    }

    ImageSource::Url(value.to_string())
}

pub fn reasoning_text(parts: &[ContentPart]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Reasoning { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

pub fn has_image(parts: &[ContentPart]) -> bool {
    parts
        .iter()
        .any(|part| matches!(part, ContentPart::Image { .. }))
}
