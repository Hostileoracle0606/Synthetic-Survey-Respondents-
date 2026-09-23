//! Persona enrichment: prompt, reply format per product category, and reply checks.
//! Gemini sees demographics, product category and screening criteria only — never the
//! research objective or the questions (docs/DATA_FLOW.md §4).

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::llm::{LlmError, StructuredRequest};
use crate::sampling::Skeleton;

pub const PROMPT_VERSION: &str = "persona.v1";
const SYSTEM: &str = include_str!("../../prompts/persona.v1.md");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnrichedPersona {
    pub ordinal: u32,
    pub name: String,
    pub summary: String,
    pub values: Vec<String>,
    pub habits: String,
    pub media_habits: String,
    pub brand_loyalties: String,
    pub category_attitudes: String,
    pub price_sensitivity: u8,
    pub biases: Vec<String>,
    pub category_profile: Map<String, Value>,
    pub passes_screen: bool,
    pub screen_reason: String,
}

fn enum_field(values: &[&str]) -> Value {
    json!({ "type": "string", "enum": values })
}

fn int_field(min: i64, max: i64) -> Value {
    json!({ "type": "integer", "minimum": min, "maximum": max })
}

/// Category-specific facts. `main_field` is what Step 2's "Top trigger" card counts.
pub fn category_profile_schema(category: Option<&str>) -> (Value, &'static str) {
    let (props, main): (Vec<(&str, Value)>, &str) = match category {
        Some("mobile_phone") => (
            vec![
                ("current_device_age_years", int_field(0, 10)),
                ("current_brand", json!({ "type": "string" })),
                (
                    "upgrade_trigger",
                    enum_field(&[
                        "battery",
                        "camera",
                        "damage",
                        "storage",
                        "performance",
                        "new_features",
                        "deal_or_contract",
                        "none",
                    ]),
                ),
                (
                    "plans_to_replace_within_12_months",
                    json!({ "type": "boolean" }),
                ),
            ],
            "upgrade_trigger",
        ),
        Some("tv") => (
            vec![
                ("current_tv_age_years", int_field(0, 20)),
                ("screen_size_inches", int_field(19, 100)),
                (
                    "main_use",
                    enum_field(&["streaming", "live_tv", "gaming", "sports", "mixed"]),
                ),
                (
                    "purchase_trigger",
                    enum_field(&[
                        "broken",
                        "bigger_screen",
                        "picture_quality",
                        "smart_features",
                        "moving_home",
                        "none",
                    ]),
                ),
            ],
            "purchase_trigger",
        ),
        Some("refrigerator") => (
            vec![
                ("current_age_years", int_field(0, 30)),
                ("household_size", int_field(1, 10)),
                (
                    "purchase_trigger",
                    enum_field(&[
                        "broken",
                        "energy_bills",
                        "capacity",
                        "renovation",
                        "moving_home",
                        "none",
                    ]),
                ),
            ],
            "purchase_trigger",
        ),
        Some("washing_machine") => (
            vec![
                ("current_age_years", int_field(0, 30)),
                ("loads_per_week", int_field(0, 30)),
                (
                    "purchase_trigger",
                    enum_field(&[
                        "broken",
                        "energy_or_water_bills",
                        "capacity",
                        "noise",
                        "moving_home",
                        "none",
                    ]),
                ),
            ],
            "purchase_trigger",
        ),
        _ => (
            vec![("category_involvement", int_field(1, 5))],
            "category_involvement",
        ),
    };
    let required: Vec<&str> = props.iter().map(|(k, _)| *k).collect();
    let properties: Map<String, Value> =
        props.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    (
        json!({ "type": "object", "properties": properties, "required": required }),
        main,
    )
}

