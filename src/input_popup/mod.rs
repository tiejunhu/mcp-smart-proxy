#[cfg(target_os = "macos")]
mod macos;
mod runner;
mod schema;
#[cfg(not(target_os = "macos"))]
mod ui_stub;

use std::error::Error;
use std::io::Read;

#[cfg(target_os = "macos")]
pub use macos::show_popup_dialog;
pub use runner::request_user_input_in_popup;
pub use schema::{PopupInputRequest, PopupOption, PopupQuestion};
#[cfg(not(target_os = "macos"))]
pub use ui_stub::show_popup_dialog;

#[cfg(not(target_os = "macos"))]
pub const POPUP_INPUT_UNSUPPORTED_MESSAGE: &str = "popup input dialogs are available only on macOS; this build does not include the Swift popup helper";

pub fn popup_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Stable snake_case identifier for the question."
                            },
                            "question": {
                                "type": "string",
                                "description": "The user-facing prompt. Supports multi-line text, but should be concise and clear. Avoid including the options in the question text, as they are displayed separately."
                            },
                        "options": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": {
                                        "type": "string",
                                        "description": "Option label using 1 to 5 words."
                                    },
                                    "description": {
                                        "type": "string",
                                        "description": "One sentence explaining the tradeoff or impact."
                                    }
                                },
                                "required": ["label", "description"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["id", "question", "options"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["questions"],
        "additionalProperties": false
    })
}

pub fn popup_input_supported() -> bool {
    cfg!(target_os = "macos")
}

pub fn read_popup_request_from_stdin() -> Result<PopupInputRequest, Box<dyn Error>> {
    let mut stdin = std::io::stdin().lock();
    let mut content = Vec::new();
    stdin.read_to_end(&mut content)?;
    Ok(serde_json::from_slice(&content)?)
}
