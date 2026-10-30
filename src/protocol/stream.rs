use std::collections::{BTreeSet, HashMap};

use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::sse::SseFrame;
use super::{
    anthropic, chat, responses, BlockKind, Protocol, ProtocolError, StopReason, StreamBlock,
    StreamEvent, Usage,
};

fn parse_frame(frame: &SseFrame, protocol: &'static str) -> Result<Value, ProtocolError> {
    serde_json::from_str(&frame.data)
        .map_err(|_| ProtocolError::invalid_response(protocol, "stream event is not valid JSON"))
}

fn event_name<'a>(frame: &'a SseFrame, value: &'a Value) -> &'a str {
    frame
        .event
        .as_deref()
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or("")
}

fn index_of(value: &Value, key: &str) -> usize {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0)
}

fn string_of(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn error_event(value: &Value) -> StreamEvent {
    let (error_type, message) = super::error_fields(value, false);
    StreamEvent::Error {
        error_type: error_type.unwrap_or_else(|| "upstream_error".to_string()),
        message: message.unwrap_or_else(|| "upstream stream error".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Decoders: upstream SSE -> IR StreamEvent
// ---------------------------------------------------------------------------

pub enum StreamDecoder {
    Chat(ChatDecoder),
    Responses(ResponsesDecoder),
    Anthropic(AnthropicDecoder),
}

impl StreamDecoder {
    pub fn new(protocol: Protocol) -> Self {
        match protocol {
            Protocol::Chat => Self::Chat(ChatDecoder::new()),
            Protocol::Responses => Self::Responses(ResponsesDecoder::new()),
            Protocol::Anthropic => Self::Anthropic(AnthropicDecoder::new()),
        }
    }

    pub fn decode(&mut self, frame: &SseFrame) -> Result<Vec<StreamEvent>, ProtocolError> {
        match self {
            Self::Chat(decoder) => decoder.decode(frame),
            Self::Responses(decoder) => decoder.decode(frame),
            Self::Anthropic(decoder) => decoder.decode(frame),
        }
    }

    pub fn finish(&mut self) -> Vec<StreamEvent> {
        match self {
            Self::Chat(decoder) => decoder.finish(),
            Self::Responses(decoder) => decoder.finish(),
            Self::Anthropic(decoder) => decoder.finish(),
        }
    }

    pub fn final_response(&self) -> Option<Value> {
        match self {
            Self::Responses(decoder) => decoder.final_response.clone(),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct ChatDecoder {
    started: bool,
    next_index: usize,
    text: Option<usize>,
    reasoning: Option<usize>,
    tools: HashMap<i64, usize>,
    open: BTreeSet<usize>,
    finish_reason: Option<StopReason>,
    usage: Usage,
    stopped: bool,
    done: bool,
}

impl ChatDecoder {
    fn new() -> Self {
        Self::default()
    }

    fn decode(&mut self, frame: &SseFrame) -> Result<Vec<StreamEvent>, ProtocolError> {
        if self.done {
            return Ok(Vec::new());
        }
        if frame.data.trim() == "[DONE]" {
            return Ok(self.close());
        }

        let value = parse_frame(frame, "chat")?;
        let mut events = Vec::new();

        if value.get("error").is_some() {
            events.push(error_event(&value));
            return Ok(events);
        }

        if !self.started {
            self.started = true;
            events.push(StreamEvent::Start {
                id: string_of(&value, "id").unwrap_or_default(),
                model: string_of(&value, "model").unwrap_or_default(),
                created_at: value.get("created").and_then(Value::as_i64).unwrap_or(0),
            });
        }

        if let Some(usage) = value.get("usage").and_then(Value::as_object) {
            let parsed = chat::parse_usage(usage);
            self.usage.merge(&parsed);
            events.push(StreamEvent::Usage(parsed));
        }

        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(Value::as_object)
        else {
            return Ok(events);
        };

        if let Some(delta) = choice.get("delta").and_then(Value::as_object) {
            if let Some(text) = delta
                .get("content")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                let index = self.ensure_text(&mut events);
                events.push(StreamEvent::TextDelta {
                    index,
                    text: text.to_string(),
                });
            }

            if let Some(text) = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                let index = self.ensure_reasoning(&mut events);
                events.push(StreamEvent::ReasoningDelta {
                    index,
                    text: text.to_string(),
                });
            }

            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    self.tool_delta(call, &mut events);
                }
            }
        }

        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(chat::parse_finish_reason(reason));
        }

        Ok(events)
    }

    fn ensure_text(&mut self, events: &mut Vec<StreamEvent>) -> usize {
        if let Some(index) = self.text {
            return index;
        }
        let index = self.allocate(BlockKind::Text, None, None, events);
        self.text = Some(index);
        index
    }

    fn ensure_reasoning(&mut self, events: &mut Vec<StreamEvent>) -> usize {
        if let Some(index) = self.reasoning {
            return index;
        }
        let index = self.allocate(BlockKind::Reasoning, None, None, events);
        self.reasoning = Some(index);
        index
    }

    fn allocate(
        &mut self,
        kind: BlockKind,
        id: Option<String>,
        name: Option<String>,
        events: &mut Vec<StreamEvent>,
    ) -> usize {
        let index = self.next_index;
        self.next_index += 1;
        self.open.insert(index);
        events.push(StreamEvent::BlockStart(StreamBlock {
            index,
            kind,
            id,
            name,
        }));
        index
    }

    fn tool_delta(&mut self, call: &Value, events: &mut Vec<StreamEvent>) {
        let Some(call) = call.as_object() else {
            return;
        };
        let upstream_index = call.get("index").and_then(Value::as_i64).unwrap_or(0);
        let function = call.get("function").and_then(Value::as_object);
        let id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let name = function
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let arguments = function
            .and_then(|function| function.get("arguments"))
            .and_then(Value::as_str)
            .unwrap_or("");

        let index = match self.tools.get(&upstream_index) {
            Some(index) => *index,
            None => {
                let index = self.allocate(BlockKind::ToolUse, id, name, events);
                self.tools.insert(upstream_index, index);
                index
            }
        };

        if !arguments.is_empty() {
            events.push(StreamEvent::ToolInputDelta {
                index,
                partial_json: arguments.to_string(),
            });
        }
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        if self.done {
            return Vec::new();
        }
        self.close()
    }

    fn close(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        for index in std::mem::take(&mut self.open) {
            events.push(StreamEvent::BlockStop { index });
        }
        if !self.stopped {
            self.stopped = true;
            events.push(StreamEvent::Stop {
                reason: self.finish_reason,
                stop_sequence: None,
            });
        }
        self.done = true;
        events.push(StreamEvent::Done);
        events
    }
}

pub struct AnthropicDecoder {
    next_index: usize,
    blocks: HashMap<usize, (usize, BlockKind)>,
    open: BTreeSet<usize>,
    usage: Usage,
    stopped: bool,
    done: bool,
}

impl AnthropicDecoder {
    fn new() -> Self {
        Self {
            next_index: 0,
            blocks: HashMap::new(),
            open: BTreeSet::new(),
            usage: Usage::default(),
            stopped: false,
            done: false,
        }
    }

    fn decode(&mut self, frame: &SseFrame) -> Result<Vec<StreamEvent>, ProtocolError> {
        if self.done {
            return Ok(Vec::new());
        }

        let value = parse_frame(frame, "anthropic")?;
        let kind = event_name(frame, &value);
        let mut events = Vec::new();

        match kind {
            "message_start" => {
                if let Some(message) = value.get("message") {
                    events.push(StreamEvent::Start {
                        id: string_of(message, "id").unwrap_or_default(),
                        model: string_of(message, "model").unwrap_or_default(),
                        created_at: 0,
                    });
                    if let Some(usage) = message.get("usage").and_then(Value::as_object) {
                        let parsed = anthropic::parse_usage(usage);
                        self.usage.merge(&parsed);
                        events.push(StreamEvent::Usage(parsed));
                    }
                }
            }
            "content_block_start" => {
                let upstream_index = index_of(&value, "index");
                let Some(block) = value.get("content_block") else {
                    return Ok(events);
                };
                let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
                let (kind, id, name) = match block_type {
                    "text" => (BlockKind::Text, None, None),
                    "thinking" => (BlockKind::Reasoning, None, None),
                    "tool_use" => (
                        BlockKind::ToolUse,
                        string_of(block, "id"),
                        string_of(block, "name"),
                    ),
                    _ => return Ok(events),
                };
                let index = self.next_index;
                self.next_index += 1;
                self.blocks.insert(upstream_index, (index, kind));
                self.open.insert(index);
                events.push(StreamEvent::BlockStart(StreamBlock {
                    index,
                    kind,
                    id,
                    name,
                }));
            }
            "content_block_delta" => {
                let upstream_index = index_of(&value, "index");
                let Some((index, _)) = self.blocks.get(&upstream_index).copied() else {
                    return Ok(events);
                };
                let Some(delta) = value.get("delta") else {
                    return Ok(events);
                };
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            events.push(StreamEvent::TextDelta {
                                index,
                                text: text.to_string(),
                            });
                        }
                    }
                    "thinking_delta" => {
                        if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
                            events.push(StreamEvent::ReasoningDelta {
                                index,
                                text: text.to_string(),
                            });
                        }
                    }
                    "signature_delta" => {
                        if let Some(signature) = delta.get("signature").and_then(Value::as_str) {
                            events.push(StreamEvent::ReasoningSignature {
                                index,
                                signature: signature.to_string(),
                            });
                        }
                    }
                    "input_json_delta" => {
                        if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                            events.push(StreamEvent::ToolInputDelta {
                                index,
                                partial_json: partial.to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let upstream_index = index_of(&value, "index");
                if let Some((index, _)) = self.blocks.remove(&upstream_index) {
                    if self.open.remove(&index) {
                        events.push(StreamEvent::BlockStop { index });
                    }
                }
            }
            "message_delta" => {
                if let Some(delta) = value.get("delta") {
                    if let Some(reason) = delta.get("stop_reason").and_then(Value::as_str) {
                        self.stopped = true;
                        events.push(StreamEvent::Stop {
                            reason: Some(parse_anthropic_stop(reason)),
                            stop_sequence: string_of(delta, "stop_sequence"),
                        });
                    }
                }
                if let Some(usage) = value.get("usage").and_then(Value::as_object) {
                    let parsed = anthropic::parse_usage(usage);
                    self.usage.merge(&parsed);
                    events.push(StreamEvent::Usage(parsed));
                }
            }
            "message_stop" => {
                events.extend(self.close());
            }
            "error" => events.push(error_event(&value)),
            _ => {}
        }

        Ok(events)
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        if self.done {
            return Vec::new();
        }
        self.close()
    }

    fn close(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        for index in std::mem::take(&mut self.open) {
            events.push(StreamEvent::BlockStop { index });
        }
        if !self.stopped {
            self.stopped = true;
            events.push(StreamEvent::Stop {
                reason: None,
                stop_sequence: None,
            });
        }
        self.done = true;
        events.push(StreamEvent::Done);
        events
    }
}

#[derive(Default)]
pub struct ResponsesDecoder {
    started: bool,
    next_index: usize,
    blocks: HashMap<(usize, usize), (usize, BlockKind)>,
    output_keys: HashMap<usize, Vec<(usize, usize)>>,
    open: BTreeSet<usize>,
    usage: Usage,
    stop: Option<StopReason>,
    stopped: bool,
    done: bool,
    final_response: Option<Value>,
}

impl ResponsesDecoder {
    fn new() -> Self {
        Self::default()
    }

    fn decode(&mut self, frame: &SseFrame) -> Result<Vec<StreamEvent>, ProtocolError> {
        if self.done {
            return Ok(Vec::new());
        }

        let value = parse_frame(frame, "responses")?;
        let kind = event_name(frame, &value);
        let mut events = Vec::new();

        match kind {
            "response.created" => {
                if !self.started {
                    if let Some(response) = value.get("response") {
                        self.started = true;
                        events.push(StreamEvent::Start {
                            id: string_of(response, "id").unwrap_or_default(),
                            model: string_of(response, "model").unwrap_or_default(),
                            created_at: response
                                .get("created_at")
                                .and_then(Value::as_i64)
                                .unwrap_or(0),
                        });
                    }
                }
            }
            "response.output_item.added" => {
                let output_index = index_of(&value, "output_index");
                let Some(item) = value.get("item") else {
                    return Ok(events);
                };
                match item.get("type").and_then(Value::as_str).unwrap_or("") {
                    "reasoning" => {
                        self.ensure_block(
                            (output_index, 0),
                            BlockKind::Reasoning,
                            None,
                            None,
                            &mut events,
                        );
                    }
                    "function_call" => {
                        self.ensure_block(
                            (output_index, 0),
                            BlockKind::ToolUse,
                            string_of(item, "call_id").or_else(|| string_of(item, "id")),
                            string_of(item, "name"),
                            &mut events,
                        );
                    }
                    _ => {}
                }
            }
            "response.content_part.added" => {
                let output_index = index_of(&value, "output_index");
                let content_index = index_of(&value, "content_index");
                let part_type = value
                    .get("part")
                    .and_then(|part| part.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if part_type == "output_text" {
                    self.ensure_block(
                        (output_index, content_index),
                        BlockKind::Text,
                        None,
                        None,
                        &mut events,
                    );
                }
            }
            "response.output_text.delta" => {
                let output_index = index_of(&value, "output_index");
                let content_index = index_of(&value, "content_index");
                let index = self.ensure_block(
                    (output_index, content_index),
                    BlockKind::Text,
                    None,
                    None,
                    &mut events,
                );
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    events.push(StreamEvent::TextDelta {
                        index,
                        text: delta.to_string(),
                    });
                }
            }
            "response.function_call_arguments.delta" => {
                let output_index = index_of(&value, "output_index");
                let index = self.ensure_block(
                    (output_index, 0),
                    BlockKind::ToolUse,
                    string_of(&value, "item_id"),
                    None,
                    &mut events,
                );
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    events.push(StreamEvent::ToolInputDelta {
                        index,
                        partial_json: delta.to_string(),
                    });
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let output_index = index_of(&value, "output_index");
                let index = self.ensure_block(
                    (output_index, 0),
                    BlockKind::Reasoning,
                    None,
                    None,
                    &mut events,
                );
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    events.push(StreamEvent::ReasoningDelta {
                        index,
                        text: delta.to_string(),
                    });
                }
            }
            "response.content_part.done" => {
                let output_index = index_of(&value, "output_index");
                let content_index = index_of(&value, "content_index");
                self.stop_key((output_index, content_index), &mut events);
            }
            "response.output_item.done" => {
                let output_index = index_of(&value, "output_index");
                self.stop_output_item(output_index, &mut events);
            }
            "response.completed" | "response.incomplete" => {
                if let Some(response) = value.get("response") {
                    self.final_response = Some(response.clone());
                    if let Ok(parsed) = responses::parse_response(response) {
                        self.usage.merge(&parsed.usage);
                        if !parsed.usage.is_empty() {
                            events.push(StreamEvent::Usage(parsed.usage));
                        }
                        self.stop = parsed.stop_reason;
                    }
                }
                events.extend(self.close());
            }
            "response.failed" => {
                if let Some(response) = value.get("response") {
                    self.final_response = Some(response.clone());
                    if let Some(error) = response.get("error") {
                        events.push(error_event(error));
                    }
                }
            }
            "error" => events.push(error_event(&value)),
            _ => {}
        }

        Ok(events)
    }

    fn ensure_block(
        &mut self,
        key: (usize, usize),
        kind: BlockKind,
        id: Option<String>,
        name: Option<String>,
        events: &mut Vec<StreamEvent>,
    ) -> usize {
        if let Some((index, _)) = self.blocks.get(&key) {
            return *index;
        }

        let index = self.next_index;
        self.next_index += 1;
        self.blocks.insert(key, (index, kind));
        self.output_keys.entry(key.0).or_default().push(key);
        self.open.insert(index);
        events.push(StreamEvent::BlockStart(StreamBlock {
            index,
            kind,
            id,
            name,
        }));
        index
    }

    fn stop_key(&mut self, key: (usize, usize), events: &mut Vec<StreamEvent>) {
        if let Some((index, _)) = self.blocks.get(&key) {
            if self.open.remove(index) {
                events.push(StreamEvent::BlockStop { index: *index });
            }
        }
    }

    fn stop_output_item(&mut self, output_index: usize, events: &mut Vec<StreamEvent>) {
        let keys = self.output_keys.remove(&output_index).unwrap_or_default();
        for key in keys {
            self.stop_key(key, events);
        }
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        if self.done {
            return Vec::new();
        }
        self.close()
    }

    fn close(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        for index in std::mem::take(&mut self.open) {
            events.push(StreamEvent::BlockStop { index });
        }
        if !self.stopped {
            self.stopped = true;
            events.push(StreamEvent::Stop {
                reason: self.stop,
                stop_sequence: None,
            });
        }
        self.done = true;
        events.push(StreamEvent::Done);
        events
    }
}

pub(crate) fn parse_anthropic_stop(value: &str) -> StopReason {
    match value {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "tool_use" => StopReason::ToolUse,
        "stop_sequence" => StopReason::StopSequence,
        "refusal" => StopReason::ContentFilter,
        _ => StopReason::Other,
    }
}

// ---------------------------------------------------------------------------
// Encoders: IR StreamEvent -> downstream SSE
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct StreamEncodeOptions {
    pub response_id: Option<String>,
    pub include_usage: bool,
    pub store: bool,
}

pub enum StreamEncoder {
    Chat(ChatEncoder),
    Responses(ResponsesEncoder),
    Anthropic(AnthropicEncoder),
}

impl StreamEncoder {
    pub fn new(protocol: Protocol, options: StreamEncodeOptions) -> Self {
        match protocol {
            Protocol::Chat => Self::Chat(ChatEncoder::new(options)),
            Protocol::Responses => Self::Responses(ResponsesEncoder::new(options)),
            Protocol::Anthropic => Self::Anthropic(AnthropicEncoder::new(options)),
        }
    }

    pub fn encode(&mut self, event: &StreamEvent) -> Vec<String> {
        match self {
            Self::Chat(encoder) => encoder.encode(event),
            Self::Responses(encoder) => encoder.encode(event),
            Self::Anthropic(encoder) => encoder.encode(event),
        }
    }

    pub fn final_response(&self) -> Option<Value> {
        match self {
            Self::Responses(encoder) => encoder.final_response.clone(),
            _ => None,
        }
    }
}

fn sse_data(value: &Value) -> String {
    format!("data: {value}\n\n")
}

fn sse_named(name: &str, value: &Value) -> String {
    format!("event: {name}\ndata: {value}\n\n")
}

fn anthropic_usage_start(usage: &Usage) -> Value {
    let mut map = Map::new();
    map.insert(
        "input_tokens".into(),
        json!(usage.input_tokens.unwrap_or(0)),
    );
    map.insert("output_tokens".into(), json!(0));
    if let Some(cached) = usage.cached_tokens {
        map.insert("cache_read_input_tokens".into(), json!(cached));
    }
    if let Some(creation) = usage.cache_creation_tokens {
        map.insert("cache_creation_input_tokens".into(), json!(creation));
    }
    Value::Object(map)
}

fn anthropic_usage_delta(usage: &Usage) -> Value {
    let mut map = Map::new();
    map.insert(
        "output_tokens".into(),
        json!(usage.output_tokens.unwrap_or(0)),
    );
    if let Some(input) = usage.input_tokens {
        map.insert("input_tokens".into(), json!(input));
    }
    if let Some(cached) = usage.cached_tokens {
        map.insert("cache_read_input_tokens".into(), json!(cached));
    }
    if let Some(creation) = usage.cache_creation_tokens {
        map.insert("cache_creation_input_tokens".into(), json!(creation));
    }
    Value::Object(map)
}

pub struct ChatEncoder {
    response_id: Option<String>,
    id: String,
    model: String,
    created_at: i64,
    role_sent: bool,
    usage: Usage,
    stop: Option<StopReason>,
    done: bool,
    include_usage: bool,
    next_tool: usize,
    tools: HashMap<usize, ChatTool>,
}

struct ChatTool {
    index: usize,
    id: String,
    name: String,
    announced: bool,
}

impl ChatEncoder {
    fn new(options: StreamEncodeOptions) -> Self {
        Self {
            response_id: options.response_id,
            id: String::new(),
            model: String::new(),
            created_at: 0,
            role_sent: false,
            usage: Usage::default(),
            stop: None,
            done: false,
            include_usage: options.include_usage,
            next_tool: 0,
            tools: HashMap::new(),
        }
    }

    fn encode(&mut self, event: &StreamEvent) -> Vec<String> {
        if self.done {
            return Vec::new();
        }

        let mut frames = Vec::new();
        match event {
            StreamEvent::Start {
                id,
                model,
                created_at,
            } => {
                self.id = self.response_id.clone().unwrap_or_else(|| id.clone());
                self.model = model.clone();
                self.created_at = if *created_at == 0 {
                    chrono::Utc::now().timestamp()
                } else {
                    *created_at
                };
                self.ensure_role_chunk(&mut frames);
            }
            StreamEvent::BlockStart(block) => {
                self.ensure_role_chunk(&mut frames);
                if block.kind == BlockKind::ToolUse {
                    let index = self.next_tool;
                    self.next_tool += 1;
                    let id = block
                        .id
                        .clone()
                        .unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple()));
                    let name = block.name.clone().unwrap_or_default();
                    frames.push(sse_data(&self.chunk(json!({
                        "index": 0,
                        "delta": {"tool_calls": [{
                            "index": index,
                            "id": id.clone(),
                            "type": "function",
                            "function": {"name": name.clone(), "arguments": ""},
                        }]},
                        "finish_reason": null,
                    }))));
                    self.tools.insert(
                        block.index,
                        ChatTool {
                            index,
                            id,
                            name,
                            announced: true,
                        },
                    );
                }
            }
            StreamEvent::TextDelta { text, .. } => {
                self.ensure_role_chunk(&mut frames);
                frames.push(sse_data(&self.chunk(json!({
                    "index": 0,
                    "delta": {"content": text},
                    "finish_reason": null,
                }))));
            }
            StreamEvent::ReasoningDelta { text, .. } => {
                self.ensure_role_chunk(&mut frames);
                frames.push(sse_data(&self.chunk(json!({
                    "index": 0,
                    "delta": {"reasoning_content": text},
                    "finish_reason": null,
                }))));
            }
            StreamEvent::ToolInputDelta {
                index,
                partial_json,
            } => {
                self.ensure_role_chunk(&mut frames);
                let Some(tool) = self.tools.get_mut(index) else {
                    return frames;
                };
                let mut function = Map::new();
                if !tool.announced {
                    function.insert("name".into(), json!(tool.name));
                    tool.announced = true;
                }
                function.insert("arguments".into(), json!(partial_json));
                let call = json!({
                    "index": tool.index,
                    "id": tool.id,
                    "type": "function",
                    "function": Value::Object(function),
                });
                frames.push(sse_data(&self.chunk(json!({
                    "index": 0,
                    "delta": {"tool_calls": [call]},
                    "finish_reason": null,
                }))));
            }
            StreamEvent::Usage(usage) => self.usage.merge(usage),
            StreamEvent::Stop {
                reason,
                stop_sequence: _,
            } => self.stop = *reason,
            StreamEvent::Done => {
                self.ensure_role_chunk(&mut frames);
                frames.push(sse_data(&self.chunk(json!({
                    "index": 0,
                    "delta": {},
                    "finish_reason": chat_finish_reason(self.stop),
                }))));
                if self.include_usage && !self.usage.is_empty() {
                    frames.push(sse_data(&json!({
                        "id": self.id,
                        "object": "chat.completion.chunk",
                        "created": self.created_at,
                        "model": self.model,
                        "choices": [],
                        "usage": self.usage.chat_json(),
                    })));
                }
                frames.push("data: [DONE]\n\n".to_string());
                self.done = true;
            }
            StreamEvent::Error {
                error_type,
                message,
            } => {
                frames.push(sse_data(&json!({
                    "error": {"message": message, "type": error_type},
                })));
                frames.push("data: [DONE]\n\n".to_string());
                self.done = true;
            }
            _ => {}
        }

        frames
    }

    fn ensure_role_chunk(&mut self, frames: &mut Vec<String>) {
        if self.role_sent {
            return;
        }
        self.role_sent = true;
        frames.push(sse_data(&self.chunk(json!({
            "index": 0,
            "delta": {"role": "assistant", "content": ""},
            "finish_reason": null,
        }))));
    }

    fn chunk(&self, choice: Value) -> Value {
        json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created_at,
            "model": self.model,
            "choices": [choice],
        })
    }
}

