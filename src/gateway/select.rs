use std::collections::HashMap;

use rand::Rng;

use crate::{
    models::{
        channel::{self, Channel, ProtocolConfig},
        now_ms,
    },
    protocol::Protocol,
};

#[derive(Debug, Clone)]
pub struct Candidate {
    pub channel: Channel,
    pub upstream_model: String,
    pub upstream_kind: String,
    pub upstream_config: ProtocolConfig,
}

pub fn candidates(channels: Vec<Channel>, model: &str, inbound: Protocol) -> Vec<Candidate> {
    let now = now_ms();
    let inbound_kind = channel::kind_for_protocol(inbound.as_str());
    let mut grouped: HashMap<i64, Vec<(Candidate, i64)>> = HashMap::new();

    for channel in channels {
        if channel.enabled == 0 {
            continue;
        }
        if channel
            .disabled_until
            .map(|disabled_until| disabled_until > now)
            .unwrap_or(false)
        {
            continue;
        }

        let Some(route) = channel::resolve_route(&channel, inbound_kind) else {
            continue;
        };
        if !route
            .config
            .models
            .iter()
            .any(|item| item == "*" || item == model)
        {
            continue;
        }

        let upstream_model = route
            .config
            .model_mapping
            .get(model)
            .cloned()
            .unwrap_or_else(|| model.to_string());

        let weight = channel.weight.max(1);
        grouped.entry(channel.priority).or_default().push((
            Candidate {
                channel,
                upstream_model,
                upstream_kind: route.upstream_kind,
                upstream_config: route.config,
            },
            weight,
        ));
    }

    let mut priorities: Vec<i64> = grouped.keys().copied().collect();
    priorities.sort_unstable_by(|left, right| right.cmp(left));

    let mut result = Vec::new();
    for priority in priorities {
        let Some(mut group) = grouped.remove(&priority) else {
            continue;
        };
        while !group.is_empty() {
            let total: i64 = group.iter().map(|(_, weight)| *weight).sum();
            let mut pick = rand::thread_rng().gen_range(0..total.max(1));
            let mut index = group.len() - 1;
            for (position, (_, weight)) in group.iter().enumerate() {
                if pick < *weight {
                    index = position;
                    break;
                }
                pick -= *weight;
            }
            let (candidate, _) = group.remove(index);
            result.push(candidate);
        }
    }

    result
}
