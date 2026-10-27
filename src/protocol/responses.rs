use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::{
    effort_for_budget, effort_name, ensure_text_only, get_bool, get_f64, get_str, get_string,
    get_u32, get_u64, image_url, parse_arguments, parse_effort, parse_image_string, text_content,
    ContentPart, ProtocolError, ReasoningConfig, ResponseFormat, Role, StopReason, ToolChoice,
    ToolDef, UnifiedMessage, UnifiedRequest, UnifiedResponse, Usage,
};

pub fn parse_request(value: &Value) -> Result<UnifiedRequest, ProtocolError> {
    let obj = value.as_object().ok_or_else(|| {
        ProtocolError::invalid_request("responses", "request must be a JSON object")
    })?;

    let model = get_str(obj, "model")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProtocolError::invalid_request("responses", "model is required"))?
        .to_string();

    let mut system = Vec::new();
    if let Some(instructions) = obj.get("instructions") {
        system.extend(parse_instructions(instructions)?);
    }

    let mut messages = Vec::new();
    match obj.get("input") {
        Some(Value::String(text)) => messages.push(UnifiedMessage {
            role: Role::User,
            content: vec![ContentPart::Text { text: text.clone() }],
        }),
        Some(Value::Array(items)) => {
            for (index, item) in items.iter().enumerate() {
                parse_input_item(item, index, &mut system, &mut messages)?;
            }
        }
        Some(Value::Null) | None => {}
        _ => {
            return Err(ProtocolError::invalid_request(
                "responses",
                "input must be a string or array",
            ))
        }
    }

    let mut tools = Vec::new();
    if let Some(values) = obj.get("tools").and_then(Value::as_array) {
        for tool in values {
            let tool = tool.as_object().ok_or_else(|| {
                ProtocolError::invalid_request("responses", "tool must be an object")
            })?;
            if get_str(tool, "type").unwrap_or("function") != "function" {
                continue;
            }
            let name = get_str(tool, "name")
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ProtocolError::invalid_request("responses", "tool name is required")
                })?
                .to_string();
            tools.push(ToolDef {
                name,
                description: get_string(tool, "description"),
                parameters: tool
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                strict: get_bool(tool, "strict"),
            });
        }
    }

    let tool_choice = parse_tool_choice(obj.get("tool_choice"))?;
    let reasoning = obj
        .get("reasoning")
        .and_then(Value::as_object)
        .map(|reasoning| ReasoningConfig {
            effort: get_str(reasoning, "effort").map(parse_effort),
            budget_tokens: None,
            summary: get_string(reasoning, "summary"),
        });
    let response_format = obj
        .get("text")
        .and_then(Value::as_object)
        .and_then(|text| text.get("format"))
        .map(parse_response_format)
        .transpose()?
        .flatten();

    Ok(UnifiedRequest {
        model,
        system,
        messages,
        tools,
        tool_choice,
        parallel_tool_calls: get_bool(obj, "parallel_tool_calls"),
        max_tokens: get_u32(obj, "max_output_tokens"),
        temperature: get_f64(obj, "temperature"),
        top_p: get_f64(obj, "top_p"),
        top_k: None,
        stop: Vec::new(),
        stream: get_bool(obj, "stream").unwrap_or(false),
        reasoning,
        response_format,
        metadata_user_id: get_string(obj, "user"),
        previous_response_id: get_string(obj, "previous_response_id"),
        store: get_bool(obj, "store"),
        n: None,
        extras: Map::new(),
    })
}

fn parse_instructions(value: &Value) -> Result<Vec<ContentPart>, ProtocolError> {
    match value {
        Value::String(text) => Ok(vec![ContentPart::Text { text: text.clone() }]),
        Value::Array(parts) => {
            let mut result = Vec::new();
            for part in parts {
                let part = part.as_object().ok_or_else(|| {
                    ProtocolError::invalid_request(
                        "responses",
                        "instructions part must be an object",
                    )
                })?;
                if matches!(
                    get_str(part, "type").unwrap_or("input_text"),
                    "input_text" | "text"
                ) {
                    if let Some(text) = get_str(part, "text") {
                        result.push(ContentPart::Text {
                            text: text.to_string(),
                        });
                    }
                }
            }
            Ok(result)
        }
        _ => Err(ProtocolError::invalid_request(
            "responses",
            "instructions must be a string or array",
        )),
    }
}

