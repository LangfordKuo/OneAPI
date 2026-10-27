use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::{
    budget_for_effort, get_bool, get_f64, get_str, get_string, get_u32, get_u64, text_content,
    ContentPart, ImageSource, ProtocolError, ReasoningConfig, ResponseFormat, Role, StopReason,
    ToolChoice, ToolDef, UnifiedMessage, UnifiedRequest, UnifiedResponse, Usage,
};

pub fn parse_request(value: &Value) -> Result<UnifiedRequest, ProtocolError> {
    let obj = value.as_object().ok_or_else(|| {
        ProtocolError::invalid_request("anthropic", "request must be a JSON object")
    })?;

    let model = get_str(obj, "model")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProtocolError::invalid_request("anthropic", "model is required"))?
        .to_string();

    let max_tokens = get_u32(obj, "max_tokens")
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            ProtocolError::invalid_request("anthropic", "max_tokens must be a positive integer")
        })?;

    let mut system = Vec::new();
    if let Some(system_value) = obj.get("system") {
        system.extend(parse_system(system_value)?);
    }

    let mut messages = Vec::new();
    if let Some(values) = obj.get("messages").and_then(Value::as_array) {
        for (index, message) in values.iter().enumerate() {
            let message = message.as_object().ok_or_else(|| {
                ProtocolError::invalid_request(
                    "anthropic",
                    format!("messages[{index}] must be an object"),
                )
            })?;
            let role = match get_str(message, "role").ok_or_else(|| {
                ProtocolError::invalid_request(
                    "anthropic",
                    format!("messages[{index}].role is required"),
                )
            })? {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                other => {
                    return Err(ProtocolError::invalid_request(
                        "anthropic",
                        format!("unsupported message role `{other}`"),
                    ))
                }
            };
            let content = parse_content_parts(message.get("content"))?;
            match role {
                Role::User => {
                    let has_tool_result = content
                        .iter()
                        .any(|part| matches!(part, ContentPart::ToolResult { .. }));
                    if has_tool_result {
                        let (tool_results, rest): (Vec<ContentPart>, Vec<ContentPart>) = content
                            .into_iter()
                            .partition(|part| matches!(part, ContentPart::ToolResult { .. }));
                        if !rest.is_empty() {
                            messages.push(UnifiedMessage {
                                role: Role::User,
                                content: rest,
                            });
                        }
                        messages.push(UnifiedMessage {
                            role: Role::Tool,
                            content: tool_results,
                        });
                    } else {
                        messages.push(UnifiedMessage {
                            role: Role::User,
                            content,
                        });
                    }
                }
                Role::Assistant => messages.push(UnifiedMessage {
                    role: Role::Assistant,
                    content,
                }),
                _ => {}
            }
        }
    }

    let mut tools = Vec::new();
    if let Some(values) = obj.get("tools").and_then(Value::as_array) {
        for tool in values {
            let tool = tool.as_object().ok_or_else(|| {
                ProtocolError::invalid_request("anthropic", "tool must be an object")
            })?;
            let name = get_str(tool, "name")
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ProtocolError::invalid_request("anthropic", "tool name is required")
                })?
                .to_string();
            tools.push(ToolDef {
                name,
                description: get_string(tool, "description"),
                parameters: tool
                    .get("input_schema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                strict: None,
            });
        }
    }

    let (tool_choice, parallel_tool_calls) = parse_tool_choice(obj.get("tool_choice"))?;
    let reasoning = obj
        .get("thinking")
        .and_then(Value::as_object)
        .and_then(|thinking| {
            if get_str(thinking, "type").unwrap_or("disabled") == "enabled" {
                Some(ReasoningConfig {
                    effort: None,
                    budget_tokens: get_u32(thinking, "budget_tokens"),
                    summary: None,
                })
            } else {
                None
            }
        });

    let response_format = obj
        .get("output_format")
        .and_then(Value::as_object)
        .and_then(|format| {
            if get_str(format, "type").unwrap_or("json_schema") != "json_schema" {
                return None;
            }
            format
                .get("schema")
                .cloned()
                .map(|schema| ResponseFormat::JsonSchema {
                    name: get_string(format, "name"),
                    schema,
                    strict: get_bool(format, "strict"),
                })
        });

    let metadata_user_id = obj
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| get_string(metadata, "user_id"));

    Ok(UnifiedRequest {
        model,
        system,
        messages,
        tools,
        tool_choice,
        parallel_tool_calls,
        max_tokens: Some(max_tokens),
        temperature: get_f64(obj, "temperature"),
        top_p: get_f64(obj, "top_p"),
        top_k: get_u32(obj, "top_k"),
        stop: super::parse_stop(obj.get("stop_sequences")),
        stream: get_bool(obj, "stream").unwrap_or(false),
        reasoning,
        response_format,
        metadata_user_id,
        previous_response_id: None,
        store: None,
        n: None,
        extras: Map::new(),
    })
}