fn chat_finish_reason(reason: Option<StopReason>) -> &'static str {
    match reason {
        Some(StopReason::MaxTokens) => "length",
        Some(StopReason::ToolUse) => "tool_calls",
        Some(StopReason::ContentFilter) => "content_filter",
        _ => "stop",
    }
}

pub struct AnthropicEncoder {
    response_id: Option<String>,
    id: String,
    model: String,
    started: bool,
    next_index: usize,
    blocks: HashMap<usize, (usize, BlockKind)>,
    open: BTreeSet<usize>,
    usage: Usage,
    stop: Option<StopReason>,
    stop_sequence: Option<String>,
    done: bool,
}

impl AnthropicEncoder {
    fn new(options: StreamEncodeOptions) -> Self {
        Self {
            response_id: options.response_id,
            id: String::new(),
            model: String::new(),
            started: false,
            next_index: 0,
            blocks: HashMap::new(),
            open: BTreeSet::new(),
            usage: Usage::default(),
            stop: None,
            stop_sequence: None,
            done: false,
        }
    }

    fn encode(&mut self, event: &StreamEvent) -> Vec<String> {
        if self.done {
            return Vec::new();
        }

        let mut frames = Vec::new();
        match event {
            StreamEvent::Start { id, model, .. } => {
                self.id = self.response_id.clone().unwrap_or_else(|| id.clone());
                if self.id.is_empty() {
                    self.id = format!("msg_{}", Uuid::new_v4().simple());
                }
                self.model = model.clone();
                self.started = true;
                frames.push(sse_named(
                    "message_start",
                    &json!({
                        "type": "message_start",
                        "message": {
                            "id": self.id,
                            "type": "message",
                            "role": "assistant",
                            "model": self.model,
                            "content": [],
                            "stop_reason": null,
                            "stop_sequence": null,
                            "usage": anthropic_usage_start(&self.usage),
                        }
                    }),
                ));
            }
            StreamEvent::BlockStart(block) => {
                self.ensure_started(&mut frames);
                match block.kind {
                    BlockKind::Text => {
                        let index = self.allocate(block.index, BlockKind::Text, &mut frames);
                        frames.push(sse_named(
                            "content_block_start",
                            &json!({
                                "type": "content_block_start",
                                "index": index,
                                "content_block": {"type": "text", "text": ""},
                            }),
                        ));
                    }
                    BlockKind::ToolUse => {
                        let index = self.allocate(block.index, BlockKind::ToolUse, &mut frames);
                        let id = block
                            .id
                            .clone()
                            .unwrap_or_else(|| format!("toolu_{}", Uuid::new_v4().simple()));
                        frames.push(sse_named(
                            "content_block_start",
                            &json!({
                                "type": "content_block_start",
                                "index": index,
                                "content_block": {
                                    "type": "tool_use",
                                    "id": id,
                                    "name": block.name.clone().unwrap_or_default(),
                                    "input": {},
                                },
                            }),
                        ));
                    }
                    // The upstream signature is not required to display the
                    // reasoning, and non-Anthropic upstreams cannot provide
                    // one. Emit the thinking block anyway; a signature_delta
                    // is forwarded when the upstream did send one.
                    BlockKind::Reasoning => {
                        let index = self.allocate(block.index, BlockKind::Reasoning, &mut frames);
                        frames.push(sse_named(
                            "content_block_start",
                            &json!({
                                "type": "content_block_start",
                                "index": index,
                                "content_block": {"type": "thinking", "thinking": ""},
                            }),
                        ));
                    }
                }
            }
            StreamEvent::TextDelta { index, text } => {
                self.ensure_started(&mut frames);
                let target = match self.blocks.get(index) {
                    Some((target, BlockKind::Text)) => *target,
                    _ => {
                        let target = self.allocate(*index, BlockKind::Text, &mut frames);
                        frames.push(sse_named(
                            "content_block_start",
                            &json!({
                                "type": "content_block_start",
                                "index": target,
                                "content_block": {"type": "text", "text": ""},
                            }),
                        ));
                        target
                    }
                };
                frames.push(sse_named(
                    "content_block_delta",
                    &json!({
                        "type": "content_block_delta",
                        "index": target,
                        "delta": {"type": "text_delta", "text": text},
                    }),
                ));
            }
            StreamEvent::ReasoningDelta { index, text } => {
                self.ensure_started(&mut frames);
                let target = match self.blocks.get(index) {
                    Some((target, BlockKind::Reasoning)) => *target,
                    _ => {
                        let target = self.allocate(*index, BlockKind::Reasoning, &mut frames);
                        frames.push(sse_named(
                            "content_block_start",
                            &json!({
                                "type": "content_block_start",
                                "index": target,
                                "content_block": {"type": "thinking", "thinking": ""},
                            }),
                        ));
                        target
                    }
                };
                frames.push(sse_named(
                    "content_block_delta",
                    &json!({
                        "type": "content_block_delta",
                        "index": target,
                        "delta": {"type": "thinking_delta", "thinking": text},
                    }),
                ));
            }
            StreamEvent::ReasoningSignature { index, signature } => {
                if let Some((target, BlockKind::Reasoning)) = self.blocks.get(index) {
                    frames.push(sse_named(
                        "content_block_delta",
                        &json!({
                            "type": "content_block_delta",
                            "index": target,
                            "delta": {"type": "signature_delta", "signature": signature},
                        }),
                    ));
                }
            }
            StreamEvent::ToolInputDelta {
                index,
                partial_json,
            } => {
                if let Some((target, BlockKind::ToolUse)) = self.blocks.get(index) {
                    frames.push(sse_named(
                        "content_block_delta",
                        &json!({
                            "type": "content_block_delta",
                            "index": target,
                            "delta": {"type": "input_json_delta", "partial_json": partial_json},
                        }),
                    ));
                }
            }
            StreamEvent::BlockStop { index } => {
                if let Some((target, _)) = self.blocks.remove(index) {
                    if self.open.remove(&target) {
                        frames.push(sse_named(
                            "content_block_stop",
                            &json!({"type": "content_block_stop", "index": target}),
                        ));
                    }
                }
            }
            StreamEvent::Usage(usage) => self.usage.merge(usage),
            StreamEvent::Stop {
                reason,
                stop_sequence,
            } => {
                self.stop = *reason;
                self.stop_sequence = stop_sequence.clone();
            }
            StreamEvent::Done => {
                self.ensure_started(&mut frames);
                for target in std::mem::take(&mut self.open) {
                    frames.push(sse_named(
                        "content_block_stop",
                        &json!({"type": "content_block_stop", "index": target}),
                    ));
                }
                frames.push(sse_named(
                    "message_delta",
                    &json!({
                        "type": "message_delta",
                        "delta": {
                            "stop_reason": anthropic_stop_reason(self.stop),
                            "stop_sequence": self.stop_sequence,
                        },
                        "usage": anthropic_usage_delta(&self.usage),
                    }),
                ));
                frames.push(sse_named("message_stop", &json!({"type": "message_stop"})));
                self.done = true;
            }
            StreamEvent::Error {
                error_type,
                message,
            } => {
                frames.push(sse_named(
                    "error",
                    &json!({
                        "type": "error",
                        "error": {"type": error_type, "message": message},
                    }),
                ));
                self.done = true;
            }
        }

        frames
    }