fn parse_input_item(
    item: &Value,
    index: usize,
    system: &mut Vec<ContentPart>,
    messages: &mut Vec<UnifiedMessage>,
) -> Result<(), ProtocolError> {
    let item = item.as_object().ok_or_else(|| {
        ProtocolError::invalid_request("responses", format!("input[{index}] must be an object"))
    })?;
    let kind = get_str(item, "type").unwrap_or("message");

    match kind {
        "message" => {
            let role = get_str(item, "role").unwrap_or("user");
            let content = parse_content_parts(item.get("content"), true)?;
            match role {
                "system" | "developer" => system.extend(content),
                "user" => messages.push(UnifiedMessage {
                    role: Role::User,
                    content,
                }),
                "assistant" => messages.push(UnifiedMessage {
                    role: Role::Assistant,
                    content,
                }),
                other => {
                    return Err(ProtocolError::invalid_request(
                        "responses",
                        format!("unsupported message role `{other}`"),
                    ))
                }
            }
        }
        "function_call" => {
            let call_id = get_str(item, "call_id")
                .or_else(|| get_str(item, "id"))
                .unwrap_or_default()
                .to_string();
            let name = get_str(item, "name")
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ProtocolError::invalid_request("responses", "function_call requires name")
                })?
                .to_string();
            let input = parse_arguments(get_str(item, "arguments").unwrap_or("{}"));
            let part = ContentPart::ToolUse {
                id: call_id,
                name,
                input,
            };
            if let Some(last) = messages.last_mut() {
                if last.role == Role::Assistant {
                    last.content.push(part);
                    return Ok(());
                }
            }
            messages.push(UnifiedMessage {
                role: Role::Assistant,
                content: vec![part],
            });
        }
        "function_call_output" => {
            let call_id = get_str(item, "call_id")
                .or_else(|| get_str(item, "id"))
                .ok_or_else(|| {
                    ProtocolError::invalid_request(
                        "responses",
                        "function_call_output requires call_id",
                    )
                })?
                .to_string();
            let content = parse_content_parts(item.get("output"), false)?;
            messages.push(UnifiedMessage {
                role: Role::Tool,
                content: vec![ContentPart::ToolResult {
                    tool_use_id: call_id,
                    content,
                    is_error: false,
                }],
            });
        }
        "reasoning" => {
            let mut text = String::new();
            if let Some(summary) = item.get("summary").and_then(Value::as_array) {
                for part in summary {
                    if let Some(value) = part.as_object().and_then(|part| get_str(part, "text")) {
                        text.push_str(value);
                    }
                }
            }
            if text.is_empty() {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if let Some(value) = part.as_object().and_then(|part| get_str(part, "text"))
                        {
                            text.push_str(value);
                        }
                    }
                }
            }
            if text.is_empty() {
                return Ok(());
            }
            let signature = get_string(item, "encrypted_content");
            let part = ContentPart::Reasoning { text, signature };
            if let Some(last) = messages.last_mut() {
                if last.role == Role::Assistant {
                    last.content.push(part);
                    return Ok(());
                }
            }
            messages.push(UnifiedMessage {
                role: Role::Assistant,
                content: vec![part],
            });
        }
        "item_reference" => {}
        _ => {}
    }

    Ok(())
}

fn parse_content_parts(
    value: Option<&Value>,
    allow_images: bool,
) -> Result<Vec<ContentPart>, ProtocolError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(text) => Ok(vec![ContentPart::Text { text: text.clone() }]),
        Value::Array(parts) => {
            let mut result = Vec::new();
            for part in parts {
                let part = part.as_object().ok_or_else(|| {
                    ProtocolError::invalid_request(
                        "responses",
                        "input content part must be an object",
                    )
                })?;
                match get_str(part, "type").unwrap_or("input_text") {
                    "input_text" | "output_text" | "text" | "refusal" => {
                        if let Some(text) =
                            get_str(part, "text").or_else(|| get_str(part, "refusal"))
                        {
                            result.push(ContentPart::Text {
                                text: text.to_string(),
                            });
                        }
                    }
                    "input_image" => {
                        if !allow_images {
                            return Err(ProtocolError::unsupported(
                                "image in tool output",
                                "responses",
                            ));
                        }
                        let source = get_str(part, "image_url")
                            .or_else(|| get_str(part, "url"))
                            .map(parse_image_string);
                        if let Some(source) = source {
                            result.push(ContentPart::Image { source });
                        }
                    }
                    _ => {}
                }
            }
            Ok(result)
        }
        _ => Err(ProtocolError::invalid_request(
            "responses",
            "content must be a string or array",
        )),
    }
}