fn parse_system(value: &Value) -> Result<Vec<ContentPart>, ProtocolError> {
    match value {
        Value::String(text) => Ok(vec![ContentPart::Text { text: text.clone() }]),
        Value::Array(blocks) => {
            let mut result = Vec::new();
            for block in blocks {
                let block = block.as_object().ok_or_else(|| {
                    ProtocolError::invalid_request("anthropic", "system block must be an object")
                })?;
                if get_str(block, "type").unwrap_or("text") == "text" {
                    if let Some(text) = get_str(block, "text") {
                        result.push(ContentPart::Text {
                            text: text.to_string(),
                        });
                    }
                }
            }
            Ok(result)
        }
        _ => Err(ProtocolError::invalid_request(
            "anthropic",
            "system must be a string or array",
        )),
    }
}

fn parse_content_parts(value: Option<&Value>) -> Result<Vec<ContentPart>, ProtocolError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(text) => Ok(vec![ContentPart::Text { text: text.clone() }]),
        Value::Array(blocks) => {
            let mut result = Vec::new();
            for block in blocks {
                let block = block.as_object().ok_or_else(|| {
                    ProtocolError::invalid_request("anthropic", "content block must be an object")
                })?;
                match get_str(block, "type").unwrap_or_default() {
                    "text" => {
                        if let Some(text) = get_str(block, "text") {
                            result.push(ContentPart::Text {
                                text: text.to_string(),
                            });
                        }
                    }
                    "image" => {
                        if let Some(source) = block.get("source").and_then(parse_image_source) {
                            result.push(ContentPart::Image { source });
                        }
                    }
                    "tool_use" => {
                        let id = get_str(block, "id").unwrap_or_default().to_string();
                        let name = get_str(block, "name").unwrap_or_default().to_string();
                        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                        result.push(ContentPart::ToolUse { id, name, input });
                    }
                    "tool_result" => {
                        let tool_use_id = get_str(block, "tool_use_id")
                            .unwrap_or_default()
                            .to_string();
                        let content = parse_content_parts(block.get("content"))?;
                        result.push(ContentPart::ToolResult {
                            tool_use_id,
                            content,
                            is_error: get_bool(block, "is_error").unwrap_or(false),
                        });
                    }
                    "thinking" => {
                        if let Some(text) = get_str(block, "thinking") {
                            result.push(ContentPart::Reasoning {
                                text: text.to_string(),
                                signature: get_string(block, "signature"),
                            });
                        }
                    }
                    "redacted_thinking" => {
                        if let Some(data) = get_str(block, "data") {
                            result.push(ContentPart::RedactedReasoning {
                                data: data.to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            Ok(result)
        }
        _ => Err(ProtocolError::invalid_request(
            "anthropic",
            "content must be a string or array",
        )),
    }
}

fn parse_image_source(value: &Value) -> Option<ImageSource> {
    let source = value.as_object()?;
    match get_str(source, "type")? {
        "base64" => Some(ImageSource::Base64 {
            media_type: get_str(source, "media_type")?.to_string(),
            data: get_str(source, "data")?.to_string(),
        }),
        "url" => Some(ImageSource::Url(get_str(source, "url")?.to_string())),
        _ => None,
    }
}

fn parse_tool_choice(value: Option<&Value>) -> Result<(ToolChoice, Option<bool>), ProtocolError> {
    let Some(value) = value else {
        return Ok((ToolChoice::Auto, None));
    };

    match value {
        Value::String(value) => match value.as_str() {
            "auto" => Ok((ToolChoice::Auto, None)),
            "any" => Ok((ToolChoice::Required, None)),
            "none" => Ok((ToolChoice::None, None)),
            other => Err(ProtocolError::invalid_request(
                "anthropic",
                format!("unsupported tool_choice `{other}`"),
            )),
        },
        Value::Object(choice) => {
            let kind = get_str(choice, "type").unwrap_or("auto");
            let parsed = match kind {
                "auto" => ToolChoice::Auto,
                "any" => ToolChoice::Required,
                "none" => ToolChoice::None,
                "tool" => ToolChoice::Tool {
                    name: get_str(choice, "name")
                        .ok_or_else(|| {
                            ProtocolError::invalid_request(
                                "anthropic",
                                "tool_choice requires a name",
                            )
                        })?
                        .to_string(),
                },
                other => {
                    return Err(ProtocolError::invalid_request(
                        "anthropic",
                        format!("unsupported tool_choice type `{other}`"),
                    ))
                }
            };
            let parallel = get_bool(choice, "disable_parallel_tool_use").map(|disable| !disable);
            Ok((parsed, parallel))
        }
        _ => Err(ProtocolError::invalid_request(
            "anthropic",
            "tool_choice must be a string or object",
        )),
    }
}

pub fn encode_request(request: &UnifiedRequest) -> Result<Value, ProtocolError> {
    if request.n.unwrap_or(1) > 1 {
        return Err(ProtocolError::unsupported("n>1", "anthropic"));
    }
    super::reject_extras(
        request,
        "anthropic",
        &[
            "logit_bias",
            "presence_penalty",
            "frequency_penalty",
            "seed",
            "audio",
            "modalities",
        ],
    )?;

    if matches!(&request.response_format, Some(ResponseFormat::JsonObject)) {
        return Err(ProtocolError::unsupported(
            "response_format json_object",
            "anthropic",
        ));
    }

    let max_tokens = request.max_tokens.unwrap_or(4096).max(1);

    let mut obj = Map::new();
    obj.insert("model".into(), Value::String(request.model.clone()));
    obj.insert("max_tokens".into(), json!(max_tokens));

    let mut system_text = text_content(&request.system);
    let mut message_blocks: Vec<(String, Vec<Value>)> = Vec::new();

    for message in &request.messages {
        let (role, blocks) = encode_message(message)?;
        if blocks.is_empty() {
            continue;
        }

        if role == "system" {
            if !system_text.is_empty() {
                system_text.push_str("\n\n");
            }
            system_text.push_str(&text_content(&message.content));
            continue;
        }

        if let Some((last_role, last_blocks)) = message_blocks.last_mut() {
            if last_role == &role {
                last_blocks.extend(blocks);
                continue;
            }
        }
        message_blocks.push((role, blocks));
    }

    if !system_text.is_empty() {
        obj.insert("system".into(), Value::String(system_text));
    }

    let messages: Vec<Value> = message_blocks
        .into_iter()
        .map(|(role, blocks)| json!({"role": role, "content": blocks}))
        .collect();
    obj.insert("messages".into(), Value::Array(messages));

    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                let mut value = Map::new();
                value.insert("name".into(), json!(tool.name));
                if let Some(description) = &tool.description {
                    value.insert("description".into(), json!(description));
                }
                value.insert("input_schema".into(), tool.parameters.clone());
                Value::Object(value)
            })
            .collect();
        obj.insert("tools".into(), Value::Array(tools));
        obj.insert(
            "tool_choice".into(),
            encode_tool_choice(&request.tool_choice, request.parallel_tool_calls),
        );
    }

    if let Some(temperature) = request.temperature {
        obj.insert("temperature".into(), json!(temperature));
    }
    if let Some(top_p) = request.top_p {
        obj.insert("top_p".into(), json!(top_p));
    }
    if let Some(top_k) = request.top_k {
        obj.insert("top_k".into(), json!(top_k));
    }
    if !request.stop.is_empty() {
        obj.insert("stop_sequences".into(), json!(request.stop));
    }
    if request.stream {
        obj.insert("stream".into(), json!(true));
    }

    if let Some(reasoning) = &request.reasoning {
        let budget = reasoning
            .budget_tokens
            .or_else(|| reasoning.effort.map(budget_for_effort))
            .unwrap_or(4096);
        if max_tokens > 1024 {
            let budget = budget.min(max_tokens - 1).max(1024);
            obj.insert(
                "thinking".into(),
                json!({"type": "enabled", "budget_tokens": budget}),
            );
        }
    }

    if let Some(user_id) = &request.metadata_user_id {
        obj.insert("metadata".into(), json!({"user_id": user_id}));
    }

    if let Some(ResponseFormat::JsonSchema {
        name,
        schema,
        strict,
    }) = &request.response_format
    {
        let mut format = Map::new();
        format.insert("type".into(), json!("json_schema"));
        if let Some(name) = name {
            format.insert("name".into(), json!(name));
        }
        format.insert("schema".into(), schema.clone());
        if let Some(strict) = strict {
            format.insert("strict".into(), json!(strict));
        }
        obj.insert("output_format".into(), Value::Object(format));
    }

    Ok(Value::Object(obj))
}

fn encode_message(message: &UnifiedMessage) -> Result<(String, Vec<Value>), ProtocolError> {
    if matches!(message.role, Role::System | Role::Developer) {
        return Ok(("system".to_string(), Vec::new()));
    }

    let role = if message.role == Role::Assistant {
        "assistant"
    } else {
        "user"
    };

    let mut blocks = Vec::new();
    for part in &message.content {
        match part {
            ContentPart::Text { text } => {
                blocks.push(json!({"type": "text", "text": text}));
            }
            ContentPart::Image { source } => {
                blocks.push(image_block(source));
            }
            ContentPart::ToolUse { id, name, input } => {
                blocks.push(json!({
                    "type": "tool_use",
                    "id": if id.is_empty() {
                        format!("toolu_{}", Uuid::new_v4())
                    } else {
                        id.clone()
                    },
                    "name": name,
                    "input": input,
                }));
            }
            ContentPart::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let mut block = Map::new();
                block.insert("type".into(), json!("tool_result"));
                block.insert("tool_use_id".into(), json!(tool_use_id));
                block.insert("content".into(), encode_tool_result_content(content)?);
                if *is_error {
                    block.insert("is_error".into(), json!(true));
                }
                blocks.push(Value::Object(block));
            }
            ContentPart::Reasoning { text, signature } => {
                if let Some(signature) = signature {
                    blocks.push(json!({
                        "type": "thinking",
                        "thinking": text,
                        "signature": signature,
                    }));
                }
            }
            ContentPart::RedactedReasoning { data } => {
                blocks.push(json!({"type": "redacted_thinking", "data": data}));
            }
        }
    }

    Ok((role.to_string(), blocks))
}