    fn ensure_started(&mut self, frames: &mut Vec<String>) {
        if self.started {
            return;
        }
        self.started = true;
        if self.id.is_empty() {
            self.id = self
                .response_id
                .clone()
                .unwrap_or_else(|| format!("msg_{}", Uuid::new_v4().simple()));
        }
        frames.push(sse_named(
            "message_start",
            &json!({
                "type": "message_start",
                "message": {
                    "id": self.id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": anthropic_usage_start(&self.usage),
                }
            }),
        ));
    }

    /// Start a new content block. Anthropic expects blocks to be sequential,
    /// so any block still open (for example reasoning followed by text, which
    /// Chat emits without block boundaries) is closed first.
    fn allocate(&mut self, ir_index: usize, kind: BlockKind, frames: &mut Vec<String>) -> usize {
        if let Some((index, _)) = self.blocks.get(&ir_index) {
            return *index;
        }
        for target in std::mem::take(&mut self.open) {
            frames.push(sse_named(
                "content_block_stop",
                &json!({"type": "content_block_stop", "index": target}),
            ));
        }
        let index = self.next_index;
        self.next_index += 1;
        self.blocks.insert(ir_index, (index, kind));
        self.open.insert(index);
        index
    }
}

pub(crate) fn anthropic_stop_reason(reason: Option<StopReason>) -> &'static str {
    match reason {
        Some(StopReason::MaxTokens) => "max_tokens",
        Some(StopReason::ToolUse) => "tool_use",
        Some(StopReason::ContentFilter) => "refusal",
        Some(StopReason::StopSequence) => "stop_sequence",
        _ => "end_turn",
    }
}