fn parse_tool_choice(value: Option<&Value>) -> Result<ToolChoice, ProtocolError> {
    match value {
        None => Ok(ToolChoice::Auto),
        Some(Value::String(value)) => match value.as_str() {
            "auto" => Ok(ToolChoice::Auto),
            "none" => Ok(ToolChoice::None),
            "required" => Ok(ToolChoice::Required),
            other => Err(ProtocolError::invalid_request(
                "responses",
                format!("unsupported tool_choice `{other}`"),
            )),
        },
        Some(Value::Object(value)) => match get_str(value, "type").unwrap_or_default() {
            "function" | "tool" => Ok(ToolChoice::Tool {
                name: get_str(value, "name")
                    .ok_or_else(|| {
                        ProtocolError::invalid_request("responses", "tool_choice requires a name")
                    })?
                    .to_string(),
            }),
            other => Err(ProtocolError::invalid_request(
                "responses",
                format!("unsupported tool_choice type `{other}`"),
            )),
        },
        _ => Err(ProtocolError::invalid_request(
            "responses",
            "tool_choice must be a string or object",
        )),
    }
}

fn parse_response_format(value: &Value) -> Result<Option<ResponseFormat>, ProtocolError> {
    let obj = value.as_object().ok_or_else(|| {
        ProtocolError::invalid_request("responses", "text.format must be an object")
    })?;
    Ok(match get_str(obj, "type").unwrap_or("text") {
        "text" => Some(ResponseFormat::Text),
        "json_object" => Some(ResponseFormat::JsonObject),
        "json_schema" => Some(ResponseFormat::JsonSchema {
            name: get_string(obj, "name"),
            schema: obj.get("schema").cloned().ok_or_else(|| {
                ProtocolError::invalid_request("responses", "text.format.schema is required")
            })?,
            strict: get_bool(obj, "strict"),
        }),
        _ => None,
    })
}