fn image_block(source: &ImageSource) -> Value {
    match source {
        ImageSource::Base64 { media_type, data } => json!({
            "type": "image",
            "source": {"type": "base64", "media_type": media_type, "data": data},
        }),
        ImageSource::Url(url) => json!({
            "type": "image",
            "source": {"type": "url", "url": url},
        }),
    }
}

fn encode_tool_result_content(parts: &[ContentPart]) -> Result<Value, ProtocolError> {
    if parts.is_empty() {
        return Ok(Value::String(String::new()));
    }

    if parts
        .iter()
        .all(|part| matches!(part, ContentPart::Text { .. }))
    {
        return Ok(Value::String(text_content(parts)));
    }

    let mut blocks = Vec::new();
    for part in parts {
        match part {
            ContentPart::Text { text } => blocks.push(json!({"type": "text", "text": text})),
            ContentPart::Image { source } => blocks.push(image_block(source)),
            _ => {
                return Err(ProtocolError::unsupported(
                    "non-text tool_result content",
                    "anthropic",
                ))
            }
        }
    }
    Ok(Value::Array(blocks))
}

fn encode_tool_choice(choice: &ToolChoice, parallel: Option<bool>) -> Value {
    let mut value = match choice {
        ToolChoice::Auto => json!({"type": "auto"}),
        ToolChoice::None => json!({"type": "none"}),
        ToolChoice::Required => json!({"type": "any"}),
        ToolChoice::Tool { name } => json!({"type": "tool", "name": name}),
    };

    if let (Some(parallel), Some(obj)) = (parallel, value.as_object_mut()) {
        obj.insert("disable_parallel_tool_use".into(), json!(!parallel));
    }

    value
}