/// Responses output item for a completed block. Used both for the streaming
/// `response.output_item.done` event and for the final `output` array so the
/// two can never disagree.
fn output_item(block: &EncodedBlock) -> Value {
    match block.kind {
        BlockKind::Text => json!({
            "type": "message",
            "id": block.item_id,
            "role": "assistant",
            "status": "completed",
            "content": [{
                "type": "output_text",
                "text": block.text,
                "annotations": [],
            }],
        }),
        BlockKind::Reasoning => {
            let mut item = json!({
                "type": "reasoning",
                "id": block.item_id,
                "summary": [{"type": "summary_text", "text": block.text}],
                "status": "completed",
            });
            if let Some(signature) = &block.signature {
                item["encrypted_content"] = json!(signature);
            }
            item
        }
        BlockKind::ToolUse => json!({
            "type": "function_call",
            "id": block.item_id,
            "call_id": block.call_id,
            "name": block.name,
            "arguments": block.arguments,
            "status": "completed",
        }),
    }
}

struct EncodedBlock {
    output_index: usize,
    kind: BlockKind,
    item_id: String,
    call_id: String,
    name: String,
    text: String,
    arguments: String,
    signature: Option<String>,
}

pub struct ResponsesEncoder {
    response_id: Option<String>,
    id: String,
    model: String,
    created_at: i64,
    store: bool,
    started: bool,
    next_output: usize,
    blocks: HashMap<usize, EncodedBlock>,
    open: BTreeSet<usize>,
    usage: Usage,
    stop: Option<StopReason>,
    stop_sequence: Option<String>,
    done: bool,
    final_response: Option<Value>,
}