pub fn reply_schema(category: Option<&str>) -> Value {
    let (profile, _) = category_profile_schema(category);
    let string = json!({ "type": "string" });
    let list = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "properties": {
            "personas": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "ordinal": { "type": "integer" },
                        "name": string, "summary": string, "values": list, "habits": string,
                        "media_habits": string, "brand_loyalties": string, "category_attitudes": string,
                        "price_sensitivity": int_field(1, 5),
                        "biases": list,
                        "category_profile": profile,
                        "passes_screen": { "type": "boolean" },
                        "screen_reason": string
                    },
                    "required": ["ordinal", "name", "summary", "values", "habits", "media_habits", "brand_loyalties",
                                 "category_attitudes", "price_sensitivity", "biases", "category_profile",
                                 "passes_screen", "screen_reason"]
                }
            }
        },
        "required": ["personas"]
    })
}

pub struct BatchInput<'a> {
    pub skeletons: &'a [Skeleton],
    pub category: Option<&'a str>,
    pub screening: &'a str,
    /// Short summaries already written for similar people, to avoid near-duplicates.
    pub already_written: &'a [String],
}

pub fn build_request(model: &str, input: &BatchInput<'_>) -> StructuredRequest {
    let people: Vec<Value> = input
        .skeletons
        .iter()
        .map(|s| {
            json!({
                "ordinal": s.ordinal, "country": s.country, "age": s.age, "gender": s.gender,
                "region": if s.region.is_empty() { Value::Null } else { json!(s.region) },
                "household_income": s.income, "occupation_group": s.occupation
            })
        })
        .collect();
    let category = input
        .category
        .map(|c| c.replace('_', " "))
        .unwrap_or_else(|| "general consumer products".into());
    let screening = if input.screening.trim().is_empty() {
        "None."
    } else {
        input.screening.trim()
    };
    let mut prompt = format!(
        "Product category: {category}\nScreening criteria: {screening}\n\nPeople (write one profile each):\n{}\n",
        serde_json::to_string_pretty(&people).unwrap_or_default()
    );
    if !input.already_written.is_empty() {
        prompt.push_str("\nAlready written for similar people (do not repeat):\n");
        for s in input.already_written {
            prompt.push_str("- ");
            prompt.push_str(s);
            prompt.push('\n');
        }
    }
    StructuredRequest {
        model: model.to_string(),
        system: SYSTEM.to_string(),
        prompt,
        schema: reply_schema(input.category),
        temperature: 1.0,
        max_output_tokens: 1_200 * input.skeletons.len() as u32 + 2_000,
    }
}

/// Checks a reply against the batch: every ordinal exactly once, fields in range, and the
/// category profile matching its schema. Any problem is a `SchemaViolation` (retried by the job).
pub fn parse_reply(
    reply: &Value,
    expected: &[u32],
    category: Option<&str>,
) -> Result<Vec<EnrichedPersona>, LlmError> {
    let bad = |m: String| LlmError::SchemaViolation(m);
    let items = reply["personas"]
        .as_array()
        .ok_or_else(|| bad("missing \"personas\" list".into()))?;
    let (profile_schema, _) = category_profile_schema(category);
    let mut out = Vec::new();
    for item in items {
        let p: EnrichedPersona =
            serde_json::from_value(item.clone()).map_err(|e| bad(format!("persona: {e}")))?;
        if !expected.contains(&p.ordinal) {
            return Err(bad(format!("unexpected ordinal {}", p.ordinal)));
        }
        if out.iter().any(|q: &EnrichedPersona| q.ordinal == p.ordinal) {
            return Err(bad(format!("ordinal {} returned twice", p.ordinal)));
        }
        if p.name.trim().is_empty() || p.summary.split_whitespace().count() < 30 {
            return Err(bad(format!(
                "persona {}: name or summary too short",
                p.ordinal
            )));
        }
        if !(1..=5).contains(&p.price_sensitivity) {
            return Err(bad(format!(
                "persona {}: price_sensitivity {} out of 1–5",
                p.ordinal, p.price_sensitivity
            )));
        }
        if p.biases.is_empty() || p.biases.len() > 6 || p.values.is_empty() {
            return Err(bad(format!(
                "persona {}: biases or values missing",
                p.ordinal
            )));
        }
        check_object(&Value::Object(p.category_profile.clone()), &profile_schema)
            .map_err(|e| bad(format!("persona {} category_profile: {e}", p.ordinal)))?;
        out.push(p);
    }
    if out.len() != expected.len() {
        return Err(bad(format!(
            "expected {} personas, got {}",
            expected.len(),
            out.len()
        )));
    }
    out.sort_by_key(|p| p.ordinal);
    Ok(out)
}