pub fn parse_response(value: &Value) -> Result<UnifiedResponse, ProtocolError> {
    let obj = value.as_object().ok_or_else(|| {
        ProtocolError::invalid_response("anthropic", "response must be a JSON object")
    })?;

    let id = get_str(obj, "id").unwrap_or_default().to_string();
    let model = get_str(obj, "model").unwrap_or_default().to_string();

    let mut content = Vec::new();
    if let Some(blocks) = obj.get("content").and_then(Value::as_array) {
        for block in blocks {
            let block = block.as_object().ok_or_else(|| {
                ProtocolError::invalid_response("anthropic", "content block must be an object")
            })?;
            match get_str(block, "type").unwrap_or_default() {
                "text" => {
                    if let Some(text) = get_str(block, "text") {
                        content.push(ContentPart::Text {
                            text: text.to_string(),
                        });
                    }
                }
                "thinking" => {
                    if let Some(text) = get_str(block, "thinking") {
                        content.push(ContentPart::Reasoning {
                            text: text.to_string(),
                            signature: get_string(block, "signature"),
                        });
                    }
                }
                "redacted_thinking" => {
                    if let Some(data) = get_str(block, "data") {
                        content.push(ContentPart::RedactedReasoning {
                            data: data.to_string(),
                        });
                    }
                }
                "tool_use" => {
                    content.push(ContentPart::ToolUse {
                        id: get_str(block, "id").unwrap_or_default().to_string(),
                        name: get_str(block, "name").unwrap_or_default().to_string(),
                        input: block.get("input").cloned().unwrap_or_else(|| json!({})),
                    });
                }
                _ => {}
            }
        }
    }

    let stop_reason = get_str(obj, "stop_reason").map(|reason| match reason {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "tool_use" => StopReason::ToolUse,
        "stop_sequence" => StopReason::StopSequence,
        "refusal" => StopReason::ContentFilter,
        _ => StopReason::Other,
    });

    let usage = obj
        .get("usage")
        .and_then(Value::as_object)
        .map(parse_usage)
        .unwrap_or_default();

    Ok(UnifiedResponse {
        id,
        model,
        created_at: 0,
        content,
        stop_reason,
        stop_sequence: get_string(obj, "stop_sequence"),
        usage,
    })
}

