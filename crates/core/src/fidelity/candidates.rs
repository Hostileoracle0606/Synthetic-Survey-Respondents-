//! Candidate benchmark questions from Gemini (decision D1): questions only. The output is a
//! draft pack whose rules are empty and must be written by a person before it can be used.

use serde_json::{json, Value};

use super::{Pack, PackQuestion, Rule, ATTRIBUTES, PACK_FORMAT};
use crate::db::surveys;
use crate::llm::StructuredRequest;
use crate::model::{ChoiceOption, NumericRange, QuestionBody, QuestionType, Scale};

pub const PROMPT_VERSION: &str = "benchmark_candidates.v1";
const SYSTEM: &str = include_str!("../../prompts/benchmark_candidates.v1.md");

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "code": { "type": "string" },
                        "attribute": { "type": "string" },
                        "text": { "type": "string" },
                        "type": { "type": "string", "enum": ["single_choice", "likert", "numeric"] },
                        "options": { "type": "array", "items": { "type": "string" } },
                        "scale_min_label": { "type": "string" },
                        "scale_max_label": { "type": "string" },
                        "number_min": { "type": "number" },
                        "number_max": { "type": "number" },
                        "unit": { "type": "string" }
                    },
                    "required": ["code", "attribute", "text", "type"]
                }
            }
        },
        "required": ["questions"]
    })
}

pub fn request(model: &str, category: &str, count: usize) -> StructuredRequest {
    StructuredRequest {
        model: model.to_string(),
        system: SYSTEM.to_string(),
        prompt: format!(
            "Product category: {}\nMarket: Canada\nPersona attributes you may tie questions to: {}\n\nWrite {count} candidate questions.\n",
            category.replace('_', " "),
            ATTRIBUTES.join(", ")
        ),
        schema: schema(),
        temperature: 0.8,
        max_output_tokens: 16_384,
    }
}

/// Turns Gemini's candidates into a draft pack. Rules are left empty on purpose.
pub fn draft_pack(reply: &Value, category: &str, version: &str) -> Pack {
    let mut questions = Vec::new();
    for (i, c) in reply["questions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let s = |k: &str| c[k].as_str().unwrap_or_default().trim().to_string();
        let Some(t) = QuestionType::from_db(&s("type")) else {
            continue;
        };
        let body = QuestionBody {
            text: s("text"),
            question_type: t,
            options: c["options"]
                .as_array()
                .map(|o| {
                    o.iter()
                        .filter_map(|x| x.as_str())
                        .enumerate()
                        .map(|(j, l)| ChoiceOption {
                            code: surveys::option_code(j),
                            label: l.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            randomize: t == QuestionType::SingleChoice,
            max_choices: None,
            scale: (t == QuestionType::Likert).then(|| Scale {
                min: 1,
                max: 5,
                min_label: s("scale_min_label"),
                max_label: s("scale_max_label"),
            }),
            numeric: (t == QuestionType::Numeric).then(|| NumericRange {
                min: c["number_min"].as_f64().unwrap_or(0.0),
                max: c["number_max"].as_f64().unwrap_or(100.0),
                unit: s("unit"),
            }),
        };
        let Ok(body) = surveys::normalise(body) else {
            continue;
        };
        questions.push(PackQuestion {
            code: format!("F{:02}", i + 1),
            question: body,
            source: "gemini-candidate".into(),
            attribute: s("attribute"),
            rule: Rule {
                cases: Vec::new(),
                otherwise: Default::default(),
                bins: Vec::new(),
            },
            rule_author: String::new(),
        });
    }
    Pack {
        format: PACK_FORMAT.into(),
        version: version.into(),
        status: "draft".into(),
        ground_truth: "planted".into(),
        category: Some(category.into()),
        reviewed_by: None,
        frozen_at: None,
        notes: Some(format!(
            "Candidates from {PROMPT_VERSION}. Keep about 20, write each rule by hand (cases, otherwise, bins for numbers) and set rule_author; then set status \"frozen\", reviewed_by and frozen_at."
        )),
        questions,
    }
}