impl ResponsesEncoder {
    fn new(options: StreamEncodeOptions) -> Self {
        Self {
            id: options.response_id.clone().unwrap_or_default(),
            response_id: options.response_id,
            model: String::new(),
            created_at: 0,
            store: options.store,
            started: false,
            next_output: 0,
            blocks: HashMap::new(),
            open: BTreeSet::new(),
            usage: Usage::default(),
            stop: None,
            stop_sequence: None,
            done: false,
            final_response: None,
        }
    }

    fn encode(&mut self, event: &StreamEvent) -> Vec<String> {
        if self.done {
            return Vec::new();
        }

        let mut frames = Vec::new();
        match event {
            StreamEvent::Start {
                id,
                model,
                created_at,
            } => {
                self.id = self.response_id.clone().unwrap_or_else(|| id.clone());
                if self.id.is_empty() {
                    self.id = format!("resp_{}", Uuid::new_v4().simple());
                }
                self.model = model.clone();
                self.created_at = if *created_at == 0 {
                    chrono::Utc::now().timestamp()
                } else {
                    *created_at
                };
                self.started = true;
                frames.push(sse_named(
                    "response.created",
                    &json!({
                        "type": "response.created",
                        "response": self.response_object("in_progress", Vec::new()),
                    }),
                ));
            }
            StreamEvent::BlockStart(block) => {
                self.ensure_started(&mut frames);
                let output_index = self.next_output;
                self.next_output += 1;
                let encoded = match block.kind {
                    BlockKind::Text => {
                        let item_id = format!("msg_{}", Uuid::new_v4().simple());
                        frames.push(sse_named(
                            "response.output_item.added",
                            &json!({
                                "type": "response.output_item.added",
                                "output_index": output_index,
                                "item": {
                                    "type": "message",
                                    "id": item_id,
                                    "role": "assistant",
                                    "status": "in_progress",
                                    "content": [],
                                },
                            }),
                        ));
                        frames.push(sse_named(
                            "response.content_part.added",
                            &json!({
                                "type": "response.content_part.added",
                                "item_id": item_id,
                                "output_index": output_index,
                                "content_index": 0,
                                "part": {"type": "output_text", "text": "", "annotations": []},
                            }),
                        ));
                        EncodedBlock {
                            output_index,
                            kind: BlockKind::Text,
                            item_id,
                            call_id: String::new(),
                            name: String::new(),
                            text: String::new(),
                            arguments: String::new(),
                            signature: None,
                        }
                    }
                    BlockKind::Reasoning => {
                        let item_id = format!("rs_{}", Uuid::new_v4().simple());
                        frames.push(sse_named(
                            "response.output_item.added",
                            &json!({
                                "type": "response.output_item.added",
                                "output_index": output_index,
                                "item": {
                                    "type": "reasoning",
                                    "id": item_id,
                                    "summary": [],
                                    "status": "in_progress",
                                },
                            }),
                        ));
                        EncodedBlock {
                            output_index,
                            kind: BlockKind::Reasoning,
                            item_id,
                            call_id: String::new(),
                            name: String::new(),
                            text: String::new(),
                            arguments: String::new(),
                            signature: None,
                        }
                    }
                    BlockKind::ToolUse => {
                        let item_id = format!("fc_{}", Uuid::new_v4().simple());
                        let call_id = block
                            .id
                            .clone()
                            .unwrap_or_else(|| format!("call_{}", Uuid::new_v4().simple()));
                        let name = block.name.clone().unwrap_or_default();
                        frames.push(sse_named(
                            "response.output_item.added",
                            &json!({
                                "type": "response.output_item.added",
                                "output_index": output_index,
                                "item": {
                                    "type": "function_call",
                                    "id": item_id,
                                    "call_id": call_id,
                                    "name": name,
                                    "arguments": "",
                                    "status": "in_progress",
                                },
                            }),
                        ));
                        EncodedBlock {
                            output_index,
                            kind: BlockKind::ToolUse,
                            item_id,
                            call_id,
                            name,
                            text: String::new(),
                            arguments: String::new(),
                            signature: None,
                        }
                    }
                };
                self.blocks.insert(block.index, encoded);
                self.open.insert(block.index);
            }
            StreamEvent::TextDelta { index, text } => {
                let Some(block) = self.blocks.get_mut(index) else {
                    return frames;
                };
                block.text.push_str(text);
                frames.push(sse_named(
                    "response.output_text.delta",
                    &json!({
                        "type": "response.output_text.delta",
                        "item_id": block.item_id,
                        "output_index": block.output_index,
                        "content_index": 0,
                        "delta": text,
                    }),
                ));
            }
            StreamEvent::ReasoningDelta { index, text } => {
                let Some(block) = self.blocks.get_mut(index) else {
                    return frames;
                };
                block.text.push_str(text);
                frames.push(sse_named(
                    "response.reasoning_summary_text.delta",
                    &json!({
                        "type": "response.reasoning_summary_text.delta",
                        "item_id": block.item_id,
                        "output_index": block.output_index,
                        "summary_index": 0,
                        "delta": text,
                    }),
                ));
            }
            StreamEvent::ReasoningSignature { index, signature } => {
                if let Some(block) = self.blocks.get_mut(index) {
                    if block.kind == BlockKind::Reasoning {
                        block.signature = Some(signature.clone());
                    }
                }
            }
            StreamEvent::ToolInputDelta {
                index,
                partial_json,
            } => {
                let Some(block) = self.blocks.get_mut(index) else {
                    return frames;
                };
                block.arguments.push_str(partial_json);
                frames.push(sse_named(
                    "response.function_call_arguments.delta",
                    &json!({
                        "type": "response.function_call_arguments.delta",
                        "item_id": block.item_id,
                        "output_index": block.output_index,
                        "delta": partial_json,
                    }),
                ));
            }
            StreamEvent::BlockStop { index } => {
                if let Some(block) = self.blocks.get(index) {
                    if self.open.remove(index) {
                        frames.extend(self.close_block(block));
                    }
                }
            }
            StreamEvent::Usage(usage) => self.usage.merge(usage),
            StreamEvent::Stop {
                reason,
                stop_sequence,
            } => {
                self.stop = *reason;
                self.stop_sequence = stop_sequence.clone();
            }
            StreamEvent::Done => {
                for index in std::mem::take(&mut self.open) {
                    if let Some(block) = self.blocks.get(&index) {
                        frames.extend(self.close_block(block));
                    }
                }
                let (status, event) = match self.stop {
                    Some(StopReason::MaxTokens) | Some(StopReason::ContentFilter) => {
                        ("incomplete", "response.incomplete")
                    }
                    _ => ("completed", "response.completed"),
                };
                let response = self.response_object(status, self.output_items());
                self.final_response = Some(response.clone());
                frames.push(sse_named(
                    event,
                    &json!({"type": event, "response": response}),
                ));
                self.done = true;
            }
            StreamEvent::Error {
                error_type,
                message,
            } => {
                frames.push(sse_named(
                    "error",
                    &json!({
                        "type": "error",
                        "code": error_type,
                        "message": message,
                        "param": null,
                    }),
                ));
                self.done = true;
            }
        }

        frames
    }