pub(crate) fn parse_usage(usage: &Map<String, Value>) -> Usage {
    let input = get_u64(usage, "input_tokens");
    let output = get_u64(usage, "output_tokens");
    let cached = get_u64(usage, "cache_read_input_tokens");
    let creation = get_u64(usage, "cache_creation_input_tokens");
    let total = match (input, output) {
        (Some(input), Some(output)) => {
            Some(input + output + cached.unwrap_or(0) + creation.unwrap_or(0))
        }
        _ => None,
    };

    Usage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: total,
        cached_tokens: cached,
        cache_creation_tokens: creation,
        reasoning_tokens: None,
    }
}

pub fn encode_response(response: &UnifiedResponse) -> Result<Value, ProtocolError> {
    let mut content = Vec::new();
    for part in &response.content {
        match part {
            ContentPart::Text { text } => content.push(json!({"type": "text", "text": text})),
            ContentPart::Reasoning {
                text,
                signature: Some(signature),
            } => {
                content.push(json!({
                    "type": "thinking",
                    "thinking": text,
                    "signature": signature,
                }));
            }
            ContentPart::Reasoning { .. } => {}
            ContentPart::RedactedReasoning { data } => {
                content.push(json!({"type": "redacted_thinking", "data": data}));
            }
            ContentPart::ToolUse { id, name, input } => {
                content.push(json!({
                    "type": "tool_use",
                    "id": if id.is_empty() {
                        format!("toolu_{}", Uuid::new_v4())
                    } else {
                        id.clone()
                    },
                    "name": name,
                    "input": input,
                }));
            }
            _ => {}
        }
    }

    let stop_reason = response
        .stop_reason
        .map(|reason| match reason {
            StopReason::EndTurn => "end_turn",
            StopReason::MaxTokens => "max_tokens",
            StopReason::ToolUse => "tool_use",
            StopReason::ContentFilter => "refusal",
            StopReason::StopSequence => "stop_sequence",
            StopReason::Other => "end_turn",
        })
        .unwrap_or("end_turn");

    let mut usage = Map::new();
    if let Some(input) = response.usage.input_tokens {
        usage.insert("input_tokens".into(), json!(input));
    }
    if let Some(output) = response.usage.output_tokens {
        usage.insert("output_tokens".into(), json!(output));
    }
    if let Some(cached) = response.usage.cached_tokens {
        usage.insert("cache_read_input_tokens".into(), json!(cached));
    }
    if let Some(creation) = response.usage.cache_creation_tokens {
        usage.insert("cache_creation_input_tokens".into(), json!(creation));
    }

    let mut obj = Map::new();
    obj.insert("id".into(), json!(response.id));
    obj.insert("type".into(), json!("message"));
    obj.insert("role".into(), json!("assistant"));
    obj.insert("model".into(), json!(response.model));
    obj.insert("content".into(), Value::Array(content));
    obj.insert("stop_reason".into(), json!(stop_reason));
    obj.insert(
        "stop_sequence".into(),
        response
            .stop_sequence
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    obj.insert("usage".into(), Value::Object(usage));

    Ok(Value::Object(obj))
}