/// Minimal check for the flat object schemas this module writes.
fn check_object(v: &Value, schema: &Value) -> Result<(), String> {
    let obj = v.as_object().ok_or("not an object")?;
    for key in schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let val = obj.get(key).ok_or_else(|| format!("missing {key}"))?;
        let s = &schema["properties"][key];
        match s["type"].as_str() {
            Some("integer") => {
                let n = val
                    .as_i64()
                    .ok_or_else(|| format!("{key} is not an integer"))?;
                if s["minimum"].as_i64().is_some_and(|m| n < m)
                    || s["maximum"].as_i64().is_some_and(|m| n > m)
                {
                    return Err(format!("{key}={n} out of range"));
                }
            }
            Some("boolean") if !val.is_boolean() => return Err(format!("{key} is not true/false")),
            Some("string") => {
                let t = val.as_str().ok_or_else(|| format!("{key} is not text"))?;
                if let Some(allowed) = s["enum"].as_array() {
                    if !allowed.iter().any(|a| a.as_str() == Some(t)) {
                        return Err(format!("{key}=\"{t}\" is not an allowed value"));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Test helper: a valid reply for a batch, used with `ScriptedLlm`. `fail_screen` marks
/// ordinals that should fail screening.
pub fn sample_reply(skeletons: &[Skeleton], category: Option<&str>, fail_screen: &[u32]) -> Value {
    let (schema, _) = category_profile_schema(category);
    let mut profile = Map::new();
    for (k, s) in schema["properties"].as_object().unwrap() {
        let v = match s["type"].as_str() {
            Some("integer") => json!(s["minimum"].as_i64().unwrap_or(1)),
            Some("boolean") => json!(true),
            _ => s["enum"].get(0).cloned().unwrap_or(json!("Brand A")),
        };
        profile.insert(k.clone(), v);
    }
    let personas: Vec<Value> = skeletons
        .iter()
        .map(|s| {
            json!({
                "ordinal": s.ordinal,
                "name": format!("Person {}", s.ordinal),
                "summary": format!("Person {} is a {}-year-old {} from the {} region who works in {}. They plan purchases carefully, compare prices online, read reviews before buying, and prefer products that last several years without trouble.", s.ordinal, s.age, s.gender.to_lowercase(), s.region, s.occupation),
                "values": ["reliability", "family"],
                "habits": "Shops online on weekends.",
                "media_habits": "News apps and podcasts.",
                "brand_loyalties": "None in particular.",
                "category_attitudes": "Practical.",
                "price_sensitivity": 3,
                "biases": ["status quo", "price anchoring"],
                "category_profile": profile,
                "passes_screen": !fail_screen.contains(&s.ordinal),
                "screen_reason": "Test reply."
            })
        })
        .collect();
    json!({ "personas": personas })
}

/// Test helper: a valid reply for the given ordinals (used with `ScriptedLlm`, which only
/// sees the prompt). `fail_screen` marks ordinals that should fail screening.
pub fn reply_for_ordinals(ordinals: &[u32], category: Option<&str>, fail_screen: &[u32]) -> Value {
    let skeletons: Vec<Skeleton> = ordinals
        .iter()
        .map(|&o| Skeleton {
            ordinal: o,
            country: "US".into(),
            age: 40,
            age_band: "30–44".into(),
            gender: "Female".into(),
            region: "South".into(),
            income: "$50k–$100k".into(),
            occupation: "Service".into(),
            quota_cell: String::new(),
        })
        .collect();
    sample_reply(&skeletons, category, fail_screen)
}

/// Ordinals listed in a persona prompt, in order (used by tests and the job's split logic).
pub fn ordinals_in_prompt(prompt: &str) -> Vec<u32> {
    prompt
        .match_indices("\"ordinal\": ")
        .filter_map(|(i, m)| {
            prompt[i + m.len()..]
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CohortConfig, QuotaGroup, QuotaRow};
    use crate::sampling::Sampler;

    fn skeletons(n: u32) -> Vec<Skeleton> {
        let cfg = CohortConfig {
            size: n,
            seed: 1,
            quotas: vec![QuotaGroup {
                key: "age".into(),
                label: "Age".into(),
                rows: vec![
                    QuotaRow {
                        label: "18–29".into(),
                        percent: 50,
                    },
                    QuotaRow {
                        label: "60+".into(),
                        percent: 50,
                    },
                ],
            }],
            screening: String::new(),
        };
        Sampler::new(&cfg, &["US".into()]).unwrap().draw().unwrap()
    }

    /// Gemini supports a subset of JSON Schema: no anyOf/oneOf/allOf/$ref, and schemas that
    /// are "very large or deeply nested" may be rejected. Returns the object nesting depth.
    fn assert_gemini_subset(v: &Value) -> usize {
        match v {
            Value::Object(o) => {
                for bad in ["anyOf", "oneOf", "allOf", "$ref", "patternProperties"] {
                    assert!(!o.contains_key(bad), "schema uses unsupported {bad}");
                }
                let inner = o.values().map(assert_gemini_subset).max().unwrap_or(0);
                inner + usize::from(o.get("type").and_then(Value::as_str) == Some("object"))
            }
            Value::Array(a) => a.iter().map(assert_gemini_subset).max().unwrap_or(0),
            _ => 0,
        }
    }

    #[test]
    fn every_category_schema_fits_gemini() {
        for c in [
            Some("mobile_phone"),
            Some("tv"),
            Some("refrigerator"),
            Some("washing_machine"),
            None,
        ] {
            let depth = assert_gemini_subset(&reply_schema(c));
            assert!(depth <= 4, "object nesting {depth} is too deep");
        }
    }

    #[test]
    fn request_never_contains_the_research_objective_and_lists_every_person() {
        let sk = skeletons(3);
        let req = build_request(
            "m",
            &BatchInput {
                skeletons: &sk,
                category: Some("mobile_phone"),
                screening: "Owns a smartphone.",
                already_written: &["A retired teacher who...".into()],
            },
        );
        for s in &sk {
            assert!(req.prompt.contains(&format!("\"ordinal\": {}", s.ordinal)));
        }
        assert!(req.prompt.contains("Owns a smartphone."));
        assert!(req.prompt.contains("do not repeat"));
        assert!(!req.prompt.to_lowercase().contains("objective"));
    }

    #[test]
    fn valid_reply_parses_and_bad_replies_are_rejected() {
        let sk = skeletons(4);
        let ords: Vec<u32> = sk.iter().map(|s| s.ordinal).collect();
        let good = sample_reply(&sk, Some("mobile_phone"), &[2]);
        let parsed = parse_reply(&good, &ords, Some("mobile_phone")).unwrap();
        assert_eq!(parsed.len(), 4);
        assert!(!parsed[1].passes_screen);

        let mut missing = good.clone();
        missing["personas"].as_array_mut().unwrap().pop();
        assert!(parse_reply(&missing, &ords, Some("mobile_phone")).is_err());

        let mut bad_enum = good.clone();
        bad_enum["personas"][0]["category_profile"]["upgrade_trigger"] = json!("vibes");
        assert!(parse_reply(&bad_enum, &ords, Some("mobile_phone")).is_err());

        let mut bad_range = good;
        bad_range["personas"][0]["price_sensitivity"] = json!(9);
        assert!(parse_reply(&bad_range, &ords, Some("mobile_phone")).is_err());
    }
}