pub fn encode_request(request: &UnifiedRequest) -> Result<Value, ProtocolError> {
    if request.top_k.is_some() {
        return Err(ProtocolError::unsupported("top_k", "responses"));
    }
    if !request.stop.is_empty() {
        return Err(ProtocolError::unsupported("stop", "responses"));
    }
    if request.n.unwrap_or(1) > 1 {
        return Err(ProtocolError::unsupported("n>1", "responses"));
    }
    super::reject_extras(
        request,
        "responses",
        &[
            "logit_bias",
            "presence_penalty",
            "frequency_penalty",
            "seed",
            "audio",
            "modalities",
        ],
    )?;

    let mut obj = Map::new();
    obj.insert("model".into(), Value::String(request.model.clone()));

    if !request.system.is_empty() {
        if request
            .system
            .iter()
            .any(|part| !matches!(part, ContentPart::Text { .. }))
        {
            return Err(ProtocolError::unsupported(
                "non-text system content",
                "responses",
            ));
        }
        obj.insert(
            "instructions".into(),
            Value::String(text_content(&request.system)),
        );
    }

    let mut input = Vec::new();
    for message in &request.messages {
        match message.role {
            Role::System | Role::Developer => {
                if message
                    .content
                    .iter()
                    .any(|part| !matches!(part, ContentPart::Text { .. }))
                {
                    return Err(ProtocolError::unsupported(
                        "non-text system content",
                        "responses",
                    ));
                }
                input.push(json!({
                    "type": "message",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": text_content(&message.content)}],
                }));
            }
            Role::User => {
                input.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": encode_content_parts(&message.content, false)?,
                }));
            }
            Role::Assistant => {
                let text = text_content(&message.content);
                if !text.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": text}],
                    }));
                }
                for part in &message.content {
                    match part {
                        ContentPart::ToolUse {
                            id,
                            name,
                            input: arguments,
                        } => {
                            input.push(json!({
                                "type": "function_call",
                                "call_id": if id.is_empty() {
                                    format!("call_{}", Uuid::new_v4())
                                } else {
                                    id.clone()
                                },
                                "name": name,
                                "arguments": serde_json::to_string(arguments)
                                    .unwrap_or_else(|_| "{}".to_string()),
                            }));
                        }
                        ContentPart::Reasoning { .. } | ContentPart::RedactedReasoning { .. } => {
                            // Reasoning items need upstream-issued ids/signatures; the gateway
                            // does not forward them when crossing protocols.
                        }
                        _ => {}
                    }
                }
            }
            Role::Tool => {
                for part in &message.content {
                    if let ContentPart::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = part
                    {
                        ensure_text_only(content, "responses")?;
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": tool_use_id,
                            "output": text_content(content),
                        }));
                    }
                }
            }
        }
    }
    obj.insert("input".into(), Value::Array(input));

    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                let mut value = Map::new();
                value.insert("type".into(), json!("function"));
                value.insert("name".into(), json!(tool.name));
                if let Some(description) = &tool.description {
                    value.insert("description".into(), json!(description));
                }
                value.insert("parameters".into(), tool.parameters.clone());
                if let Some(strict) = tool.strict {
                    value.insert("strict".into(), json!(strict));
                }
                Value::Object(value)
            })
            .collect();
        obj.insert("tools".into(), Value::Array(tools));
        obj.insert(
            "tool_choice".into(),
            encode_tool_choice(&request.tool_choice),
        );
    }

    if let Some(parallel) = request.parallel_tool_calls {
        obj.insert("parallel_tool_calls".into(), json!(parallel));
    }
    if let Some(max_tokens) = request.max_tokens {
        obj.insert("max_output_tokens".into(), json!(max_tokens));
    }
    if let Some(temperature) = request.temperature {
        obj.insert("temperature".into(), json!(temperature));
    }
    if let Some(top_p) = request.top_p {
        obj.insert("top_p".into(), json!(top_p));
    }
    if request.stream {
        obj.insert("stream".into(), json!(true));
    }
    obj.insert("store".into(), json!(false));

    if let Some(reasoning) = &request.reasoning {
        let effort = reasoning
            .effort
            .or_else(|| reasoning.budget_tokens.map(effort_for_budget));
        if effort.is_some() || reasoning.summary.is_some() {
            let mut value = Map::new();
            if let Some(effort) = effort {
                value.insert("effort".into(), json!(effort_name(effort)));
            }
            if let Some(summary) = &reasoning.summary {
                value.insert("summary".into(), json!(summary));
            }
            obj.insert("reasoning".into(), Value::Object(value));
        }
    }

    if let Some(format) = &request.response_format {
        obj.insert(
            "text".into(),
            json!({"format": encode_response_format(format)?}),
        );
    }
    if let Some(user) = &request.metadata_user_id {
        obj.insert("user".into(), json!(user));
    }

    Ok(Value::Object(obj))
}

fn encode_content_parts(
    parts: &[ContentPart],
    assistant: bool,
) -> Result<Vec<Value>, ProtocolError> {
    let mut result = Vec::new();
    for part in parts {
        match part {
            ContentPart::Text { text } => result.push(json!({
                "type": if assistant { "output_text" } else { "input_text" },
                "text": text,
            })),
            ContentPart::Image { source } => result.push(json!({
                "type": "input_image",
                "image_url": image_url(source),
            })),
            _ => {
                return Err(ProtocolError::unsupported(
                    "non-text message content",
                    "responses",
                ))
            }
        }
    }
    Ok(result)
}

fn encode_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Tool { name } => json!({"type": "function", "name": name}),
    }
}

fn encode_response_format(format: &ResponseFormat) -> Result<Value, ProtocolError> {
    match format {
        ResponseFormat::Text => Ok(json!({"type": "text"})),
        ResponseFormat::JsonObject => Ok(json!({"type": "json_object"})),
        ResponseFormat::JsonSchema {
            name,
            schema,
            strict,
        } => Ok(json!({
            "type": "json_schema",
            "name": name.clone().unwrap_or_else(|| "response".to_string()),
            "schema": schema,
            "strict": strict,
        })),
    }
}

