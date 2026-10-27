use std::collections::HashMap;

use rand::Rng;

use crate::models::{channel::Channel, now_ms};

#[derive(Debug, Clone)]
pub struct Candidate {
    pub channel: Channel,
    pub upstream_model: String,
}

pub fn candidates(channels: Vec<Channel>, model: &str) -> Vec<Candidate> {
    let now = now_ms();
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

        let models: Vec<String> = serde_json::from_str(&channel.models).unwrap_or_default();
        if !models.iter().any(|item| item == "*" || item == model) {
            continue;
        }

        let mapping: HashMap<String, String> =
            serde_json::from_str(&channel.model_mapping).unwrap_or_default();
        let upstream_model = mapping
            .get(model)
            .cloned()
            .unwrap_or_else(|| model.to_string());

        let weight = channel.weight.max(1);
        grouped.entry(channel.priority).or_default().push((
            Candidate {
                channel,
                upstream_model,
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