    fn ensure_started(&mut self, frames: &mut Vec<String>) {
        if self.started {
            return;
        }
        self.started = true;
        if self.id.is_empty() {
            self.id = self
                .response_id
                .clone()
                .unwrap_or_else(|| format!("resp_{}", Uuid::new_v4().simple()));
        }
        frames.push(sse_named(
            "response.created",
            &json!({
                "type": "response.created",
                "response": self.response_object("in_progress", Vec::new()),
            }),
        ));
    }

    fn close_block(&self, block: &EncodedBlock) -> Vec<String> {
        match block.kind {
            BlockKind::Text => vec![
                sse_named(
                    "response.output_text.done",
                    &json!({
                        "type": "response.output_text.done",
                        "item_id": block.item_id,
                        "output_index": block.output_index,
                        "content_index": 0,
                        "text": block.text,
                    }),
                ),
                sse_named(
                    "response.content_part.done",
                    &json!({
                        "type": "response.content_part.done",
                        "item_id": block.item_id,
                        "output_index": block.output_index,
                        "content_index": 0,
                        "part": {
                            "type": "output_text",
                            "text": block.text,
                            "annotations": [],
                        },
                    }),
                ),
                sse_named(
                    "response.output_item.done",
                    &json!({
                        "type": "response.output_item.done",
                        "output_index": block.output_index,
                        "item": output_item(block),
                    }),
                ),
            ],
            BlockKind::Reasoning => {
                vec![
                    sse_named(
                        "response.reasoning_summary_text.done",
                        &json!({
                            "type": "response.reasoning_summary_text.done",
                            "item_id": block.item_id,
                            "output_index": block.output_index,
                            "summary_index": 0,
                            "text": block.text,
                        }),
                    ),
                    sse_named(
                        "response.output_item.done",
                        &json!({
                            "type": "response.output_item.done",
                            "output_index": block.output_index,
                            "item": output_item(block),
                        }),
                    ),
                ]
            }
            BlockKind::ToolUse => vec![
                sse_named(
                    "response.function_call_arguments.done",
                    &json!({
                        "type": "response.function_call_arguments.done",
                        "item_id": block.item_id,
                        "output_index": block.output_index,
                        "arguments": block.arguments,
                    }),
                ),
                sse_named(
                    "response.output_item.done",
                    &json!({
                        "type": "response.output_item.done",
                        "output_index": block.output_index,
                        "item": output_item(block),
                    }),
                ),
            ],
        }
    }

    fn output_items(&self) -> Vec<Value> {
        let mut blocks: Vec<&EncodedBlock> = self.blocks.values().collect();
        blocks.sort_by_key(|block| block.output_index);
        blocks.into_iter().map(output_item).collect()
    }

    fn response_object(&self, status: &str, output: Vec<Value>) -> Value {
        let mut object = Map::new();
        object.insert("id".into(), json!(self.id));
        object.insert("object".into(), json!("response"));
        object.insert("created_at".into(), json!(self.created_at));
        object.insert("status".into(), json!(status));
        object.insert("model".into(), json!(self.model));
        object.insert("output".into(), Value::Array(output));
        object.insert("usage".into(), self.usage.responses_json());
        object.insert("store".into(), json!(self.store));
        if status == "incomplete" {
            let reason = match self.stop {
                Some(StopReason::ContentFilter) => "content_filter",
                _ => "max_output_tokens",
            };
            object.insert("incomplete_details".into(), json!({"reason": reason}));
        }
        Value::Object(object)
    }
}