pub fn parse_response(value: &Value) -> Result<UnifiedResponse, ProtocolError> {
    let obj = value.as_object().ok_or_else(|| {
        ProtocolError::invalid_response("responses", "response must be a JSON object")
    })?;

    let id = get_str(obj, "id").unwrap_or_default().to_string();
    let model = get_str(obj, "model").unwrap_or_default().to_string();
    let created_at = obj.get("created_at").and_then(Value::as_i64).unwrap_or(0);

    let mut content = Vec::new();
    if let Some(output) = obj.get("output").and_then(Value::as_array) {
        for item in output {
            let item = item.as_object().ok_or_else(|| {
                ProtocolError::invalid_response("responses", "output item must be an object")
            })?;
            match get_str(item, "type").unwrap_or_default() {
                "message" => {
                    content.extend(parse_content_parts(item.get("content"), true)?);
                }
                "reasoning" => {
                    let mut text = String::new();
                    if let Some(summary) = item.get("summary").and_then(Value::as_array) {
                        for part in summary {
                            if let Some(value) =
                                part.as_object().and_then(|part| get_str(part, "text"))
                            {
                                text.push_str(value);
                            }
                        }
                    }
                    if text.is_empty() {
                        if let Some(parts) = item.get("content").and_then(Value::as_array) {
                            for part in parts {
                                if let Some(value) =
                                    part.as_object().and_then(|part| get_str(part, "text"))
                                {
                                    text.push_str(value);
                                }
                            }
                        }
                    }
                    let signature = get_string(item, "encrypted_content");
                    if !text.is_empty() || signature.is_some() {
                        content.push(ContentPart::Reasoning { text, signature });
                    }
                }
                "function_call" => {
                    let id = get_str(item, "call_id")
                        .or_else(|| get_str(item, "id"))
                        .unwrap_or_default()
                        .to_string();
                    let name = get_str(item, "name").unwrap_or_default().to_string();
                    let input = parse_arguments(get_str(item, "arguments").unwrap_or("{}"));
                    content.push(ContentPart::ToolUse { id, name, input });
                }
                _ => {}
            }
        }
    }

    let status = get_str(obj, "status").unwrap_or("completed");
    let has_tool_use = content
        .iter()
        .any(|part| matches!(part, ContentPart::ToolUse { .. }));
    let stop_reason = if status == "incomplete" {
        match obj
            .get("incomplete_details")
            .and_then(Value::as_object)
            .and_then(|details| get_str(details, "reason"))
            .unwrap_or_default()
        {
            "max_output_tokens" => Some(StopReason::MaxTokens),
            "content_filter" => Some(StopReason::ContentFilter),
            _ => Some(StopReason::Other),
        }
    } else if has_tool_use {
        Some(StopReason::ToolUse)
    } else {
        Some(StopReason::EndTurn)
    };

    let usage = obj
        .get("usage")
        .and_then(Value::as_object)
        .map(parse_usage)
        .unwrap_or_default();

    Ok(UnifiedResponse {
        id,
        model,
        created_at,
        content,
        stop_reason,
        stop_sequence: None,
        usage,
    })
}

pub(crate) fn parse_usage(usage: &Map<String, Value>) -> Usage {
    let input_total = get_u64(usage, "input_tokens");
    let output = get_u64(usage, "output_tokens");
    let cached = usage
        .get("input_tokens_details")
        .and_then(Value::as_object)
        .and_then(|details| get_u64(details, "cached_tokens"));
    let input = match (input_total, cached) {
        (Some(total), Some(cached)) => Some(total.saturating_sub(cached)),
        (Some(total), None) => Some(total),
        _ => None,
    };
    let total = get_u64(usage, "total_tokens").or_else(|| match (input_total, output) {
        (Some(input), Some(output)) => Some(input + output),
        _ => None,
    });

    Usage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: total,
        cached_tokens: cached,
        cache_creation_tokens: None,
        reasoning_tokens: usage
            .get("output_tokens_details")
            .and_then(Value::as_object)
            .and_then(|details| get_u64(details, "reasoning_tokens")),
    }
}

