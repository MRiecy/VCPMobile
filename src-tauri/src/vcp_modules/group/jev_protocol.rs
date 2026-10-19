//! The decision wire format. Group scheduling owns candidate order and thresholds.

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::marker::PhantomData;

#[derive(Debug, Clone, Serialize)]
pub struct JevDecisionRequest {
    pub state: Value,
    pub questions: BTreeMap<String, JevQuestion>,
}

/// Only the question kinds consumed by group arbitration are exposed here.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum JevQuestion {
    Choice {
        instructions: String,
        criteria: BTreeMap<String, String>,
    },
    Noul {
        instructions: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct JevDecisionResponse {
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub answers: BTreeMap<String, JevAnswer>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JevAnswer {
    #[serde(default, deserialize_with = "deserialize_optional_unique_map")]
    pub probabilities: Option<BTreeMap<String, f64>>,
    #[serde(default)]
    pub noul: Option<f64>,
    #[serde(default)]
    pub choice: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

struct UniqueMap<T>(BTreeMap<String, T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for UniqueMap<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for MapVisitor<T> {
            type Value = UniqueMap<T>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an object with unique keys")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, T>()? {
                    if values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate decision key"));
                    }
                }
                Ok(UniqueMap(values))
            }
        }
        deserializer.deserialize_map(MapVisitor(PhantomData))
    }
}

fn deserialize_unique_map<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, T>, D::Error> {
    UniqueMap::deserialize(deserializer).map(|map| map.0)
}

fn deserialize_optional_unique_map<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<BTreeMap<String, T>>, D::Error> {
    Option::<UniqueMap<T>>::deserialize(deserializer).map(|map| map.map(|map| map.0))
}

impl JevDecisionRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !matches!(
            self.state,
            Value::String(_) | Value::Object(_) | Value::Array(_)
        ) {
            return Err("Jev state 必须是文本、对象或数组。");
        }
        if self.questions.is_empty() {
            return Err("Jev 请求必须包含至少一个问题。");
        }
        for (name, question) in &self.questions {
            if name.trim().is_empty() {
                return Err("Jev 问题名称不能为空。");
            }
            let instructions = match question {
                JevQuestion::Choice {
                    instructions,
                    criteria,
                } => {
                    if !(2..=255).contains(&criteria.len()) {
                        return Err("Jev choice 必须包含 2 至 255 个选项。");
                    }
                    if criteria
                        .iter()
                        .any(|(key, value)| key.trim().is_empty() || value.trim().is_empty())
                    {
                        return Err("Jev choice 选项名称和说明不能为空。");
                    }
                    instructions
                }
                JevQuestion::Noul { instructions } => instructions,
            };
            if instructions.trim().is_empty() {
                return Err("Jev 问题说明不能为空。");
            }
        }
        Ok(())
    }
}

fn probability(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

impl JevDecisionResponse {
    /// Missing answers are protocol failures, never an implicit zero/stop decision.
    /// Optional diagnostic fields do not decide which member gets to speak.
    pub fn validate_for(&self, request: &JevDecisionRequest) -> Result<(), &'static str> {
        for (name, question) in &request.questions {
            let answer = self
                .answers
                .get(name)
                .ok_or("Jev 响应缺少请求的问题答案。")?;
            if answer.confidence.is_some_and(|value| !probability(value)) {
                return Err("Jev 响应置信度必须是 0 至 1 的有限数值。");
            }
            match question {
                JevQuestion::Choice { criteria, .. } => {
                    let probabilities = answer
                        .probabilities
                        .as_ref()
                        .ok_or("Jev choice 响应缺少 probabilities 对象。")?;
                    if probabilities.len() != criteria.len()
                        || criteria.keys().any(|key| !probabilities.contains_key(key))
                    {
                        return Err("Jev choice 响应的选项与本次请求不一致。");
                    }
                    if probabilities.values().any(|value| !probability(*value)) {
                        return Err("Jev choice 概率必须是 0 至 1 的有限数值。");
                    }
                    if answer
                        .choice
                        .as_ref()
                        .is_some_and(|choice| !criteria.contains_key(choice))
                    {
                        return Err("Jev choice 响应引用了本次请求之外的选项。");
                    }
                }
                JevQuestion::Noul { .. } => {
                    let value = answer.noul.ok_or("Jev noul 响应缺少概率数值。")?;
                    if !probability(value) {
                        return Err("Jev noul 概率必须是 0 至 1 的有限数值。");
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(super) fn request() -> JevDecisionRequest {
        JevDecisionRequest {
            state: json!({"recentMessages": []}),
            questions: BTreeMap::from([
                (
                    "speaker_routing".into(),
                    JevQuestion::Choice {
                        instructions: "谁应该发言？".into(),
                        criteria: BTreeMap::from([
                            ("agent_0".into(), "成员".into()),
                            ("end_conversation".into(), "结束".into()),
                        ]),
                    },
                ),
                (
                    "continue_discussion".into(),
                    JevQuestion::Noul {
                        instructions: "是否继续？".into(),
                    },
                ),
            ]),
        }
    }

    fn response() -> Value {
        json!({"id":"provider-request", "answers":{
            "speaker_routing":{"probabilities":{"agent_0":0.0,"end_conversation":1.0},"choice":"end_conversation","confidence":1.0},
            "continue_discussion":{"noul":0.0}
        }})
    }

    #[test]
    fn valid_zero_is_distinct_from_absent_null_or_coerced_probability() {
        let request = request();
        let valid: JevDecisionResponse = serde_json::from_value(response()).unwrap();
        assert!(valid.validate_for(&request).is_ok());
        for value in [
            Value::Null,
            json!(true),
            json!("0"),
            json!(-0.1),
            json!(1.1),
        ] {
            let mut invalid = response();
            invalid["answers"]["continue_discussion"]["noul"] = value;
            let rejected = serde_json::from_value::<JevDecisionResponse>(invalid)
                .map(|response| response.validate_for(&request).is_err())
                .unwrap_or(true);
            assert!(rejected);
        }
        let empty: JevDecisionResponse = serde_json::from_value(json!({"answers":{}})).unwrap();
        assert!(empty.validate_for(&request).is_err());
        assert!(serde_json::from_value::<JevDecisionResponse>(json!({"answers":[]})).is_err());
    }

    #[test]
    fn response_must_cover_exactly_the_requested_choice_options() {
        let request = request();
        for probabilities in [
            json!({"agent_0":0.4}),
            json!({"agent_0":0.4,"different":0.6}),
            json!({"agent_0":0.4,"end_conversation":0.6,"extra":0.0}),
            json!({"agent_0":null,"end_conversation":0.6}),
        ] {
            let mut invalid = response();
            invalid["answers"]["speaker_routing"]["probabilities"] = probabilities;
            let rejected = serde_json::from_value::<JevDecisionResponse>(invalid)
                .map(|response| response.validate_for(&request).is_err())
                .unwrap_or(true);
            assert!(rejected);
        }
    }

    #[test]
    fn duplicate_question_or_candidate_keys_are_not_silently_overwritten() {
        for raw in [
            r#"{"answers":{"continue_discussion":{"noul":0.0},"continue_discussion":{"noul":1.0}}}"#,
            r#"{"answers":{"speaker_routing":{"probabilities":{"agent_0":0.1,"agent_0":0.9,"end_conversation":0.1}}}}"#,
        ] {
            assert!(serde_json::from_str::<JevDecisionResponse>(raw).is_err());
        }
    }
}
