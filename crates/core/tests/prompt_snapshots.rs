//! TEST_PLAN S11 `prompt_snapshot`: the rendered requests for fixed inputs, snapshotted with
//! `insta` under the prompt's version. Changing a prompt's wording, its reply schema or its
//! settings without bumping the version (e.g. `answer.v2` → `answer.v3`) fails here; a bumped
//! version gets a new snapshot that is reviewed with `cargo insta review`.

use survey_core::engine::{answer, critic, draft};
use survey_core::llm::StructuredRequest;
use survey_core::model::{
    ChoiceOption, NumericRange, Question, QuestionBody, QuestionOrigin, QuestionType, ResearchType,
    ReviewStatus, Scale,
};

fn render(r: &StructuredRequest) -> String {
    format!(
        "model: {}\ntemperature: {}\nmax_output_tokens: {}\n\n--- system ---\n{}\n--- prompt ---\n{}\n--- schema ---\n{}\n",
        r.model,
        r.temperature,
        r.max_output_tokens,
        r.system.trim_end(),
        r.prompt.trim_end(),
        serde_json::to_string_pretty(&r.schema).unwrap()
    )
}

fn body(t: QuestionType, text: &str) -> QuestionBody {
    QuestionBody {
        text: text.into(),
        question_type: t,
        options: if matches!(t, QuestionType::SingleChoice | QuestionType::MultiChoice) {
            ["Battery life", "Camera", "Price drop", "None of these"]
                .iter()
                .enumerate()
                .map(|(i, l)| ChoiceOption {
                    code: ((b'A' + i as u8) as char).to_string(),
                    label: l.to_string(),
                })
                .collect()
        } else {
            vec![]
        },
        randomize: true,
        max_choices: (t == QuestionType::MultiChoice).then_some(2),
        scale: (t == QuestionType::Likert).then(|| Scale {
            min: 1,
            max: 7,
            min_label: "Not at all likely".into(),
            max_label: "Extremely likely".into(),
        }),
        numeric: (t == QuestionType::Numeric).then(|| NumericRange {
            min: 0.0,
            max: 2500.0,
            unit: "CAD".into(),
        }),
    }
}

fn question(id: i64, t: QuestionType, text: &str) -> Question {
    Question {
        id,
        code: format!("Q{id}"),
        order_index: id as u32,
        body: body(t, text),
        is_active: true,
        origin: QuestionOrigin::Ai,
        review_status: ReviewStatus::Accepted,
        objective: Some("Purchase intent".into()),
        rationale: Some("Measures intent.".into()),
        critique: None,
    }
}

#[test]
fn survey_draft_prompt() {
    let brief = draft::DraftBrief {
        research_type: ResearchType::MarketResponse,
        product_category: Some("mobile_phone".into()),
        countries: vec!["Canada".into()],
        title: "Smartphone upgrade intent".into(),
        objective: "Understand what drives Canadians to replace their smartphone.".into(),
    };
    insta::assert_snapshot!(
        draft::PROMPT_VERSION,
        render(&draft::build_request("pro", &brief))
    );
    // Suggest more reuses the draft's system prompt and version.
    insta::assert_snapshot!(
        format!("{}.suggest_more", draft::PROMPT_VERSION),
        render(&draft::build_more_request(
            "pro",
            &brief,
            &["Which brand is your current phone?".into()]
        ))
    );
}

#[test]
fn answer_prompt() {
    let qs = [
        question(
            1,
            QuestionType::SingleChoice,
            "What would most likely make you replace your phone?",
        ),
        question(
            2,
            QuestionType::MultiChoice,
            "Which of these matter when choosing a phone?",
        ),
        question(
            3,
            QuestionType::Likert,
            "How likely are you to buy a phone in the next 12 months?",
        ),
        question(
            4,
            QuestionType::Numeric,
            "What is the most you would pay for your next phone?",
        ),
        question(
            5,
            QuestionType::OpenEnded,
            "What, if anything, puts you off upgrading?",
        ),
    ];
    let pairs: Vec<_> = qs
        .iter()
        .map(|q| (q, answer::shown_order(q, 7, 1)))
        .collect();
    let persona = "Name: Marie Tremblay\nAge: 58\nGender: Female\nLives in: Quebec, Canada\n";
    insta::assert_snapshot!(
        answer::PROMPT_VERSION,
        render(&answer::build_request(
            "flash",
            "Thanks for taking part. There are no right or wrong answers.",
            &pairs,
            persona
        ))
    );
}

#[test]
fn critic_prompt() {
    insta::assert_snapshot!(
        critic::PROMPT_VERSION,
        render(&critic::build_request(
            "pro",
            &body(
                QuestionType::MultiChoice,
                "Which of these would make you upgrade, and how soon?"
            )
        ))
    );
}

/// A bumped version leaves the old snapshot behind; it must be deleted so the directory
/// always shows the prompts the app sends today.
#[test]
fn only_current_prompt_versions_have_snapshots() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let mut found: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".snap"))
        .collect();
    found.sort();
    let mut expected: Vec<String> = [
        draft::PROMPT_VERSION.to_string(),
        format!("{}.suggest_more", draft::PROMPT_VERSION),
        answer::PROMPT_VERSION.to_string(),
        critic::PROMPT_VERSION.to_string(),
    ]
    .iter()
    .map(|v| format!("prompt_snapshots__{v}.snap"))
    .collect();
    expected.sort();
    assert_eq!(found, expected, "delete snapshots of old prompt versions");
}