pub fn encode_response(response: &UnifiedResponse) -> Result<Value, ProtocolError> {
    let mut output = Vec::new();
    let mut text_buffer = String::new();

    fn flush_text(output: &mut Vec<Value>, text: &mut String) {
        if !text.is_empty() {
            output.push(json!({
                "type": "message",
                "id": format!("msg_{}", Uuid::new_v4()),
                "role": "assistant",
                "status": "completed",
                "content": [{"type": "output_text", "text": std::mem::take(text), "annotations": []}],
            }));
        }
    }

    for part in &response.content {
        match part {
            ContentPart::Text { text } => text_buffer.push_str(text),
            ContentPart::Reasoning { text, signature } => {
                flush_text(&mut output, &mut text_buffer);
                let mut item = Map::new();
                item.insert("type".into(), json!("reasoning"));
                item.insert("id".into(), json!(format!("rs_{}", Uuid::new_v4())));
                item.insert(
                    "summary".into(),
                    json!([{"type": "summary_text", "text": text}]),
                );
                item.insert("status".into(), json!("completed"));
                if let Some(signature) = signature {
                    item.insert("encrypted_content".into(), json!(signature));
                }
                output.push(Value::Object(item));
            }
            ContentPart::RedactedReasoning { data } => {
                flush_text(&mut output, &mut text_buffer);
                output.push(json!({
                    "type": "reasoning",
                    "id": format!("rs_{}", Uuid::new_v4()),
                    "summary": [],
                    "encrypted_content": data,
                    "status": "completed",
                }));
            }
            ContentPart::ToolUse {
                id,
                name,
                input: arguments,
            } => {
                flush_text(&mut output, &mut text_buffer);
                output.push(json!({
                    "type": "function_call",
                    "id": format!("fc_{}", Uuid::new_v4()),
                    "call_id": if id.is_empty() {
                        format!("call_{}", Uuid::new_v4())
                    } else {
                        id.clone()
                    },
                    "name": name,
                    "arguments": serde_json::to_string(arguments)
                        .unwrap_or_else(|_| "{}".to_string()),
                    "status": "completed",
                }));
            }
            _ => {}
        }
    }
    flush_text(&mut output, &mut text_buffer);

    let status = match response.stop_reason {
        Some(StopReason::MaxTokens) => "incomplete",
        Some(StopReason::ContentFilter) => "incomplete",
        _ => "completed",
    };
    let incomplete_reason = match response.stop_reason {
        Some(StopReason::MaxTokens) => Some("max_output_tokens"),
        Some(StopReason::ContentFilter) => Some("content_filter"),
        _ => None,
    };

    let mut usage = Map::new();
    let input_tokens = response.usage.input_tokens.map(|input| {
        input
            + response.usage.cached_tokens.unwrap_or(0)
            + response.usage.cache_creation_tokens.unwrap_or(0)
    });
    if let Some(value) = input_tokens {
        usage.insert("input_tokens".into(), json!(value));
    }
    if let Some(value) = response.usage.output_tokens {
        usage.insert("output_tokens".into(), json!(value));
    }
    if let Some(value) = response.usage.total_tokens.or_else(|| {
        input_tokens
            .zip(response.usage.output_tokens)
            .map(|(input, output)| input + output)
    }) {
        usage.insert("total_tokens".into(), json!(value));
    }
    if let Some(cached) = response.usage.cached_tokens {
        usage.insert(
            "input_tokens_details".into(),
            json!({"cached_tokens": cached}),
        );
    }
    if let Some(reasoning) = response.usage.reasoning_tokens {
        usage.insert(
            "output_tokens_details".into(),
            json!({"reasoning_tokens": reasoning}),
        );
    }

    let mut obj = Map::new();
    obj.insert("id".into(), json!(response.id));
    obj.insert("object".into(), json!("response"));
    obj.insert("created_at".into(), json!(response.created_at));
    obj.insert("status".into(), json!(status));
    obj.insert("model".into(), json!(response.model));
    obj.insert("output".into(), Value::Array(output));
    obj.insert("usage".into(), Value::Object(usage));
    if let Some(reason) = incomplete_reason {
        obj.insert("incomplete_details".into(), json!({"reason": reason}));
    }

    Ok(Value::Object(obj))
}
