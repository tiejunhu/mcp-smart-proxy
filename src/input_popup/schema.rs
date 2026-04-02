use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const OTHER_LABEL: &str = "Other";
const OTHER_DESCRIPTION: &str = "Enter a custom answer instead of choosing a fixed option.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PopupInputRequest {
    pub questions: Vec<PopupQuestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PopupQuestion {
    pub id: String,
    pub question: String,
    pub options: Vec<PopupOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PopupOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PopupInputResponse {
    pub answers: BTreeMap<String, PopupAnswerValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PopupAnswerValue {
    pub answers: Vec<String>,
}

impl PopupInputRequest {
    pub fn normalized(mut self) -> Self {
        for question in &mut self.questions {
            question.options.retain(|option| !is_other_option(option));
            question.options.push(PopupOption {
                label: OTHER_LABEL.to_string(),
                description: OTHER_DESCRIPTION.to_string(),
            });
        }

        self
    }
}

impl PopupInputResponse {
    pub fn cancelled() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub fn from_answers(answers: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            answers: answers
                .into_iter()
                .map(|(question_id, answer)| {
                    (
                        question_id,
                        PopupAnswerValue {
                            answers: vec![answer],
                        },
                    )
                })
                .collect(),
        }
    }
}

pub fn is_other_label(label: &str) -> bool {
    label.trim().eq_ignore_ascii_case(OTHER_LABEL)
}

pub fn is_other_option(option: &PopupOption) -> bool {
    is_other_label(&option.label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_request_always_appends_other() {
        let request = PopupInputRequest {
            questions: vec![PopupQuestion {
                id: "choice".to_string(),
                question: "Pick one".to_string(),
                options: vec![
                    PopupOption {
                        label: "First".to_string(),
                        description: "The first fixed option.".to_string(),
                    },
                    PopupOption {
                        label: "Other".to_string(),
                        description: "A caller-provided other option.".to_string(),
                    },
                ],
            }],
        }
        .normalized();

        assert_eq!(request.questions[0].options.len(), 2);
        assert_eq!(request.questions[0].options[0].label, "First");
        assert_eq!(request.questions[0].options[1].label, "Other");
        assert_eq!(
            request.questions[0].options[1].description,
            "Enter a custom answer instead of choosing a fixed option."
        );
    }

    #[test]
    fn builds_response_from_selected_answers() {
        let response = PopupInputResponse::from_answers([(
            "question_one".to_string(),
            "Custom answer".to_string(),
        )]);

        assert_eq!(
            response.answers["question_one"].answers,
            vec!["Custom answer".to_string()]
        );
    }

    #[test]
    fn rejects_removed_header_field() {
        let error = serde_json::from_str::<PopupInputRequest>(
            r#"{
                "questions": [
                    {
                        "id": "delivery_strategy",
                        "header": "Strategy",
                        "question": "Pick one",
                        "options": [
                            {
                                "label": "Fast path",
                                "description": "Prefer the quickest option."
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect_err("popup requests should reject the removed header field");

        assert!(error.to_string().contains("unknown field `header`"));
    }
}
